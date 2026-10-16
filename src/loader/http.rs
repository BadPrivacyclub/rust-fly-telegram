//! HTTP for modules: I use one shared client, a real User-Agent, size limits, and error messages
//! a user can act on ("rdap.org answered 404 Not Found") instead of raw reqwest output.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;

/// My default and maximum request timeouts, in seconds.
pub const DEFAULT_TIMEOUT: u64 = 25;
pub const MAX_TIMEOUT: u64 = 120;
/// The largest text or JSON body I give a module.
pub const TEXT_LIMIT: usize = 4 * 1024 * 1024;
/// The largest file I save for `ctx:download_url`.
pub const DOWNLOAD_LIMIT: usize = 50 * 1024 * 1024;

const USER_AGENT: &str = concat!(
    "fly-telegram/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/BadPrivacyclub/rust-fly-telegram)"
);

/// The proxy from the `proxy_url` setting. Empty means I connect directly.
static PROXY: Mutex<String> = Mutex::new(String::new());
/// The client I built for the proxy in use. I rebuild it only when the proxy changes.
static CLIENT: Mutex<Option<(String, reqwest::Client)>> = Mutex::new(None);

/// I send module requests through this SOCKS5 proxy from now on; empty means direct.
pub fn set_proxy(url: &str) {
    *PROXY.lock().unwrap_or_else(|e| e.into_inner()) = url.trim().to_string();
}

fn client() -> anyhow::Result<reqwest::Client> {
    let proxy = PROXY.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut cached = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((cached_proxy, client)) = cached.as_ref() {
        if *cached_proxy == proxy {
            return Ok(client.clone());
        }
    }
    let mut builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(10));
    if !proxy.is_empty() {
        let route = reqwest::Proxy::all(&proxy).context("the proxy URL is invalid")?;
        builder = builder.proxy(route);
    }
    let client = builder.build().context("could not set up the HTTP client")?;
    *cached = Some((proxy, client.clone()));
    Ok(client)
}

/// One request as a module describes it to me.
#[derive(Default)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Body>,
    pub timeout: Option<u64>,
    pub limit: Option<usize>,
}

pub enum Body {
    Text(String),
    Multipart(reqwest::multipart::Form),
}

/// What came back. I treat non-2xx statuses as a response, not an error.
pub struct Response {
    pub status: u16,
    pub reason: String,
    pub url: String,
    pub content_type: String,
    pub body: Vec<u8>,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// I fail with a readable message on non-2xx statuses.
    pub fn into_success(self) -> anyhow::Result<Self> {
        match self.status_error() {
            None => Ok(self),
            Some(message) => anyhow::bail!(message),
        }
    }

    /// I describe a non-2xx status readably, or return `None` on success.
    pub fn status_error(&self) -> Option<String> {
        if self.ok() {
            return None;
        }
        let mut message = format!(
            "{} answered HTTP {} {}",
            host_of(&self.url),
            self.status,
            self.reason
        );
        if let Some(detail) = error_detail(&self.text()) {
            message.push_str(": ");
            message.push_str(&detail);
        }
        Some(message)
    }

    pub fn json(&self) -> anyhow::Result<serde_json::Value> {
        serde_json::from_slice(&self.body).map_err(|_| {
            let snippet = snippet(&self.text(), 120);
            if snippet.is_empty() {
                anyhow::anyhow!("{} sent an empty response", host_of(&self.url))
            } else {
                anyhow::anyhow!(
                    "{} did not return JSON (got: {snippet})",
                    host_of(&self.url)
                )
            }
        })
    }
}

pub fn check_url(url: &str) -> anyhow::Result<reqwest::Url> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        anyhow::bail!("URL must start with http:// or https://");
    }
    reqwest::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid URL {url}: {e}"))
}

pub async fn send(request: Request) -> anyhow::Result<Response> {
    let url = check_url(&request.url)?;
    let host = url.host_str().unwrap_or("server").to_string();
    let method_name = if request.method.trim().is_empty() {
        "GET".to_string()
    } else {
        request.method.trim().to_uppercase()
    };
    let method = reqwest::Method::from_bytes(method_name.as_bytes())
        .map_err(|_| anyhow::anyhow!("unknown HTTP method {method_name}"))?;
    let timeout = request
        .timeout
        .unwrap_or(DEFAULT_TIMEOUT)
        .clamp(1, MAX_TIMEOUT);
    let limit = request.limit.unwrap_or(TEXT_LIMIT);

    let mut builder = client()?
        .request(method, url)
        .timeout(Duration::from_secs(timeout));
    for (name, value) in request.headers {
        builder = builder.header(name, value);
    }
    builder = match request.body {
        Some(Body::Text(text)) => builder.body(text),
        Some(Body::Multipart(form)) => builder.multipart(form),
        None => builder,
    };

    let mut response = builder
        .send()
        .await
        .map_err(|e| describe(&host, timeout, &e))?;
    let status = response.status();
    let final_url = response.url().to_string();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if response
        .content_length()
        .is_some_and(|len| len as usize > limit)
    {
        anyhow::bail!("{host} sent more than {}", human_size(limit));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| describe(&host, timeout, &e))?
    {
        if body.len() + chunk.len() > limit {
            anyhow::bail!("{host} sent more than {}", human_size(limit));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Response {
        status: status.as_u16(),
        reason: status.canonical_reason().unwrap_or("").to_string(),
        url: final_url,
        content_type,
        body,
    })
}

/// I send GET or another method with a text body and fail on non-2xx statuses.
pub async fn text(
    method: &str,
    url: &str,
    body: Option<String>,
    headers: Vec<(String, String)>,
) -> anyhow::Result<String> {
    let response = send(Request {
        method: method.to_string(),
        url: url.to_string(),
        headers,
        body: body.map(Body::Text),
        ..Request::default()
    })
    .await?
    .into_success()?;
    Ok(response.text())
}

pub async fn json(
    method: &str,
    url: &str,
    body: Option<String>,
    mut headers: Vec<(String, String)>,
) -> anyhow::Result<serde_json::Value> {
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("accept"))
    {
        headers.push((
            "Accept".into(),
            "application/json, application/rdap+json;q=0.9, */*;q=0.5".into(),
        ));
    }
    let response = send(Request {
        method: method.to_string(),
        url: url.to_string(),
        headers,
        body: body.map(Body::Text),
        ..Request::default()
    })
    .await?
    .into_success()?;
    response.json()
}

