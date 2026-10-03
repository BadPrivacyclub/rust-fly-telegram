use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use grammers_client::update::Message;
use grammers_client::Client;
use grammers_session::types::{PeerId, PeerKind};
use mlua::{Function, Lua, Table};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{OnceCell, RwLock};
use tracing::{error, info, warn};

pub mod context;
pub mod installer;
pub mod manifest;
pub mod meta;

use ed25519_dalek::VerifyingKey;

use crate::app::Services;
use crate::core_settings;
use crate::i18n::{self, Lang};
use crate::notify::{escape_html, Topic};
use crate::{config, crypto, telegram};
use context::Ctx;
use manifest::{ModuleInfo, ModuleManifest};
use meta::{ConfigField, ModuleHelp};

pub(crate) const UI_LIBRARY: &str = include_str!("ui.lua");

/// Permission required for a sandboxed module to receive `on_message` events.
pub const READ_PERMISSION: &str = "telegram.read";

struct Module {
    table: Table,
    commands: HashMap<String, String>,
    manifest: ModuleManifest,
    source: String,
    file_name: String,
    help: ModuleHelp,
    config: Vec<ConfigField>,
    has_on_message: bool,
}

/// What a handler needs to know about the account that received the message.
#[derive(Clone)]
pub struct Account {
    pub client: Client,
    pub session_file: String,
    pub own_user_id: Arc<OnceCell<PeerId>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModuleSummary {
    #[serde(flatten)]
    pub info: ModuleInfo,
    pub file_name: String,
    pub enabled: bool,
    pub bundled: bool,
    pub category: String,
    pub help: ModuleHelp,
    pub config: Vec<ConfigEntry>,
    pub events: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConfigEntry {
    #[serde(flatten)]
    pub field: ConfigField,
    /// Current value; secrets are never sent back, only whether they are set.
    pub value: Value,
    pub is_set: bool,
}

pub struct Loader {
    lua: Arc<Lua>,
    pub services: Arc<Services>,
    modules_dir: PathBuf,
    modules: RwLock<HashMap<String, Module>>,
    verifying_key: Option<VerifyingKey>,
    ui: Table,
}

impl Loader {
    pub async fn new(services: Arc<Services>, modules_dir: impl AsRef<Path>) -> Result<Self> {
        let verifying_key = crypto::load_verifying_key(Path::new(config::SIGNING_PUB_KEY_FILE))
            .unwrap_or_else(|e| {
                warn!("could not load signing public key: {e}");
                None
            });
        let lua = Lua::new();
        let ui: Table = lua
            .load(UI_LIBRARY)
            .set_name("ui")
            .eval()
            .context("loading ui library")?;
        lua.globals().set("ui", ui.clone())?;
        let json = json_library(&lua)?;
        lua.globals().set("json", json.clone())?;
        ui.set("__json", json)?;
        Ok(Self {
            lua: Arc::new(lua),
            services,
            modules_dir: modules_dir.as_ref().to_path_buf(),
            modules: RwLock::new(HashMap::new()),
            verifying_key,
            ui,
        })
    }

    pub fn modules_dir(&self) -> &Path {
        &self.modules_dir
    }

    pub async fn load_all(&self) -> Result<()> {
        let abs = self
            .modules_dir
            .canonicalize()
            .unwrap_or_else(|_| self.modules_dir.clone());
        info!("loading modules from {abs:?}");

        if !self.modules_dir.exists() {
            warn!("modules directory {abs:?} does not exist, creating it");
            tokio::fs::create_dir_all(&self.modules_dir).await?;
            return Ok(());
        }

        let mut paths = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.modules_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "lua") {
                paths.push(path);
            }
        }
        paths.sort();
        for path in paths {
            if let Err(e) = self.load_file(&path).await {
                error!("failed to load {:?}: {e:#}", path);
            }
        }

        let count = self.modules.read().await.len();
        info!("loaded {count} module(s)");
        Ok(())
    }

