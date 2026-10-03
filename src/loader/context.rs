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

use super::manifest::ModuleManifest;
use super::{installer, Account, Loader};

const GROUP_INFO_SCAN_LIMIT: usize = 20_000;
const GROUP_INFO_SEARCH_PAGE_LIMIT: i32 = 100;

/// Database keys only trusted modules may read or write: credentials, panel and core settings.
const PROTECTED_KEYS: &[&str] = &[
    db_key::API_HASH,
    db_key::API_ID,
    db_key::PHONE,
    db_key::PROXY_URL,
    db_key::SESSION_FILE,
    db_key::SESSION_FILES,
    crate::core_settings::key::BOT_TOKEN,
];
const PROTECTED_PREFIXES: &[&str] = &["web.", "core.", "notify.", "pmguard.", "cfg.", "auto."];

/// Maximum scheduled-message delay Telegram accepts.
const MAX_SCHEDULE_SECONDS: u64 = 365 * 24 * 60 * 60;

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub db: Arc<Database>,
    pub runtime: Arc<RuntimeState>,
    pub(super) modules_dir: std::path::PathBuf,
    loader: Arc<Loader>,
    session_file: String,
    own_user_id: Option<PeerId>,
    module_name: String,
    permissions: Vec<String>,
    trusted: bool,
    pub message: Arc<Mutex<Option<Message>>>,
}

impl Ctx {
    pub fn new(loader: Arc<Loader>, account: &Account, manifest: &ModuleManifest) -> Self {
        Self {
            client: account.client.clone(),
            db: Arc::clone(&loader.services.db),
            runtime: Arc::clone(&loader.services.runtime),
            modules_dir: loader.modules_dir().to_path_buf(),
            session_file: account.session_file.clone(),
            own_user_id: account.own_user_id.get().copied(),
            loader,
            module_name: manifest.name.clone(),
            permissions: manifest.permissions.clone(),
            trusted: manifest.trusted,
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

    fn require_admin(&self) -> mlua::Result<()> {
        self.require_permission("core.admin")
    }

    fn check_key(&self, key: &str) -> mlua::Result<()> {
        if self.trusted {
            return Ok(());
        }
        let own_cfg = format!("cfg.{}.", self.module_name);
        let protected = PROTECTED_KEYS.contains(&key)
            || (PROTECTED_PREFIXES
                .iter()
                .any(|prefix| key.starts_with(prefix))
                && !key.starts_with(&own_cfg));
        if protected {
            return Err(mlua::Error::runtime(format!(
                "module '{}' may not access protected key '{key}'",
                self.module_name
            )));
        }
        Ok(())
    }

    async fn current_message(&self) -> Option<Message> {
        self.message.lock().await.as_ref().cloned()
    }

    async fn current_peer(&self) -> anyhow::Result<PeerRef> {
        let msg = self
            .current_message()
            .await
            .ok_or_else(|| anyhow::anyhow!("no message context"))?;
        telegram::resolve_message_peer(&self.client, &msg).await
    }

    async fn account_handle(&self) -> anyhow::Result<crate::account_tools::AccountHandle> {
        self.loader
            .services
            .accounts
            .by_session(&self.session_file)
            .await
            .ok_or_else(|| anyhow::anyhow!("this account is not registered yet"))
    }

    async fn lang(&self) -> crate::i18n::Lang {
        crate::core_settings::language(&self.db).await
    }

    async fn prefix(&self) -> String {
        crate::core_settings::prefixes(&self.db)
            .await
            .into_iter()
            .next()
            .unwrap_or_else(|| ".".to_string())
    }
}

fn rt<E: std::fmt::Display>(error: E) -> mlua::Error {
    mlua::Error::runtime(error.to_string())
}

/// Lua view of a message for `ctx:message()` and `on_message` handlers.
pub fn message_table(
    lua: &mlua::Lua,
    msg: &Message,
    own_user_id: Option<PeerId>,
    is_command: bool,
) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    let kind = msg.peer_id().kind();
    table.set("id", msg.id())?;
    table.set("chat_id", msg.peer_id().bot_api_dialog_id())?;
    table.set("text", msg.text())?;
    table.set("outgoing", msg.outgoing())?;
    table.set("mentioned", msg.mentioned())?;
    table.set(
        "is_private",
        matches!(kind, PeerKind::User | PeerKind::UserSelf),
    )?;
    table.set("is_saved", matches!(kind, PeerKind::UserSelf))?;
    table.set(
        "is_group",
        matches!(kind, PeerKind::Chat | PeerKind::Channel),
    )?;
    table.set("is_command", is_command)?;
    table.set("date", msg.date().timestamp())?;
    table.set("has_media", msg.media().is_some())?;
    if let Some(reply_to) = msg.reply_to_message_id() {
        table.set("reply_to_id", reply_to)?;
    }
    if let Some(sender_id) = msg.sender_id() {
        table.set("sender_id", sender_id.bot_api_dialog_id())?;
        table.set(
            "from_self",
            Some(sender_id) == own_user_id || msg.outgoing(),
        )?;
    } else {
        table.set("from_self", msg.outgoing())?;
    }
    if let Some(sender) = msg.sender() {
        if let Some(name) = sender.name() {
            table.set("sender_name", name)?;
        }
        if let Some(username) = sender.username() {
            table.set("sender_username", username)?;
        }
        if let Peer::User(user) = sender {
            table.set("sender_is_bot", user.is_bot())?;
        }
    }
    if let Some(peer) = msg.peer() {
        if let Some(name) = peer.name() {
            table.set("chat_title", name)?;
        }
    }
    if let Some(action) = msg.action() {
        let (name, users) = match action {
            tl::enums::MessageAction::ChatAddUser(add) => ("join", add.users.clone()),
            tl::enums::MessageAction::ChatJoinedByLink(_)
            | tl::enums::MessageAction::ChatJoinedByRequest => (
                "join",
                msg.sender_id()
                    .map(|id| id.bot_api_dialog_id())
                    .into_iter()
                    .collect(),
            ),
            tl::enums::MessageAction::ChatDeleteUser(left) => ("leave", vec![left.user_id]),
            _ => ("other", Vec::new()),
        };
        table.set("action", name)?;
        table.set("action_users", users)?;
    }
    Ok(table)
}

impl UserData for Ctx {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method("reply", |_, ctx, text: String| async move {
            if let Some(msg) = ctx.current_message().await {
                telegram::msg_respond(&ctx.runtime, &msg, &text)
                    .await
                    .map_err(rt)?;
            }
            Ok(())
        });

