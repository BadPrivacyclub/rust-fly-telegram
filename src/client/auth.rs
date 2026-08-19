use std::sync::Arc;

use anyhow::{Context, Result};
use grammers_client::client::{LoginToken, UpdateStream, UpdatesConfiguration};
use grammers_client::{Client, SignInError};
use grammers_mtsender::{ConnectionParams, SenderPool};
use grammers_session::storages::SqliteSession;
use grammers_session::updates::UpdatesLike;
use tokio::sync::mpsc;
use tracing::info;

use crate::config::{db_key, DEFAULT_SESSION_FILE, SESSIONS_DIR};
use crate::database::Database;

pub struct PendingAuth {
    pub client: Client,
    pub token: LoginToken,
    pub session_file: String,
    pub updates: mpsc::UnboundedReceiver<UpdatesLike>,
    pub pool: tokio::task::JoinHandle<()>,
}

pub enum SignInOutcome {
    NeedPassword {
        hint: Option<String>,
        pending: PendingAuth,
    },
    Failed(anyhow::Error),
}

pub struct Connection {
    pub client: Client,
    pub updates: UpdateStream,
    pub pool: tokio::task::JoinHandle<()>,
    pub session_file: String,
}

/// Uses an existing session or runs the selected authorization flow.
/// Web authorization runs before credential reads because it populates the database on first use.
pub async fn connect(db: Arc<Database>, use_web: bool) -> Result<Connection> {
    let mut session_file = read_optional_config(&db, db_key::SESSION_FILE)
        .await
        .map(|value| normalize_session_file(&value))
        .unwrap_or_else(|| normalize_session_file(DEFAULT_SESSION_FILE));
    if use_web && !session_exists(&session_file) {
        crate::web::run_until_authorized(Arc::clone(&db)).await?;
        session_file = read_optional_config(&db, db_key::SESSION_FILE)
            .await
            .map(|value| normalize_session_file(&value))
            .unwrap_or_else(|| normalize_session_file(DEFAULT_SESSION_FILE));
    }

    let api_id: i32 = read_config(&db, db_key::API_ID)
        .await?
        .parse()
        .context("api_id must be an integer")?;
    let api_hash = read_config(&db, db_key::API_HASH).await?;

    let proxy_url = read_optional_config(&db, db_key::PROXY_URL).await;
    let (client, updates_rx, pool) = open_pool(api_id, &session_file, proxy_url).await?;

    if !client.is_authorized().await? {
        authorize_via_cli(&client, &db, &api_hash).await?;
    }

    remember_session_file(&db, &session_file).await?;
    finish_connection(client, updates_rx, pool, session_file).await
}

pub async fn connect_session(db: Arc<Database>, session_file: String) -> Result<Connection> {
    let session_file = normalize_session_file(&session_file);
    let api_id: i32 = read_config(&db, db_key::API_ID)
        .await?
        .parse()
        .context("api_id must be an integer")?;
    let api_hash = read_config(&db, db_key::API_HASH).await?;
    let proxy_url = read_optional_config(&db, db_key::PROXY_URL).await;
    let (client, updates_rx, pool) = open_pool(api_id, &session_file, proxy_url).await?;

    if !client.is_authorized().await? {
        authorize_via_cli(&client, &db, &api_hash).await?;
    }

    finish_connection(client, updates_rx, pool, session_file).await
}

pub async fn session_files(db: &Database) -> Vec<String> {
    let mut sessions = Vec::new();
    if session_exists(DEFAULT_SESSION_FILE) {
        push_unique_session(&mut sessions, DEFAULT_SESSION_FILE);
    }

    if let Some(session_file) = read_optional_config(db, db_key::SESSION_FILE).await {
        push_unique_session(&mut sessions, &session_file);
    }

    if let Some(values) = db.get(db_key::SESSION_FILES).await.as_array() {
        for value in values {
            if let Some(session_file) = value.as_str() {
                push_unique_session(&mut sessions, session_file);
            }
        }
    }

    discover_session_files(&mut sessions).await;
    sessions
}

