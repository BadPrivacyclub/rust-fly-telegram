//! Backups of configuration, database, and modules as `.tar.gz` archives.
//!
//! Telegram sessions, the service env file, and anti-delete archives are never included:
//! a backup is meant to be stored in Saved Messages, and a session file there would let
//! anyone who reads it take over the account.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use grammers_client::message::InputMessage;

use crate::config;
use crate::loader::context::Ctx;
use crate::telegram;

pub const BACKUP_DIR: &str = "backups";
const MAX_ARCHIVE_BYTES: u64 = 50 * 1024 * 1024;

/// Files and directories captured in a backup, relative to the data directory.
const INCLUDED: &[&str] = &[
    config::DATABASE_FILE,
    config::DATABASE_ENCRYPTED_FILE,
    crate::settings::CONFIG_FILE,
    config::SIGNING_PUB_KEY_FILE,
    config::MODULES_DIR,
];

pub fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-to-civil conversion; avoids a date dependency.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Writes a new archive under `backups/` and returns its path.
pub fn create_archive(root: &Path) -> Result<PathBuf> {
    let dir = root.join(BACKUP_DIR);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("fly-telegram-backup-{}.tar.gz", timestamp()));
    let file = std::fs::File::create(&path)?;
    let mut builder = tar::Builder::new(GzEncoder::new(file, Compression::default()));
    builder.follow_symlinks(false);

    for entry in INCLUDED {
        let source = root.join(entry);
        if source.is_dir() {
            for file in std::fs::read_dir(&source)? {
                let file = file?;
                let name = file.file_name().to_string_lossy().to_string();
                let is_module = name.ends_with(".lua") || name.ends_with(".manifest.json");
                if file.file_type()?.is_file() && is_module {
                    builder.append_path_with_name(file.path(), format!("{entry}/{name}"))?;
                }
            }
        } else if source.is_file() {
            builder.append_path_with_name(&source, entry)?;
        }
    }
    builder.into_inner()?.finish()?;
    Ok(path)
}

fn is_allowed_entry(path: &Path) -> bool {
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return false;
    }
    let text = path.to_string_lossy().replace('\\', "/");
    if INCLUDED
        .iter()
        .any(|entry| *entry != config::MODULES_DIR && text == *entry)
    {
        return true;
    }
    let Some(name) = text.strip_prefix("modules/") else {
        return false;
    };
    !name.contains('/') && (name.ends_with(".lua") || name.ends_with(".manifest.json"))
}

/// Extracts a backup over the data directory, keeping the current database aside first.
/// Returns the restored paths.
pub fn restore_archive(root: &Path, archive: &Path) -> Result<Vec<String>> {
    let size = std::fs::metadata(archive)?.len();
    if size > MAX_ARCHIVE_BYTES {
        anyhow::bail!("backup archive is too large");
    }

    // Validate everything before touching the data directory.
    let mut entries = Vec::new();
    {
        let mut reader = tar::Archive::new(GzDecoder::new(std::fs::File::open(archive)?));
        for entry in reader
            .entries()
            .context("not a fly-telegram backup archive")?
        {
            let mut entry = entry?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let path = entry.path()?.to_path_buf();
            if !is_allowed_entry(&path) {
                anyhow::bail!("backup contains an unexpected file: {}", path.display());
            }
            let mut content = Vec::new();
            entry
                .by_ref()
                .take(MAX_ARCHIVE_BYTES)
                .read_to_end(&mut content)?;
            entries.push((path, content));
        }
    }
    if !entries.iter().any(|(path, _)| {
        let text = path.to_string_lossy();
        text == config::DATABASE_FILE || text == config::DATABASE_ENCRYPTED_FILE
    }) {
        anyhow::bail!("backup does not contain a database");
    }

    let keep_dir = root
        .join(BACKUP_DIR)
        .join(format!("pre-restore-{}", timestamp()));
    std::fs::create_dir_all(&keep_dir)?;
    for file in [config::DATABASE_FILE, config::DATABASE_ENCRYPTED_FILE] {
        let current = root.join(file);
        if current.exists() {
            std::fs::copy(&current, keep_dir.join(file))?;
            std::fs::remove_file(&current)?;
        }
    }

    let mut restored = Vec::new();
    for (path, content) in entries {
        let target = root.join(&path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, content)?;
        restored.push(path.to_string_lossy().replace('\\', "/"));
    }
    Ok(restored)
}

