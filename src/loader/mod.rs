use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use grammers_client::update::Message;
use grammers_client::Client;
use grammers_session::types::PeerKind;
use mlua::{Function, Lua, Table};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

pub mod context;
mod installer;

use crate::database::Database;
use crate::runtime::RuntimeState;
use crate::telegram;
use context::Ctx;

/// A loaded Lua module and its registered command → handler mappings.
struct Module {
    table: Table,
    /// Command name (without prefix) → handler function name in the table.
    commands: HashMap<String, String>,
}

/// Manages loading, unloading, and dispatching to Lua modules.
pub struct Loader {
    lua: Arc<Lua>,
    db: Arc<Database>,
    runtime: Arc<RuntimeState>,
    modules_dir: PathBuf,
    modules: RwLock<HashMap<String, Module>>,
}

impl Loader {
    /// Creates a loader bound to the module directory.
    pub async fn new(
        db: Arc<Database>,
        runtime: Arc<RuntimeState>,
        modules_dir: impl AsRef<Path>,
    ) -> Result<Self> {
        Ok(Self {
            lua: Arc::new(Lua::new()),
            db,
            runtime,
            modules_dir: modules_dir.as_ref().to_path_buf(),
            modules: RwLock::new(HashMap::new()),
        })
    }

    /// Loads all `.lua` files from the module directory.
    pub async fn load_all(&self) -> Result<()> {
        if !self.modules_dir.exists() {
            tokio::fs::create_dir_all(&self.modules_dir).await?;
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(&self.modules_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "lua") {
                if let Err(e) = self.load_file(&path).await {
                    error!("failed to load {:?}: {e}", path);
                }
            }
        }

        Ok(())
    }

    /// Loads a single Lua file as a module.
    pub async fn load_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("invalid file name")?
            .to_string();

        let source = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("reading {path:?}"))?;

        let table: Table = self
            .lua
            .load(&source)
            .set_name(&name)
            .eval()
            .with_context(|| format!("executing {path:?}"))?;

        let commands = collect_commands(&table)?;

        info!("loaded module '{name}' with {} command(s)", commands.len());

        self.modules
            .write()
            .await
            .insert(name, Module { table, commands });

        Ok(())
    }

    /// Unloads a module by name.
    pub async fn unload(&self, name: &str) -> bool {
        let removed = self.modules.write().await.remove(name).is_some();
        if removed {
            info!("unloaded module '{name}'");
        } else {
            warn!("tried to unload unknown module '{name}'");
        }
        removed
    }

    /// Dispatches an incoming message to the matching command handler.
    pub async fn handle_message(&self, client: Client, msg: Message) -> Result<()> {
        if !is_own_command_message(&client, &msg).await {
            return Ok(());
        }

        let text = msg.text().to_string();

        // Command prefix is `.`.
        let Some(body) = text.strip_prefix('.') else {
            return Ok(());
        };

        let mut parts = body.splitn(2, ' ');
        let cmd = parts.next().unwrap_or("").to_lowercase();
        let args = parts.next().unwrap_or("").to_string();

        let ctx = Ctx::new(
            client.clone(),
            Arc::clone(&self.db),
            Arc::clone(&self.runtime),
            self.modules_dir.clone(),
        )
        .with_message(msg.clone());

        let modules = self.modules.read().await;

        for module in modules.values() {
            if let Some(handler_name) = module.commands.get(&cmd) {
                let handler: Function =
                    module.table.get(handler_name.as_str()).with_context(|| {
                        format!("handler '{handler_name}' not found in module table")
                    })?;

                handler
                    .call_async::<()>((ctx.clone(), args.clone()))
                    .await
                    .with_context(|| format!("handler '{handler_name}' returned error"))?;

                return Ok(());
            }
        }

        if self.handle_builtin_command(&client, &msg, &cmd).await? {
            return Ok(());
        }

        Ok(())
    }

    /// Returns loaded module names for dashboard display.
    pub async fn module_names(&self) -> Vec<String> {
        let mut names = self
            .modules
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    async fn handle_builtin_command(
        &self,
        client: &Client,
        msg: &Message,
        cmd: &str,
    ) -> Result<bool> {
        let text = match cmd {
            "ping" => Some("pong"),
            "help" => Some(BUILTIN_HELP),
            _ => None,
        };
        let Some(text) = text else {
            return Ok(false);
        };

        let peer_ref = telegram::resolve_message_peer(client, msg).await?;
        telegram::edit_or_send_text(client, &self.runtime, peer_ref, msg.id(), text).await?;

        Ok(true)
    }
}

async fn is_own_command_message(client: &Client, msg: &Message) -> bool {
    if msg.outgoing() || matches!(msg.peer_id().kind(), PeerKind::UserSelf) {
        return true;
    }

    let Some(sender_id) = msg.sender_id() else {
        return false;
    };

    client
        .get_me()
        .await
        .is_ok_and(|user| user.id() == sender_id)
}

const BUILTIN_HELP: &str = r#"fly-telegram

Base commands:
.ping - check command handling
.help - show this help
.install <file-or-url> [name] - install a Lua module
.install as reply - install the replied .lua module
.note set|get|clear - manage a saved note
.alias set|get|del - manage text aliases
.del <count> - delete recent messages
.id - show chat/message/user IDs
.sd <seconds> <text> - self-destruct message
.ytdl <url> - run yt-dlp for a media URL
.afk on [text]|off - auto-reply when mentioned
.autoread on|off - mark incoming messages as read
.antidelete on|off - log deleted cached messages
.eval <code> - evaluate Lua code
.term <command> - run a shell command

Module install examples:
.install C:\path\module.lua
.install https://example.com/module.lua module_name

Installed modules are saved into modules/ and hot-loaded by the watcher."#;

/// Reads `module.commands = { cmd = "handler_fn" }` from a module table.
fn collect_commands(table: &Table) -> Result<HashMap<String, String>> {
    let Ok(cmds_table) = table.get::<Table>("commands") else {
        return Ok(HashMap::new());
    };

    let mut map = HashMap::new();
    for pair in cmds_table.pairs::<String, String>() {
        let (cmd, handler) = pair.context("invalid entry in commands table")?;
        map.insert(cmd, handler);
    }
    Ok(map)
}
