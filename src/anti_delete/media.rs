use std::path::{Path, PathBuf};

use anyhow::Result;
use grammers_client::media::Media;
use grammers_client::peer::Peer;
use grammers_client::update::Message;
use grammers_client::Client;

use super::{AccountSnapshot, ChatSnapshot};
use crate::crypto;

pub(super) const MEDIA_DIR: &str = "data/deleted_media";
const MEDIA_DOWNLOAD_TIMEOUT_SECS: u64 = 45;
/// Files waiting to be uploaded to the archive channel; they leave this folder within minutes.
const PENDING_DIR: &str = "pending";
const SEALED_SUFFIX: &str = ".enc";
/// A file changed more recently than this may still be downloading.
const SETTLED_AFTER: std::time::Duration = std::time::Duration::from_secs(300);

pub(super) struct MediaSnapshot {
    pub(super) media_type: Option<String>,
    pub(super) media_path: Option<String>,
    pub(super) media_name: Option<String>,
    pub(super) media_size: Option<i64>,
}

pub(super) async fn snapshot_media(
    account: &AccountSnapshot,
    chat: &ChatSnapshot,
    msg: &Message,
    download: bool,
    key: Option<&[u8; 32]>,
) -> MediaSnapshot {
    let Some(media) = msg.media() else {
        return empty_media_snapshot();
    };
    let media_info = describe_media(&media);
    let media_path = match media_download_path(account, chat, msg.id(), &media_info) {
        Some(path) if download => download_message_media(msg, &path, key).await,
        _ => None,
    };

    MediaSnapshot {
        media_type: Some(media_info.media_type),
        media_path,
        media_name: media_info.media_name,
        media_size: media_info.media_size,
    }
}

/// I save media that cannot be forwarded (protected chats) until I know if it gets deleted.
pub(super) async fn download_for_archive(
    account: &AccountSnapshot,
    msg: &Message,
) -> Option<String> {
    let media = msg.media()?;
    let info = describe_media(&media);
    if !info.downloadable {
        return None;
    }
    let path = Path::new(MEDIA_DIR)
        .join(PENDING_DIR)
        .join(safe_component(&account.id))
        .join(safe_component(&msg.peer_id().to_string()))
        .join(format!("{}.{}", msg.id(), info.extension));
    // The channel upload needs the plain file, so this one stays as it is until it leaves.
    download_message_media(msg, &path, None).await
}

fn sealed_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(SEALED_SUFFIX);
    PathBuf::from(name)
}

/// I replace a plain file with its encrypted copy. The archive keeps recording the plain path.
async fn seal_file(path: &Path, key: &[u8; 32]) -> Result<()> {
    let plain = tokio::fs::read(path).await?;
    let sealed = crypto::seal_bytes(&plain, key)?;
    crate::storage::atomic_write(&sealed_path(path), &sealed).await?;
    tokio::fs::remove_file(path).await?;
    Ok(())
}

/// I accept only files under the media folder: the path comes from a request or the archive.
fn is_media_path(path: &Path) -> bool {
    path.starts_with(MEDIA_DIR)
        && !path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
}

pub(super) async fn read(path: &str, key: &[u8; 32]) -> Result<Option<Vec<u8>>> {
    let path = PathBuf::from(path.replace('\\', "/"));
    if !is_media_path(&path) {
        return Ok(None);
    }
    let sealed = sealed_path(&path);
    if sealed.exists() {
        return Ok(Some(crypto::open_bytes(
            &tokio::fs::read(sealed).await?,
            key,
        )?));
    }
    if path.is_file() {
        return Ok(Some(tokio::fs::read(path).await?));
    }
    Ok(None)
}

pub(super) async fn remove(path: &str) {
    let path = PathBuf::from(path.replace('\\', "/"));
    if is_media_path(&path) {
        let _ = tokio::fs::remove_file(sealed_path(&path)).await;
        let _ = tokio::fs::remove_file(&path).await;
    }
}

/// I encrypt the plain files older versions left in the media folder.
pub(super) async fn seal_existing(key: &[u8; 32]) {
    let mut folders = vec![PathBuf::from(MEDIA_DIR)];
    while let Some(folder) = folders.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&folder).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let Ok(meta) = entry.metadata().await else {
                continue;
            };
            if meta.is_dir() {
                if entry.file_name() != PENDING_DIR {
                    folders.push(path);
                }
                continue;
            }
            let settled = meta
                .modified()
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age >= SETTLED_AFTER);
            let plain = !entry.file_name().to_string_lossy().ends_with(SEALED_SUFFIX);
            if meta.is_file() && plain && settled {
                if let Err(error) = seal_file(&path, key).await {
                    tracing::warn!("could not encrypt {path:?}: {error:#}");
                }
            }
        }
    }
}

