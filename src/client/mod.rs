use std::sync::Arc;

use anyhow::Result;
use grammers_client::tl;
use grammers_client::update::{Message, MessageDeletion, Update};
use grammers_session::types::PeerKind;
use rand::Rng;
use tokio::sync::{Mutex, OnceCell};
use tracing::{error, info};

pub mod auth;

use crate::anti_delete::{self, AccountSnapshot, CachedDeletedMessage};
use crate::app::Services;
use crate::database::Database;
use crate::i18n;
use crate::loader::{Account as LoaderAccount, Loader};
use crate::notify::Topic;
use crate::runtime::RuntimeState;
use crate::{core_settings, restart, telegram};

const MESSAGE_CACHE_LIMIT: usize = 1000;

pub struct RunOptions {
    pub services: Arc<Services>,
    pub loader: Arc<Loader>,
    pub bot: Arc<crate::bot::BotManager>,
    /// Use the browser setup wizard when no session exists, instead of terminal prompts.
    pub browser_setup: bool,
}

pub async fn run(options: RunOptions) -> Result<()> {
    let RunOptions {
        services,
        loader,
        bot,
        browser_setup,
    } = options;
    let db = Arc::clone(&services.db);
    let setup_web = browser_setup.then(|| Arc::clone(&services));
    let primary_connection = auth::connect(Arc::clone(&db), setup_web).await?;
    // The setup wizard may have stored a bot token; pick it up.
    if core_settings::bot_token(&db).await.is_some() && services.notifier.bot().await.is_none() {
        bot.restart().await;
    }

    if services.config.web.enabled {
        let services_web = Arc::clone(&services);
        let loader_web = Arc::clone(&loader);
        tokio::spawn(async move {
            if let Err(e) = crate::web::run_dashboard(services_web, loader_web, bot).await {
                error!("web panel stopped: {e}");
            }
        });
    }

    let primary_session = primary_connection.session_file.clone();
    spawn_connection(Arc::clone(&loader), primary_connection);

    for session_file in auth::session_files(&db).await {
        if session_file == primary_session {
            continue;
        }
        spawn_session(Arc::clone(&loader), session_file);
    }

    tokio::signal::ctrl_c().await?;
    info!("Ctrl+C received, shutting down userbot");
    Ok(())
}

pub fn spawn_session(loader: Arc<Loader>, session_file: String) {
    tokio::spawn(async move {
        match auth::connect_session(Arc::clone(&loader.services.db), session_file).await {
            Ok(connection) => spawn_connection(loader, connection),
            Err(e) => error!("failed to start account session: {e}"),
        }
    });
}

fn spawn_connection(loader: Arc<Loader>, connection: auth::Connection) {
    tokio::spawn(async move {
        let session_file = connection.session_file.clone();
        let services = Arc::clone(&loader.services);
        if let Err(e) = run_connection(loader, connection).await {
            error!("account update loop stopped: {e}");
        }
        services
            .runtime
            .set_account_disconnected(&session_file)
            .await;
        if let Some(handle) = services.accounts.by_session(&session_file).await {
            services.accounts.remove(handle.user_id).await;
        }
    });
}

