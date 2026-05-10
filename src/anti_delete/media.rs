use std::path::{Path, PathBuf};

use anyhow::Result;
use grammers_client::media::Media;
use grammers_client::peer::Peer;
use grammers_client::update::Message;
use grammers_client::Client;

use super::{AccountSnapshot, ChatSnapshot};

const MEDIA_DIR: &str = "data/deleted_media";
const MEDIA_DOWNLOAD_TIMEOUT_SECS: u64 = 45;

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
) -> MediaSnapshot {
    let Some(media) = msg.media() else {
        return empty_media_snapshot();
    };
    let media_info = describe_media(&media);
    let media_path = match media_download_path(account, chat, msg.id(), &media_info) {
        Some(path) => download_message_media(msg, &path).await,
        None => None,
    };

    MediaSnapshot {
        media_type: Some(media_info.media_type),
        media_path,
        media_name: media_info.media_name,
        media_size: media_info.media_size,
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

async fn download_message_media(msg: &Message, path: &Path) -> Option<String> {
    if path.exists() {
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
    fn safe_component_rejects_path_chars() {
        assert_eq!(safe_component("../abc"), "abc");
        assert_eq!(safe_component(""), "unknown");
    }
}
