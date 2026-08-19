use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use grammers_client::media::Media;
use grammers_client::message::{InputMessage, Message as TelegramMessage};
use grammers_client::peer::Peer;
use grammers_client::tl;
use grammers_client::update::Message;
use grammers_client::Client;
use grammers_session::types::{PeerId, PeerKind, PeerRef};
use mlua::{LuaSerdeExt, UserData, UserDataMethods};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::config::{db_key, env_key};
use crate::database::Database;
use crate::runtime::RuntimeState;
use crate::telegram;

use super::installer;

const GROUP_INFO_SCAN_LIMIT: usize = 20_000;
const GROUP_INFO_SEARCH_PAGE_LIMIT: i32 = 100;

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub db: Arc<Database>,
    pub runtime: Arc<RuntimeState>,
    pub(super) modules_dir: std::path::PathBuf,
    module_name: String,
    permissions: Vec<String>,
    trusted: bool,
    pub message: Arc<Mutex<Option<Message>>>,
}

impl Ctx {
    pub fn new(
        client: Client,
        db: Arc<Database>,
        runtime: Arc<RuntimeState>,
        modules_dir: std::path::PathBuf,
        module_name: String,
        permissions: Vec<String>,
        trusted: bool,
    ) -> Self {
        Self {
            client,
            db,
            runtime,
            modules_dir,
            module_name,
            permissions,
            trusted,
            message: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_message(self, msg: Message) -> Self {
        Self {
            message: Arc::new(Mutex::new(Some(msg))),
            ..self
        }
    }

    fn has_permission(&self, permission: &str) -> bool {
        self.trusted || self.permissions.iter().any(|value| value == permission)
    }

    fn require_permission(&self, permission: &str) -> mlua::Result<()> {
        if self.has_permission(permission) {
            Ok(())
        } else {
            Err(mlua::Error::runtime(format!(
                "module '{}' needs permission '{}'",
                self.module_name, permission
            )))
        }
    }
}

impl UserData for Ctx {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method("reply", |_, ctx, text: String| async move {
            let msg = {
                let guard = ctx.message.lock().await;
                guard.as_ref().cloned()
            };
            if let Some(msg) = msg {
                telegram::msg_respond(&ctx.runtime, &msg, &text)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
            }
            Ok(())
        });

        methods.add_async_method("edit", |_, ctx, text: String| async move {
            let msg = {
                let guard = ctx.message.lock().await;
                guard.as_ref().cloned()
            };
            if let Some(msg) = msg {
                telegram::msg_edit_or_respond(&ctx.runtime, &msg, &text)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
            }
            Ok(())
        });

        methods.add_async_method("delete", |_, ctx, ()| async move {
            let msg = {
                let guard = ctx.message.lock().await;
                guard.as_ref().cloned()
            };
            if let Some(msg) = msg {
                ctx.runtime.wait_for_telegram_send().await;
                msg.delete()
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
            }
            Ok(())
        });

        methods.add_async_method("db_get", |lua, ctx, key: String| async move {
            let value = ctx.db.get(&key).await;
            lua.to_value(&value)
        });

        methods.add_async_method(
            "db_set",
            |_, ctx, (key, value): (String, mlua::Value)| async move {
                let json = match value {
                    mlua::Value::String(s) => serde_json::Value::String(s.to_str()?.to_string()),
                    mlua::Value::Integer(i) => serde_json::Value::Number(i.into()),
                    mlua::Value::Boolean(b) => serde_json::Value::Bool(b),
                    _ => serde_json::Value::Null,
                };
                ctx.db
                    .set(key, json)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))
            },
        );

        methods.add_async_method(
            "install_module",
            |_, ctx, (source, name): (String, Option<String>)| async move {
                ctx.require_permission("modules.install")?;
                installer::install_module(&ctx, source, name).await
            },
        );