async fn run_connection(loader: Arc<Loader>, connection: auth::Connection) -> Result<()> {
    let services = Arc::clone(&loader.services);
    let db = Arc::clone(&services.db);
    let runtime = Arc::clone(&services.runtime);
    let auth::Connection {
        client,
        mut updates,
        pool: _pool,
        session_file,
    } = connection;
    let (account, own_user_id) = anti_delete::account_snapshot(&client, &session_file).await;
    let cached_own_user_id = Arc::new(OnceCell::new());
    if let Some(own_user_id) = own_user_id {
        cached_own_user_id
            .set(own_user_id)
            .expect("own user cache is initialized only once");
    }
    {
        let client = client.clone();
        let account_id = account.id.clone();
        tokio::spawn(
            async move { anti_delete::download_account_avatar(&client, &account_id).await },
        );
    }
    runtime
        .set_account_connected(
            session_file.clone(),
            account.id.clone(),
            account.name.clone(),
        )
        .await;
    runtime.set_connected(Some(account.name.clone())).await;
    let user_id = own_user_id
        .map(|id| id.bot_api_dialog_id())
        .unwrap_or_default();
    services
        .accounts
        .register(crate::account_tools::AccountHandle {
            client: client.clone(),
            session_file: session_file.clone(),
            user_id,
            name: account.name.clone(),
        })
        .await;
    info!(
        "userbot account '{}' connected, starting update loop",
        account.name
    );

    let lang = core_settings::language(&db).await;
    restart::complete_notice(&db, &runtime, &client, &session_file, lang).await;
    services
        .notifier
        .send(
            Topic::Startup,
            &format!(
                "✈️ <b>{}</b> <code>v{}</code>\n👤 {}",
                i18n::tr(lang, "bot.started"),
                crate::VERSION,
                crate::notify::escape_html(&account.name)
            ),
        )
        .await;

    let loader_account = LoaderAccount {
        client: client.clone(),
        session_file: session_file.clone(),
        own_user_id: Arc::clone(&cached_own_user_id),
    };
    let deleted_message_cache = Arc::new(Mutex::new(Vec::<CachedDeletedMessage>::new()));

    loop {
        let update = match updates.next().await {
            Ok(update) => update,
            Err(e) => {
                error!("update error for {session_file}: {e}");
                continue;
            }
        };
        runtime.record_account_update(&session_file).await;

        match update {
            Update::NewMessage(msg) => {
                cache_message(&client, Arc::clone(&deleted_message_cache), &account, &msg).await;
                spawn_message_handlers(Arc::clone(&loader), loader_account.clone(), msg, true);
            }
            Update::MessageEdited(msg) => {
                cache_message(&client, Arc::clone(&deleted_message_cache), &account, &msg).await;
                spawn_message_handlers(Arc::clone(&loader), loader_account.clone(), msg, false);
            }
            Update::MessageDeleted(deletion) => {
                let services = Arc::clone(&services);
                let cache = Arc::clone(&deleted_message_cache);
                tokio::spawn(async move {
                    if let Err(e) = handle_deleted_messages(services, cache, deletion).await {
                        error!("delete handler error: {e}");
                    }
                });
            }
            _ => {}
        }
    }
}

fn spawn_message_handlers(loader: Arc<Loader>, account: LoaderAccount, msg: Message, is_new: bool) {
    if is_new {
        let services = Arc::clone(&loader.services);
        let client = account.client.clone();
        let event_msg = msg.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_new_message_events(services, client, event_msg).await {
                error!("message event handler error: {e}");
            }
        });

        let auto_services = Arc::clone(&loader.services);
        let auto_session = account.session_file.clone();
        let auto_msg = msg.clone();
        tokio::spawn(async move {
            if let Some(handle) = auto_services.accounts.by_session(&auto_session).await {
                let engine = Arc::clone(&auto_services.automations);
                crate::automations::on_message(auto_services, engine, handle, auto_msg).await;
            }
        });

        let events_loader = Arc::clone(&loader);
        let events_account = account.clone();
        let events_msg = msg.clone();
        tokio::spawn(async move {
            events_loader
                .dispatch_message_event(events_account, events_msg)
                .await;
        });
    }

    tokio::spawn(async move {
        if let Err(e) = loader.handle_message(account, msg).await {
            error!("loader error: {e}");
        }
    });
}

async fn handle_new_message_events(
    services: Arc<Services>,
    client: grammers_client::Client,
    msg: Message,
) -> Result<()> {
    let db = Arc::clone(&services.db);
    let runtime = Arc::clone(&services.runtime);
    if msg.outgoing() {
        return Ok(());
    }

    if handle_group_captcha(&db, &runtime, &client, &msg).await? {
        return Ok(());
    }

    if handle_group_service_cleanup(&db, &runtime, &client, &msg).await? {
        return Ok(());
    }

    if handle_pm_guard(&services, &client, &msg).await? {
        return Ok(());
    }

    if db_bool(&db, "handlers.autoread.enabled").await {
        telegram::mark_message_as_read(&msg, &runtime).await?;
    }

    if db_bool(&db, "handlers.afk.enabled").await && msg.mentioned() {
        let reason = optional_db_string(&db, "handlers.afk.reason")
            .await
            .unwrap_or_else(|| "I'm busy right now and will reply later.".to_string());
        telegram::msg_respond(&runtime, &msg, &reason).await?;
    }

    Ok(())
}