pub async fn multipart_json(
    method: &str,
    url: &str,
    form: reqwest::multipart::Form,
    headers: Vec<(String, String)>,
) -> anyhow::Result<serde_json::Value> {
    let response = send(Request {
        method: method.to_string(),
        url: url.to_string(),
        headers,
        body: Some(Body::Multipart(form)),
        timeout: Some(90),
        ..Request::default()
    })
    .await?
    .into_success()?;
    response.json()
}

pub async fn download(url: &str, path: &Path) -> anyhow::Result<()> {
    let response = send(Request {
        method: "GET".into(),
        url: url.to_string(),
        timeout: Some(90),
        limit: Some(DOWNLOAD_LIMIT),
        ..Request::default()
    })
    .await?
    .into_success()?;
    tokio::fs::write(path, response.body).await?;
    Ok(())
}

pub fn host_of(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "server".to_string())
}

fn describe(host: &str, timeout: u64, error: &reqwest::Error) -> anyhow::Error {
    if error.is_timeout() {
        anyhow::anyhow!("{host} did not respond within {timeout} s, try again later")
    } else if error.is_connect() {
        anyhow::anyhow!(
            "could not connect to {host}: check the internet connection or the proxy settings"
        )
    } else if error.is_redirect() {
        anyhow::anyhow!("{host} redirected too many times")
    } else if error.is_body() || error.is_decode() {
        anyhow::anyhow!("the connection to {host} broke while reading the response")
    } else {
        anyhow::anyhow!("request to {host} failed: {}", root_cause(error))
    }
}

fn root_cause(error: &(dyn std::error::Error + 'static)) -> String {
    let mut current = error;
    while let Some(next) = current.source() {
        current = next;
    }
    current.to_string()
}

/// I take a short reason from an error body: a JSON `message`, `error`, or `title` field, or plain text.
fn error_detail(body: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        for key in [
            "message",
            "error",
            "title",
            "description",
            "detail",
            "reason",
        ] {
            match value.get(key) {
                Some(serde_json::Value::String(text)) if !text.trim().is_empty() => {
                    return Some(snippet(text, 160));
                }
                Some(serde_json::Value::Object(inner)) => {
                    if let Some(serde_json::Value::String(text)) = inner.get("message") {
                        return Some(snippet(text, 160));
                    }
                }
                _ => {}
            }
        }
        return None;
    }
    let trimmed = body.trim();
    if trimmed.is_empty() || trimmed.starts_with('<') {
        return None;
    }
    Some(snippet(trimmed, 160))
}

fn snippet(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out = flat.chars().take(max).collect::<String>();
    out.push('…');
    out
}

fn human_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{} MiB", bytes / (1024 * 1024))
    } else {
        format!("{} KiB", bytes / 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_detail_prefers_json_message() {
        assert_eq!(
            error_detail(r#"{"errorCode":404,"title":"Not Found"}"#).as_deref(),
            Some("Not Found")
        );
        assert_eq!(
            error_detail(r#"{"error":{"message":"bad key"}}"#).as_deref(),
            Some("bad key")
        );
        assert_eq!(error_detail("<html>oops</html>"), None);
        assert_eq!(
            error_detail("Unknown location").as_deref(),
            Some("Unknown location")
        );
    }

    #[test]
    fn non_success_status_is_readable() {
        let response = Response {
            status: 404,
            reason: "Not Found".into(),
            url: "https://rdap.org/domain/nope.invalid".into(),
            content_type: String::new(),
            body: Vec::new(),
        };
        let error = response.into_success().err().unwrap().to_string();
        assert_eq!(error, "rdap.org answered HTTP 404 Not Found");
    }

    #[test]
    fn rejects_non_http_urls() {
        assert!(check_url("file:///etc/passwd").is_err());
        assert!(check_url("https://example.com/x").is_ok());
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// I serve one canned HTTP response per connection on a random local port.
    async fn serve(status: &'static str, body: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let body = body.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        format!("http://{addr}/x")
    }

    #[tokio::test]
    async fn statuses_bodies_and_limits() {
        let url = serve("404 Not Found", br#"{"title":"Not Found"}"#.to_vec()).await;
        let response = send(Request {
            url: url.clone(),
            ..Request::default()
        })
        .await
        .unwrap();
        assert_eq!(response.status, 404);
        assert_eq!(
            response.status_error().unwrap(),
            "127.0.0.1 answered HTTP 404 Not Found: Not Found"
        );
        assert!(json("GET", &url, None, Vec::new()).await.is_err());

        let url = serve("200 OK", br#"{"a":1}"#.to_vec()).await;
        assert_eq!(json("GET", &url, None, Vec::new()).await.unwrap()["a"], 1);

        let url = serve("200 OK", vec![b'x'; 5 * 1024 * 1024]).await;
        let error = text("GET", &url, None, Vec::new()).await.unwrap_err();
        assert!(error.to_string().contains("more than 4 MiB"), "{error}");

        let error = text("GET", "http://127.0.0.1:9/", None, Vec::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("could not connect"), "{error}");
    }
}