        methods.add_async_method(
            "install_replied_module",
            |_, ctx, name: Option<String>| async move {
                ctx.require_permission("modules.install")?;
                installer::install_replied_module(ctx.clone(), name)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))
            },
        );

        methods.add_async_method(
            "install_plugin",
            |_, ctx, (source, name): (String, Option<String>)| async move {
                ctx.require_permission("modules.install")?;
                installer::install_module(&ctx, source, name).await
            },
        );

        methods.add_async_method(
            "install_replied_plugin",
            |_, ctx, name: Option<String>| async move {
                ctx.require_permission("modules.install")?;
                installer::install_replied_module(ctx.clone(), name)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))
            },
        );

        methods.add_async_method("uptime_seconds", |_, ctx, ()| async move {
            Ok(ctx.runtime.uptime_seconds())
        });

        methods.add_method("now_ms", |_, _, ()| {
            Ok(SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0))
        });

        methods.add_async_method("runtime_stats", |lua, ctx, ()| async move {
            let stats = serde_json::json!({
                "uptime_seconds": ctx.runtime.uptime_seconds(),
                "cpu_percent": ctx.runtime.process_cpu_percent().await,
            });
            lua.to_value(&stats)
        });

        methods.add_async_method("sanitize", |_, ctx, text: String| async move {
            Ok(sanitize_text(&ctx, &text).await)
        });

        methods.add_async_method("message_text", |_, ctx, ()| async move {
            let guard = ctx.message.lock().await;
            Ok(guard
                .as_ref()
                .map(|message| message.text().to_string())
                .unwrap_or_default())
        });

        methods.add_async_method("replied_text", |_, ctx, ()| async move {
            let guard = ctx.message.lock().await;
            let Some(message) = guard.as_ref() else {
                return Ok(String::new());
            };
            let reply = message
                .get_reply()
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
            Ok(reply
                .as_ref()
                .map(|message| message.text().to_string())
                .unwrap_or_default())
        });

        methods.add_method("module_info", |lua, ctx, ()| {
            let info = serde_json::json!({
                "name": ctx.module_name.clone(),
                "permissions": ctx.permissions.clone(),
                "trusted": ctx.trusted,
            });
            lua.to_value(&info)
        });

        methods.add_async_method("http_get", |_, ctx, url: String| async move {
            ctx.require_permission("network")?;
            http_get_text(&url)
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))
        });

        methods.add_async_method("http_json_get", |lua, ctx, url: String| async move {
            ctx.require_permission("network")?;
            let text = http_get_text(&url)
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
            let value = serde_json::from_str::<serde_json::Value>(&text)
                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
            lua.to_value(&value)
        });

        methods.add_async_method(
            "http_request",
            |_,
             ctx,
             (method, url, body, headers): (
                String,
                String,
                Option<String>,
                Option<mlua::Table>,
            )| async move {
                ctx.require_permission("network")?;
                let headers = header_pairs(headers)?;
                http_request_text(&method, &url, body, headers)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))
            },
        );

        methods.add_async_method(
            "http_json_request",
            |lua,
             ctx,
             (method, url, body, headers): (
                String,
                String,
                Option<String>,
                Option<mlua::Table>,
            )| async move {
                ctx.require_permission("network")?;
                let headers = header_pairs(headers)?;
                let text = http_request_text(&method, &url, body, headers)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                let value = serde_json::from_str::<serde_json::Value>(&text)
                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                lua.to_value(&value)
            },
        );

        methods.add_async_method(
            "http_json_multipart_file_request",
            |lua,
             ctx,
             (method, url, file_field, path, fields, headers): (
                String,
                String,
                String,
                String,
                Option<mlua::Table>,
                Option<mlua::Table>,
            )| async move {
                ctx.require_permission("network")?;
                ctx.require_permission("telegram.media")?;
                let fields = field_pairs(fields)?;
                let headers = header_pairs(headers)?;
                let text = http_multipart_file_request_text(
                    &method,
                    &url,
                    &file_field,
                    &path,
                    fields,
                    headers,
                )
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                let value = serde_json::from_str::<serde_json::Value>(&text)
                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                lua.to_value(&value)
            },
        );

        methods.add_method("env_get", |_, ctx, key: String| {
            ctx.require_permission("secrets")?;
            Ok(std::env::var(key).ok())
        });

        methods.add_async_method(
            "download_replied_media",
            |_, ctx, name: Option<String>| async move {
                ctx.require_permission("telegram.media")?;
                download_replied_media(ctx.clone(), name)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))
            },
        );

        methods.add_async_method(
            "download_url",
            |_, ctx, (url, name): (String, Option<String>)| async move {
                ctx.require_permission("network")?;
                ctx.require_permission("telegram.media")?;
                download_url_to_file(&url, name.as_deref())
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))
            },
        );

        methods.add_async_method(
            "send_file",
            |_, ctx, (path, caption): (String, Option<String>)| async move {
                ctx.require_permission("telegram.media")?;
                send_file(ctx.clone(), path, caption.unwrap_or_default())
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))
            },
        );

        methods.add_async_method("run_term", |_, ctx, command: String| async move {
            ctx.require_permission("shell")?;
            run_shell_command(ctx.clone(), command)
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))
        });

        methods.add_async_method("update_project", |_, ctx, ()| async move {
            ctx.require_permission("shell")?;
            update_project(ctx.clone())
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))
        });

        methods.add_async_method("delete_last_own", |_, ctx, count: u32| async move {
            ctx.require_permission("telegram.history")?;
            delete_last_own_messages(ctx.clone(), count)
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))
        });

        methods.add_async_method("message_info", |_, ctx, ()| async move {
            ctx.require_permission("telegram.history")?;
            build_message_info(ctx.clone())
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))
        });

        methods.add_async_method("sleep", |_, _, seconds: u64| async move {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            Ok(())
        });

        methods.add_async_method("sleep_ms", |_, _, millis: u64| async move {
            tokio::time::sleep(Duration::from_millis(millis)).await;
            Ok(())
        });
    }
}