async fn handle_group_captcha(
    db: &Database,
    runtime: &RuntimeState,
    client: &grammers_client::Client,
    msg: &Message,
) -> Result<bool> {
    if !db_bool(db, "group.captcha.enabled").await {
        return Ok(false);
    }
    if !matches!(msg.peer_id().kind(), PeerKind::Chat | PeerKind::Channel) {
        return Ok(false);
    }

    if let Some(action) = msg.action() {
        let joined = joined_user_ids(action, msg.sender_id().map(|id| id.bot_api_dialog_id()));
        if joined.is_empty() {
            return Ok(false);
        }
        for user_id in joined {
            let code = rand::thread_rng().gen_range(1000..=9999).to_string();
            let key = captcha_key(msg.peer_id().bot_api_dialog_id(), user_id);
            db.set(&key, serde_json::Value::String(code.clone()))
                .await?;
            let text = optional_db_string(db, "group.captcha.text")
                .await
                .unwrap_or_else(|| "Welcome. Send this code to pass CAPTCHA: {code}".to_string())
                .replace("{code}", &code)
                .replace("{user_id}", &user_id.to_string());
            telegram::msg_respond(runtime, msg, &text).await?;
        }
        return Ok(false);
    }

    let Some(sender_id) = msg.sender_id().map(|id| id.bot_api_dialog_id()) else {
        return Ok(false);
    };
    let key = captcha_key(msg.peer_id().bot_api_dialog_id(), sender_id);
    let pending = db.get(&key).await;
    let Some(code) = pending.as_str().map(str::to_string) else {
        return Ok(false);
    };

    let peer_ref = telegram::resolve_message_peer(client, msg).await?;
    if msg.text().trim() == code {
        db.remove(&key).await?;
        telegram::msg_respond(runtime, msg, "CAPTCHA passed.").await?;
    } else {
        telegram::delete_messages(client, runtime, peer_ref, &[msg.id()]).await?;
    }
    Ok(true)
}

async fn handle_group_service_cleanup(
    db: &Database,
    runtime: &RuntimeState,
    client: &grammers_client::Client,
    msg: &Message,
) -> Result<bool> {
    if !db_bool(db, "group.clean_joins.enabled").await {
        return Ok(false);
    }
    if !matches!(msg.peer_id().kind(), PeerKind::Chat | PeerKind::Channel) {
        return Ok(false);
    }
    let Some(action) = msg.action() else {
        return Ok(false);
    };
    if !is_join_leave_action(action) {
        return Ok(false);
    }
    let peer_ref = telegram::resolve_message_peer(client, msg).await?;
    telegram::delete_messages(client, runtime, peer_ref, &[msg.id()]).await?;
    Ok(true)
}

fn is_join_leave_action(action: &tl::enums::MessageAction) -> bool {
    matches!(
        action,
        tl::enums::MessageAction::ChatAddUser(_)
            | tl::enums::MessageAction::ChatDeleteUser(_)
            | tl::enums::MessageAction::ChatJoinedByLink(_)
            | tl::enums::MessageAction::ChatJoinedByRequest
    )
}

fn joined_user_ids(action: &tl::enums::MessageAction, fallback_sender: Option<i64>) -> Vec<i64> {
    match action {
        tl::enums::MessageAction::ChatAddUser(action) => action.users.clone(),
        tl::enums::MessageAction::ChatJoinedByLink(_)
        | tl::enums::MessageAction::ChatJoinedByRequest => fallback_sender.into_iter().collect(),
        _ => Vec::new(),
    }
}

fn captcha_key(chat_id: i64, user_id: i64) -> String {
    format!("group.captcha.pending.{chat_id}.{user_id}")
}

