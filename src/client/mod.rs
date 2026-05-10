use std::sync::Arc;

use anyhow::Result;
use grammers_client::update::{Message, MessageDeletion, Update};
use tokio::sync::Mutex;
use tracing::{error, info};

pub mod auth;

use crate::anti_delete::{self, AccountSnapshot, CachedDeletedMessage};
use crate::database::Database;
use crate::loader::Loader;
use crate::runtime::RuntimeState;
use crate::telegram;

const MESSAGE_CACHE_LIMIT: usize = 1000;

/// Connects to Telegram and runs the main userbot update loop.
///
/// `use_web` — if true, opens the axum login page instead of CLI prompts
/// when no valid session exists.
pub async fn run(
    db: Arc<Database>,
    loader: Arc<Loader>,
    runtime: Arc<RuntimeState>,
    use_web: bool,
) -> Result<()> {
    let primary_connection = auth::connect(Arc::clone(&db), use_web).await?;

    {
        let db_web = Arc::clone(&db);
        let loader_web = Arc::clone(&loader);
        let runtime_web = Arc::clone(&runtime);
        tokio::spawn(async move {
            if let Err(e) = crate::web::run_dashboard(db_web, loader_web, runtime_web).await {
                error!("dashboard server stopped: {e}");
            }
        });
    }

    let primary_session = primary_connection.session_file.clone();
    spawn_connection(
        Arc::clone(&db),
        Arc::clone(&loader),
        Arc::clone(&runtime),
        primary_connection,
    );

    for session_file in auth::session_files(&db).await {
        if session_file == primary_session {
            continue;
        }
        spawn_session(
            Arc::clone(&db),
            Arc::clone(&loader),
            Arc::clone(&runtime),
            session_file,
        );
    }

    tokio::signal::ctrl_c().await?;
    info!("Ctrl+C received, shutting down userbot");
    Ok(())
}

/// Starts an update loop for an existing session file.
pub fn spawn_session(
    db: Arc<Database>,
    loader: Arc<Loader>,
    runtime: Arc<RuntimeState>,
    session_file: String,
) {
    tokio::spawn(async move {
        match auth::connect_session(Arc::clone(&db), session_file).await {
            Ok(connection) => spawn_connection(db, loader, runtime, connection),
            Err(e) => error!("failed to start account session: {e}"),
        }
    });
}

fn spawn_connection(
    db: Arc<Database>,
    loader: Arc<Loader>,
    runtime: Arc<RuntimeState>,
    connection: auth::Connection,
) {
    tokio::spawn(async move {
        if let Err(e) = run_connection(db, loader, runtime, connection).await {
            error!("account update loop stopped: {e}");
        }
    });
}

async fn run_connection(
    db: Arc<Database>,
    loader: Arc<Loader>,
    runtime: Arc<RuntimeState>,
    connection: auth::Connection,
) -> Result<()> {
    let auth::Connection {
        client,
        mut updates,
        pool: _pool,
        session_file,
    } = connection;
    let account = anti_delete::account_snapshot(&client, &session_file).await;
    runtime
        .set_account_connected(
            session_file.clone(),
            account.id.clone(),
            account.name.clone(),
        )
        .await;
    runtime.set_connected(Some(account.name.clone())).await;
    info!(
        "userbot account '{}' connected, starting update loop",
        account.name
    );
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
                spawn_message_handlers(
                    Arc::clone(&db),
                    Arc::clone(&loader),
                    Arc::clone(&runtime),
                    session_file.clone(),
                    client.clone(),
                    msg,
                );
            }
            Update::MessageEdited(msg) => {
                cache_message(&client, Arc::clone(&deleted_message_cache), &account, &msg).await;
                spawn_message_handlers(
                    Arc::clone(&db),
                    Arc::clone(&loader),
                    Arc::clone(&runtime),
                    session_file.clone(),
                    client.clone(),
                    msg,
                );
            }
            Update::MessageDeleted(deletion) => {
                let db_clone = Arc::clone(&db);
                let cache = Arc::clone(&deleted_message_cache);
                tokio::spawn(async move {
                    if let Err(e) = handle_deleted_messages(db_clone, cache, deletion).await {
                        error!("delete handler error: {e}");
                    }
                });
            }
            _ => {}
        }
    }
}

fn spawn_message_handlers(
    db: Arc<Database>,
    loader: Arc<Loader>,
    runtime: Arc<RuntimeState>,
    session_file: String,
    client: grammers_client::Client,
    msg: Message,
) {
    if msg.text().starts_with('.') {
        let runtime_for_command = Arc::clone(&runtime);
        let session_file_for_command = session_file.clone();
        tokio::spawn(async move {
            runtime_for_command
                .record_account_command(&session_file_for_command)
                .await;
        });
    }

    let event_db = Arc::clone(&db);
    let event_runtime = Arc::clone(&runtime);
    let event_client = client.clone();
    let event_msg = msg.clone();
    tokio::spawn(async move {
        if let Err(e) =
            handle_new_message_events(event_db, event_runtime, event_client, event_msg).await
        {
            error!("message event handler error: {e}");
        }
    });

    tokio::spawn(async move {
        if let Err(e) = loader.handle_message(client, msg).await {
            error!("loader error: {e}");
        }
    });
}

async fn handle_new_message_events(
    db: Arc<Database>,
    runtime: Arc<RuntimeState>,
    client: grammers_client::Client,
    msg: Message,
) -> Result<()> {
    if msg.outgoing() {
        return Ok(());
    }

    if db_bool(&db, "handlers.autoread.enabled").await {
        telegram::mark_message_as_read(&msg, &runtime).await?;
    }

    if db_bool(&db, "handlers.afk.enabled").await && msg.mentioned() {
        let reason = optional_db_string(&db, "handlers.afk.reason")
            .await
            .unwrap_or_else(|| "I'm busy right now and will reply later.".to_string());
        let peer_ref = telegram::resolve_message_peer(&client, &msg).await?;
        telegram::reply_text(&client, &runtime, peer_ref, msg.id(), &reason).await?;
    }

    Ok(())
}

async fn handle_deleted_messages(
    db: Arc<Database>,
    cache: Arc<Mutex<Vec<CachedDeletedMessage>>>,
    deletion: MessageDeletion,
) -> Result<()> {
    if !db_bool(&db, "handlers.antidelete.enabled").await {
        return Ok(());
    }

    let deleted = take_deleted_messages(cache, &deletion).await;
    if deleted.is_empty() {
        return Ok(());
    }

    for message in deleted {
        anti_delete::record_deleted_message(&db, &message, unix_timestamp()).await?;
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
        let matches_channel = channel_id.is_none() || message.channel_id == channel_id;
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