async fn download_replied_media(ctx: Ctx, name: Option<String>) -> anyhow::Result<String> {
    let guard = ctx.message.lock().await;
    let Some(message) = guard.as_ref() else {
        anyhow::bail!("No message context.");
    };
    let Some(reply) = message.get_reply().await? else {
        anyhow::bail!("Reply to a message with downloadable media.");
    };
    let file_name = name
        .as_deref()
        .map(safe_file_name)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("media-{}.bin", uuid::Uuid::new_v4()));
    let path = Path::new("data").join("downloads").join(file_name);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if !reply.download_media(&path).await? {
        anyhow::bail!("Replied message has no downloadable media.");
    }
    Ok(path.to_string_lossy().to_string())
}

async fn download_url_to_file(url: &str, name: Option<&str>) -> anyhow::Result<String> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        anyhow::bail!("URL must start with http:// or https://");
    }
    let file_name = name
        .map(safe_file_name)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            url.rsplit('/')
                .next()
                .map(safe_file_name)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| format!("download-{}.bin", uuid::Uuid::new_v4()));
    let path = Path::new("data").join("downloads").join(file_name);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    if bytes.len() > 50 * 1024 * 1024 {
        anyhow::bail!("download is larger than 50 MiB");
    }
    tokio::fs::write(&path, bytes).await?;
    Ok(path.to_string_lossy().to_string())
}

async fn send_file(ctx: Ctx, path: String, caption: String) -> anyhow::Result<()> {
    let msg = {
        let guard = ctx.message.lock().await;
        guard.as_ref().cloned()
    };
    let Some(msg) = msg else {
        return Ok(());
    };
    let path = safe_data_path(&path)?;
    let uploaded = ctx.client.upload_file(&path).await?;
    ctx.runtime.wait_for_telegram_send().await;
    msg.respond(InputMessage::new().text(caption).file(uploaded))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

fn safe_data_path(path: &str) -> anyhow::Result<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        anyhow::bail!("path must be relative and must not contain '..'");
    }
    Ok(path.to_path_buf())
}

fn safe_file_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

async fn http_get_text(url: &str) -> anyhow::Result<String> {
    http_request_text("GET", url, None, Vec::new()).await
}

fn header_pairs(headers: Option<mlua::Table>) -> mlua::Result<Vec<(String, String)>> {
    let Some(headers) = headers else {
        return Ok(Vec::new());
    };
    let mut pairs = Vec::new();
    for pair in headers.pairs::<String, String>() {
        pairs.push(pair?);
    }
    Ok(pairs)
}

fn field_pairs(fields: Option<mlua::Table>) -> mlua::Result<Vec<(String, String)>> {
    let Some(fields) = fields else {
        return Ok(Vec::new());
    };
    let mut pairs = Vec::new();
    for pair in fields.pairs::<String, String>() {
        pairs.push(pair?);
    }
    Ok(pairs)
}

async fn http_request_text(
    method: &str,
    url: &str,
    body: Option<String>,
    headers: Vec<(String, String)>,
) -> anyhow::Result<String> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        anyhow::bail!("URL must start with http:// or https://");
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()?;
    let method = reqwest::Method::from_bytes(method.trim().to_uppercase().as_bytes())?;
    let mut request = client.request(method, url);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    if let Some(body) = body {
        request = request.body(body);
    }
    let bytes = request.send().await?.error_for_status()?.bytes().await?;
    if bytes.len() > 262_144 {
        anyhow::bail!("HTTP response is larger than 256 KiB");
    }
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

