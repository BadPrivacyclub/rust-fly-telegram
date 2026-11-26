//! Encrypted backups in a private channel of the first account.
//!
//! The archive (database, its storage key, config, modules) is encrypted with a backup
//! password of its own, posted to the channel, downloaded again and decrypted to check it,
//! and older copies beyond the kept number are deleted. Sessions are never included: the
//! backup lives on the same account, so it cannot bring back a lost one. Session recovery
//! with access keys covers that.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use grammers_client::Client;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::app::Services;
use crate::database::Database;
use crate::private_channel::PrivateChannel;

pub const PASSWORD_KEY: &str = "backup.offsite.password";
pub const EVERY_HOURS_KEY: &str = "backup.offsite.every_hours";
pub const KEEP_KEY: &str = "backup.offsite.keep";
pub const LAST_KEY: &str = "backup.offsite.last";
const CHANNEL_KEY: &str = "backup.offsite.channel";
const CHANNEL_TITLE: &str = "fly-telegram backups";
/// Marks backup messages in the channel.
const TAG: &str = "#flybackup";
const DEFAULT_KEEP: u64 = 5;
const MAX_KEEP: u64 = 50;
const TICK: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub id: i32,
    pub name: String,
    /// Unix time in seconds.
    pub date: i64,
    pub size: i64,
}

#[derive(Serialize)]
pub struct Settings {
    pub password_set: bool,
    pub every_hours: u64,
    pub keep: u64,
    pub last: Option<i64>,
}

pub async fn settings(db: &Database) -> Settings {
    Settings {
        password_set: password(db).await.is_some(),
        every_hours: db.get(EVERY_HOURS_KEY).await.as_u64().unwrap_or(0),
        keep: keep(db).await,
        last: db.get(LAST_KEY).await.as_i64(),
    }
}

async fn password(db: &Database) -> Option<String> {
    db.get(PASSWORD_KEY)
        .await
        .as_str()
        .filter(|password| !password.is_empty())
        .map(str::to_string)
}

async fn keep(db: &Database) -> u64 {
    db.get(KEEP_KEY)
        .await
        .as_u64()
        .unwrap_or(DEFAULT_KEEP)
        .clamp(1, MAX_KEEP)
}

pub async fn save_settings(
    db: &Database,
    password: Option<&str>,
    every_hours: Option<u64>,
    keep: Option<u64>,
) -> Result<()> {
    if let Some(password) = password {
        if password.is_empty() {
            db.remove(PASSWORD_KEY).await?;
        } else if password.chars().count() < 8 {
            anyhow::bail!("the backup password needs at least 8 characters");
        } else {
            db.set(PASSWORD_KEY, Value::String(password.to_string()))
                .await?;
        }
    }
    if let Some(hours) = every_hours {
        db.set(EVERY_HOURS_KEY, Value::from(hours.min(24 * 30)))
            .await?;
    }
    if let Some(keep) = keep {
        db.set(KEEP_KEY, Value::from(keep.clamp(1, MAX_KEEP)))
            .await?;
    }
    Ok(())
}

async fn first_account(services: &Services) -> Result<Client> {
    let mut accounts = services.accounts.list().await;
    accounts.sort_by(|a, b| a.session_file.cmp(&b.session_file));
    accounts
        .into_iter()
        .next()
        .map(|handle| handle.client)
        .context("no Telegram account is connected")
}

async fn channel(services: &Services, client: &Client) -> Result<PrivateChannel> {
    crate::private_channel::ensure(
        client,
        &services.db,
        CHANNEL_KEY,
        CHANNEL_TITLE,
        "Encrypted fly-telegram backups. Restore them from the panel, Backups.",
        true,
    )
    .await
}

/// Encrypts an archive. The output starts with a line naming the format.
pub fn seal(plain: &[u8], password: &str) -> Result<Vec<u8>> {
    let mut out = b"FLYBAK1\n".to_vec();
    out.extend(crate::crypto::encrypt_with_password(plain, password)?);
    Ok(out)
}

pub fn open(sealed: &[u8], password: &str) -> Result<Vec<u8>> {
    let body = sealed
        .strip_prefix(b"FLYBAK1\n")
        .context("not a fly-telegram backup")?;
    crate::crypto::decrypt_with_password(body, password)
        .map_err(|_| anyhow::anyhow!("wrong backup password or a damaged file"))
}