        methods.add_async_method("answer", |_, ctx, text: String| async move {
            if let Some(msg) = ctx.current_message().await {
                for chunk in telegram::split_text(&text) {
                    ctx.runtime.wait_for_telegram_send().await;
                    msg.reply(telegram::formatted_message_input(&chunk))
                        .await
                        .map_err(rt)?;
                }
            }
            Ok(())
        });

        methods.add_async_method("edit", |_, ctx, text: String| async move {
            if let Some(msg) = ctx.current_message().await {
                telegram::msg_edit_or_respond(&ctx.runtime, &msg, &text)
                    .await
                    .map_err(rt)?;
            }
            Ok(())
        });

        methods.add_async_method("delete", |_, ctx, ()| async move {
            if let Some(msg) = ctx.current_message().await {
                ctx.runtime.wait_for_telegram_send().await;
                msg.delete().await.map_err(rt)?;
            }
            Ok(())
        });

        methods.add_async_method("message", |lua, ctx, ()| async move {
            match ctx.current_message().await {
                Some(msg) => Ok(Some(message_table(&lua, &msg, ctx.own_user_id, false)?)),
                None => Ok(None),
            }
        });

        methods.add_async_method("replied", |lua, ctx, ()| async move {
            let Some(msg) = ctx.current_message().await else {
                return Ok(None);
            };
            let reply = msg.get_reply().await.map_err(rt)?;
            match reply {
                Some(reply) => {
                    let table = lua.create_table()?;
                    table.set("id", reply.id())?;
                    table.set("text", reply.text())?;
                    table.set("outgoing", reply.outgoing())?;
                    table.set("has_media", reply.media().is_some())?;
                    if let Some(sender_id) = reply.sender_id() {
                        table.set("sender_id", sender_id.bot_api_dialog_id())?;
                    }
                    if let Some(sender) = reply.sender() {
                        if let Some(name) = sender.name() {
                            table.set("sender_name", name)?;
                        }
                        if let Some(username) = sender.username() {
                            table.set("sender_username", username)?;
                        }
                    }
                    Ok(Some(table))
                }
                None => Ok(None),
            }
        });

        methods.add_async_method(
            "schedule",
            |_, ctx, (seconds, text, target): (u64, String, Option<String>)| async move {
                if !(10..=MAX_SCHEDULE_SECONDS).contains(&seconds) {
                    return Err(mlua::Error::runtime(
                        "schedule delay must be between 10 seconds and 365 days",
                    ));
                }
                let peer = match target.as_deref().unwrap_or("here") {
                    "me" | "saved" => telegram::saved_messages(),
                    _ => ctx.current_peer().await.map_err(rt)?,
                };
                let at = SystemTime::now() + Duration::from_secs(seconds);
                telegram::send_scheduled(&ctx.client, &ctx.runtime, peer, &text, at)
                    .await
                    .map_err(rt)
            },
        );