async fn http_multipart_file_request_text(
    method: &str,
    url: &str,
    file_field: &str,
    path: &str,
    fields: Vec<(String, String)>,
    headers: Vec<(String, String)>,
) -> anyhow::Result<String> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        anyhow::bail!("URL must start with http:// or https://");
    }
    let path = safe_data_path(path)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("upload.bin")
        .to_string();
    let bytes = tokio::fs::read(&path).await?;
    if bytes.len() > 25 * 1024 * 1024 {
        anyhow::bail!("multipart upload is larger than 25 MiB");
    }

    let mut form = reqwest::multipart::Form::new().part(
        file_field.to_string(),
        reqwest::multipart::Part::bytes(bytes).file_name(file_name),
    );
    for (name, value) in fields {
        form = form.text(name, value);
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .build()?;
    let method = reqwest::Method::from_bytes(method.trim().to_uppercase().as_bytes())?;
    let mut request = client.request(method, url).multipart(form);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let bytes = request.send().await?.error_for_status()?.bytes().await?;
    if bytes.len() > 262_144 {
        anyhow::bail!("HTTP response is larger than 256 KiB");
    }
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

async fn sanitize_text(ctx: &Ctx, text: &str) -> String {
    let mut output = text.to_string();
    for key in [db_key::API_HASH, db_key::PROXY_URL] {
        if let Some(secret) = ctx.db.get(key).await.as_str() {
            mask_secret(&mut output, secret);
        }
    }
    for key in [env_key::TELOXIDE_TOKEN, env_key::FLY_MASTER_PASSWORD] {
        if let Ok(secret) = std::env::var(key) {
            mask_secret(&mut output, &secret);
        }
    }
    output
}

fn mask_secret(output: &mut String, secret: &str) {
    let secret = secret.trim();
    if secret.len() >= 8 {
        *output = output.replace(secret, "*****");
    }
}

async fn run_shell_command(ctx: Ctx, command: String) -> anyhow::Result<()> {
    if command.trim().is_empty() {
        edit_current_message(&ctx, "**Usage**  \n`.term <command>`").await?;
        return Ok(());
    }

    edit_current_message(&ctx, "**Running...**").await?;

    let mut child = shell_command(&command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().map(BufReader::new);
    let stderr = child.stderr.take().map(BufReader::new);
    let mut stdout_lines = stdout.map(|reader| reader.lines());
    let mut stderr_lines = stderr.map(|reader| reader.lines());
    let mut output = String::new();
    let mut interval = tokio::time::interval(Duration::from_millis(1200));
    let mut stdout_done = stdout_lines.is_none();
    let mut stderr_done = stderr_lines.is_none();

    loop {
        tokio::select! {
            line = async {
                match stdout_lines.as_mut() {
                    Some(lines) => lines.next_line().await,
                    None => Ok(None),
                }
            }, if !stdout_done => {
                match line? {
                    Some(line) => push_output_line(&mut output, &line),
                    None => stdout_done = true,
                }
            }
            line = async {
                match stderr_lines.as_mut() {
                    Some(lines) => lines.next_line().await,
                    None => Ok(None),
                }
            }, if !stderr_done => {
                match line? {
                    Some(line) => push_output_line(&mut output, &line),
                    None => stderr_done = true,
                }
            }
            _ = interval.tick() => {
                let text = format_term_output(&sanitize_text(&ctx, &output).await, false);
                let _ = edit_current_message_only(&ctx, &preview_text(&text)).await;
            }
            status = child.wait(), if stdout_done && stderr_done => {
                let status = status?;
                let mut final_output = sanitize_text(&ctx, &output).await;
                if final_output.trim().is_empty() {
                    final_output = "(no output)".to_string();
                }
                let text = format!(
                    "**Exit:** `{}`\n\n{}",
                    status.code().map_or("signal".to_string(), |code| code.to_string()),
                    format_term_output(&final_output, true),
                );
                edit_current_message(&ctx, &text).await?;
                break;
            }
        }
    }

    Ok(())
}

async fn update_project(ctx: Ctx) -> anyhow::Result<()> {
    edit_current_message(&ctx, "Checking for updates...").await?;

    let old_head = run_command_capture("git rev-parse HEAD").await?;
    let pull_output = run_command_capture("git pull").await?;
    let new_head = run_command_capture("git rev-parse HEAD").await?;

    if old_head.trim() == new_head.trim() {
        let text = format!(
            "**Already up to date.**\n\n```text\n{}\n```",
            pull_output.trim()
        );
        edit_current_message(&ctx, &sanitize_text(&ctx, &text).await).await?;
        return Ok(());
    }

    let diff_command = format!(
        "git diff --name-only {} {}",
        old_head.trim(),
        new_head.trim()
    );
    let changed_files = run_command_capture(&diff_command).await?;
    let rust_changed = changed_files.lines().any(is_rust_project_file);
    let lua_changed = changed_files
        .lines()
        .any(|path| path.starts_with("modules/") || path.starts_with("rust-fly-telegram/modules/"));

    if rust_changed {
        edit_current_message(&ctx, "**Rust changes found.**  \nBuilding release...").await?;
        let build_output = run_command_capture("cargo build --release").await?;
        let text = format!(
            "**Updated and built release.**\n\n```text\n{}\n```\n\n**Restarting...**",
            sanitize_text(&ctx, &build_output).await
        );
        edit_current_message(&ctx, &text).await?;
        std::process::exit(0);
    }

    let text = if lua_changed {
        "**Lua modules updated.**  \nWatcher will reload changed scripts."
    } else {
        "**Updated.**  \nNo Rust or Lua module changes detected."
    };
    edit_current_message(&ctx, text).await?;
    Ok(())
}

async fn run_command_capture(command: &str) -> anyhow::Result<String> {
    let output = shell_command(command).output().await?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));

    if output.status.success() {
        Ok(text)
    } else {
        anyhow::bail!("command failed: {command}\n{text}")
    }
}