    pub async fn load_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("invalid file name")?
            .to_string();
        let file_name = path
            .file_name()
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
            crate::bundled::is_pristine(&file_name, &source),
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
                .set_environment(sandbox_environment(&self.lua, &self.ui)?)
                .eval()
                .with_context(|| format!("executing sandboxed {path:?}"))?
        };
        let commands = collect_commands(&table)?;
        let mut command_names = commands.keys().cloned().collect::<Vec<_>>();
        command_names.sort();
        let help = meta::parse_help(&self.lua, &table, &command_names);
        let config = meta::parse_config(&self.lua, &table);
        let has_on_message = table.get::<Function>("on_message").is_ok();
        if has_on_message
            && !manifest.trusted
            && !manifest.permissions.iter().any(|p| p == READ_PERMISSION)
        {
            warn!(
                "module '{name}' defines on_message but lacks the '{READ_PERMISSION}' permission; events are not delivered"
            );
        }

        info!(
            "loaded module '{name}' with {} command(s){}",
            commands.len(),
            if manifest.trusted { "" } else { " (sandboxed)" }
        );

        self.modules.write().await.insert(
            name,
            Module {
                table,
                commands,
                manifest,
                source,
                file_name,
                help,
                config,
                has_on_message,
            },
        );

        Ok(())
    }

    pub async fn unload(&self, name: &str) -> bool {
        let removed = self.modules.write().await.remove(name).is_some();
        if removed {
            info!("unloaded module '{name}'");
        }
        removed
    }

    pub async fn reload(&self, name: &str) -> Result<()> {
        let file_name = self
            .modules
            .read()
            .await
            .get(name)
            .map(|module| module.file_name.clone())
            .unwrap_or_else(|| format!("{name}.lua"));
        let path = self.modules_dir.join(file_name);
        if !path.exists() {
            anyhow::bail!("module file {path:?} does not exist");
        }
        self.load_file(&path).await
    }

    /// Deletes a module file and its manifest. Bundled modules come back on the next start
    /// unless they are disabled, so the panel offers "disable" for those instead.
    pub async fn remove(&self, name: &str) -> Result<()> {
        let file_name = self
            .modules
            .read()
            .await
            .get(name)
            .map(|module| module.file_name.clone())
            .with_context(|| format!("unknown module '{name}'"))?;
        let path = self.modules_dir.join(&file_name);
        self.unload(name).await;
        let _ = tokio::fs::remove_file(manifest::manifest_path(&path)).await;
        tokio::fs::remove_file(&path)
            .await
            .with_context(|| format!("removing {path:?}"))?;
        Ok(())
    }

    pub async fn source(&self, name: &str) -> Option<String> {
        self.modules
            .read()
            .await
            .get(name)
            .map(|module| module.source.clone())
    }

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

    pub async fn summaries(&self) -> Vec<ModuleSummary> {
        let disabled = core_settings::disabled_modules(&self.services.db).await;
        let mut summaries = Vec::new();
        let modules = self.modules.read().await;
        for (name, module) in modules.iter() {
            let mut config = Vec::new();
            for field in &module.config {
                let stored = self.services.db.get(&config_key(name, &field.key)).await;
                let is_set = !stored.is_null();
                let value = if field.secret {
                    Value::Null
                } else if is_set {
                    stored
                } else {
                    field.default.clone()
                };
                config.push(ConfigEntry {
                    field: field.clone(),
                    value,
                    is_set,
                });
            }
            summaries.push(ModuleSummary {
                info: manifest::module_info(&module.manifest, &module.commands, &module.source),
                file_name: module.file_name.clone(),
                enabled: !disabled.contains(name),
                bundled: crate::bundled::bundled_source(&module.file_name).is_some(),
                category: module.help.category.clone(),
                help: module.help.clone(),
                config,
                events: module.has_on_message,
            });
        }
        summaries.sort_by(|a, b| {
            i18n::category_rank(&a.category)
                .cmp(&i18n::category_rank(&b.category))
                .then(a.info.name.cmp(&b.info.name))
        });
        summaries
    }

    /// Grants permissions to a sandboxed module by rewriting its manifest, then reloads it.
    pub async fn set_permissions(&self, name: &str, permissions: Vec<String>) -> Result<()> {
        let file_name = self
            .modules
            .read()
            .await
            .get(name)
            .map(|module| module.file_name.clone())
            .with_context(|| format!("unknown module '{name}'"))?;
        let mut permissions = permissions
            .into_iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>();
        if let Some(unknown) = permissions
            .iter()
            .find(|p| !manifest::KNOWN_PERMISSIONS.contains(&p.as_str()))
        {
            anyhow::bail!("unknown permission '{unknown}'");
        }
        permissions.sort();
        permissions.dedup();

        let module_path = self.modules_dir.join(&file_name);
        let manifest_path = manifest::manifest_path(&module_path);
        let mut manifest = if manifest_path.exists() {
            let raw = tokio::fs::read_to_string(&manifest_path).await?;
            serde_json::from_str::<ModuleManifest>(&raw)?
        } else {
            let source = tokio::fs::read_to_string(&module_path).await?;
            ModuleManifest::inferred(
                name.to_string(),
                manifest::module_commands(&source),
                &source,
            )
        };
        manifest.permissions = permissions;
        tokio::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?).await?;
        self.load_file(&module_path).await
    }

    pub async fn set_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        if !self.modules.read().await.contains_key(name) {
            anyhow::bail!("unknown module '{name}'");
        }
        core_settings::set_module_enabled(&self.services.db, name, enabled).await
    }

    pub async fn config_fields(&self, module: &str) -> Option<Vec<ConfigField>> {
        self.modules
            .read()
            .await
            .get(module)
            .map(|module| module.config.clone())
    }

    pub async fn config_value(&self, module: &str, key: &str) -> Value {
        let stored = self.services.db.get(&config_key(module, key)).await;
        if !stored.is_null() {
            return stored;
        }
        self.modules
            .read()
            .await
            .get(module)
            .and_then(|module| module.config.iter().find(|field| field.key == key))
            .map(|field| field.default.clone())
            .unwrap_or(Value::Null)
    }

    /// Validates and stores a setting. `Value::Null` resets it to the default.
    pub async fn set_config_value(&self, module: &str, key: &str, raw: Value) -> Result<Value> {
        let field = self
            .config_fields(module)
            .await
            .with_context(|| format!("unknown module '{module}'"))?
            .into_iter()
            .find(|field| field.key == key)
            .with_context(|| format!("module '{module}' has no setting '{key}'"))?;
        let db_key = config_key(module, key);
        if raw.is_null()
            || raw.as_str().is_some_and(|text| {
                text.trim().is_empty()
                    && field.kind != meta::FieldKind::Text
                    && field.kind != meta::FieldKind::String
            })
        {
            self.services.db.remove(&db_key).await?;
            return Ok(field.default);
        }
        let value = meta::coerce(&field, &raw)?;
        self.services.db.set(db_key, value.clone()).await?;
        Ok(value)
    }

    pub async fn help_text(&self, query: &str, lang: Lang, prefix: &str) -> String {
        let summaries = self.summaries().await;
        let query = query.trim().trim_start_matches(prefix).to_lowercase();
        if query.is_empty() {
            return render_help_overview(&summaries, lang, prefix);
        }
        let found = summaries
            .iter()
            .find(|module| module.info.name.to_lowercase() == query)
            .or_else(|| {
                summaries
                    .iter()
                    .find(|module| module.info.commands.contains(&query))
            });
        match found {
            Some(module) => render_module_card(module, lang, prefix, Some(&query)),
            None => format!(
                "❔ **{}** `{}`\n\n__{}__",
                i18n::tr(lang, "help.not_found"),
                query.replace('`', "'"),
                i18n::tr(lang, "help.footer").replace("{p}", prefix)
            ),
        }
    }

    pub async fn handle_message(self: &Arc<Self>, account: Account, msg: Message) -> Result<()> {
        let db = &self.services.db;
        let own = is_own_command_message(&account.client, &msg, &account.own_user_id).await;
        let sudo = !own
            && msg
                .sender_id()
                .is_some_and(|id| matches!(id.kind(), PeerKind::User))
            && {
                let sender = msg.sender_id().map(|id| id.bot_api_dialog_id());
                let users = core_settings::sudo_users(db).await;
                sender.is_some_and(|id| users.contains(&id))
            };
        if !own && !sudo {
            return Ok(());
        }

        let prefixes = core_settings::prefixes(db).await;
        let Some((cmd, args)) = core_settings::parse_command(msg.text(), &prefixes) else {
            return Ok(());
        };

        let disabled = core_settings::disabled_modules(db).await;
        let target = {
            let modules = self.modules.read().await;
            modules
                .iter()
                .filter(|(name, _)| !disabled.contains(name))
                .find_map(|(_, module)| {
                    module.commands.get(&cmd).map(|handler| {
                        (
                            module.table.clone(),
                            handler.clone(),
                            module.manifest.clone(),
                        )
                    })
                })
        };

        let lang = core_settings::language(db).await;
        if sudo && core_settings::SUDO_DENYLIST.contains(&cmd.as_str()) {
            if target.is_some() {
                let text = format!("⛔ {}", i18n::tr(lang, "error.sudo_denied"));
                telegram::msg_respond(&self.services.runtime, &msg, &text).await?;
            }
            return Ok(());
        }

        let Some((table, handler_name, manifest)) = target else {
            if cmd == "help" {
                // Fallback when the help module is missing or disabled.
                let text = self.help_text(&args, lang, &prefixes[0]).await;
                telegram::msg_edit_or_respond(&self.services.runtime, &msg, &text).await?;
            }
            return Ok(());
        };

        let runtime = &self.services.runtime;
        runtime.record_account_command(&account.session_file).await;
        runtime.record_command_name(&cmd);

        let ctx = Ctx::new(Arc::clone(self), &account, &manifest).with_message(msg.clone());
        let handler: Function = table
            .get(handler_name.as_str())
            .with_context(|| format!("handler '{handler_name}' not found in module table"))?;

        if let Err(e) = handler.call_async::<()>((ctx, args)).await {
            let message = clean_lua_error(&e);
            runtime.record_error();
            error!(
                "module '{}' handler '{handler_name}': {message}",
                manifest.name
            );
            let err_text = format!(
                "❌ **{}** `{}.{handler_name}`\n\n```text\n{}\n```",
                i18n::tr(lang, "error.title"),
                manifest.name,
                message.replace("```", "'''")
            );
            let _ = telegram::msg_edit_or_respond(runtime, &msg, &err_text).await;
            self.services
                .notifier
                .send(
                    Topic::Errors,
                    &format!(
                        "❌ <b>{}</b> <code>{}.{}</code>\n<pre>{}</pre>",
                        i18n::tr(lang, "bot.module_error"),
                        escape_html(&manifest.name),
                        escape_html(&handler_name),
                        escape_html(&crate::notify::clip(&message, 1500))
                    ),
                )
                .await;
        }

        Ok(())
    }

    /// Delivers a message to every enabled module that defines `on_message`.
    pub async fn dispatch_message_event(self: &Arc<Self>, account: Account, msg: Message) {
        let disabled = core_settings::disabled_modules(&self.services.db).await;
        let listeners = {
            let modules = self.modules.read().await;
            modules
                .iter()
                .filter(|(name, module)| {
                    module.has_on_message
                        && !disabled.contains(name)
                        && (module.manifest.trusted
                            || module
                                .manifest
                                .permissions
                                .iter()
                                .any(|p| p == READ_PERMISSION))
                })
                .filter_map(|(_, module)| {
                    module
                        .table
                        .get::<Function>("on_message")
                        .ok()
                        .map(|handler| (handler, module.manifest.clone()))
                })
                .collect::<Vec<_>>()
        };
        if listeners.is_empty() {
            return;
        }

        let prefixes = core_settings::prefixes(&self.services.db).await;
        let is_command = core_settings::parse_command(msg.text(), &prefixes).is_some();
        let own_id = account.own_user_id.get().copied();
        for (handler, manifest) in listeners {
            let ctx = Ctx::new(Arc::clone(self), &account, &manifest).with_message(msg.clone());
            let event = match context::message_table(&self.lua, &msg, own_id, is_command) {
                Ok(event) => event,
                Err(e) => {
                    warn!("could not build message event: {e}");
                    return;
                }
            };
            if let Err(e) = handler.call_async::<()>((ctx, event)).await {
                self.services.runtime.record_error();
                warn!(
                    "module '{}' on_message: {}",
                    manifest.name,
                    clean_lua_error(&e)
                );
            }
        }
    }
}