        methods.add_async_method("send_saved", |_, ctx, text: String| async move {
            telegram::send_markdown(&ctx.client, &ctx.runtime, telegram::saved_messages(), &text)
                .await
                .map_err(rt)
        });

        methods.add_async_method("pin", |_, ctx, ()| async move {
            ctx.require_permission("telegram.history")?;
            let msg = ctx
                .current_message()
                .await
                .ok_or_else(|| rt("no message"))?;
            let reply = msg
                .get_reply()
                .await
                .map_err(rt)?
                .ok_or_else(|| rt("reply to the message to pin"))?;
            ctx.runtime.wait_for_telegram_send().await;
            reply.pin().await.map_err(rt)
        });

        methods.add_async_method("unpin", |_, ctx, ()| async move {
            ctx.require_permission("telegram.history")?;
            let msg = ctx
                .current_message()
                .await
                .ok_or_else(|| rt("no message"))?;
            let reply = msg
                .get_reply()
                .await
                .map_err(rt)?
                .ok_or_else(|| rt("reply to the message to unpin"))?;
            ctx.runtime.wait_for_telegram_send().await;
            reply.unpin().await.map_err(rt)
        });

        methods.add_async_method("purge", |_, ctx, ()| async move {
            ctx.require_permission("telegram.history")?;
            purge_from_reply(ctx.clone()).await.map_err(rt)
        });

        methods.add_async_method(
            "tag_all",
            |_, ctx, (text, limit): (Option<String>, Option<usize>)| async move {
                ctx.require_permission("telegram.history")?;
                tag_all(ctx.clone(), text.unwrap_or_default(), limit.unwrap_or(100))
                    .await
                    .map_err(rt)
            },
        );

        methods.add_async_method("react", |_, ctx, emoji: String| async move {
            let msg = ctx
                .current_message()
                .await
                .ok_or_else(|| rt("no message"))?;
            ctx.runtime.wait_for_telegram_send().await;
            let reaction = tl::enums::Reaction::Emoji(tl::types::ReactionEmoji { emoticon: emoji });
            msg.react(vec![reaction]).await.map_err(rt)
        });

        methods.add_async_method("db_get", |lua, ctx, key: String| async move {
            ctx.check_key(&key)?;
            let value = ctx.db.get(&key).await;
            lua.to_value(&value)
        });

        methods.add_async_method(
            "db_set",
            |lua, ctx, (key, value): (String, mlua::Value)| async move {
                ctx.check_key(&key)?;
                if value.is_nil() {
                    return ctx.db.remove(&key).await.map_err(rt);
                }
                let json = lua_to_json(&lua, value)?;
                ctx.db.set(key, json).await.map_err(rt)
            },
        );

        methods.add_async_method("db_keys", |lua, ctx, prefix: String| async move {
            ctx.check_key(&prefix)?;
            let keys = ctx
                .db
                .keys_with_prefix(&prefix)
                .await
                .into_iter()
                .filter(|key| ctx.check_key(key).is_ok())
                .collect::<Vec<_>>();
            lua.to_value(&keys)
        });

        methods.add_async_method("cfg", |lua, ctx, key: String| async move {
            let value = ctx.loader.config_value(&ctx.module_name, &key).await;
            lua.to_value(&value)
        });

        methods.add_async_method(
            "cfg_set",
            |lua, ctx, (key, value): (String, mlua::Value)| async move {
                let json = if value.is_nil() {
                    serde_json::Value::Null
                } else {
                    lua_to_json(&lua, value)?
                };
                let stored = ctx
                    .loader
                    .set_config_value(&ctx.module_name, &key, json)
                    .await
                    .map_err(rt)?;
                lua.to_value(&stored)
            },
        );

        methods.add_async_method("lang", |_, ctx, ()| async move {
            Ok(ctx.lang().await.code().to_string())
        });

        methods.add_async_method("prefix", |_, ctx, ()| async move { Ok(ctx.prefix().await) });

        methods.add_async_method("help", |_, ctx, query: Option<String>| async move {
            let lang = ctx.lang().await;
            let prefix = ctx.prefix().await;
            Ok(ctx
                .loader
                .help_text(query.as_deref().unwrap_or(""), lang, &prefix)
                .await)
        });

        methods.add_async_method("inline_help", |_, ctx, query: Option<String>| async move {
            inline_help(ctx.clone(), query.unwrap_or_default())
                .await
                .map_err(rt)
        });