pub(super) async fn download_avatar(
    client: &Client,
    account: &AccountSnapshot,
    peer: Option<&Peer>,
) -> Result<Option<String>> {
    let Some(peer) = peer else {
        return Ok(None);
    };
    let Some(photo) = peer.photo(true).await else {
        return Ok(None);
    };

    let path = avatar_path(account, peer);
    if path.exists() {
        return Ok(Some(path.to_string_lossy().to_string()));
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    client.download_media(&photo, &path).await?;
    Ok(Some(path.to_string_lossy().to_string()))
}

fn empty_media_snapshot() -> MediaSnapshot {
    MediaSnapshot {
        media_type: None,
        media_path: None,
        media_name: None,
        media_size: None,
    }
}

struct MediaInfo {
    media_type: String,
    media_name: Option<String>,
    media_size: Option<i64>,
    extension: &'static str,
    downloadable: bool,
}

fn describe_media(media: &Media) -> MediaInfo {
    match media {
        Media::Photo(photo) => MediaInfo {
            media_type: "photo".to_string(),
            media_name: None,
            media_size: photo.size().map(|value| value as i64),
            extension: "jpg",
            downloadable: true,
        },
        Media::Document(document) => describe_document_media(document),
        Media::Sticker(sticker) => {
            let mut info = describe_document_media(&sticker.document);
            info.media_type = "sticker".to_string();
            info
        }
        Media::Contact(_) => typed_media_info("contact"),
        Media::Poll(_) => typed_media_info("poll"),
        Media::Geo(_) | Media::GeoLive(_) => typed_media_info("geo"),
        Media::Dice(_) => typed_media_info("dice"),
        Media::Venue(_) => typed_media_info("venue"),
        Media::WebPage(_) => typed_media_info("webpage"),
        _ => typed_media_info("unknown"),
    }
}

fn describe_document_media(document: &grammers_client::media::Document) -> MediaInfo {
    let mime_type = document.mime_type().unwrap_or("");
    let media_type = if mime_type.starts_with("video/") {
        "video"
    } else if mime_type.starts_with("image/") {
        "image"
    } else if mime_type.starts_with("audio/") {
        "audio"
    } else {
        "file"
    };

    MediaInfo {
        media_type: media_type.to_string(),
        media_name: document
            .name()
            .filter(|name| !name.is_empty())
            .map(str::to_string),
        media_size: document.size().map(|value| value as i64),
        extension: extension_from_mime(mime_type),
        downloadable: true,
    }
}

fn typed_media_info(media_type: &str) -> MediaInfo {
    MediaInfo {
        media_type: media_type.to_string(),
        media_name: None,
        media_size: None,
        extension: "bin",
        downloadable: false,
    }
}

fn extension_from_mime(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "application/pdf" => "pdf",
        _ => "bin",
    }
}

fn media_download_path(
    account: &AccountSnapshot,
    chat: &ChatSnapshot,
    message_id: i32,
    media: &MediaInfo,
) -> Option<PathBuf> {
    if !media.downloadable {
        return None;
    }

    Some(
        Path::new(MEDIA_DIR)
            .join(safe_component(&account.id))
            .join(safe_component(&chat.id))
            .join(format!("{message_id}.{}", media.extension)),
    )
}

async fn download_message_media(
    msg: &Message,
    path: &Path,
    key: Option<&[u8; 32]>,
) -> Option<String> {
    if path.exists() || sealed_path(path).exists() {
        return Some(path.to_string_lossy().to_string());
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok()?;
    }

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(MEDIA_DOWNLOAD_TIMEOUT_SECS),
        msg.download_media(path),
    )
    .await
    .ok()?
    .ok()?;
    if result {
        if let Some(key) = key {
            if let Err(error) = seal_file(path, key).await {
                tracing::warn!("could not encrypt {path:?}: {error:#}");
            }
        }
    }

    result.then(|| path.to_string_lossy().to_string())
}

fn avatar_path(account: &AccountSnapshot, peer: &Peer) -> PathBuf {
    let peer_id = peer.id().to_string().replace('-', "_");
    Path::new("data")
        .join("avatars")
        .join(safe_component(&account.id))
        .join(format!("{peer_id}.jpg"))
}

fn safe_component(value: &str) -> String {
    let safe = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        .collect::<String>();
    if safe.is_empty() {
        "unknown".to_string()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_files_in_the_media_folder_are_served() {
        assert!(is_media_path(Path::new("data/deleted_media/1/2/3.jpg")));
        assert!(!is_media_path(Path::new("database.json")));
        assert!(!is_media_path(Path::new(
            "data/deleted_media/../../database.json"
        )));
        assert!(!is_media_path(Path::new("/etc/passwd")));
    }

    #[tokio::test]
    async fn stored_media_is_encrypted_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("fly_media_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("1.jpg");
        std::fs::write(&path, b"picture bytes").unwrap();
        let key = crypto::random_key();

        seal_file(&path, &key).await.unwrap();
        assert!(!path.exists());
        let sealed = std::fs::read(sealed_path(&path)).unwrap();
        assert!(!sealed.windows(7).any(|w| w == b"picture"));
        assert_eq!(crypto::open_bytes(&sealed, &key).unwrap(), b"picture bytes");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn safe_component_rejects_path_chars() {
        assert_eq!(safe_component("../abc"), "abc");
        assert_eq!(safe_component(""), "unknown");
    }
}