fn temp_path(root: &Path, name: &str) -> PathBuf {
    root.join(crate::backup::BACKUP_DIR).join(name)
}

/// Makes a backup now and returns what was posted.
pub async fn run(services: &Arc<Services>) -> Result<Item> {
    let db = &services.db;
    let password = password(db).await.context("set a backup password first")?;
    let client = first_account(services).await?;
    let channel = channel(services, &client).await?;
    let peer = channel
        .peer_ref()
        .context("the backup channel is not usable")?;

    let root = std::env::current_dir()?;
    let (name, sealed, digest) = {
        let root = root.clone();
        tokio::task::spawn_blocking(move || -> Result<(String, Vec<u8>, Vec<u8>)> {
            let archive = crate::backup::create_archive_with(&root, true)?;
            let plain = std::fs::read(&archive)?;
            let _ = std::fs::remove_file(&archive);
            let name = format!("fly-telegram-backup-{}.flybak", crate::backup::timestamp());
            let digest = Sha256::digest(&plain).to_vec();
            Ok((name, seal(&plain, &password)?, digest))
        })
        .await??
    };
    let upload = temp_path(&root, &name);
    tokio::fs::write(&upload, &sealed).await?;
    let uploaded = client.upload_file(&upload).await;
    let _ = tokio::fs::remove_file(&upload).await;
    let uploaded = uploaded?;
    services.runtime.wait_for_telegram_send().await;
    let caption = format!("{TAG} {name}\n{} bytes", sealed.len());
    let message = client
        .send_message(
            peer,
            grammers_client::message::InputMessage::new()
                .text(caption)
                .document(uploaded),
        )
        .await?;

    // The copy in the channel is downloaded and opened again before it counts.
    let check = temp_path(&root, &format!("check-{name}"));
    let downloaded = message.download_media(&check).await;
    let bytes = tokio::fs::read(&check).await;
    let _ = tokio::fs::remove_file(&check).await;
    if !downloaded? {
        anyhow::bail!("the posted backup has no file");
    }
    let password = self::password(db).await.unwrap_or_default();
    let reopened = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
        Ok(Sha256::digest(open(&bytes?, &password)?).to_vec())
    })
    .await??;
    if reopened != digest {
        anyhow::bail!("the posted backup does not match the archive");
    }

    prune(&client, peer, keep(db).await).await;
    let now = chrono::Utc::now().timestamp();
    db.set(LAST_KEY, Value::from(now)).await?;
    Ok(Item {
        id: message.id(),
        name,
        date: now,
        size: sealed.len() as i64,
    })
}

/// Backups in the channel, newest first.
pub async fn list(services: &Services) -> Result<Vec<Item>> {
    let client = first_account(services).await?;
    let Some(channel) = crate::private_channel::stored(&services.db, CHANNEL_KEY).await else {
        return Ok(Vec::new());
    };
    let peer = channel
        .peer_ref()
        .context("the backup channel is not usable")?;
    let mut items = Vec::new();
    let mut messages = client.iter_messages(peer).limit(300);
    while let Some(message) = messages.next().await? {
        if let Some(item) = item_of(&message) {
            items.push(item);
        }
    }
    Ok(items)
}

fn item_of(message: &grammers_client::message::Message) -> Option<Item> {
    let rest = message.text().strip_prefix(TAG)?.trim();
    let name = rest.lines().next()?.trim().to_string();
    let size = match message.media() {
        Some(grammers_client::media::Media::Document(document)) => {
            document.size().map_or(0, |size| size as i64)
        }
        _ => return None,
    };
    Some(Item {
        id: message.id(),
        name,
        date: message.date().timestamp(),
        size,
    })
}

async fn prune(client: &Client, peer: grammers_session::types::PeerRef, keep: u64) {
    let mut ids = Vec::new();
    let mut messages = client.iter_messages(peer).limit(300);
    loop {
        match messages.next().await {
            Ok(Some(message)) => {
                if item_of(&message).is_some() {
                    ids.push(message.id());
                }
            }
            Ok(None) => break,
            Err(error) => {
                warn!("could not list old backups: {error}");
                return;
            }
        }
    }
    let old = ids.into_iter().skip(keep as usize).collect::<Vec<_>>();
    if !old.is_empty() {
        if let Err(error) = client.delete_messages(peer, &old).await {
            warn!("could not delete old backups: {error}");
        }
    }
}