async fn handle_pm_guard(
    services: &Services,
    client: &grammers_client::Client,
    msg: &Message,
) -> Result<bool> {
    let db = &services.db;
    let runtime = &services.runtime;
    if !db_bool(db, "pmguard.enabled").await {
        return Ok(false);
    }
    if !matches!(msg.peer_id().kind(), PeerKind::User) {
        return Ok(false);
    }

    let Some(sender_id) = msg.sender_id() else {
        return Ok(false);
    };
    let sender = sender_id.bot_api_dialog_id().to_string();
    if db.csv_contains("pmguard.allow", &sender).await {
        return Ok(false);
    }

    if db.csv_contains("pmguard.deny", &sender).await {
        let key = format!("pmguard.denied_seen.{sender}");
        if !db_bool(db, &key).await {
            let text = optional_db_string(db, "pmguard.deny_text")
                .await
                .unwrap_or_else(|| "PM blocked by userbot security.".to_string());
            telegram::msg_respond(runtime, msg, &text).await?;
            db.set(key, serde_json::Value::Bool(true)).await?;
        }
        return Ok(true);
    }

    let key = format!("pmguard.challenge_seen.{sender}");
    if db_bool(db, &key).await {
        return Ok(true);
    }

    let text = optional_db_string(db, "pmguard.challenge_text")
        .await
        .unwrap_or_else(|| {
            "Hi. PM security is enabled. Please wait until I approve this chat.".to_string()
        });
    telegram::msg_respond(runtime, msg, &text).await?;
    db.set(key, serde_json::Value::Bool(true)).await?;

    // Let the owner decide from the bot without opening the chat.
    if services.notifier.enabled(Topic::PmGuard).await {
        let name = msg
            .sender()
            .and_then(|peer| peer.name().map(str::to_string))
            .unwrap_or_else(|| sender.clone());
        let username = msg
            .sender()
            .and_then(|peer| peer.username().map(|u| format!(" @{u}")))
            .unwrap_or_default();
        if let Ok(peer) = telegram::resolve_message_peer(client, msg).await {
            services.notifier.pm_pending.lock().await.insert(
                sender_id.bot_api_dialog_id(),
                crate::notify::PendingPm {
                    client: client.clone(),
                    peer: telegram::StoredPeer::from_ref(peer),
                    name: name.clone(),
                },
            );
        }
        let lang = core_settings::language(db).await;
        let html = format!(
            "🛡 <b>{}</b>\n👤 <a href=\"tg://user?id={sender}\">{}</a>{} · <code>{sender}</code>\n\n{}",
            i18n::tr(lang, "bot.pm_new"),
            crate::notify::escape_html(&name),
            crate::notify::escape_html(&username),
            crate::notify::escape_html(&crate::notify::clip(msg.text(), 500)),
        );
        services
            .notifier
            .send_with_keyboard(
                Topic::PmGuard,
                &html,
                Some(crate::bot::pm_keyboard(lang, sender_id.bot_api_dialog_id())),
            )
            .await;
    }
    Ok(true)
}

async fn handle_deleted_messages(
    services: Arc<Services>,
    cache: Arc<Mutex<Vec<CachedDeletedMessage>>>,
    deletion: MessageDeletion,
) -> Result<()> {
    let db = &services.db;
    if !db_bool(db, "handlers.antidelete.enabled").await {
        return Ok(());
    }

    let deleted = take_deleted_messages(cache, &deletion).await;
    if deleted.is_empty() {
        return Ok(());
    }

    let notify = services.notifier.enabled(Topic::AntiDelete).await;
    for message in deleted {
        anti_delete::record_deleted_message(db, &message, unix_timestamp()).await?;
        if notify {
            services
                .notifier
                .send(Topic::AntiDelete, &anti_delete::notification_html(&message))
                .await;
        }
    }

    Ok(())
}

async fn cache_message(
    client: &grammers_client::Client,
    cache: Arc<Mutex<Vec<CachedDeletedMessage>>>,
    account: &AccountSnapshot,
    msg: &Message,
) {
    let cached = anti_delete::snapshot_message(client, account, msg).await;

    let mut cache = cache.lock().await;
    cache.push(cached);
    if cache.len() > MESSAGE_CACHE_LIMIT {
        let overflow = cache.len() - MESSAGE_CACHE_LIMIT;
        cache.drain(..overflow);
    }
}

async fn take_deleted_messages(
    cache: Arc<Mutex<Vec<CachedDeletedMessage>>>,
    deletion: &MessageDeletion,
) -> Vec<CachedDeletedMessage> {
    let ids = deletion.messages();
    let channel_id = deletion.channel_id();
    let mut cache = cache.lock().await;
    let mut removed = Vec::new();
    cache.retain(|message| {
        let matches_id = ids.contains(&message.message_id);
        // Message IDs repeat across channels, so a channel deletion only matches that channel
        // and a deletion without a channel never matches a channel message.
        let matches_channel = message.channel_id == channel_id;
        if matches_id && matches_channel {
            removed.push(message.clone());
            false
        } else {
            true
        }
    });
    removed
}

fn unix_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

async fn db_bool(db: &Database, key: &str) -> bool {
    db.get(key).await.as_bool().unwrap_or(false)
}

async fn optional_db_string(db: &Database, key: &str) -> Option<String> {
    db.get(key)
        .await
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
