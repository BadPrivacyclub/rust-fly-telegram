//! Web panel access control.
//!
//! The panel is a local-only admin surface, but a browser on the same machine can still be
//! tricked into sending requests to it, so I protect it in three layers:
//!
//! 1. **Host check**: I serve only `localhost`, `127.0.0.1`, and `[::1]` host names,
//!    which defeats DNS rebinding.
//! 2. **CSRF check**: state-changing requests must carry `X-Fly-Request: 1` and, when the
//!    browser sends them, a loopback `Origin` and a same-origin `Sec-Fetch-Site`. Cross-origin
//!    pages cannot add custom headers without a CORS preflight, which I never approve.
//! 3. **Session**: an `HttpOnly`, `SameSite=Strict` cookie obtained with the panel password
//!    or a one-time login link printed to the console or sent with `.panel` in Telegram.
//!    The link keeps its token in the URL fragment, which browsers never send to a server,
//!    so it stays out of request logs, and the login page removes it from the history.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use argon2::password_hash::{rand_core::OsRng as HashRng, SaltString};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::database::Database;

pub const SESSION_COOKIE: &str = "fly_session";
pub const CSRF_HEADER: &str = "x-fly-request";
pub const PASSWORD_HASH_KEY: &str = "web.password_hash";

const SESSION_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// A session nobody has used for this long ends, even before `SESSION_TTL`.
const SESSION_IDLE: Duration = Duration::from_secs(12 * 60 * 60);
const LINK_TOKEN_TTL: Duration = Duration::from_secs(15 * 60);
const STARTUP_TOKEN_TTL: Duration = Duration::from_secs(60 * 60);
const LOGIN_ATTEMPT_WINDOW: Duration = Duration::from_secs(60);
const LOGIN_ATTEMPT_LIMIT: usize = 10;
const USER_AGENT_CHARS: usize = 160;
/// Requests an open tab repeats by itself; they keep a session valid but do not count as use.
const BACKGROUND_PATHS: &[&str] = &["/api/status", "/api/logs", "/api/jobs"];

struct PanelSession {
    expires: Instant,
    last_used: Instant,
    created_at: u64,
    last_used_at: u64,
    user_agent: String,
}

impl PanelSession {
    fn is_live(&self, now: Instant) -> bool {
        self.expires > now && now.duration_since(self.last_used) < SESSION_IDLE
    }
}

/// What the panel shows about a session. `id` is a hash, so the list can't be used to log in.
#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: u64,
    pub last_used_at: u64,
    pub user_agent: String,
    pub current: bool,
}

#[derive(Default)]
pub struct PanelAuth {
    sessions: Mutex<HashMap<String, PanelSession>>,
    tokens: Mutex<HashMap<String, Instant>>,
    failed_logins: Mutex<Vec<Instant>>,
}

impl PanelAuth {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// I issue a one-time login token. Redeeming it creates a session and invalidates the token.
    pub async fn issue_link_token(&self) -> String {
        self.issue_token(LINK_TOKEN_TTL).await
    }

    pub async fn issue_startup_token(&self) -> String {
        self.issue_token(STARTUP_TOKEN_TTL).await
    }

    async fn issue_token(&self, ttl: Duration) -> String {
        let token = random_token();
        let mut tokens = self.tokens.lock().await;
        let now = Instant::now();
        tokens.retain(|_, expires| *expires > now);
        tokens.insert(token.clone(), now + ttl);
        token
    }

    pub async fn redeem_token(&self, token: &str) -> bool {
        let mut tokens = self.tokens.lock().await;
        match tokens.remove(token) {
            Some(expires) => expires > Instant::now(),
            None => false,
        }
    }

    pub async fn create_session(&self, user_agent: &str) -> String {
        let id = random_token();
        let mut sessions = self.sessions.lock().await;
        let now = Instant::now();
        sessions.retain(|_, session| session.is_live(now));
        sessions.insert(
            id.clone(),
            PanelSession {
                expires: now + SESSION_TTL,
                last_used: now,
                created_at: unix_now(),
                last_used_at: unix_now(),
                user_agent: user_agent.chars().take(USER_AGENT_CHARS).collect(),
            },
        );
        id
    }

    /// I start a new session after a login and end the one the browser came with, so an id
    /// planted before the login is worth nothing afterwards.
    pub async fn rotate_session(&self, headers: &HeaderMap) -> String {
        if let Some(old) = session_from_headers(headers) {
            self.revoke_session(&old).await;
        }
        let user_agent = headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        self.create_session(user_agent).await
    }