pub async fn send_backup(ctx: &Ctx) -> Result<String> {
    let root = std::env::current_dir()?;
    let path = tokio::task::spawn_blocking(move || create_archive(&root)).await??;
    let encrypted = ctx.db.encryption_enabled().await;
    let caption = if encrypted {
        "🗄 **fly-telegram backup**  \nDatabase is encrypted with your master password.".to_string()
    } else {
        "🗄 **fly-telegram backup**  \n⚠️ Database is not encrypted: it contains your API hash and bot token. Set a master password in the panel to encrypt backups.".to_string()
    };
    let uploaded = ctx.client.upload_file(&path).await?;
    ctx.runtime.wait_for_telegram_send().await;
    ctx.client
        .send_message(
            telegram::saved_messages(),
            telegram::formatted_message_input(&caption).document(uploaded),
        )
        .await?;
    Ok(path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default())
}

pub async fn restore_from_reply(ctx: &Ctx) -> Result<Vec<String>> {
    let msg = ctx
        .message
        .lock()
        .await
        .as_ref()
        .cloned()
        .context("no message context")?;
    let reply = msg
        .get_reply()
        .await?
        .context("reply to a backup .tar.gz file")?;
    let root = std::env::current_dir()?;
    let download = root
        .join(BACKUP_DIR)
        .join(format!("incoming-{}.tar.gz", timestamp()));
    tokio::fs::create_dir_all(root.join(BACKUP_DIR)).await?;
    if !reply.download_media(&download).await? {
        anyhow::bail!("the replied message has no file");
    }
    let archive = download.clone();
    let restored = tokio::task::spawn_blocking(move || restore_archive(&root, &archive)).await??;
    let _ = tokio::fs::remove_file(&download).await;
    Ok(restored)
}

#[allow(dead_code)]
pub fn document_message(caption: &str) -> InputMessage {
    telegram::formatted_message_input(caption)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fly_backup_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("modules")).unwrap();
        dir
    }

    #[test]
    fn archive_round_trip_excludes_sessions() {
        let root = temp_root();
        std::fs::write(root.join("database.json"), r#"{"a":1}"#).unwrap();
        std::fs::write(root.join("fly-telegram.session"), "secret").unwrap();
        std::fs::write(root.join("modules/x.lua"), "return {}").unwrap();
        std::fs::write(root.join("config.toml"), "[web]\nport = 9000\n").unwrap();

        let archive = create_archive(&root).unwrap();
        std::fs::write(root.join("database.json"), r#"{"a":2}"#).unwrap();
        std::fs::remove_file(root.join("modules/x.lua")).unwrap();

        let restored = restore_archive(&root, &archive).unwrap();
        assert!(restored.contains(&"modules/x.lua".to_string()));
        assert!(!restored.iter().any(|path| path.contains("session")));
        assert_eq!(
            std::fs::read_to_string(root.join("database.json")).unwrap(),
            r#"{"a":1}"#
        );
        // The overwritten database is kept aside.
        let kept = std::fs::read_dir(root.join(BACKUP_DIR))
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("pre-restore-"));
        assert!(kept);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unexpected_paths() {
        assert!(is_allowed_entry(Path::new("modules/a.lua")));
        assert!(is_allowed_entry(Path::new("database.json.enc")));
        assert!(!is_allowed_entry(Path::new("../etc/passwd")));
        assert!(!is_allowed_entry(Path::new("/database.json")));
        assert!(!is_allowed_entry(Path::new("fly-telegram.session")));
        assert!(!is_allowed_entry(Path::new("modules/sub/a.lua")));
        assert!(!is_allowed_entry(Path::new("modules/a.sh")));
    }

    #[test]
    fn timestamp_is_well_formed() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(timestamp().len(), 15);
    }
}