        methods.add_async_method("notify", |_, ctx, text: String| async move {
            let html = format!(
                "🧩 <b>{}</b>\n{}",
                crate::notify::escape_html(&ctx.module_name),
                crate::notify::escape_html(&text)
            );
            ctx.loader
                .services
                .notifier
                .send(crate::notify::Topic::Module, &html)
                .await;
            Ok(())
        });

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
                    .map_err(rt)
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
                    .map_err(rt)
            },
        );

        methods.add_method("module_info", |lua, ctx, ()| {
            let info = serde_json::json!({
                "name": ctx.module_name.clone(),
                "permissions": ctx.permissions.clone(),
                "trusted": ctx.trusted,
            });
            lua.to_value(&info)
        });

        methods.add_async_method("modules_list", |lua, ctx, ()| async move {
            ctx.require_admin()?;
            let summaries = ctx.loader.summaries().await;
            let list = summaries
                .iter()
                .map(|module| {
                    serde_json::json!({
                        "name": module.info.name,
                        "version": module.info.version,
                        "enabled": module.enabled,
                        "trusted": module.info.trusted,
                        "bundled": module.bundled,
                        "category": module.category,
                        "description": if module.help.description.is_empty() { &module.info.description } else { &module.help.description },
                        "commands": module.info.commands,
                        "settings": module.config.len(),
                    })
                })
                .collect::<Vec<_>>();
            lua.to_value(&list)
        });

        methods.add_async_method(
            "module_set_enabled",
            |_, ctx, (name, enabled): (String, bool)| async move {
                ctx.require_admin()?;
                ctx.loader.set_enabled(&name, enabled).await.map_err(rt)
            },
        );

        methods.add_async_method("module_reload", |_, ctx, name: String| async move {
            ctx.require_admin()?;
            ctx.loader.reload(&name).await.map_err(rt)
        });

        methods.add_async_method("module_remove", |_, ctx, name: String| async move {
            ctx.require_admin()?;
            ctx.loader.remove(&name).await.map_err(rt)
        });

        methods.add_async_method(
            "module_restore",
            |lua, ctx, name: Option<String>| async move {
                ctx.require_admin()?;
                let restored =
                    crate::bundled::restore(&ctx.modules_dir, name.as_deref()).map_err(rt)?;
                lua.to_value(&restored)
            },
        );

        methods.add_async_method("module_config", |lua, ctx, name: String| async move {
            ctx.require_admin()?;
            let summary = ctx
                .loader
                .summaries()
                .await
                .into_iter()
                .find(|module| module.info.name == name)
                .ok_or_else(|| rt(format!("unknown module '{name}'")))?;
            let entries = summary
                .config
                .iter()
                .map(|entry| {
                    let shown = if entry.field.secret {
                        crate::loader::meta::display_value(&entry.field, &if entry.is_set { serde_json::Value::Bool(true) } else { serde_json::Value::Null })
                    } else {
                        crate::loader::meta::display_value(&entry.field, &entry.value)
                    };
                    serde_json::json!({
                        "key": entry.field.key,
                        "type": entry.field.kind,
                        "value": shown,
                        "default": crate::loader::meta::display_value(&entry.field, &entry.field.default),
                        "description": entry.field.description,
                        "options": entry.field.options,
                        "secret": entry.field.secret,
                    })
                })
                .collect::<Vec<_>>();
            lua.to_value(&entries)
        });

        methods.add_async_method(
            "module_config_set",
            |_, ctx, (name, key, value): (String, String, Option<String>)| async move {
                ctx.require_admin()?;
                let raw = value
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null);
                let stored = ctx
                    .loader
                    .set_config_value(&name, &key, raw)
                    .await
                    .map_err(rt)?;
                let field = ctx
                    .loader
                    .config_fields(&name)
                    .await
                    .and_then(|fields| fields.into_iter().find(|field| field.key == key));
                Ok(match field {
                    Some(field) => crate::loader::meta::display_value(&field, &stored),
                    None => stored.to_string(),
                })
            },
        );

        methods.add_async_method("prefixes", |lua, ctx, ()| async move {
            lua.to_value(&crate::core_settings::prefixes(&ctx.db).await)
        });

        methods.add_async_method("set_prefixes", |lua, ctx, values: Vec<String>| async move {
            ctx.require_admin()?;
            let stored = crate::core_settings::set_prefixes(&ctx.db, &values)
                .await
                .map_err(rt)?;
            lua.to_value(&stored)
        });

        methods.add_async_method("set_lang", |_, ctx, code: String| async move {
            ctx.require_admin()?;
            let lang = crate::i18n::Lang::parse(&code);
            crate::core_settings::set_language(&ctx.db, lang)
                .await
                .map_err(rt)?;
            Ok(lang.code().to_string())
        });

        methods.add_async_method("sudo_list", |lua, ctx, ()| async move {
            ctx.require_admin()?;
            lua.to_value(&crate::core_settings::sudo_users(&ctx.db).await)
        });

        methods.add_async_method("sudo_set", |_, ctx, users: Vec<i64>| async move {
            ctx.require_admin()?;
            crate::core_settings::set_sudo_users(&ctx.db, &users)
                .await
                .map_err(rt)
        });

        methods.add_async_method("panel_link", |_, ctx, ()| async move {
            ctx.require_admin()?;
            Ok(ctx.loader.services.panel_link().await)
        });

        methods.add_async_method("restart", |_, ctx, ()| async move {
            ctx.require_admin()?;
            request_restart(&ctx).await.map_err(rt)?;
            Ok(())
        });

        methods.add_async_method("check_update", |lua, ctx, ()| async move {
            ctx.require_admin()?;
            let source = crate::updater::is_source_checkout();
            let info = if source {
                serde_json::json!({ "current": crate::VERSION, "source_checkout": true })
            } else {
                let release = crate::updater::latest_release().await.map_err(rt)?;
                serde_json::json!({
                    "current": crate::VERSION,
                    "latest": release.version,
                    "newer": crate::updater::is_newer(&release.version, crate::VERSION),
                    "notes": release.notes,
                    "url": release.page_url,
                    "has_binary": release.binary_url.is_some(),
                    "source_checkout": false,
                })
            };
            lua.to_value(&info)
        });

        methods.add_async_method("self_update", |_, ctx, ()| async move {
            ctx.require_admin()?;
            let release = crate::updater::latest_release().await.map_err(rt)?;
            if !crate::updater::is_newer(&release.version, crate::VERSION) {
                return Ok(false);
            }
            crate::updater::install(&release).await.map_err(rt)?;
            request_restart(&ctx).await.map_err(rt)?;
            Ok(true)
        });

        methods.add_async_method("account", |lua, ctx, ()| async move {
            ctx.require_admin()?;
            let handle = ctx.account_handle().await.map_err(rt)?;
            lua.to_value(&serde_json::json!({
                "id": handle.user_id,
                "name": handle.name,
                "session": handle.session_file,
            }))
        });

        methods.add_async_method("account_counts", |lua, ctx, ()| async move {
            ctx.require_admin()?;
            let handle = ctx.account_handle().await.map_err(rt)?;
            let counts = crate::account_tools::counts(&handle).await.map_err(rt)?;
            lua.to_value(&counts)
        });

        methods.add_async_method(
            "cleanup_preview",
            |lua, ctx, options: mlua::Value| async move {
                ctx.require_admin()?;
                let options: crate::account_tools::CleanupOptions = lua.from_value(options)?;
                if options.is_empty() {
                    return Err(rt(
                        "choose at least one of: contacts, groups, channels, private_chats, bots",
                    ));
                }
                let handle = ctx.account_handle().await.map_err(rt)?;
                let plan = crate::account_tools::plan_cleanup(&handle, &options)
                    .await
                    .map_err(rt)?;
                use crate::account_tools::DialogKind as K;
                let sample = plan
                    .dialogs
                    .iter()
                    .take(15)
                    .map(|d| d.title.clone())
                    .collect::<Vec<_>>();
                let summary = serde_json::json!({
                    "contacts": plan.contacts.len(),
                    "users": plan.count(K::User),
                    "bots": plan.count(K::Bot),
                    "groups": plan.count(K::Group) + plan.count(K::Supergroup),
                    "channels": plan.count(K::Channel),
                    "kept": plan.kept,
                    "owned": plan.owned_skipped,
                    "total": plan.total(),
                    "sample": sample,
                });
                let code = ctx
                    .loader
                    .services
                    .accounts
                    .store_plan(handle.user_id, options, plan)
                    .await;
                let mut summary = summary;
                summary["code"] = serde_json::Value::String(code);
                lua.to_value(&summary)
            },
        );

        methods.add_async_method("cleanup_run", |_, ctx, code: String| async move {
            ctx.require_admin()?;
            let services = Arc::clone(&ctx.loader.services);
            let (user_id, options, plan) = services
                .accounts
                .take_plan(&code)
                .await
                .ok_or_else(|| rt("unknown or expired confirmation code; run a new preview"))?;
            let handle = services
                .accounts
                .get(user_id)
                .await
                .ok_or_else(|| rt("that account is no longer connected"))?;
            let job = services
                .jobs
                .start("cleanup", "Account cleanup", &handle.name)
                .await;
            let id = job.id().to_string();
            services.spawn_job(
                job.clone(),
                crate::account_tools::run_cleanup(handle, plan, options, job),
            );
            Ok(id)
        });

        methods.add_async_method(
            "account_bulk",
            |_, ctx, (action, days): (String, Option<u32>)| async move {
                ctx.require_admin()?;
                let action = crate::account_tools::BulkAction::parse(&action).ok_or_else(|| {
                    rt("unknown action; use read_all, archive_inactive, mute_all, export")
                })?;
                let handle = ctx.account_handle().await.map_err(rt)?;
                let services = Arc::clone(&ctx.loader.services);
                let job = services
                    .jobs
                    .start("bulk", action.title(), &handle.name)
                    .await;
                let id = job.id().to_string();
                services.spawn_job(
                    job.clone(),
                    crate::account_tools::run_bulk(handle, action, days.unwrap_or(30), job),
                );
                Ok(id)
            },
        );

        methods.add_async_method("jobs", |lua, ctx, ()| async move {
            ctx.require_admin()?;
            let mut jobs = ctx.loader.services.jobs.list().await;
            jobs.truncate(10);
            for job in &mut jobs {
                let keep = job.log.len().saturating_sub(5);
                job.log.drain(..keep);
            }
            lua.to_value(&jobs)
        });

        methods.add_async_method("job_cancel", |_, ctx, id: String| async move {
            ctx.require_admin()?;
            Ok(ctx.loader.services.jobs.cancel(id.trim()).await)
        });

        methods.add_async_method(
            "profile_update",
            |lua, ctx, update: mlua::Value| async move {
                ctx.require_admin()?;
                let update: crate::account_tools::ProfileUpdate = lua.from_value(update)?;
                let handle = ctx.account_handle().await.map_err(rt)?;
                crate::account_tools::update_profile(&handle, &update)
                    .await
                    .map_err(rt)
            },
        );

        methods.add_async_method("auto_rules", |lua, ctx, ()| async move {
            ctx.require_admin()?;
            let rules = crate::automations::load_rules(&ctx.loader.services).await;
            lua.to_value(&rules)
        });

        methods.add_async_method("auto_save", |lua, ctx, rule: mlua::Value| async move {
            ctx.require_admin()?;
            let mut rule: crate::automations::Rule = lua.from_value(rule)?;
            if rule.accounts.is_empty() {
                if let Ok(handle) = ctx.account_handle().await {
                    rule.accounts = vec![handle.user_id];
                }
            }
            let saved = crate::automations::upsert_rule(&ctx.loader.services, rule)
                .await
                .map_err(rt)?;
            lua.to_value(&saved)
        });

        methods.add_async_method(
            "auto_toggle",
            |_, ctx, (id, enabled): (String, bool)| async move {
                ctx.require_admin()?;
                crate::automations::set_enabled(&ctx.loader.services, &id, enabled)
                    .await
                    .map_err(rt)
            },
        );

        methods.add_async_method("auto_delete", |_, ctx, id: String| async move {
            ctx.require_admin()?;
            crate::automations::delete_rule(&ctx.loader.services, &id)
                .await
                .map_err(rt)
        });

        methods.add_async_method("uptime_seconds", |_, ctx, ()| async move {
            Ok(ctx.runtime.uptime_seconds())
        });

        methods.add_method("sha256", |_, _, text: mlua::String| {
            Ok(crate::loader::manifest::source_sha256(
                &text.to_string_lossy(),
            ))
        });

        methods.add_method("uuid", |_, _, ()| Ok(uuid::Uuid::new_v4().to_string()));

        methods.add_method("now_ms", |_, _, ()| {
            Ok(SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0))
        });

        methods.add_async_method("runtime_stats", |lua, ctx, ()| async move {
            let accounts = ctx.runtime.accounts().await;
            let stats = serde_json::json!({
                "version": crate::VERSION,
                "uptime_seconds": ctx.runtime.uptime_seconds(),
                "cpu_percent": ctx.runtime.process_cpu_percent().await,
                "memory_bytes": crate::runtime::process_memory_bytes(),
                "updates_seen": ctx.runtime.updates_seen(),
                "commands_seen": ctx.runtime.commands_seen(),
                "errors_seen": ctx.runtime.errors_seen(),
                "modules": ctx.loader.module_names().await.len(),
                "accounts": accounts.len(),
                "top_commands": ctx.runtime.top_commands(5),
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "session": ctx.session_file.clone(),
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
            let reply = message.get_reply().await.map_err(rt)?;
            Ok(reply
                .as_ref()
                .map(|message| message.text().to_string())
                .unwrap_or_default())
        });

        methods.add_async_method("http_get", |_, ctx, url: String| async move {
            ctx.require_permission("network")?;
            http_get_text(&url).await.map_err(rt)
        });

        methods.add_async_method("http_json_get", |lua, ctx, url: String| async move {
            ctx.require_permission("network")?;
            let text = http_get_text(&url).await.map_err(rt)?;
            let value = serde_json::from_str::<serde_json::Value>(&text).map_err(rt)?;
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
                    .map_err(rt)
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
                    .map_err(rt)?;
                let value = serde_json::from_str::<serde_json::Value>(&text).map_err(rt)?;
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
                .map_err(rt)?;
                let value = serde_json::from_str::<serde_json::Value>(&text).map_err(rt)?;
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
                download_replied_media(ctx.clone(), name).await.map_err(rt)
            },
        );

        methods.add_async_method(
            "download_url",
            |_, ctx, (url, name): (String, Option<String>)| async move {
                ctx.require_permission("network")?;
                ctx.require_permission("telegram.media")?;
                download_url_to_file(&url, name.as_deref())
                    .await
                    .map_err(rt)
            },
        );

        methods.add_async_method(
            "send_file",
            |_, ctx, (path, caption): (String, Option<String>)| async move {
                ctx.require_permission("telegram.media")?;
                send_file(ctx.clone(), path, caption.unwrap_or_default())
                    .await
                    .map_err(rt)
            },
        );

        methods.add_async_method("run_term", |_, ctx, command: String| async move {
            ctx.require_permission("shell")?;
            run_shell_command(ctx.clone(), command).await.map_err(rt)
        });

        methods.add_async_method(
            "run_process",
            |_, ctx, (program, args): (String, Vec<String>)| async move {
                ctx.require_permission("shell")?;
                run_process(ctx.clone(), program, args).await.map_err(rt)
            },
        );

        methods.add_async_method("update_project", |_, ctx, ()| async move {
            ctx.require_permission("shell")?;
            update_project(ctx.clone()).await.map_err(rt)
        });

        methods.add_async_method("backup", |_, ctx, ()| async move {
            ctx.require_admin()?;
            crate::backup::send_backup(&ctx).await.map_err(rt)
        });

        methods.add_async_method("restore_backup", |_, ctx, ()| async move {
            ctx.require_admin()?;
            crate::backup::restore_from_reply(&ctx).await.map_err(rt)
        });

        methods.add_async_method("delete_last_own", |_, ctx, count: u32| async move {
            ctx.require_permission("telegram.history")?;
            delete_last_own_messages(ctx.clone(), count)
                .await
                .map_err(rt)
        });

        methods.add_async_method("message_info", |_, ctx, ()| async move {
            ctx.require_permission("telegram.history")?;
            build_message_info(ctx.clone()).await.map_err(rt)
        });

        methods.add_async_method("sleep", |_, _, seconds: u64| async move {
            tokio::time::sleep(Duration::from_secs(seconds.min(86_400))).await;
            Ok(())
        });

        methods.add_async_method("sleep_ms", |_, _, millis: u64| async move {
            tokio::time::sleep(Duration::from_millis(millis.min(86_400_000))).await;
            Ok(())
        });
    }
}