    /// `active` is false for requests an open tab sends by itself.
    pub async fn is_valid_session(&self, id: &str, active: bool) -> bool {
        let mut sessions = self.sessions.lock().await;
        let now = Instant::now();
        let Some(session) = sessions.get_mut(id) else {
            return false;
        };
        if !session.is_live(now) {
            sessions.remove(id);
            return false;
        }
        if active {
            session.last_used = now;
            session.last_used_at = unix_now();
        }
        true
    }

    pub async fn revoke_session(&self, id: &str) {
        self.sessions.lock().await.remove(id);
    }

    pub async fn list_sessions(&self, current: Option<&str>) -> Vec<SessionInfo> {
        let mut sessions = self.sessions.lock().await;
        let now = Instant::now();
        sessions.retain(|_, session| session.is_live(now));
        let mut list = sessions
            .iter()
            .map(|(id, session)| SessionInfo {
                id: public_id(id),
                created_at: session.created_at,
                last_used_at: session.last_used_at,
                user_agent: session.user_agent.clone(),
                current: current == Some(id.as_str()),
            })
            .collect::<Vec<_>>();
        list.sort_by_key(|session| std::cmp::Reverse(session.last_used_at));
        list
    }

    /// I end the session with this public id and report whether there was one.
    pub async fn revoke_by_public_id(&self, public: &str) -> bool {
        let mut sessions = self.sessions.lock().await;
        let before = sessions.len();
        sessions.retain(|id, _| public_id(id) != public);
        sessions.len() != before
    }

    /// I end every session except `keep` and return how many ended.
    pub async fn revoke_others(&self, keep: Option<&str>) -> usize {
        let mut sessions = self.sessions.lock().await;
        let before = sessions.len();
        sessions.retain(|id, _| keep == Some(id.as_str()));
        before - sessions.len()
    }

    /// I throttle logins globally; the panel has a single operator.
    pub async fn allow_login_attempt(&self) -> bool {
        let failures = self.failed_logins.lock().await;
        let now = Instant::now();
        failures
            .iter()
            .filter(|at| now.duration_since(**at) < LOGIN_ATTEMPT_WINDOW)
            .count()
            < LOGIN_ATTEMPT_LIMIT
    }

    pub async fn record_failed_login(&self) {
        let mut failures = self.failed_logins.lock().await;
        let now = Instant::now();
        failures.retain(|at| now.duration_since(*at) < LOGIN_ATTEMPT_WINDOW);
        failures.push(now);
    }
}

fn public_id(session: &str) -> String {
    crate::database::hex(&Sha256::digest(session.as_bytes())[..8])
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub async fn password_is_set(db: &Database) -> bool {
    db.get(PASSWORD_HASH_KEY)
        .await
        .as_str()
        .is_some_and(|value| !value.is_empty())
}

pub async fn set_password(db: &Database, password: Option<&str>) -> anyhow::Result<()> {
    match password.map(str::trim).filter(|value| !value.is_empty()) {
        Some(password) => {
            if password.chars().count() < 6 {
                anyhow::bail!("panel password must be at least 6 characters");
            }
            let salt = SaltString::generate(&mut HashRng);
            let hash = Argon2::default()
                .hash_password(password.as_bytes(), &salt)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .to_string();
            db.set(PASSWORD_HASH_KEY, serde_json::Value::String(hash))
                .await
        }
        None => db.remove(PASSWORD_HASH_KEY).await,
    }
}

pub async fn verify_password(db: &Database, password: &str) -> bool {
    let stored = db.get(PASSWORD_HASH_KEY).await;
    let Some(stored) = stored.as_str() else {
        return false;
    };
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn session_cookie(id: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
        SESSION_TTL.as_secs()
    ))
    .expect("hex session ids are valid header values")
}

pub fn clear_cookie() -> HeaderValue {
    HeaderValue::from_static("fly_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

pub fn session_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.to_string())
}

pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default().to_string()
    } else {
        host.rsplit_once(':')
            .filter(|(_, port)| port.chars().all(|ch| ch.is_ascii_digit()))
            .map(|(name, _)| name.to_string())
            .unwrap_or(host)
    };
    matches!(name.as_str(), "localhost" | "127.0.0.1" | "::1")
}

fn origin_is_loopback(origin: &str) -> bool {
    origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .is_some_and(is_loopback_host)
}

fn forbidden(message: &'static str) -> Response {
    (StatusCode::FORBIDDEN, message).into_response()
}

/// I apply host and CSRF checks to every request, including the first-run setup wizard.
pub async fn guard(request: Request<Body>, next: Next) -> Response {
    let headers = request.headers();
    let host_ok = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(is_loopback_host);
    if !host_ok {
        return forbidden("fly-telegram panel only accepts local connections");
    }

    let unsafe_method = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if unsafe_method {
        if headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok()) != Some("1") {
            return forbidden("missing X-Fly-Request header");
        }
        if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
            if !origin_is_loopback(origin) {
                return forbidden("cross-origin request rejected");
            }
        }
        // `none` is a request the user typed; nothing a page sends looks like that.
        if let Some(site) = headers
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
        {
            if !matches!(site, "same-origin" | "none") {
                return forbidden("cross-site request rejected");
            }
        }
    }

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