fn is_rust_project_file(path: &str) -> bool {
    matches!(path, "Cargo.toml" | "Cargo.lock")
        || path.starts_with("src/")
        || path.starts_with("rust-fly-telegram/src/")
        || path == "rust-fly-telegram/Cargo.toml"
        || path == "rust-fly-telegram/Cargo.lock"
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", command]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", command]);
        cmd
    }
}

fn push_output_line(output: &mut String, line: &str) {
    output.push_str(line);
    output.push('\n');
    if output.len() > 20_000 {
        let keep_from = output.len().saturating_sub(20_000);
        output.replace_range(..keep_from, "");
    }
}

fn format_term_output(output: &str, done: bool) -> String {
    let prefix = if done { "Done" } else { "Running" };
    format!(
        "**{prefix}**\n\n```text\n{}\n```",
        escape_code_block(output)
    )
}

fn escape_code_block(text: &str) -> String {
    text.replace("```", "`\u{200b}``")
}

fn preview_text(text: &str) -> String {
    let mut preview = text.chars().rev().take(3800).collect::<String>();
    preview = preview.chars().rev().collect();
    if preview.len() < text.len() {
        format!("... trimmed live output ...\n{preview}")
    } else {
        preview
    }
}

async fn edit_current_message(ctx: &Ctx, text: &str) -> anyhow::Result<()> {
    let msg = {
        let guard = ctx.message.lock().await;
        guard.as_ref().cloned()
    };
    let Some(msg) = msg else {
        return Ok(());
    };
    telegram::msg_edit_or_respond(&ctx.runtime, &msg, text).await
}

async fn edit_current_message_only(ctx: &Ctx, text: &str) -> anyhow::Result<()> {
    let msg = {
        let guard = ctx.message.lock().await;
        guard.as_ref().cloned()
    };
    let Some(msg) = msg else {
        return Ok(());
    };
    telegram::msg_edit_only(&ctx.runtime, &msg, text).await
}

async fn delete_last_own_messages(ctx: Ctx, count: u32) -> anyhow::Result<()> {
    let guard = ctx.message.lock().await;
    let Some(msg) = guard.as_ref() else {
        return Ok(());
    };
    let peer_ref = telegram::resolve_message_peer(&ctx.client, msg).await?;
    let mut messages = ctx
        .client
        .search_messages(peer_ref)
        .sent_by_self()
        .offset_id(msg.id() + 1);
    let mut ids = Vec::new();
    while ids.len() < count as usize {
        let Some(message) = messages.next().await? else {
            break;
        };
        ids.push(message.id());
    }
    if ids.is_empty() {
        edit_current_message(&ctx, "**Delete**  \nNo own messages found.").await?;
        return Ok(());
    }
    telegram::delete_messages(&ctx.client, &ctx.runtime, peer_ref, &ids).await?;
    Ok(())
}