fn lua_to_json(lua: &mlua::Lua, value: mlua::Value) -> mlua::Result<serde_json::Value> {
    // Empty Lua tables serialize as empty objects; that is fine for stored state.
    lua.from_value::<serde_json::Value>(value)
}

/// Persists where to report completion, edits the command message, and re-executes.
pub async fn request_restart(ctx: &Ctx) -> anyhow::Result<()> {
    let lang = ctx.lang().await;
    if let Some(msg) = ctx.current_message().await {
        let peer = telegram::resolve_message_peer(&ctx.client, &msg).await?;
        crate::restart::save_notice(
            &ctx.db,
            &crate::restart::RestartNotice {
                session_file: ctx.session_file.clone(),
                peer: telegram::StoredPeer::from_ref(peer),
                message_id: msg.id(),
                requested_at_ms: crate::restart::now_ms(),
            },
        )
        .await;
        let text = format!("🔄 __{}__", crate::i18n::tr(lang, "restart.running"));
        let _ = telegram::msg_edit_or_respond(&ctx.runtime, &msg, &text).await;
    }
    crate::restart::restart_now()
}

/// Sends help as an inline result from the control bot, so it has navigation buttons.
/// Returns false when no bot is running or inline mode is not enabled for it.
async fn inline_help(ctx: Ctx, query: String) -> anyhow::Result<bool> {
    let Some(username) = ctx.loader.services.notifier.bot_username().await else {
        return Ok(false);
    };
    let Some(msg) = ctx.current_message().await else {
        return Ok(false);
    };
    let peer = telegram::resolve_message_peer(&ctx.client, &msg).await?;
    let Some(bot) = ctx.client.resolve_username(&username).await? else {
        return Ok(false);
    };
    let Some(bot_ref) = bot.to_ref().await else {
        return Ok(false);
    };
    let mut results = ctx
        .client
        .inline_query(bot_ref, format!("help {}", query.trim()).trim())
        .peer(peer);
    let Some(result) = results.next().await? else {
        return Ok(false);
    };
    ctx.runtime.wait_for_telegram_send().await;
    result.send(peer).await?;
    ctx.runtime.wait_for_telegram_send().await;
    let _ = msg.delete().await;
    Ok(true)
}