/// Restores backup message `id`, with `password` or the stored backup password. The caller
/// restarts afterwards.
pub async fn restore(
    services: &Services,
    id: i32,
    password: Option<String>,
) -> Result<Vec<String>> {
    let db = &services.db;
    let password = match password.filter(|p| !p.is_empty()) {
        Some(password) => password,
        None => self::password(db)
            .await
            .context("enter the backup password")?,
    };
    let client = first_account(services).await?;
    let channel = crate::private_channel::stored(db, CHANNEL_KEY)
        .await
        .context("there is no backup channel yet")?;
    let peer = channel
        .peer_ref()
        .context("the backup channel is not usable")?;
    let message = client
        .get_messages_by_id(peer, &[id])
        .await?
        .into_iter()
        .flatten()
        .next()
        .context("this backup is gone")?;
    let root = std::env::current_dir()?;
    tokio::fs::create_dir_all(root.join(crate::backup::BACKUP_DIR)).await?;
    let download = temp_path(
        &root,
        &format!("incoming-{}.flybak", crate::backup::timestamp()),
    );
    let downloaded = message.download_media(&download).await;
    let sealed = tokio::fs::read(&download).await;
    let _ = tokio::fs::remove_file(&download).await;
    if !downloaded? {
        anyhow::bail!("this message has no backup file");
    }
    let sealed = sealed?;
    tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
        let plain = open(&sealed, &password)?;
        let archive = temp_path(
            &root,
            &format!("incoming-{}.tar.gz", crate::backup::timestamp()),
        );
        std::fs::write(&archive, plain)?;
        let restored = crate::backup::restore_archive(&root, &archive);
        let _ = std::fs::remove_file(&archive);
        restored
    })
    .await?
}

/// Makes a backup whenever the set number of hours has passed since the last one.
pub fn spawn(services: Arc<Services>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        loop {
            interval.tick().await;
            let db = &services.db;
            let every = db.get(EVERY_HOURS_KEY).await.as_u64().unwrap_or(0);
            if every == 0 || password(db).await.is_none() {
                continue;
            }
            let last = db.get(LAST_KEY).await.as_i64().unwrap_or(0);
            if chrono::Utc::now().timestamp() - last < (every * 3600) as i64 {
                continue;
            }
            if services.accounts.list().await.is_empty() {
                continue;
            }
            if let Err(error) = run(&services).await {
                warn!("scheduled backup failed: {error:#}");
                services
                    .notifier
                    .send(
                        crate::notify::Topic::Errors,
                        &format!(
                            "<b>Backup failed</b>\n{}",
                            crate::notify::escape_html(&format!("{error:#}"))
                        ),
                    )
                    .await;
                // Wait a full interval before trying again.
                let _ = db
                    .set(LAST_KEY, Value::from(chrono::Utc::now().timestamp()))
                    .await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_backups_open_only_with_the_password() {
        let sealed = seal(b"archive bytes", "correct horse").unwrap();
        assert!(sealed.starts_with(b"FLYBAK1\n"));
        assert_eq!(open(&sealed, "correct horse").unwrap(), b"archive bytes");
        assert!(open(&sealed, "wrong").is_err());
        assert!(open(b"something else", "correct horse").is_err());
    }

    #[tokio::test]
    async fn settings_are_validated() {
        let path = std::env::temp_dir().join(format!("fly_offsite_{}.json", uuid::Uuid::new_v4()));
        let db = Database::load(&path).await.unwrap();
        assert!(save_settings(&db, Some("short"), None, None).await.is_err());
        save_settings(&db, Some("long enough"), Some(24), Some(500))
            .await
            .unwrap();
        let current = settings(&db).await;
        assert!(current.password_set);
        assert_eq!(current.every_hours, 24);
        assert_eq!(current.keep, MAX_KEEP);
        save_settings(&db, Some(""), None, None).await.unwrap();
        assert!(!settings(&db).await.password_set);
        let _ = tokio::fs::remove_file(&path).await;
        let _ = tokio::fs::remove_file(path.with_extension("json.key")).await;
    }
}
