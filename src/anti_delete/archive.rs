//! Channel storage for the anti-delete archive.
//!
//! When the operator turns it on, I forward incoming media (and, optionally, text) to a private
//! channel of the account right away: once a message is deleted, it can no longer be forwarded.
//! The forward keeps the original author. When the message is deleted I reply to that copy with
//! a card (where and when it was sent, the chat type, the IDs) tagged with hashtags, so it is easy
//! to search in Telegram. Copies of messages that were not deleted are removed again, so the
//! channel holds only what was deleted, and nothing piles up on disk.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use grammers_client::message::InputMessage;
use grammers_client::tl;
use grammers_client::update::Message;
use grammers_client::Client;
use grammers_mtsender::InvocationError;
use grammers_session::types::{PeerAuth, PeerId, PeerRef};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::{AccountSnapshot, CachedDeletedMessage};
use crate::database::Database;
use crate::i18n::Lang;

/// Settings toggles: keep deleted media in a channel instead of on disk, and forward text too.
pub const CHANNEL_KEY: &str = "handlers.antidelete.channel";
pub const FORWARD_TEXT_KEY: &str = "handlers.antidelete.forward_text";
const CHANNEL_PREFIX: &str = "antidelete.archive.";
const CHANNEL_TITLE: &str = "Deleted messages";
/// I keep forwards at least this far apart so a busy chat cannot trigger FLOOD_WAIT.
const FORWARD_GAP: Duration = Duration::from_millis(450);
const MAX_FLOOD_WAIT: u32 = 30;
/// How many forwarded copies I remember; this matches the message cache of the update loop.
const COPY_LIMIT: usize = 1200;
const CARD_MARK: &str = "🗑";
const CLEANUP_SCAN: usize = 3000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveChannel {
    pub channel_id: i64,
    pub access_hash: i64,
}

impl ArchiveChannel {
    pub fn peer_ref(&self) -> Option<PeerRef> {
        Some(PeerRef {
            id: PeerId::channel(self.channel_id)?,
            auth: PeerAuth::from_hash(self.access_hash),
        })
    }

    pub fn link(channel_id: i64, message_id: i64) -> String {
        format!("https://t.me/c/{channel_id}/{message_id}")
    }
}

/// Where a deleted message lives in the archive channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveRef {
    pub channel_id: i64,
    /// The forwarded original (or the uploaded media), when there is one.
    pub copy_id: Option<i32>,
    /// The info card with hashtags.
    pub card_id: Option<i32>,
}

pub async fn enabled(db: &Database) -> bool {
    db.get(CHANNEL_KEY).await.as_bool().unwrap_or(false)
}

fn channel_key(account_id: &str) -> String {
    format!("{CHANNEL_PREFIX}{account_id}")
}

pub async fn stored_channel(db: &Database, account_id: &str) -> Option<ArchiveChannel> {
    serde_json::from_value(db.get(&channel_key(account_id)).await).ok()
}

/// The key I use for a message: channel messages have their own numbering.
pub type Key = (Option<i64>, i32);

pub fn key_of(message: &CachedDeletedMessage) -> Key {
    (message.channel_id, message.message_id)
}

#[derive(Clone, Debug)]
enum Copy {
    Forwarded(i32),
    /// The chat forbids forwarding, so I saved the file locally and upload it on deletion.
    Local(String),
}

/// One per connected account.
pub struct Archive {
    client: Client,
    db: Arc<Database>,
    account: AccountSnapshot,
    channel: Mutex<Option<ArchiveChannel>>,
    copies: Mutex<(HashMap<Key, Copy>, VecDeque<Key>)>,
    last_forward: Mutex<Instant>,
}

fn rpc_name(error: &InvocationError) -> Option<&str> {
    match error {
        InvocationError::Rpc(rpc) => Some(rpc.name.as_str()),
        _ => None,
    }
}

impl Archive {
    pub fn new(client: Client, db: Arc<Database>, account: AccountSnapshot) -> Arc<Self> {
        Arc::new(Self {
            client,
            db,
            account,
            channel: Mutex::new(None),
            copies: Mutex::new((HashMap::new(), VecDeque::new())),
            last_forward: Mutex::new(Instant::now() - FORWARD_GAP),
        })
    }