async fn purge_from_reply(ctx: Ctx) -> anyhow::Result<usize> {
    let msg = ctx
        .current_message()
        .await
        .ok_or_else(|| anyhow::anyhow!("no message context"))?;
    let reply_id = msg
        .reply_to_message_id()
        .ok_or_else(|| anyhow::anyhow!("reply to the first message to delete"))?;
    let peer = telegram::resolve_message_peer(&ctx.client, &msg).await?;
    let mut ids = Vec::new();
    let mut iter = ctx.client.iter_messages(peer).offset_id(msg.id() + 1);
    while let Some(message) = iter.next().await? {
        if message.id() < reply_id {
            break;
        }
        ids.push(message.id());
        if ids.len() >= 1000 {
            break;
        }
    }
    let count = ids.len();
    for chunk in ids.chunks(100) {
        telegram::delete_messages(&ctx.client, &ctx.runtime, peer, chunk).await?;
    }
    Ok(count)
}

async fn tag_all(ctx: Ctx, text: String, limit: usize) -> anyhow::Result<usize> {
    let msg = ctx
        .current_message()
        .await
        .ok_or_else(|| anyhow::anyhow!("no message context"))?;
    let peer = telegram::resolve_message_peer(&ctx.client, &msg).await?;
    if !matches!(peer.id.kind(), PeerKind::Chat | PeerKind::Channel) {
        anyhow::bail!("tagging works in groups only");
    }
    let limit = limit.clamp(1, 500);
    let mut mentions = Vec::new();
    let mut participants = ctx.client.iter_participants(peer);
    while let Some(participant) = participants.next().await? {
        let user = participant.user;
        if user.is_bot() || user.deleted() || Some(user.id()) == ctx.own_user_id {
            continue;
        }
        let Some(user_ref) = user.to_ref().await else {
            continue;
        };
        let name = user
            .first_name()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("user")
            .chars()
            .take(24)
            .collect::<String>();
        mentions.push((name, user_ref));
        if mentions.len() >= limit {
            break;
        }
    }

    // Five mentions per message keeps Telegram from flagging the batch as spam.
    let total = mentions.len();
    for batch in mentions.chunks(5) {
        let mut body = if text.trim().is_empty() {
            String::new()
        } else {
            format!("{}\n", text.trim())
        };
        let mut entities = Vec::new();
        for (index, (name, user_ref)) in batch.iter().enumerate() {
            if index > 0 {
                body.push_str(", ");
            }
            let offset = body.encode_utf16().count() as i32;
            body.push_str(name);
            entities.push(tl::enums::MessageEntity::InputMessageEntityMentionName(
                tl::types::InputMessageEntityMentionName {
                    offset,
                    length: name.encode_utf16().count() as i32,
                    user_id: (*user_ref).into(),
                },
            ));
        }
        ctx.runtime.wait_for_telegram_send().await;
        ctx.client
            .send_message(peer, InputMessage::new().text(body).fmt_entities(entities))
            .await?;
    }
    Ok(total)
}

async fn run_process(ctx: Ctx, program: String, args: Vec<String>) -> anyhow::Result<()> {
    let program = program.trim().to_string();
    if program.is_empty() || program.contains(['/', '\\']) && !Path::new(&program).exists() {
        anyhow::bail!("invalid program");
    }
    let shown = std::iter::once(program.clone())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    edit_current_message(&ctx, &format!("⏳ `{}`", shown.replace('`', "'"))).await?;
    let output = Command::new(&program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .output()
        .await?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let text = sanitize_text(&ctx, &text).await;
    let body = if text.trim().is_empty() {
        "(no output)".to_string()
    } else {
        text.chars()
            .rev()
            .take(3500)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    };
    let status = output
        .status
        .code()
        .map_or("signal".to_string(), |code| code.to_string());
    edit_current_message(
        &ctx,
        &format!(
            "{} **Exit** `{status}`\n\n```text\n{}\n```",
            if output.status.success() {
                "✅"
            } else {
                "❌"
            },
            escape_code_block(&body)
        ),
    )
    .await
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
        request_restart(&ctx).await?;
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