/// I require a valid session cookie. Pages redirect to the login screen, APIs answer 401.
pub async fn require_session(
    State(auth): State<Arc<PanelAuth>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let active = !BACKGROUND_PATHS.contains(&request.uri().path());
    let valid = match session_from_headers(request.headers()) {
        Some(id) => auth.is_valid_session(&id, active).await,
        None => false,
    };
    if valid {
        return next.run(request).await;
    }

    let path = request.uri().path();
    if path.starts_with("/api/") || path.starts_with("/media/") || request.method() != Method::GET {
        (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({ "ok": false, "error": "unauthorized" })),
        )
            .into_response()
    } else {
        Redirect::to("/auth").into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts() {
        for host in [
            "localhost",
            "localhost:8080",
            "127.0.0.1:8080",
            "[::1]:8080",
            "LOCALHOST",
        ] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "example.com",
            "127.0.0.1.evil.com:80",
            "192.168.1.5:8080",
            "",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }

    #[test]
    fn parses_session_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; fly_session=abc123"),
        );
        assert_eq!(session_from_headers(&headers).as_deref(), Some("abc123"));
    }

    #[tokio::test]
    async fn tokens_are_single_use() {
        let auth = PanelAuth::new();
        let token = auth.issue_link_token().await;
        assert!(auth.redeem_token(&token).await);
        assert!(!auth.redeem_token(&token).await);
        assert!(!auth.redeem_token("bogus").await);
    }

    #[tokio::test]
    async fn sessions_can_be_revoked() {
        let auth = PanelAuth::new();
        let id = auth.create_session("test").await;
        assert!(auth.is_valid_session(&id, true).await);
        auth.revoke_session(&id).await;
        assert!(!auth.is_valid_session(&id, true).await);
    }

    #[tokio::test]
    async fn idle_sessions_end_and_background_requests_do_not_extend_them() {
        let auth = PanelAuth::new();
        let id = auth.create_session("test").await;
        let long_ago = Instant::now() - SESSION_IDLE - Duration::from_secs(1);
        let almost = Instant::now() - SESSION_IDLE + Duration::from_secs(60);

        auth.sessions.lock().await.get_mut(&id).unwrap().last_used = almost;
        assert!(auth.is_valid_session(&id, false).await);
        let last_used = auth.sessions.lock().await.get(&id).unwrap().last_used;
        assert_eq!(last_used, almost, "a background request is not use");

        auth.sessions.lock().await.get_mut(&id).unwrap().last_used = long_ago;
        assert!(!auth.is_valid_session(&id, true).await);
        assert!(auth.list_sessions(None).await.is_empty());
    }

    #[tokio::test]
    async fn sessions_are_listed_by_hash_and_revoked_one_by_one_or_together() {
        let auth = PanelAuth::new();
        let mine = auth.create_session("mine").await;
        let other = auth.create_session("other").await;
        let third = auth.create_session("third").await;

        let list = auth.list_sessions(Some(&mine)).await;
        assert_eq!(list.len(), 3);
        assert!(list.iter().all(|s| s.id != mine && s.id != other));
        assert_eq!(list.iter().filter(|s| s.current).count(), 1);

        let target = list.iter().find(|s| s.user_agent == "other").unwrap();
        assert!(auth.revoke_by_public_id(&target.id).await);
        assert!(!auth.revoke_by_public_id(&target.id).await);
        assert!(!auth.is_valid_session(&other, true).await);

        assert_eq!(auth.revoke_others(Some(&mine)).await, 1);
        assert!(auth.is_valid_session(&mine, true).await);
        assert!(!auth.is_valid_session(&third, true).await);
    }

    #[tokio::test]
    async fn password_round_trip() {
        let path = std::env::temp_dir().join(format!("fly_pw_{}.json", uuid::Uuid::new_v4()));
        let db = Database::load(&path).await.unwrap();
        assert!(!password_is_set(&db).await);
        assert!(set_password(&db, Some("12")).await.is_err());
        set_password(&db, Some("hunter22")).await.unwrap();
        assert!(password_is_set(&db).await);
        assert!(verify_password(&db, "hunter22").await);
        assert!(!verify_password(&db, "wrong").await);
        set_password(&db, None).await.unwrap();
        assert!(!password_is_set(&db).await);
        let _ = tokio::fs::remove_file(path).await;
    }
}