async fn build_message_info(ctx: Ctx) -> anyhow::Result<String> {
    let guard = ctx.message.lock().await;
    let Some(msg) = guard.as_ref() else {
        return Ok("**Info**  \nNo message context.".to_string());
    };
    let reply = msg.get_reply().await?;
    let target = reply.as_ref().unwrap_or(msg);
    let peer_ref = telegram::resolve_message_peer(&ctx.client, msg).await?;
    let sender_id = target.sender_id();
    let sender_ref = target.sender_ref().await;
    let target_is_own = target.outgoing();

    let mut lines = Vec::new();
    lines.push("**Info**".to_string());
    lines.push(format!("**Chat ID:** `{}`", format_peer_id(peer_ref.id)));
    lines.push(format!("**Message ID:** `{}`", target.id()));
    lines.push(format!(
        "**Target:** `{}`",
        if reply.is_some() { "reply" } else { "current" }
    ));

    if !matches!(peer_ref.id.kind(), PeerKind::UserSelf) {
        let chat_dc = chat_dc_id(&ctx.client, msg, peer_ref)
            .await?
            .map(|dc_id| dc_id.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(format!("**DC:** `{chat_dc}`"));
    }

    if let Some(sender_id) = sender_id.filter(|_| !target_is_own) {
        lines.push(format!("**Sender ID:** `{}`", format_peer_id(sender_id)));
        lines.push(format!(
            "**Estimated registration:** `{}`",
            estimate_telegram_registration(sender_id)
        ));
    }

    if let Some(peer) = target.sender().filter(|_| !target_is_own) {
        append_peer_identity(&mut lines, peer);
    }

    if let Some(file_dc) = message_file_dc_id(target) {
        let file_dc = file_dc
            .map(|dc_id| dc_id.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(format!("**File DC:** `{file_dc}`"));
    }

    if is_group_peer(peer_ref.id) && !target_is_own {
        if let Some(sender_id) = sender_id {
            let stats =
                collect_group_sender_stats(&ctx.client, peer_ref, sender_id, sender_ref).await?;
            lines.push(format!("**Group messages by user:** `{}`", stats.count));
            if let Some(first_seen) = stats.first_seen {
                lines.push(format!("**First group message:** `{first_seen}`"));
            }
            if stats.truncated {
                lines.push(format!(
                    "**Group scan:** `first {GROUP_INFO_SCAN_LIMIT} messages only`"
                ));
            }
        }
    }

    Ok(lines.join("  \n"))
}

fn format_peer_id(peer_id: PeerId) -> i64 {
    peer_id.bot_api_dialog_id()
}

fn is_group_peer(peer_id: PeerId) -> bool {
    matches!(peer_id.kind(), PeerKind::Chat | PeerKind::Channel)
}

fn append_peer_identity(lines: &mut Vec<String>, peer: &Peer) {
    if let Some(name) = peer.name().filter(|name| !name.is_empty()) {
        lines.push(format!("**Nickname:** `{}`", escape_inline_code(name)));
    }
    if let Some(username) = peer.username().filter(|username| !username.is_empty()) {
        lines.push(format!("**Username:** `@{}`", escape_inline_code(username)));
    }
}

fn escape_inline_code(text: &str) -> String {
    text.replace('`', "'")
}

async fn chat_dc_id(
    client: &Client,
    msg: &Message,
    peer_ref: PeerRef,
) -> anyhow::Result<Option<i32>> {
    if matches!(peer_ref.id.kind(), PeerKind::UserSelf) {
        return Ok(None);
    }

    if let Some(dc_id) = msg.peer().and_then(peer_profile_dc_id) {
        return Ok(Some(dc_id));
    }

    match client.resolve_peer(peer_ref).await {
        Ok(peer) => Ok(peer_profile_dc_id(&peer)),
        Err(_) => Ok(None),
    }
}

fn peer_profile_dc_id(peer: &Peer) -> Option<i32> {
    match peer {
        Peer::User(user) => user.photo().map(|photo| photo.dc_id),
        Peer::Group(group) => group.photo().map(|photo| photo.dc_id),
        Peer::Channel(channel) => channel.photo().map(|photo| photo.dc_id),
    }
}

fn message_file_dc_id(message: &TelegramMessage) -> Option<Option<i32>> {
    match message.media()? {
        Media::Photo(photo) => Some(photo.raw.photo.as_ref().and_then(photo_dc_id)),
        Media::Document(document) => Some(document.raw.document.as_ref().and_then(document_dc_id)),
        Media::Sticker(sticker) => Some(
            sticker
                .document
                .raw
                .document
                .as_ref()
                .and_then(document_dc_id),
        ),
        _ => None,
    }
}

fn photo_dc_id(photo: &tl::enums::Photo) -> Option<i32> {
    match photo {
        tl::enums::Photo::Photo(photo) => Some(photo.dc_id),
        tl::enums::Photo::Empty(_) => None,
    }
}

fn document_dc_id(document: &tl::enums::Document) -> Option<i32> {
    match document {
        tl::enums::Document::Document(document) => Some(document.dc_id),
        tl::enums::Document::Empty(_) => None,
    }
}

#[derive(Debug, Default)]
struct GroupSenderStats {
    count: usize,
    first_seen: Option<String>,
    truncated: bool,
}

async fn collect_group_sender_stats(
    client: &Client,
    peer_ref: PeerRef,
    sender_id: PeerId,
    sender_ref: Option<PeerRef>,
) -> anyhow::Result<GroupSenderStats> {
    if let Some(sender_ref) = sender_ref {
        if let Some(stats) = search_group_sender_stats(client, peer_ref, sender_ref).await? {
            return Ok(stats);
        }
    }

    scan_group_sender_stats(client, peer_ref, sender_id).await
}

async fn search_group_sender_stats(
    client: &Client,
    peer_ref: PeerRef,
    sender_ref: PeerRef,
) -> anyhow::Result<Option<GroupSenderStats>> {
    if !matches!(sender_ref.id.kind(), PeerKind::User | PeerKind::UserSelf) {
        return Ok(None);
    }

    let mut request = tl::functions::messages::Search {
        peer: peer_ref.into(),
        q: String::new(),
        from_id: Some(sender_ref.into()),
        saved_peer_id: None,
        saved_reaction: None,
        top_msg_id: None,
        filter: tl::enums::MessagesFilter::InputMessagesFilterEmpty,
        min_date: 0,
        max_date: 0,
        offset_id: 0,
        add_offset: 0,
        limit: GROUP_INFO_SEARCH_PAGE_LIMIT,
        max_id: 0,
        min_id: 0,
        hash: 0,
    };
    let mut stats = GroupSenderStats::default();
    let mut scanned = 0usize;

    loop {
        let response = client.invoke(&request).await?;
        let (messages, total) = search_messages_and_total(response);
        if let Some(total) = total {
            stats.count = total;
        }

        if messages.is_empty() {
            return Ok(Some(stats));
        }

        for message in &messages {
            if let Some(timestamp) = raw_message_timestamp(message) {
                stats.first_seen = Some(format_unix_timestamp(timestamp));
            }
        }
        scanned += messages.len();

        let Some(last_id) = messages.last().map(|message| message.id()) else {
            return Ok(Some(stats));
        };
        if messages.len() < GROUP_INFO_SEARCH_PAGE_LIMIT as usize || last_id <= 1 {
            return Ok(Some(stats));
        }
        if scanned >= GROUP_INFO_SCAN_LIMIT {
            stats.truncated = true;
            return Ok(Some(stats));
        }

        request.offset_id = last_id;
    }
}

fn search_messages_and_total(
    response: tl::enums::messages::Messages,
) -> (Vec<tl::enums::Message>, Option<usize>) {
    match response {
        tl::enums::messages::Messages::Messages(messages) => {
            let total = messages.messages.len();
            (messages.messages, Some(total))
        }
        tl::enums::messages::Messages::Slice(messages) => {
            (messages.messages, Some(messages.count as usize))
        }
        tl::enums::messages::Messages::ChannelMessages(messages) => {
            (messages.messages, Some(messages.count as usize))
        }
        tl::enums::messages::Messages::NotModified(messages) => {
            (Vec::new(), Some(messages.count as usize))
        }
    }
}

fn raw_message_timestamp(message: &tl::enums::Message) -> Option<i32> {
    match message {
        tl::enums::Message::Message(message) => Some(message.date),
        tl::enums::Message::Service(message) => Some(message.date),
        tl::enums::Message::Empty(_) => None,
    }
}

async fn scan_group_sender_stats(
    client: &Client,
    peer_ref: PeerRef,
    sender_id: PeerId,
) -> anyhow::Result<GroupSenderStats> {
    let mut messages = client.iter_messages(peer_ref);
    let mut stats = GroupSenderStats::default();
    let mut scanned = 0usize;

    while scanned < GROUP_INFO_SCAN_LIMIT {
        let Some(message) = messages.next().await? else {
            return Ok(stats);
        };
        scanned += 1;

        if message.sender_id() == Some(sender_id) {
            stats.count += 1;
            stats.first_seen = Some(format_message_date(&message));
        }
    }

    stats.truncated = messages.next().await?.is_some();
    Ok(stats)
}

fn format_message_date(message: &TelegramMessage) -> String {
    message.date().format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

fn format_unix_timestamp(timestamp: i32) -> String {
    let days = i64::from(timestamp).div_euclid(86_400);
    let seconds = i64::from(timestamp).rem_euclid(86_400);
    let (year, month, day) = date_from_days_since_epoch(days);
    let hour = seconds / 3600;
    let minute = seconds % 3600 / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

fn estimate_telegram_registration(peer_id: PeerId) -> String {
    if !matches!(peer_id.kind(), PeerKind::User | PeerKind::UserSelf) {
        return "not a user".to_string();
    }

    match estimate_registration_month(peer_id.bot_api_dialog_id()) {
        RegistrationEstimate::Date { year, month } => format!("{} {year}", month_name(month)),
        RegistrationEstimate::TooEarly => "error: first account IDs started around 100".to_string(),
        RegistrationEstimate::SkippedRange => {
            "error: Telegram skipped this range during 64-bit migration".to_string()
        }
        RegistrationEstimate::TooLarge => "ID is too large, likely from the future".to_string(),
        RegistrationEstimate::Unknown => "unknown".to_string(),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RegistrationEstimate {
    Date { year: i32, month: u8 },
    TooEarly,
    SkippedRange,
    TooLarge,
    Unknown,
}

fn estimate_registration_month(target_id: i64) -> RegistrationEstimate {
    const ANCHORS: &[(i64, i32, u8, u8)] = &[
        (100, 2013, 8, 14),
        (35_000_000, 2014, 1, 1),
        (100_000_000, 2015, 1, 1),
        (250_000_000, 2016, 1, 1),
        (400_000_000, 2017, 1, 1),
        (550_000_000, 2018, 1, 1),
        (750_000_000, 2019, 1, 1),
        (1_000_000_000, 2020, 1, 1),
        (1_500_000_000, 2021, 1, 1),
        (2_147_483_647, 2021, 12, 1),
        (5_000_000_000, 2022, 1, 1),
        (5_650_000_000, 2023, 1, 1),
        (6_300_000_000, 2024, 1, 1),
        (7_200_000_000, 2025, 1, 1),
        (8_100_000_000, 2026, 1, 1),
        (9_000_000_000, 2027, 1, 1),
    ];

    if target_id < 100 {
        return RegistrationEstimate::TooEarly;
    }
    if 2_147_483_647 < target_id && target_id < 5_000_000_000 {
        return RegistrationEstimate::SkippedRange;
    }
    if target_id > ANCHORS[ANCHORS.len() - 1].0 {
        return RegistrationEstimate::TooLarge;
    }

    for window in ANCHORS.windows(2) {
        let (start_id, start_year, start_month, start_day) = window[0];
        let (end_id, end_year, end_month, end_day) = window[1];
        if start_id <= target_id && target_id <= end_id {
            let start_days = days_since_epoch(start_year, start_month, start_day);
            let end_days = days_since_epoch(end_year, end_month, end_day);
            let ratio = (target_id - start_id) as f64 / (end_id - start_id) as f64;
            let estimated_days =
                start_days + ((end_days - start_days) as f64 * ratio).round() as i64;
            let (year, month, _) = date_from_days_since_epoch(estimated_days);
            return RegistrationEstimate::Date { year, month };
        }
    }

    RegistrationEstimate::Unknown
}

fn month_name(month: u8) -> &'static str {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    MONTHS
        .get(month.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("Unknown")
}

fn days_since_epoch(year: i32, month: u8, day: u8) -> i64 {
    let year = year - (month <= 2) as i32;
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    (era * 146_097 + day_of_era - 719_468) as i64
}

fn date_from_days_since_epoch(days: i64) -> (i32, u8, u8) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era as i32 + era as i32 * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += (month <= 2) as i32;
    (year, month as u8, day as u8)
}

#[cfg(test)]
mod info_tests {
    use super::{estimate_registration_month, RegistrationEstimate};

    #[test]
    fn estimate_registration_rejects_skipped_range() {
        assert_eq!(
            estimate_registration_month(3_000_000_000),
            RegistrationEstimate::SkippedRange
        );
    }

    #[test]
    fn estimate_registration_interpolates_anchor_month() {
        assert_eq!(
            estimate_registration_month(5_000_000_000),
            RegistrationEstimate::Date {
                year: 2022,
                month: 1,
            }
        );
    }
}