pub fn config_key(module: &str, key: &str) -> String {
    format!("cfg.{module}.{key}")
}

fn clean_lua_error(error: &mlua::Error) -> String {
    let text = error.to_string();
    // Drop the Rust-side stack noise mlua appends to callback errors.
    text.split("\nstack traceback:")
        .next()
        .unwrap_or(&text)
        .trim()
        .to_string()
}

fn render_help_overview(summaries: &[ModuleSummary], lang: Lang, prefix: &str) -> String {
    let enabled_commands = summaries
        .iter()
        .filter(|module| module.enabled)
        .map(|module| module.info.commands.len())
        .sum::<usize>();
    let mut out = format!(
        "✈️ **fly-telegram** `v{}`  \n`{}` {} · `{}` {} · {} `{}`",
        crate::VERSION,
        summaries.len(),
        i18n::tr(lang, "help.modules"),
        enabled_commands,
        i18n::tr(lang, "help.commands"),
        i18n::tr(lang, "help.prefix"),
        prefix
    );

    let mut current_category: Option<String> = None;
    for module in summaries {
        let category = i18n::category_title(lang, &module.category);
        if current_category.as_deref() != Some(category.as_str()) {
            out.push_str(&format!("\n\n**▸ {category}**"));
            current_category = Some(category);
        }
        let commands = module
            .info
            .commands
            .iter()
            .map(|command| format!("`{prefix}{command}`"))
            .collect::<Vec<_>>()
            .join(" ");
        let name = if module.enabled {
            format!("**{}**", module.info.name)
        } else {
            format!("~~{}~~", module.info.name)
        };
        if commands.is_empty() {
            out.push_str(&format!("  \n{name}"));
        } else {
            out.push_str(&format!("  \n{name}: {commands}"));
        }
    }
    out.push_str(&format!(
        "\n\n__{}__",
        i18n::tr(lang, "help.footer").replace("{p}", prefix)
    ));
    out
}

