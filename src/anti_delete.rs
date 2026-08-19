use std::path::Path;

use anyhow::{Context, Result};
use grammers_client::peer::Peer;
use grammers_client::update::Message;
use grammers_client::Client;
use grammers_session::types::PeerId;
use libsql::{params, Builder, Connection, Database as SqliteDatabase};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use crate::crypto;
use crate::database::Database;

mod media;

use media::{download_avatar, snapshot_media};

const DATABASE_PATH: &str = "data/antidelete.sqlite";
const ENCRYPTED_DATABASE_PATH: &str = "data/antidelete.sqlite.enc";
const DEFAULT_DELETE_LOG_LIMIT: i64 = 100;
const MAX_DELETE_LOG_LIMIT: i64 = 500;

static SQLITE_LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountSnapshot {
    pub id: String,
    pub name: String,
    pub session_file: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatSnapshot {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub username: Option<String>,
    pub link: Option<String>,
    pub avatar_path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedDeletedMessage {
    pub account: AccountSnapshot,
    pub chat: ChatSnapshot,
    pub message_id: i32,
    pub channel_id: Option<i64>,
    pub sender_id: Option<String>,
    pub text: String,
    pub sent_at: String,
    pub media_type: Option<String>,
    pub media_path: Option<String>,
    pub media_name: Option<String>,
    pub media_size: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct DeleteLogFilters {
    pub q: Option<String>,
    pub account: Option<String>,
    pub chat: Option<String>,
    pub media_type: Option<String>,
    pub limit: Option<i64>,
}

pub async fn store(db: &Database) -> Value {
    let password = db.master_password().await;
    read_store(password)
        .await
        .unwrap_or_else(|e| serde_json::json!({ "error": e.to_string() }))
}

pub async fn delete_log(db: &Database, filters: DeleteLogFilters) -> Value {
    let password = db.master_password().await;
    read_delete_log(password, filters)
        .await
        .unwrap_or_else(|e| serde_json::json!({ "error": e.to_string() }))
}

pub async fn snapshot_message(
    client: &Client,
    account: &AccountSnapshot,
    msg: &Message,
) -> CachedDeletedMessage {
    let chat = snapshot_chat(client, account, msg).await;
    let peer_id = msg.peer_id();
    let media = snapshot_media(account, &chat, msg).await;
    CachedDeletedMessage {
        account: account.clone(),
        chat,
        message_id: msg.id(),
        channel_id: peer_id
            .kind()
            .eq(&grammers_session::types::PeerKind::Channel)
            .then(|| peer_id.bare_id()),
        sender_id: msg.sender_id().map(|id| id.to_string()),
        text: msg.text().to_string(),
        sent_at: msg.date().to_rfc3339(),
        media_type: media.media_type,
        media_path: media.media_path,
        media_name: media.media_name,
        media_size: media.media_size,
    }
}

pub async fn record_deleted_message(
    db: &Database,
    cached: &CachedDeletedMessage,
    deleted_at: String,
) -> Result<()> {
    let password = db.master_password().await;
    let lock = SQLITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().await;
    prepare_database_file(password.as_deref()).await?;

    let database = open_database().await?;
    let connection = database.connect()?;
    init_schema(&connection).await?;
    upsert_account(&connection, &cached.account).await?;
    upsert_chat(&connection, &cached.account.id, &cached.chat).await?;
    insert_message(&connection, cached, &deleted_at).await?;
    drop(connection);
    drop(database);

    seal_database_file(password.as_deref()).await
}

pub async fn rewrap_storage(
    current_password: Option<&str>,
    next_password: Option<&str>,
) -> Result<()> {
    let lock = SQLITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().await;
    prepare_database_file(current_password).await?;

    if next_password.is_some() {
        seal_database_file(next_password).await
    } else {
        remove_encrypted_database().await
    }
}

pub async fn account_snapshot(
    client: &Client,
    session_file: &str,
) -> (AccountSnapshot, Option<PeerId>) {
    match client.get_me().await {
        Ok(user) => {
            let user_id = user.id();
            (
                AccountSnapshot {
                    id: user_id.to_string(),
                    name: user.first_name().unwrap_or("Unknown account").to_string(),
                    session_file: session_file.to_string(),
                },
                Some(user_id),
            )
        }
        Err(_) => (
            AccountSnapshot {
                id: "unknown".to_string(),
                name: "Unknown account".to_string(),
                session_file: session_file.to_string(),
            },
            None,
        ),
    }
}

async fn read_store(password: Option<String>) -> Result<Value> {
    let lock = SQLITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().await;
    prepare_database_file(password.as_deref()).await?;

    let database = open_database().await?;
    let connection = database.connect()?;
    init_schema(&connection).await?;
    let value = read_store_from_connection(&connection).await?;
    drop(connection);
    drop(database);

    seal_database_file(password.as_deref()).await?;
    Ok(value)
}

async fn read_delete_log(password: Option<String>, filters: DeleteLogFilters) -> Result<Value> {
    let lock = SQLITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().await;
    prepare_database_file(password.as_deref()).await?;

    let database = open_database().await?;
    let connection = database.connect()?;
    init_schema(&connection).await?;
    let value = read_delete_log_from_connection(&connection, filters).await?;
    drop(connection);
    drop(database);

    seal_database_file(password.as_deref()).await?;
    Ok(value)
}

async fn prepare_database_file(password: Option<&str>) -> Result<()> {
    if let Some(parent) = Path::new(DATABASE_PATH).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let encrypted_path = Path::new(ENCRYPTED_DATABASE_PATH);
    let database_path = Path::new(DATABASE_PATH);
    if let Some(password) = password {
        if encrypted_path.exists() {
            let encrypted = tokio::fs::read(encrypted_path).await?;
            let plain = crypto::decrypt_with_password(&encrypted, password)
                .context("failed to decrypt anti-delete database")?;
            tokio::fs::write(database_path, plain).await?;
        }
    } else if encrypted_path.exists() && !database_path.exists() {
        anyhow::bail!("anti-delete database is encrypted; master password is required");
    }
    Ok(())
}

async fn seal_database_file(password: Option<&str>) -> Result<()> {
    let Some(password) = password else {
        return Ok(());
    };
    let database_path = Path::new(DATABASE_PATH);
    if !database_path.exists() {
        return Ok(());
    }

    let plain = tokio::fs::read(database_path).await?;
    let encrypted = crypto::encrypt_with_password(&plain, password)
        .context("failed to encrypt anti-delete database")?;
    tokio::fs::write(ENCRYPTED_DATABASE_PATH, encrypted).await?;
    tokio::fs::remove_file(database_path).await?;
    Ok(())
}

async fn remove_encrypted_database() -> Result<()> {
    let encrypted_path = Path::new(ENCRYPTED_DATABASE_PATH);
    if encrypted_path.exists() {
        tokio::fs::remove_file(encrypted_path).await?;
    }
    Ok(())
}

async fn open_database() -> Result<SqliteDatabase> {
    Builder::new_local(DATABASE_PATH)
        .build()
        .await
        .context("failed to open anti-delete sqlite database")
}

async fn init_schema(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS accounts (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                session_file TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chats (
                account_id TEXT NOT NULL,
                id TEXT NOT NULL,
                title TEXT NOT NULL,
                kind TEXT NOT NULL,
                username TEXT,
                link TEXT,
                avatar_path TEXT,
                PRIMARY KEY (account_id, id)
            );

            CREATE TABLE IF NOT EXISTS deleted_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                account_id TEXT NOT NULL,
                chat_id TEXT NOT NULL,
                message_id INTEGER NOT NULL,
                sender_id TEXT,
                text TEXT NOT NULL,
                sent_at TEXT NOT NULL,
                deleted_at TEXT NOT NULL,
                media_type TEXT,
                media_path TEXT,
                media_name TEXT,
                media_size INTEGER
            );
            "#,
        )
        .await?;

    ensure_deleted_messages_columns(connection).await?;
    connection
        .execute_batch(
            r#"

            CREATE INDEX IF NOT EXISTS idx_deleted_messages_account_chat
                ON deleted_messages (account_id, chat_id, id DESC);

            CREATE INDEX IF NOT EXISTS idx_deleted_messages_media_type
                ON deleted_messages (media_type);

            CREATE INDEX IF NOT EXISTS idx_deleted_messages_deleted_at
                ON deleted_messages (deleted_at);
            "#,
        )
        .await?;
    Ok(())
}

async fn ensure_deleted_messages_columns(connection: &Connection) -> Result<()> {
    let mut rows = connection
        .query("PRAGMA table_info(deleted_messages)", ())
        .await?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next().await? {
        columns.push(row.get::<String>(1)?);
    }

    for (column, definition) in [
        ("media_type", "TEXT"),
        ("media_path", "TEXT"),
        ("media_name", "TEXT"),
        ("media_size", "INTEGER"),
    ] {
        if !columns.iter().any(|value| value == column) {
            let sql = format!("ALTER TABLE deleted_messages ADD COLUMN {column} {definition}");
            connection.execute(&sql, ()).await?;
        }
    }
    Ok(())
}

async fn upsert_account(connection: &Connection, account: &AccountSnapshot) -> Result<()> {
    connection
        .execute(
            r#"
            INSERT INTO accounts (id, name, session_file)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                session_file = excluded.session_file
            "#,
            params![
                account.id.as_str(),
                account.name.as_str(),
                account.session_file.as_str()
            ],
        )
        .await?;
    Ok(())
}

async fn upsert_chat(connection: &Connection, account_id: &str, chat: &ChatSnapshot) -> Result<()> {
    connection
        .execute(
            r#"
            INSERT INTO chats (account_id, id, title, kind, username, link, avatar_path)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(account_id, id) DO UPDATE SET
                title = excluded.title,
                kind = excluded.kind,
                username = excluded.username,
                link = excluded.link,
                avatar_path = excluded.avatar_path
            "#,
            params![
                account_id,
                chat.id.as_str(),
                chat.title.as_str(),
                chat.kind.as_str(),
                chat.username.as_deref(),
                chat.link.as_deref(),
                chat.avatar_path.as_deref()
            ],
        )
        .await?;
    Ok(())
}

async fn insert_message(
    connection: &Connection,
    cached: &CachedDeletedMessage,
    deleted_at: &str,
) -> Result<()> {
    connection
        .execute(
            r#"
            INSERT INTO deleted_messages (
                account_id,
                chat_id,
                message_id,
                sender_id,
                text,
                sent_at,
                deleted_at,
                media_type,
                media_path,
                media_name,
                media_size
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            "#,
            params![
                cached.account.id.as_str(),
                cached.chat.id.as_str(),
                cached.message_id,
                cached.sender_id.as_deref(),
                cached.text.as_str(),
                cached.sent_at.as_str(),
                deleted_at,
                cached.media_type.as_deref(),
                cached.media_path.as_deref(),
                cached.media_name.as_deref(),
                cached.media_size
            ],
        )
        .await?;
    Ok(())
}

async fn read_store_from_connection(connection: &Connection) -> Result<Value> {
    let mut root = Map::new();
    let mut accounts = Map::new();
    let mut rows = connection
        .query(
            r#"
            WITH ranked_messages AS (
                SELECT
                    account_id,
                    chat_id,
                    message_id,
                    sender_id,
                    text,
                    sent_at,
                    deleted_at,
                    media_type,
                    media_path,
                    media_name,
                    media_size,
                    id,
                    ROW_NUMBER() OVER (
                        PARTITION BY account_id, chat_id
                        ORDER BY id DESC
                    ) AS message_rank
                FROM deleted_messages
            )
            SELECT
                accounts.id,
                accounts.name,
                accounts.session_file,
                chats.id,
                chats.title,
                chats.kind,
                chats.username,
                chats.link,
                chats.avatar_path,
                ranked_messages.message_id,
                ranked_messages.sender_id,
                ranked_messages.text,
                ranked_messages.sent_at,
                ranked_messages.deleted_at,
                ranked_messages.media_type,
                ranked_messages.media_path,
                ranked_messages.media_name,
                ranked_messages.media_size
            FROM accounts
            LEFT JOIN chats ON chats.account_id = accounts.id
            LEFT JOIN ranked_messages
                ON ranked_messages.account_id = chats.account_id
                AND ranked_messages.chat_id = chats.id
                AND ranked_messages.message_rank <= 100
            ORDER BY accounts.name, chats.title, ranked_messages.id DESC
            "#,
            (),
        )
        .await?;

    while let Some(row) = rows.next().await? {
        let account_id = row.get::<String>(0)?;
        if !accounts.contains_key(&account_id) {
            let account = AccountSnapshot {
                id: account_id.clone(),
                name: row.get(1)?,
                session_file: row.get(2)?,
            };
            accounts.insert(
                account_id.clone(),
                serde_json::json!({ "account": account, "chats": {} }),
            );
        }

        let Some(chat_id) = row.get::<Option<String>>(3)? else {
            continue;
        };
        let chats = accounts
            .get_mut(&account_id)
            .and_then(|account| account.get_mut("chats"))
            .and_then(Value::as_object_mut)
            .expect("account chats are initialized as an object");
        if !chats.contains_key(&chat_id) {
            let chat = ChatSnapshot {
                id: chat_id.clone(),
                title: row.get(4)?,
                kind: row.get(5)?,
                username: row.get(6)?,
                link: row.get(7)?,
                avatar_path: row.get(8)?,
            };
            chats.insert(
                chat_id.clone(),
                serde_json::json!({ "chat": chat, "messages": [] }),
            );
        }

        let Some(message_id) = row.get::<Option<i32>>(9)? else {
            continue;
        };
        chats
            .get_mut(&chat_id)
            .and_then(|chat| chat.get_mut("messages"))
            .and_then(Value::as_array_mut)
            .expect("chat messages are initialized as an array")
            .push(serde_json::json!({
                "message_id": message_id,
                "sender_id": row.get::<Option<String>>(10)?,
                "text": row.get::<String>(11)?,
                "sent_at": row.get::<String>(12)?,
                "deleted_at": row.get::<String>(13)?,
                "media_type": row.get::<Option<String>>(14)?,
                "media_path": row.get::<Option<String>>(15)?,
                "media_name": row.get::<Option<String>>(16)?,
                "media_size": row.get::<Option<i64>>(17)?,
            }));
    }

    root.insert("accounts".to_string(), Value::Object(accounts));
    Ok(Value::Object(root))
}

async fn read_delete_log_from_connection(
    connection: &Connection,
    filters: DeleteLogFilters,
) -> Result<Value> {
    let query = normalized_like(filters.q.as_deref());
    let account = normalized_exact(filters.account.as_deref());
    let chat = normalized_like(filters.chat.as_deref());
    let media_type = normalized_exact(filters.media_type.as_deref());
    let limit = filters
        .limit
        .unwrap_or(DEFAULT_DELETE_LOG_LIMIT)
        .clamp(1, MAX_DELETE_LOG_LIMIT);

    let mut rows = connection
        .query(
            r#"
            SELECT
                deleted_messages.id,
                deleted_messages.message_id,
                deleted_messages.sender_id,
                deleted_messages.text,
                deleted_messages.sent_at,
                deleted_messages.deleted_at,
                deleted_messages.media_type,
                deleted_messages.media_path,
                deleted_messages.media_name,
                deleted_messages.media_size,
                accounts.id,
                accounts.name,
                accounts.session_file,
                chats.id,
                chats.title,
                chats.kind,
                chats.username,
                chats.link,
                chats.avatar_path
            FROM deleted_messages
            JOIN accounts ON accounts.id = deleted_messages.account_id
            JOIN chats
                ON chats.account_id = deleted_messages.account_id
                AND chats.id = deleted_messages.chat_id
            WHERE
                (
                    ?1 IS NULL
                    OR lower(accounts.name) LIKE ?1
                    OR lower(accounts.id) LIKE ?1
                    OR lower(chats.title) LIKE ?1
                    OR lower(chats.id) LIKE ?1
                    OR lower(coalesce(chats.username, '')) LIKE ?1
                    OR lower(coalesce(deleted_messages.sender_id, '')) LIKE ?1
                    OR lower(coalesce(deleted_messages.media_type, '')) LIKE ?1
                    OR lower(coalesce(deleted_messages.media_name, '')) LIKE ?1
                    OR lower(deleted_messages.text) LIKE ?1
                )
                AND (
                    ?2 IS NULL
                    OR accounts.id = ?2
                    OR accounts.session_file = ?2
                )
                AND (
                    ?3 IS NULL
                    OR lower(chats.id) LIKE ?3
                    OR lower(chats.title) LIKE ?3
                    OR lower(coalesce(chats.username, '')) LIKE ?3
                )
                AND (?4 IS NULL OR deleted_messages.media_type = ?4)
            ORDER BY deleted_messages.id DESC
            LIMIT ?5
            "#,
            params![
                query.as_deref(),
                account.as_deref(),
                chat.as_deref(),
                media_type.as_deref(),
                limit
            ],
        )
        .await?;

    let mut messages = Vec::new();
    while let Some(row) = rows.next().await? {
        messages.push(serde_json::json!({
            "id": row.get::<i64>(0)?,
            "message_id": row.get::<i32>(1)?,
            "sender_id": row.get::<Option<String>>(2)?,
            "text": row.get::<String>(3)?,
            "sent_at": row.get::<String>(4)?,
            "deleted_at": row.get::<String>(5)?,
            "media_type": row.get::<Option<String>>(6)?,
            "media_path": row.get::<Option<String>>(7)?,
            "media_name": row.get::<Option<String>>(8)?,
            "media_size": row.get::<Option<i64>>(9)?,
            "account": {
                "id": row.get::<String>(10)?,
                "name": row.get::<String>(11)?,
                "session_file": row.get::<String>(12)?,
            },
            "chat": {
                "id": row.get::<String>(13)?,
                "title": row.get::<String>(14)?,
                "kind": row.get::<String>(15)?,
                "username": row.get::<Option<String>>(16)?,
                "link": row.get::<Option<String>>(17)?,
                "avatar_path": row.get::<Option<String>>(18)?,
            },
        }));
    }

    Ok(serde_json::json!({
        "messages": messages,
        "limit": limit,
    }))
}

async fn snapshot_chat(client: &Client, account: &AccountSnapshot, msg: &Message) -> ChatSnapshot {
    let peer_id = msg.peer_id();
    let peer = msg.peer();
    let username = peer.and_then(Peer::username).map(str::to_string);
    let link = username
        .as_ref()
        .map(|value| format!("https://t.me/{value}"));
    let avatar_path = download_avatar(client, account, peer).await.ok().flatten();

    ChatSnapshot {
        id: peer_id.to_string(),
        title: peer
            .and_then(Peer::name)
            .unwrap_or("Unknown chat")
            .to_string(),
        kind: format!("{:?}", peer_id.kind()),
        username,
        link,
        avatar_path,
    }
}

fn normalized_like(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{}%", value.to_lowercase()))
}

fn normalized_exact(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "all")
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::time::Instant;

    #[tokio::test]
    async fn read_store_builds_hierarchy_and_limits_each_chat_to_100_messages() {
        let path = env::temp_dir().join(format!(
            "fly_antidelete_store_{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let database = Builder::new_local(&path)
            .build()
            .await
            .expect("test sqlite database should be created");
        let connection = database
            .connect()
            .expect("test sqlite connection should be opened");
        init_schema(&connection)
            .await
            .expect("test schema should initialize");

        connection
            .execute_batch(
                r#"
                INSERT INTO accounts VALUES ('empty', 'Empty', 'empty.session');
                INSERT INTO accounts VALUES ('active', 'Active', 'active.session');
                INSERT INTO chats VALUES ('active', 'quiet', 'Quiet', 'User', NULL, NULL, NULL);
                INSERT INTO chats VALUES ('active', 'busy', 'Busy', 'User', 'busy', 'https://t.me/busy', NULL);
                "#,
            )
            .await
            .expect("account and chat fixtures should load");
        for message_id in 0..105 {
            connection
                .execute(
                    r#"
                    INSERT INTO deleted_messages (
                        account_id, chat_id, message_id, text, sent_at, deleted_at
                    ) VALUES ('active', 'busy', ?1, ?2, 'sent', 'deleted')
                    "#,
                    params![message_id, format!("message {message_id}")],
                )
                .await
                .expect("message fixture should load");
        }

        let store = read_store_from_connection(&connection)
            .await
            .expect("store should load with one joined query");

        assert_eq!(store["accounts"]["empty"]["account"]["name"], "Empty");
        assert_eq!(
            store["accounts"]["empty"]["chats"]
                .as_object()
                .expect("empty account chats should be an object")
                .len(),
            0
        );
        assert_eq!(
            store["accounts"]["active"]["chats"]["quiet"]["messages"]
                .as_array()
                .expect("quiet chat messages should be an array")
                .len(),
            0
        );
        let messages = store["accounts"]["active"]["chats"]["busy"]["messages"]
            .as_array()
            .expect("busy chat messages should be an array");
        assert_eq!(messages.len(), 100);
        assert_eq!(messages.first().expect("newest message")["message_id"], 104);
        assert_eq!(
            messages.last().expect("oldest retained message")["message_id"],
            5
        );

        drop(connection);
        drop(database);
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    #[ignore = "manual performance benchmark"]
    async fn benchmark_read_store_from_connection() {
        let path = env::temp_dir().join(format!(
            "fly_antidelete_benchmark_{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let database = Builder::new_local(&path)
            .build()
            .await
            .expect("benchmark sqlite database should be created");
        let connection = database
            .connect()
            .expect("benchmark sqlite connection should be opened");
        init_schema(&connection)
            .await
            .expect("benchmark schema should initialize");

        let mut fixture = String::from("BEGIN;");
        for account in 0..10 {
            fixture.push_str(&format!(
                "INSERT INTO accounts VALUES ('a{account}', 'Account {account:02}', 's{account}');"
            ));
            for chat in 0..20 {
                fixture.push_str(&format!(
                    "INSERT INTO chats VALUES ('a{account}', 'c{chat}', 'Chat {chat:02}', 'User', NULL, NULL, NULL);"
                ));
                for message in 0..10 {
                    fixture.push_str(&format!(
                        "INSERT INTO deleted_messages (account_id, chat_id, message_id, text, sent_at, deleted_at) VALUES ('a{account}', 'c{chat}', {message}, 'text', 'sent', 'deleted');"
                    ));
                }
            }
        }
        fixture.push_str("COMMIT;");
        connection
            .execute_batch(&fixture)
            .await
            .expect("benchmark fixture should load");

        let legacy_value = legacy_read_store_from_connection(&connection)
            .await
            .expect("legacy warm-up read should succeed");
        let legacy_started = Instant::now();
        for _ in 0..10 {
            legacy_read_store_from_connection(&connection)
                .await
                .expect("legacy benchmark read should succeed");
        }
        let legacy_elapsed = legacy_started.elapsed();

        let joined_value = read_store_from_connection(&connection)
            .await
            .expect("joined warm-up read should succeed");
        assert_eq!(joined_value, legacy_value);
        let joined_started = Instant::now();
        for _ in 0..10 {
            read_store_from_connection(&connection)
                .await
                .expect("joined benchmark read should succeed");
        }
        let joined_elapsed = joined_started.elapsed();
        eprintln!("read_store 10 iterations: legacy={legacy_elapsed:?}, joined={joined_elapsed:?}");

        drop(connection);
        drop(database);
        let _ = tokio::fs::remove_file(path).await;
    }

    async fn legacy_read_store_from_connection(connection: &Connection) -> Result<Value> {
        let mut accounts = Map::new();
        let mut account_rows = connection
            .query(
                "SELECT id, name, session_file FROM accounts ORDER BY name",
                (),
            )
            .await?;
        while let Some(row) = account_rows.next().await? {
            let account = AccountSnapshot {
                id: row.get(0)?,
                name: row.get(1)?,
                session_file: row.get(2)?,
            };
            accounts.insert(
                account.id.clone(),
                legacy_read_account(connection, account).await?,
            );
        }
        Ok(serde_json::json!({ "accounts": accounts }))
    }

    async fn legacy_read_account(
        connection: &Connection,
        account: AccountSnapshot,
    ) -> Result<Value> {
        let mut chats = Map::new();
        let mut chat_rows = connection
            .query(
                r#"
                SELECT id, title, kind, username, link, avatar_path
                FROM chats WHERE account_id = ?1 ORDER BY title
                "#,
                params![account.id.as_str()],
            )
            .await?;
        while let Some(row) = chat_rows.next().await? {
            let chat = ChatSnapshot {
                id: row.get(0)?,
                title: row.get(1)?,
                kind: row.get(2)?,
                username: row.get(3)?,
                link: row.get(4)?,
                avatar_path: row.get(5)?,
            };
            let messages = legacy_read_chat_messages(connection, &account.id, &chat.id).await?;
            chats.insert(
                chat.id.clone(),
                serde_json::json!({ "chat": chat, "messages": messages }),
            );
        }
        Ok(serde_json::json!({ "account": account, "chats": chats }))
    }

    async fn legacy_read_chat_messages(
        connection: &Connection,
        account_id: &str,
        chat_id: &str,
    ) -> Result<Vec<Value>> {
        let mut rows = connection
            .query(
                r#"
                SELECT message_id, sender_id, text, sent_at, deleted_at,
                    media_type, media_path, media_name, media_size
                FROM deleted_messages
                WHERE account_id = ?1 AND chat_id = ?2
                ORDER BY id DESC LIMIT 100
                "#,
                params![account_id, chat_id],
            )
            .await?;
        let mut messages = Vec::new();
        while let Some(row) = rows.next().await? {
            messages.push(serde_json::json!({
                "message_id": row.get::<i32>(0)?,
                "sender_id": row.get::<Option<String>>(1)?,
                "text": row.get::<String>(2)?,
                "sent_at": row.get::<String>(3)?,
                "deleted_at": row.get::<String>(4)?,
                "media_type": row.get::<Option<String>>(5)?,
                "media_path": row.get::<Option<String>>(6)?,
                "media_name": row.get::<Option<String>>(7)?,
                "media_size": row.get::<Option<i64>>(8)?,
            }));
        }
        Ok(messages)
    }

    #[tokio::test]
    async fn init_schema_migrates_existing_delete_log_media_columns() {
        let path = env::temp_dir().join(format!(
            "fly_antidelete_schema_{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let database = Builder::new_local(&path)
            .build()
            .await
            .expect("test sqlite database should be created");
        let connection = database
            .connect()
            .expect("test sqlite connection should be opened");

        connection
            .execute_batch(
                r#"
                CREATE TABLE deleted_messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    account_id TEXT NOT NULL,
                    chat_id TEXT NOT NULL,
                    message_id INTEGER NOT NULL,
                    sender_id TEXT,
                    text TEXT NOT NULL,
                    sent_at TEXT NOT NULL,
                    deleted_at TEXT NOT NULL
                );
                "#,
            )
            .await
            .expect("legacy delete-log table should be created");

        init_schema(&connection)
            .await
            .expect("schema migration should add media columns before indexing");

        let mut rows = connection
            .query("PRAGMA table_info(deleted_messages)", ())
            .await
            .expect("table info should be queryable");
        let mut columns = Vec::new();
        while let Some(row) = rows.next().await.expect("table info row should load") {
            columns.push(row.get::<String>(1).expect("column name should load"));
        }

        for column in ["media_type", "media_path", "media_name", "media_size"] {
            assert!(
                columns.iter().any(|value| value == column),
                "{column} should be added to legacy delete-log tables"
            );
        }

        drop(connection);
        drop(database);
        let _ = tokio::fs::remove_file(path).await;
    }
}
