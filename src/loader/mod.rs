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
pub mod manifest;

use ed25519_dalek::VerifyingKey;

use crate::database::Database;
use crate::runtime::RuntimeState;
use crate::{config, crypto, telegram};
use context::Ctx;
use manifest::{ModuleInfo, ModuleManifest};

/// A loaded Lua module and its registered command → handler mappings.
struct Module {
    table: Table,
    /// Command name (without prefix) → handler function name in the table.
    commands: HashMap<String, String>,
    manifest: ModuleManifest,
    source: String,
}

/// Manages loading, unloading, and dispatching to Lua modules.
pub struct Loader {
    lua: Arc<Lua>,
    db: Arc<Database>,
    runtime: Arc<RuntimeState>,
    modules_dir: PathBuf,
    modules: RwLock<HashMap<String, Module>>,
    verifying_key: Option<VerifyingKey>,
}

impl Loader {
    /// Creates a loader bound to the module directory.
    pub async fn new(
        db: Arc<Database>,
        runtime: Arc<RuntimeState>,
        modules_dir: impl AsRef<Path>,
    ) -> Result<Self> {
        let verifying_key =
            crypto::load_verifying_key(Path::new(config::SIGNING_PUB_KEY_FILE)).unwrap_or_else(
                |e| {
                    warn!("could not load signing public key: {e}");
                    None
                },
            );
        Ok(Self {
            lua: Arc::new(Lua::new()),
            db,
            runtime,
            modules_dir: modules_dir.as_ref().to_path_buf(),
            modules: RwLock::new(HashMap::new()),
            verifying_key,
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

        let static_commands = manifest::module_commands(&source);
        let manifest = manifest::load_manifest(
            path,
            &name,
            static_commands,
            &source,
            self.verifying_key.as_ref(),
        )
        .await?;
        let table: Table = if manifest.trusted {
            self.lua
                .load(&source)
                .set_name(&name)
                .eval()
                .with_context(|| format!("executing {path:?}"))?
        } else {
            self.lua
                .load(&source)
                .set_name(&name)
                .set_environment(sandbox_environment(&self.lua)?)
                .eval()
                .with_context(|| format!("executing sandboxed {path:?}"))?
        };
        let commands = collect_commands(&table)?;

        info!("loaded module '{name}' with {} command(s)", commands.len());

        self.modules.write().await.insert(
            name,
            Module {
                table,
                commands,
                manifest,
                source,
            },
        );

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

        let modules = self.modules.read().await;

        for module in modules.values() {
            if let Some(handler_name) = module.commands.get(&cmd) {
                let ctx = Ctx::new(
                    client.clone(),
                    Arc::clone(&self.db),
                    Arc::clone(&self.runtime),
                    self.modules_dir.clone(),
                    module.manifest.name.clone(),
                    module.manifest.permissions.clone(),
                    module.manifest.trusted,
                )
                .with_message(msg.clone());
                let handler: Function =
                    module.table.get(handler_name.as_str()).with_context(|| {
                        format!("handler '{handler_name}' not found in module table")
                    })?;

                if let Err(e) = handler
                    .call_async::<()>((ctx.clone(), args.clone()))
                    .await
                {
                    let err_text = format!(
                        "**Error** `{}.{handler_name}`\n\n```text\n{e}\n```",
                        module.manifest.name
                    );
                    error!("loader: module '{}' handler '{handler_name}': {e}", module.manifest.name);
                    let _ = telegram::msg_edit_or_respond(&self.runtime, &msg, &err_text).await;
                }

                return Ok(());
            }
        }

        if self.handle_builtin_command(&msg, &cmd).await? {
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

    /// Returns dashboard-ready module metadata.
    pub async fn module_info(&self) -> Vec<ModuleInfo> {
        let mut modules = self
            .modules
            .read()
            .await
            .values()
            .map(|module| manifest::module_info(&module.manifest, &module.commands, &module.source))
            .collect::<Vec<_>>();
        modules.sort_by(|left, right| left.name.cmp(&right.name));
        modules
    }

    async fn handle_builtin_command(&self, msg: &Message, cmd: &str) -> Result<bool> {
        let text = match cmd {
            "ping" => Some("pong"),
            "help" => Some(BUILTIN_HELP),
            _ => None,
        };
        let Some(text) = text else {
            return Ok(false);
        };

        telegram::msg_edit_or_respond(&self.runtime, msg, text).await?;
        Ok(true)
    }
}

fn sandbox_environment(lua: &Lua) -> Result<Table> {
    let globals = lua.globals();
    let env = lua.create_table()?;
    for name in [
        "assert", "error", "ipairs", "next", "pairs", "pcall", "select", "tonumber", "tostring",
        "type", "xpcall",
    ] {
        env.set(name, globals.get::<mlua::Value>(name)?)?;
    }
    for name in ["coroutine", "math", "string", "table", "utf8"] {
        env.set(name, globals.get::<Table>(name)?)?;
    }
    env.set("_G", env.clone())?;
    Ok(env)
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

const BUILTIN_HELP: &str = r#"**✈️ fly-telegram**

**▸ Core**
`.ping` — connectivity check
`.help` — this message
`.eval <code>` — evaluate Lua expression
`.term <cmd>` — run a shell command

**▸ Messaging**
`.note set|get|clear` — saved note
`.alias set|get|del` — text aliases
`.del <n>` — delete last N messages
`.sd <sec> <text>` — self-destruct message

**▸ Modules**
`.install <path-or-url> [name]` — install from file or URL
`.install` _(reply to .lua)_ — install replied module
`.market search|info|install` — module marketplace

**▸ Info & OSINT**
`.info` — chat / user / DC / group info
`.ip <ip>` · `.domain <d>` · `.rdap <d>` — OSINT lookups
`.ytdl <url>` — run yt-dlp

**▸ Files**
`.dl` · `.sendfile` · `.urlupload` · `.rename`

**▸ AI**
`.ai provider <name>` — set active AI provider
`.ask` · `.summarize` · `.translate` · `.transcribe`

**▸ Automation**
`.afk on [text]|off` — away mode with auto-reply
`.autoread on|off` — mark incoming messages as read
`.antidelete on|off` — log deleted cached messages
`.pmguard on|off|status` — private message guard

**▸ Groups**
`.cleanjoins on|off|status` — remove join messages
`.captcha on|off|status` — join captcha

**▸ Music** _(external worker)_
`.play <query>` · `.queue` · `.skip` · `.stop`

**▸ Fun**
`.type` · `.scroll` · `.magic` · `.heart [text]` — text animations
`.gifts` · `.taskbot` — dry-run hooks

_Modules are saved to_ `modules/` _and hot-reloaded on save._"#;

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