fn render_module_card(
    module: &ModuleSummary,
    lang: Lang,
    prefix: &str,
    highlight: Option<&str>,
) -> String {
    let trust = if module.info.trusted {
        i18n::tr(lang, "help.trusted")
    } else {
        i18n::tr(lang, "help.sandboxed")
    };
    let mut out = format!(
        "🧩 **{}** `v{}` · {}{}",
        module.info.name,
        module.info.version,
        trust,
        if module.enabled {
            String::new()
        } else {
            format!(" · ⚪ {}", i18n::tr(lang, "help.disabled"))
        }
    );
    let description = if !module.help.description.is_empty() {
        module.help.description.as_str()
    } else if !module.info.description.is_empty() {
        module.info.description.as_str()
    } else {
        i18n::tr(lang, "help.no_description")
    };
    out.push_str(&format!("  \n__{description}__\n"));

    for command in &module.help.commands {
        let marker = if highlight == Some(command.name.as_str()) {
            "▶"
        } else {
            "▸"
        };
        let usage = if command.args.is_empty() {
            format!("`{prefix}{}`", command.name)
        } else {
            format!(
                "`{prefix}{} {}`",
                command.name,
                command.args.replace('`', "'")
            )
        };
        if command.description.is_empty() {
            out.push_str(&format!("  \n{marker} {usage}"));
        } else {
            out.push_str(&format!("  \n{marker} {usage}: {}", command.description));
        }
    }

    if !module.info.permissions.is_empty() {
        out.push_str(&format!(
            "\n\n🔐 {}: `{}`",
            i18n::tr(lang, "help.permissions"),
            module.info.permissions.join(", ")
        ));
    }
    if !module.config.is_empty() {
        out.push_str(&format!(
            "\n⚙️ {}: `{prefix}cfg {}`",
            i18n::tr(lang, "help.settings"),
            module.info.name
        ));
    }
    out
}