    /// I find the archive channel, or create a private one (archived and muted).
    pub async fn channel(&self) -> Result<ArchiveChannel> {
        let mut slot = self.channel.lock().await;
        if let Some(channel) = *slot {
            return Ok(channel);
        }
        if let Some(channel) = stored_channel(&self.db, &self.account.id).await {
            *slot = Some(channel);
            return Ok(channel);
        }
        let lang = crate::core_settings::language(&self.db).await;
        let about = match lang {
            Lang::Ru => "Архив удалённых сообщений fly-telegram. Ищите по хештегам: #deleted, #photo, #user123…",
            Lang::En => "fly-telegram archive of deleted messages. Search by hashtag: #deleted, #photo, #user123…",
        };
        let updates = self
            .client
            .invoke(&tl::functions::channels::CreateChannel {
                broadcast: true,
                megagroup: false,
                for_import: false,
                forum: false,
                title: CHANNEL_TITLE.into(),
                about: about.into(),
                geo_point: None,
                address: None,
                ttl_period: None,
            })
            .await
            .context("creating the archive channel")?;
        let chats = match &updates {
            tl::enums::Updates::Updates(u) => u.chats.clone(),
            tl::enums::Updates::Combined(u) => u.chats.clone(),
            _ => Vec::new(),
        };
        let channel = chats
            .iter()
            .find_map(|chat| match chat {
                tl::enums::Chat::Channel(c) => c.access_hash.map(|hash| ArchiveChannel {
                    channel_id: c.id,
                    access_hash: hash,
                }),
                _ => None,
            })
            .context("Telegram did not return the archive channel")?;
        self.db
            .set(
                channel_key(&self.account.id),
                serde_json::to_value(channel)?,
            )
            .await?;
        self.tuck_away(channel).await;
        *slot = Some(channel);
        Ok(channel)
    }

    async fn tuck_away(&self, channel: ArchiveChannel) {
        let peer = tl::enums::InputPeer::Channel(tl::types::InputPeerChannel {
            channel_id: channel.channel_id,
            access_hash: channel.access_hash,
        });
        let _ = self
            .client
            .invoke(&tl::functions::folders::EditPeerFolders {
                folder_peers: vec![tl::enums::InputFolderPeer::Peer(
                    tl::types::InputFolderPeer {
                        peer: peer.clone(),
                        folder_id: 1,
                    },
                )],
            })
            .await;
        let _ = self
            .client
            .invoke(&tl::functions::account::UpdateNotifySettings {
                peer: tl::enums::InputNotifyPeer::Peer(tl::types::InputNotifyPeer { peer }),
                settings: tl::enums::InputPeerNotifySettings::Settings(
                    tl::types::InputPeerNotifySettings {
                        show_previews: None,
                        silent: None,
                        mute_until: Some(i32::MAX),
                        sound: None,
                        stories_muted: None,
                        stories_hide_sender: None,
                        stories_sound: None,
                    },
                ),
            })
            .await;
    }

    async fn remember(&self, key: Key, copy: Copy) {
        let evicted = {
            let mut copies = self.copies.lock().await;
            let (map, order) = &mut *copies;
            if map.insert(key, copy).is_none() {
                order.push_back(key);
            }
            let mut evicted = Vec::new();
            while order.len() > COPY_LIMIT {
                if let Some(old) = order.pop_front() {
                    if let Some(copy) = map.remove(&old) {
                        evicted.push(copy);
                    }
                }
            }
            evicted
        };
        self.discard(evicted).await;
    }

    /// I remove copies of messages that were not deleted.
    async fn discard(&self, copies: Vec<Copy>) {
        let mut ids = Vec::new();
        for copy in copies {
            match copy {
                Copy::Forwarded(id) => ids.push(id),
                Copy::Local(path) => {
                    let _ = tokio::fs::remove_file(path).await;
                }
            }
        }
        if ids.is_empty() {
            return;
        }
        let Ok(channel) = self.channel().await else {
            return;
        };
        if let Some(peer) = channel.peer_ref() {
            for chunk in ids.chunks(100) {
                let _ = self.client.delete_messages(peer, chunk).await;
            }
        }
    }

