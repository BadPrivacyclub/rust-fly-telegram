use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use grammers_client::update::Message;
use grammers_client::Client;
use grammers_session::types::{PeerAuth, PeerRef};
use mlua::{LuaSerdeExt, UserData, UserDataMethods};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::config::{db_key, env_key};
use crate::database::Database;
use crate::runtime::RuntimeState;
use crate::telegram;

use super::installer;

/// Lua context object passed to every module handler.
///
/// Exposes Telegram actions and the database to Lua scripts.
#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub db: Arc<Database>,
    pub runtime: Arc<RuntimeState>,
    pub(super) modules_dir: std::path::PathBuf,
    /// The message that triggered the current handler.
    pub message: Arc<Mutex<Option<Message>>>,
}

impl Ctx {
    pub fn new(
        client: Client,
        db: Arc<Database>,
        runtime: Arc<RuntimeState>,
        modules_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            client,
            db,
            runtime,
            modules_dir,
            message: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_message(self, msg: Message) -> Self {
        Self {
            message: Arc::new(Mutex::new(Some(msg))),
            ..self
        }
    }
}

impl UserData for Ctx {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // Replies to the current message.
        methods.add_async_method("reply", |_, ctx, text: String| async move {
            let guard = ctx.message.lock().await;
            if let Some(msg) = guard.as_ref() {
                let peer_ref = telegram::resolve_message_peer(&ctx.client, msg)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                telegram::send_text(&ctx.client, &ctx.runtime, peer_ref, &text)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
            }
            Ok(())
        });

        // Edits the current message.
        methods.add_async_method("edit", |_, ctx, text: String| async move {
            let guard = ctx.message.lock().await;
            if let Some(msg) = guard.as_ref() {
                let peer_ref = telegram::resolve_message_peer(&ctx.client, msg)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                telegram::edit_or_send_text(&ctx.client, &ctx.runtime, peer_ref, msg.id(), &text)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
            }
            Ok(())
        });

        // Deletes the current message.
        methods.add_async_method("delete", |_, ctx, ()| async move {
            let guard = ctx.message.lock().await;
            if let Some(msg) = guard.as_ref() {
                let peer_ref = telegram::resolve_message_peer(&ctx.client, msg)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                telegram::delete_messages(&ctx.client, &ctx.runtime, peer_ref, &[msg.id()])
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
            }
            Ok(())
        });

        // Returns a value from the database.
        methods.add_async_method("db_get", |lua, ctx, key: String| async move {
            let value = ctx.db.get(&key).await;
            lua.to_value(&value)
        });

        // Stores a value in the database.
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
                installer::install_module(&ctx, source, name).await
            },
        );

        methods.add_async_method(
            "install_replied_module",
            |_, ctx, name: Option<String>| async move {
                installer::install_replied_module(ctx.clone(), name)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))
            },
        );

        methods.add_async_method(
            "install_plugin",
            |_, ctx, (source, name): (String, Option<String>)| async move {
                installer::install_module(&ctx, source, name).await
            },
        );

        methods.add_async_method(
            "install_replied_plugin",
            |_, ctx, name: Option<String>| async move {
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

        methods.add_async_method("run_term", |_, ctx, command: String| async move {
            run_shell_command(ctx.clone(), command)
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))
        });

        methods.add_async_method("update_project", |_, ctx, ()| async move {
            update_project(ctx.clone())
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))
        });

        methods.add_async_method("run_capture", |_, _ctx, command: String| async move {
            Ok(run_command_capture(&command).await.unwrap_or_default())
        });

        methods.add_async_method("delete_last_own", |_, ctx, count: u32| async move {
            delete_last_own_messages(ctx.clone(), count)
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))
        });

        methods.add_async_method("message_info", |_, ctx, ()| async move {
            build_message_info(ctx.clone())
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))
        });

        methods.add_async_method("sleep", |_, _, seconds: u64| async move {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            Ok(())
        });
    }
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

    let old_head = match run_command_capture("git rev-parse HEAD").await {
        Ok(h) => h,
        Err(e) => {
            let text = format!("**Update failed.**\n\n```text\n{e}\n```");
            edit_current_message(&ctx, &text).await?;
            return Ok(());
        }
    };

    let pull_output = match run_command_capture("git pull").await {
        Ok(o) => o,
        Err(e) => {
            let text = format!("**Pull failed.**\n\n```text\n{e}\n```");
            edit_current_message(&ctx, &sanitize_text(&ctx, &text).await).await?;
            return Ok(());
        }
    };

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
        match run_command_capture("cargo build --release").await {
            Ok(build_output) => {
                let text = format!(
                    "**Updated and built release.**\n\n```text\n{}\n```\n\n**Restarting...**",
                    sanitize_text(&ctx, &build_output).await
                );
                edit_current_message(&ctx, &text).await?;
                std::process::exit(0);
            }
            Err(e) => {
                let text = format!(
                    "**Build failed.**\n\n```text\n{}\n```",
                    sanitize_text(&ctx, &e.to_string()).await
                );
                edit_current_message(&ctx, &text).await?;
                return Ok(());
            }
        }
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
    let guard = ctx.message.lock().await;
    let Some(msg) = guard.as_ref() else {
        return Ok(());
    };
    let peer_ref = telegram::resolve_message_peer(&ctx.client, msg).await?;
    telegram::edit_or_send_text(&ctx.client, &ctx.runtime, peer_ref, msg.id(), text).await?;
    Ok(())
}

async fn edit_current_message_only(ctx: &Ctx, text: &str) -> anyhow::Result<()> {
    let guard = ctx.message.lock().await;
    let Some(msg) = guard.as_ref() else {
        return Ok(());
    };
    let peer_ref = telegram::resolve_message_peer(&ctx.client, msg).await?;
    telegram::edit_text(&ctx.client, &ctx.runtime, peer_ref, msg.id(), text).await?;
    Ok(())
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
        return Ok("**Message**  \nNo message context.".to_string());
    };
    let reply = msg.get_reply().await?;
    let target = reply.as_ref().unwrap_or(msg);

    let mut lines = Vec::new();
    lines.push("**Message info**".to_string());
    lines.push(format!(
        "**Chat ID:** `{}`",
        i64::from(telegram::resolve_message_peer(&ctx.client, msg).await?)
    ));
    lines.push(format!("**Message ID:** `{}`", target.id()));
    if let Some(sender_id) = target.sender_id() {
        lines.push(format!(
            "**Sender ID:** `{}`",
            i64::from(PeerRef {
                id: sender_id,
                auth: PeerAuth::default(),
            })
        ));
    }
    if target.media().is_some() {
        lines.push("**Media:** `yes`".to_string());
    }
    Ok(lines.join("  \n"))
}