pub async fn remember_session_file(db: &Database, session_file: &str) -> Result<()> {
    let session_file = normalize_session_file(session_file);
    let mut sessions = session_files(db).await;
    push_unique_session(&mut sessions, &session_file);
    db.set(
        db_key::SESSION_FILES,
        serde_json::Value::Array(
            sessions
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    )
    .await
}

async fn finish_connection(
    client: Client,
    updates_rx: mpsc::UnboundedReceiver<UpdatesLike>,
    pool: tokio::task::JoinHandle<()>,
    session_file: String,
) -> Result<Connection> {
    info!("authorized as {:?}", client.get_me().await?.first_name());

    let update_stream = client
        .stream_updates(
            updates_rx,
            UpdatesConfiguration {
                catch_up: false,
                ..Default::default()
            },
        )
        .await;

    Ok(Connection {
        client,
        updates: update_stream,
        pool,
        session_file,
    })
}

pub async fn connect_and_send_code(
    api_id: i32,
    api_hash: &str,
    phone: &str,
    session_file: &str,
    proxy_url: Option<String>,
) -> Result<PendingAuth> {
    let (client, updates, pool) = open_pool(api_id, session_file, proxy_url).await?;

    let token = client
        .request_login_code(phone, api_hash)
        .await
        .context("failed to send login code to Telegram")?;

    Ok(PendingAuth {
        client,
        token,
        session_file: session_file.to_string(),
        updates,
        pool,
    })
}

/// Preserves pending state only when a 2FA password is still required.
/// Terminal outcomes abort the pool so it cannot remain detached.
pub async fn complete_sign_in(
    pending: PendingAuth,
    code: &str,
    password: &str,
) -> std::result::Result<String, SignInOutcome> {
    let PendingAuth {
        client,
        token,
        session_file,
        updates,
        pool,
    } = pending;

    match client.sign_in(&token, code).await {
        Ok(_) => {
            // The web handler cannot retain this pool; connect opens a fresh one after authorization.
            abort_pool(pool).await;
        }
        Err(SignInError::PasswordRequired(pw_token)) => {
            if password.is_empty() {
                let hint = pw_token.hint().map(str::to_string);
                return Err(SignInOutcome::NeedPassword {
                    hint,
                    pending: PendingAuth {
                        client,
                        token,
                        session_file,
                        updates,
                        pool,
                    },
                });
            }
            if let Err(error) = client.check_password(pw_token, password.as_bytes()).await {
                abort_pool(pool).await;
                return Err(SignInOutcome::Failed(anyhow::anyhow!(error.to_string())));
            }
            abort_pool(pool).await;
        }
        Err(e) => {
            abort_pool(pool).await;
            return Err(SignInOutcome::Failed(anyhow::anyhow!(e.to_string())));
        }
    }

    Ok(session_file)
}

async fn authorize_via_cli(client: &Client, db: &Database, api_hash: &str) -> Result<()> {
    let phone = read_config(db, db_key::PHONE).await?;

    let token = client
        .request_login_code(&phone, api_hash)
        .await
        .context("failed to request login code")?;

    let code = prompt("Enter the login code: ")?;

    match client.sign_in(&token, &code).await {
        Ok(_) => {}
        Err(SignInError::PasswordRequired(pw_token)) => {
            let password = prompt(&format!(
                "Two-factor authentication required (hint: {})\nPassword: ",
                pw_token.hint().unwrap_or("none")
            ))?;
            client
                .check_password(pw_token, password.as_bytes())
                .await
                .context("incorrect 2FA password")?;
        }
        Err(e) => anyhow::bail!("sign-in failed: {e}"),
    }

    Ok(())
}

fn session_exists(session_file: &str) -> bool {
    std::path::Path::new(session_file).exists()
}

async fn open_pool(
    api_id: i32,
    session_file: &str,
    proxy_url: Option<String>,
) -> Result<(
    Client,
    mpsc::UnboundedReceiver<UpdatesLike>,
    tokio::task::JoinHandle<()>,
)> {
    if let Some(parent) = std::path::Path::new(session_file).parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    let session = Arc::new(SqliteSession::open(session_file).await?);
    let params = ConnectionParams {
        proxy_url,
        ..Default::default()
    };
    let SenderPool {
        runner,
        handle,
        updates,
    } = SenderPool::with_configuration(Arc::clone(&session), api_id, params);
    let client = Client::new(handle);
    let pool = tokio::spawn(runner.run());
    Ok((client, updates, pool))
}

async fn read_config(db: &Database, key: &str) -> Result<String> {
    let value = db.get(key).await;
    if let Some(s) = value.as_str() {
        return Ok(s.to_string());
    }
    let input = prompt(&format!("Enter {key}: "))?;
    db.set(key, serde_json::Value::String(input.clone()))
        .await
        .with_context(|| format!("failed to save {key}"))?;
    Ok(input)
}

async fn read_optional_config(db: &Database, key: &str) -> Option<String> {
    let value = db.get(key).await;
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn push_unique_session(sessions: &mut Vec<String>, session_file: &str) {
    let session_file = normalize_session_file(session_file);
    if !sessions.iter().any(|value| value == &session_file) {
        sessions.push(session_file);
    }
}

fn normalize_session_file(session_file: &str) -> String {
    session_file.trim().replace('\\', "/")
}

async fn abort_pool(pool: tokio::task::JoinHandle<()>) {
    pool.abort();
    let _ = pool.await;
}

async fn discover_session_files(sessions: &mut Vec<String>) {
    let Ok(mut entries) = tokio::fs::read_dir(SESSIONS_DIR).await else {
        return;
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "session")
        {
            push_unique_session(sessions, &path.to_string_lossy());
        }
    }
}

fn prompt(message: &str) -> Result<String> {
    use std::io::Write;
    print!("{message}");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_unique_session_normalizes_path_separators() {
        let mut sessions = Vec::new();
        push_unique_session(&mut sessions, "sessions/test.session");
        push_unique_session(&mut sessions, "sessions\\test.session");

        assert_eq!(sessions, vec!["sessions/test.session"]);
    }
}