    /// I drop the copies of messages that left the update loop's cache without being deleted.
    pub async fn forget(&self, keys: &[Key]) {
        let removed = {
            let mut copies = self.copies.lock().await;
            let (map, order) = &mut *copies;
            let removed = keys
                .iter()
                .filter_map(|k| map.remove(k))
                .collect::<Vec<_>>();
            if !removed.is_empty() {
                let gone = keys.iter().collect::<HashSet<_>>();
                order.retain(|k| !gone.contains(k));
            }
            removed
        };
        self.discard(removed).await;
    }

    /// I copy a new message into the channel while it still exists.
    pub async fn keep(&self, msg: &Message, forward_text: bool) {
        let has_media = msg.media().is_some();
        if !has_media && !(forward_text && !msg.text().trim().is_empty()) {
            return;
        }
        if msg.action().is_some() {
            return;
        }
        let Ok(channel) = self.channel().await else {
            return;
        };
        let Some(destination) = channel.peer_ref() else {
            return;
        };
        if msg.peer_id().kind() == grammers_session::types::PeerKind::Channel
            && msg.peer_id().bare_id() == channel.channel_id
        {
            return;
        }
        let Ok(source) = crate::telegram::resolve_message_peer(&self.client, msg).await else {
            return;
        };
        let key = (
            (msg.peer_id().kind() == grammers_session::types::PeerKind::Channel)
                .then(|| msg.peer_id().bare_id()),
            msg.id(),
        );

        for attempt in 0..2 {
            {
                let mut last = self.last_forward.lock().await;
                let wait = FORWARD_GAP.saturating_sub(last.elapsed());
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
                *last = Instant::now();
            }
            match self
                .client
                .forward_messages(destination, &[msg.id()], source)
                .await
            {
                Ok(list) => {
                    if let Some(Some(copy)) = list.into_iter().next() {
                        self.remember(key, Copy::Forwarded(copy.id())).await;
                        return;
                    }
                    break;
                }
                Err(InvocationError::Rpc(rpc)) if rpc.name.starts_with("FLOOD") && attempt == 0 => {
                    let wait = rpc.value.unwrap_or(5);
                    if wait > MAX_FLOOD_WAIT {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(u64::from(wait))).await;
                }
                Err(error) => {
                    if !matches!(
                        rpc_name(&error),
                        Some("CHAT_FORWARDS_RESTRICTED") | Some("MESSAGE_ID_INVALID")
                    ) {
                        tracing::debug!("anti-delete forward failed: {error}");
                    }
                    break;
                }
            }
        }

        // Forwarding is not allowed here: I keep the file until I know whether it gets deleted.
        if has_media {
            if let Some(path) = super::media::download_for_archive(&self.account, msg).await {
                self.remember(key, Copy::Local(path)).await;
            }
        }
    }

    /// I post the card for a deleted message and return where it lives in the channel.
    pub async fn record(
        &self,
        message: &CachedDeletedMessage,
        deleted_at: i64,
    ) -> Option<ArchiveRef> {
        let channel = self.channel().await.ok()?;
        let peer = channel.peer_ref()?;
        let copy = {
            let mut copies = self.copies.lock().await;
            let (map, order) = &mut *copies;
            let key = key_of(message);
            let copy = map.remove(&key);
            if copy.is_some() {
                order.retain(|k| *k != key);
            }
            copy
        };
        let lang = crate::core_settings::language(&self.db).await;
        let card = card_text(message, deleted_at, lang);

        let mut copy_id = None;
        match copy {
            Some(Copy::Forwarded(id)) => copy_id = Some(id),
            Some(Copy::Local(path)) => {
                if let Ok(uploaded) = self.client.upload_file(&path).await {
                    let input = if message.media_type.as_deref() == Some("photo") {
                        InputMessage::new().text("").photo(uploaded)
                    } else {
                        InputMessage::new().text("").document(uploaded)
                    };
                    if let Ok(sent) = self.client.send_message(peer, input).await {
                        copy_id = Some(sent.id());
                        let _ = tokio::fs::remove_file(&path).await;
                    }
                }
            }
            None => {}
        }
        let card_id = self
            .client
            .send_message(
                peer,
                InputMessage::new()
                    .text(card)
                    .link_preview(false)
                    .reply_to(copy_id),
            )
            .await
            .ok()
            .map(|m| m.id());
        Some(ArchiveRef {
            channel_id: channel.channel_id,
            copy_id,
            card_id,
        })
    }