fn sandbox_environment(lua: &Lua, ui: &Table) -> Result<Table> {
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
    env.set("ui", ui.clone())?;
    env.set("json", ui.get::<Table>("__json")?)?;
    env.set("_G", env.clone())?;
    Ok(env)
}

/// `json.encode(value)` and `json.decode(text)` for modules; safer than hand-built JSON.
pub(crate) fn json_library(lua: &Lua) -> Result<Table> {
    use mlua::LuaSerdeExt;
    let json = lua.create_table()?;
    json.set(
        "encode",
        lua.create_function(|lua, value: mlua::Value| {
            let value: Value = lua.from_value(value)?;
            serde_json::to_string(&value).map_err(mlua::Error::external)
        })?,
    )?;
    json.set(
        "decode",
        lua.create_function(|lua, text: String| {
            let value: Value = serde_json::from_str(&text).map_err(mlua::Error::external)?;
            lua.to_value(&value)
        })?,
    )?;
    Ok(json)
}

async fn is_own_command_message(
    client: &Client,
    msg: &Message,
    own_user_id: &OnceCell<PeerId>,
) -> bool {
    if msg.outgoing() || matches!(msg.peer_id().kind(), PeerKind::UserSelf) {
        return true;
    }

    let Some(sender_id) = msg.sender_id() else {
        return false;
    };

    own_user_id
        .get_or_try_init(|| async { client.get_me().await.map(|user| user.id()) })
        .await
        .is_ok_and(|own_user_id| *own_user_id == sender_id)
}