    /// After a restart my memory of pending copies is gone. I delete forwarded messages that no
    /// card answers, so only deleted messages stay in the channel.
    pub async fn cleanup(&self) {
        let Some(channel) = stored_channel(&self.db, &self.account.id).await else {
            return;
        };
        let Some(peer) = channel.peer_ref() else {
            return;
        };
        let mut referenced = HashSet::new();
        let mut candidates = Vec::new();
        let mut iter = self.client.iter_messages(peer).limit(CLEANUP_SCAN);
        while let Ok(Some(message)) = iter.next().await {
            if message.text().starts_with(CARD_MARK) {
                if let Some(reply) = message.reply_to_message_id() {
                    referenced.insert(reply);
                }
            } else {
                candidates.push(message.id());
            }
        }
        let orphans = candidates
            .into_iter()
            .filter(|id| !referenced.contains(id))
            .collect::<Vec<_>>();
        for chunk in orphans.chunks(100) {
            let _ = self.client.delete_messages(peer, chunk).await;
        }
    }
}

fn chat_kind(kind: &str) -> &'static str {
    match kind {
        "User" => "private",
        "UserSelf" => "saved",
        "Chat" => "group",
        "Channel" => "channel",
        _ => "chat",
    }
}

fn tag(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

fn local(ts: i64, format: &str) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format(format).to_string())
        .unwrap_or_else(|| ts.to_string())
}

/// The info card: who, where, when, what, plus hashtags for search.
pub fn card_text(message: &CachedDeletedMessage, deleted_at: i64, lang: Lang) -> String {
    let ru = lang == Lang::Ru;
    let sent = chrono::DateTime::parse_from_rfc3339(&message.sent_at)
        .map(|d| d.timestamp())
        .unwrap_or(0);
    let kind = chat_kind(&message.chat.kind);
    let chat_id = message.chat.id.trim_start_matches('-').to_string();
    let mut lines = vec![format!(
        "{CARD_MARK} {}",
        if ru {
            "Удалённое сообщение"
        } else {
            "Deleted message"
        }
    )];

    let sender_name = message.sender_name.clone().unwrap_or_else(|| {
        if ru {
            "неизвестно".into()
        } else {
            "unknown".into()
        }
    });
    let mut sender = format!("👤 {}: {sender_name}", if ru { "Автор" } else { "From" });
    if let Some(username) = &message.sender_username {
        sender.push_str(&format!(" (@{username})"));
    }
    if let Some(id) = &message.sender_id {
        sender.push_str(&format!(" · ID {id}"));
    }
    lines.push(sender);

    let mut chat = format!(
        "💬 {}: {} · {kind} · ID {}",
        if ru { "Чат" } else { "Chat" },
        message.chat.title,
        message.chat.id
    );
    if let Some(link) = &message.chat.link {
        chat.push_str(&format!(" · {link}"));
    }
    lines.push(chat);
    lines.push(format!(
        "🕒 {}: {}",
        if ru { "Отправлено" } else { "Sent" },
        local(sent, "%d.%m.%Y %H:%M:%S")
    ));
    lines.push(format!(
        "❌ {}: {}",
        if ru { "Удалено" } else { "Deleted" },
        local(deleted_at, "%d.%m.%Y %H:%M:%S")
    ));
    lines.push(format!(
        "🆔 {}: {}",
        if ru { "Сообщение" } else { "Message" },
        message.message_id
    ));
    if let Some(media) = &message.media_type {
        let mut line = format!("📎 {}: {media}", if ru { "Медиа" } else { "Media" });
        if let Some(name) = &message.media_name {
            line.push_str(&format!(" · {name}"));
        }
        if let Some(size) = message.media_size {
            line.push_str(&format!(
                " · {}",
                crate::export::fetch::human_bytes(size as u64)
            ));
        }
        lines.push(line);
    }
    lines.push(format!(
        "🔐 {}: {}",
        if ru { "Аккаунт" } else { "Account" },
        message.account.name
    ));
    if !message.text.trim().is_empty() {
        lines.push(String::new());
        lines.push(crate::notify::clip(&message.text, 3000));
    }

    let mut tags = vec![
        "#deleted".to_string(),
        format!("#{kind}"),
        format!("#chat{}", tag(&chat_id)),
        format!("#d{}", local(deleted_at, "%Y%m%d")),
    ];
    if let Some(id) = &message.sender_id {
        tags.push(format!("#user{}", tag(id.trim_start_matches('-'))));
    }
    if let Some(username) = &message.sender_username {
        tags.push(format!("#{}", tag(username)));
    }
    match &message.media_type {
        Some(media) => tags.push(format!("#{}", tag(media))),
        None => tags.push("#text".into()),
    }
    lines.push(String::new());
    lines.push(tags.join(" "));
    lines.join("\n")
}

/// I read the media of an archived message for the panel, up to `limit` bytes.
pub async fn download(
    client: &Client,
    channel: ArchiveChannel,
    message_id: i32,
    limit: usize,
) -> Result<Option<(Vec<u8>, String)>> {
    let peer = channel.peer_ref().context("invalid archive channel")?;
    let Some(Some(message)) = client
        .get_messages_by_id(peer, &[message_id])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let Some(media) = message.media() else {
        return Ok(None);
    };
    let mime = match &media {
        grammers_client::media::Media::Photo(_) => "image/jpeg".to_string(),
        grammers_client::media::Media::Document(d) => d
            .mime_type()
            .unwrap_or("application/octet-stream")
            .to_string(),
        grammers_client::media::Media::Sticker(s) => {
            s.document.mime_type().unwrap_or("image/webp").to_string()
        }
        _ => return Ok(None),
    };
    let mut bytes = Vec::new();
    let mut chunks = client.iter_download(&media);
    while let Some(chunk) = chunks.next().await? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > limit {
            anyhow::bail!("the file is too large to preview");
        }
    }
    Ok(Some((bytes, mime)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anti_delete::ChatSnapshot;

    fn message() -> CachedDeletedMessage {
        CachedDeletedMessage {
            account: AccountSnapshot {
                id: "42".into(),
                name: "Alex".into(),
                session_file: "a.session".into(),
            },
            chat: ChatSnapshot {
                id: "-1001234".into(),
                title: "Team".into(),
                kind: "Channel".into(),
                username: Some("team".into()),
                link: Some("https://t.me/team".into()),
                avatar_path: None,
            },
            message_id: 77,
            channel_id: Some(1234),
            sender_id: Some("555".into()),
            sender_name: Some("Bob".into()),
            sender_username: Some("bob_k".into()),
            text: "hello".into(),
            sent_at: "2026-10-15T10:00:00+00:00".into(),
            media_type: Some("photo".into()),
            media_path: None,
            media_name: None,
            media_size: Some(2048),
            archive: None,
        }
    }

    #[test]
    fn cards_carry_details_and_hashtags() {
        let card = card_text(&message(), 1_760_522_400, Lang::En);
        assert!(card.starts_with(CARD_MARK));
        for part in [
            "Bob (@bob_k) · ID 555",
            "Team · channel · ID -1001234",
            "https://t.me/team",
            "Message: 77",
            "photo",
            "hello",
            "#deleted",
            "#channel",
            "#chat1001234",
            "#user555",
            "#bob_k",
            "#photo",
        ] {
            assert!(card.contains(part), "missing {part} in\n{card}");
        }
        let ru = card_text(&message(), 1_760_522_400, Lang::Ru);
        assert!(ru.contains("Удалённое сообщение") && ru.contains("#deleted"));
    }

    #[test]
    fn links_point_to_private_posts() {
        let channel = ArchiveChannel {
            channel_id: 1234,
            access_hash: 1,
        };
        assert_eq!(ArchiveChannel::link(1234, 9), "https://t.me/c/1234/9");
        assert!(channel.peer_ref().is_some());
    }
}