fn collect_commands(table: &Table) -> Result<HashMap<String, String>> {
    let Ok(cmds_table) = table.get::<Table>("commands") else {
        return Ok(HashMap::new());
    };

    let mut map = HashMap::new();
    for pair in cmds_table.pairs::<String, String>() {
        let (cmd, handler) = pair.context("invalid entry in commands table")?;
        map.insert(cmd.to_lowercase(), handler);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn loader_with(files: &[(&str, &str)]) -> (Arc<Loader>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("fly_loader_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, content) in files {
            std::fs::write(dir.join(name), content).unwrap();
        }
        let services = crate::app::test_services().await;
        let loader = Arc::new(Loader::new(services, &dir).await.unwrap());
        loader.load_all().await.unwrap();
        (loader, dir)
    }

    const NOTES: &str = r#"
local M = {}
M.commands = { note = "note_cmd" }
M.help = { category = "messaging", description = "Saved notes",
  commands = { note = { args = "set <text>", desc = "Save a note" } } }
M.config = { { key = "limit", type = "number", default = 3 } }
function M.note_cmd(ctx, args) return ui.ok("x") end
return M
"#;

    #[tokio::test]
    async fn help_lists_modules_and_cards() {
        let (loader, dir) = loader_with(&[("notes.lua", NOTES)]).await;
        let overview = loader.help_text("", Lang::En, ".").await;
        assert!(overview.contains("**notes**: `.note`"), "{overview}");
        assert!(overview.contains("Messaging"));

        let card = loader.help_text("note", Lang::En, "!").await;
        assert!(card.contains("`!note set <text>`: Save a note"), "{card}");
        assert!(card.contains("`!cfg notes`"));

        let missing = loader.help_text("nope", Lang::Ru, ".").await;
        assert!(missing.contains("Ничего не найдено"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn config_values_validate_and_reset() {
        let (loader, dir) = loader_with(&[("notes.lua", NOTES)]).await;
        assert_eq!(loader.config_value("notes", "limit").await, Value::from(3));
        loader
            .set_config_value("notes", "limit", Value::String("7".into()))
            .await
            .unwrap();
        assert_eq!(loader.config_value("notes", "limit").await, Value::from(7));
        assert!(loader
            .set_config_value("notes", "limit", Value::String("x".into()))
            .await
            .is_err());
        assert!(loader
            .set_config_value("notes", "missing", Value::Null)
            .await
            .is_err());
        loader
            .set_config_value("notes", "limit", Value::Null)
            .await
            .unwrap();
        assert_eq!(loader.config_value("notes", "limit").await, Value::from(3));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn sandboxed_modules_get_ui_but_not_os() {
        let (loader, dir) = loader_with(&[(
            "probe.lua",
            r#"local M = {} M.commands = {} M.has_ui = ui ~= nil M.has_os = os ~= nil return M"#,
        )])
        .await;
        let modules = loader.modules.read().await;
        let table = &modules["probe"].table;
        assert!(table.get::<bool>("has_ui").unwrap());
        assert!(!table.get::<bool>("has_os").unwrap());
        drop(modules);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn disabled_modules_are_marked() {
        let (loader, dir) = loader_with(&[("notes.lua", NOTES)]).await;
        loader.set_enabled("notes", false).await.unwrap();
        let summaries = loader.summaries().await;
        assert!(!summaries[0].enabled);
        assert!(loader.set_enabled("ghost", false).await.is_err());
        let overview = loader.help_text("", Lang::En, ".").await;
        assert!(overview.contains("~~notes~~"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn json_library_round_trips() {
        let (loader, dir) = loader_with(&[(
            "j.lua",
            r#"local M = {} M.commands = {}
               M.out = json.encode({ a = 1, b = "x\"y" })
               M.back = json.decode('{"n": [1, 2]}').n[2]
               return M"#,
        )])
        .await;
        let modules = loader.modules.read().await;
        let table = &modules["j"].table;
        let out: String = table.get("out").unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["b"], "x\"y");
        assert_eq!(table.get::<i64>("back").unwrap(), 2);
        drop(modules);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn bundled_modules_load_trusted_with_help() {
        let dir = std::env::temp_dir().join(format!("fly_bundle_load_{}", uuid::Uuid::new_v4()));
        crate::bundled::sync(&dir).unwrap();
        let services = crate::app::test_services().await;
        let loader = Arc::new(Loader::new(services, &dir).await.unwrap());
        loader.load_all().await.unwrap();
        let summaries = loader.summaries().await;
        let expected = crate::bundled::BUNDLED_FILES
            .iter()
            .filter(|(name, _)| name.ends_with(".lua"))
            .count();
        assert_eq!(summaries.len(), expected);
        for module in &summaries {
            assert!(
                module.info.trusted,
                "{} should be trusted",
                module.info.name
            );
            assert!(
                !module.help.description.is_empty(),
                "{} lacks help",
                module.info.name
            );
            assert!(
                !module.category.is_empty(),
                "{} lacks a category",
                module.info.name
            );
        }
        let help = loader.help_text("", Lang::En, ".").await;
        assert!(
            help.contains("`.remind`") && help.contains("`.cfg`"),
            "{help}"
        );
        // Commands must be unique across bundled modules.
        let mut seen = std::collections::HashSet::new();
        for module in &summaries {
            for command in &module.info.commands {
                assert!(seen.insert(command.clone()), "duplicate command {command}");
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn calculator_is_correct_and_cannot_run_code() {
        let lua = Lua::new();
        let ui: Table = lua.load(UI_LIBRARY).eval().unwrap();
        lua.globals().set("ui", ui).unwrap();
        let utils: Table = lua
            .load(crate::bundled::bundled_source("utils.lua").unwrap())
            .eval()
            .unwrap();
        let eval: Function = utils.get("_evaluate").unwrap();
        let num = |expr: &str| eval.call::<f64>(expr).unwrap();
        assert_eq!(num("2 * (3 + 4) ^ 2"), 98.0);
        assert_eq!(num("-2 ^ 2"), -4.0);
        assert_eq!(num("2 ^ -1"), 0.5);
        assert_eq!(num("10 % 4 + 1 / 2"), 2.5);
        assert!((num("sqrt(2) * sqrt(2)") - 2.0).abs() < 1e-9);
        assert!((num("pi") - std::f64::consts::PI).abs() < 1e-12);
        for bad in ["os.exit()", "1 +", "(1", "print(1)", "2 3"] {
            assert!(eval.call::<f64>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn base64_helpers_round_trip() {
        let lua = Lua::new();
        let ui: Table = lua.load(UI_LIBRARY).eval().unwrap();
        lua.globals().set("ui", ui).unwrap();
        let extras: Table = lua
            .load(crate::bundled::bundled_source("extras.lua").unwrap())
            .eval()
            .unwrap();
        let encode: Function = extras.get("_b64_encode").unwrap();
        let decode: Function = extras.get("_b64_decode").unwrap();
        use base64::Engine;
        for sample in ["", "f", "fo", "foo", "hello world", "привет"] {
            let expected = base64::engine::general_purpose::STANDARD.encode(sample);
            assert_eq!(encode.call::<String>(sample).unwrap(), expected, "{sample}");
            assert_eq!(decode.call::<String>(expected).unwrap(), sample);
        }
    }

    #[test]
    fn ui_library_helpers_work() {
        let lua = Lua::new();
        let ui: Table = lua.load(UI_LIBRARY).eval().unwrap();
        let parse: Function = ui.get("parse_duration").unwrap();
        assert_eq!(parse.call::<Option<i64>>("1h30m").unwrap(), Some(5400));
        assert_eq!(parse.call::<Option<i64>>("45").unwrap(), Some(45));
        assert_eq!(parse.call::<Option<i64>>("soon").unwrap(), None);
        let mono: Function = ui.get("mono").unwrap();
        assert_eq!(mono.call::<String>("a`b").unwrap(), "`a'b`");
        let bar: Function = ui.get("bar").unwrap();
        assert_eq!(bar.call::<String>((0.5, 4)).unwrap(), "▰▰▱▱");
    }
}
