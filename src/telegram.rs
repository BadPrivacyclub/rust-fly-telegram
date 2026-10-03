use anyhow::Result;
use grammers_client::message::InputMessage;
use grammers_client::parsers::parse_markdown_message;
use grammers_client::update::Message;
use grammers_client::Client;
use grammers_session::types::{PeerAuth, PeerId, PeerKind, PeerRef};

use crate::runtime::RuntimeState;

const TELEGRAM_TEXT_LIMIT: usize = 3900;

pub fn formatted_message_input(markdown: &str) -> InputMessage {
    let (text, entities) = formatted_message_parts(markdown);
    InputMessage::default().text(text).fmt_entities(entities)
}

fn formatted_message_parts(
    markdown: &str,
) -> (String, Vec<grammers_client::tl::enums::MessageEntity>) {
    parse_markdown_message(&defuse_mention_links(markdown))
}

const MENTION_PREFIX: &str = "tg://user?id=";

/// grammers' markdown parser unwraps the numeric ID after `tg://user?id=`, so a malformed
/// link in module output (or echoed user text) would panic the handler. Break the prefix
/// of any mention link whose ID is not a plain integer.
fn defuse_mention_links(markdown: &str) -> std::borrow::Cow<'_, str> {
    if !markdown.contains(MENTION_PREFIX) {
        return std::borrow::Cow::Borrowed(markdown);
    }
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(index) = rest.find(MENTION_PREFIX) {
        let (before, after) = rest.split_at(index);
        out.push_str(before);
        let id_part = &after[MENTION_PREFIX.len()..];
        let id_end = id_part.find(')').unwrap_or(id_part.len());
        if id_part[..id_end].parse::<i64>().is_ok() {
            out.push_str(MENTION_PREFIX);
        } else {
            out.push_str("tg://user?id\u{2060}=");
        }
        rest = id_part;
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// Compact, storable form of a peer reference: Bot API dialog ID plus access hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredPeer {
    pub id: i64,
    #[serde(default)]
    pub hash: i64,
    #[serde(default)]
    pub is_self: bool,
}

impl StoredPeer {
    pub fn from_ref(peer: PeerRef) -> Self {
        Self {
            id: peer.id.bot_api_dialog_id(),
            hash: peer.auth.hash(),
            is_self: matches!(peer.id.kind(), PeerKind::UserSelf),
        }
    }

    pub fn to_ref(self) -> Option<PeerRef> {
        if self.is_self {
            return Some(PeerRef {
                id: PeerId::self_user(),
                auth: PeerAuth::default(),
            });
        }
        let id = if self.id > 0 {
            PeerId::user(self.id)?
        } else if self.id >= -999_999_999_999 {
            PeerId::chat(-self.id)?
        } else {
            PeerId::channel(-self.id - 1_000_000_000_000)?
        };
        Some(PeerRef {
            id,
            auth: PeerAuth::from_hash(self.hash),
        })
    }
}

pub fn saved_messages() -> PeerRef {
    PeerRef {
        id: PeerId::self_user(),
        auth: PeerAuth::default(),
    }
}

/// Sends a message that Telegram itself delivers later. Scheduled messages survive
/// restarts of the userbot and arrive with a notification, which makes them ideal reminders.
pub async fn send_scheduled(
    client: &Client,
    runtime: &RuntimeState,
    peer: PeerRef,
    markdown: &str,
    at: std::time::SystemTime,
) -> Result<()> {
    runtime.wait_for_telegram_send().await;
    client
        .send_message(
            peer,
            formatted_message_input(markdown).schedule_date(Some(at)),
        )
        .await?;
    Ok(())
}

pub async fn send_markdown(
    client: &Client,
    runtime: &RuntimeState,
    peer: PeerRef,
    markdown: &str,
) -> Result<()> {
    for chunk in split_text(markdown) {
        runtime.wait_for_telegram_send().await;
        client
            .send_message(peer, formatted_message_input(&chunk))
            .await?;
    }
    Ok(())
}

pub async fn msg_edit_or_respond(runtime: &RuntimeState, msg: &Message, text: &str) -> Result<()> {
    let chunks = split_text(text);
    let Some(first_chunk) = chunks.first() else {
        return Ok(());
    };
    runtime.wait_for_telegram_send().await;
    if msg
        .edit(formatted_message_input(first_chunk))
        .await
        .is_err()
    {
        runtime.wait_for_telegram_send().await;
        msg.respond(formatted_message_input(first_chunk))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    for chunk in chunks.into_iter().skip(1) {
        runtime.wait_for_telegram_send().await;
        msg.respond(formatted_message_input(&chunk))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    Ok(())
}

pub async fn msg_edit_only(runtime: &RuntimeState, msg: &Message, text: &str) -> Result<()> {
    let chunks = split_text(text);
    let Some(first_chunk) = chunks.first() else {
        return Ok(());
    };
    runtime.wait_for_telegram_send().await;
    msg.edit(formatted_message_input(first_chunk))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

pub async fn msg_respond(runtime: &RuntimeState, msg: &Message, text: &str) -> Result<()> {
    for chunk in split_text(text) {
        runtime.wait_for_telegram_send().await;
        msg.respond(formatted_message_input(&chunk))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    Ok(())
}

pub async fn delete_messages(
    client: &Client,
    runtime: &RuntimeState,
    peer_ref: PeerRef,
    ids: &[i32],
) -> Result<()> {
    runtime.wait_for_telegram_send().await;
    client.delete_messages(peer_ref, ids).await?;
    Ok(())
}

pub async fn mark_message_as_read(message: &Message, runtime: &RuntimeState) -> Result<()> {
    runtime.wait_for_telegram_send().await;
    message.mark_as_read().await?;
    Ok(())
}

/// Falls back to an unhashed peer ID because only channels require an access hash.
/// This produces an explicit API error when channel metadata is unavailable.
pub async fn resolve_message_peer(client: &Client, msg: &Message) -> Result<PeerRef> {
    if is_saved_messages_peer(client, msg).await {
        return Ok(PeerRef {
            id: PeerId::self_user(),
            auth: PeerAuth::default(),
        });
    }

    if let Some(peer_ref) = msg.peer_ref().await {
        return Ok(peer_ref);
    }

    Ok(PeerRef {
        id: msg.peer_id(),
        auth: PeerAuth::default(),
    })
}

pub fn split_text(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if current.chars().count() >= TELEGRAM_TEXT_LIMIT {
            chunks.push(current);
            current = String::new();
        }
        current.push(ch);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

async fn is_saved_messages_peer(client: &Client, msg: &Message) -> bool {
    if matches!(msg.peer_id().kind(), PeerKind::UserSelf) {
        return true;
    }

    if !msg.outgoing() || !matches!(msg.peer_id().kind(), PeerKind::User) {
        return false;
    }

    client
        .get_me()
        .await
        .is_ok_and(|user| user.id() == msg.peer_id())
}

#[cfg(test)]
mod tests {
    use super::{formatted_message_input, formatted_message_parts, split_text};

    #[test]
    fn formatted_message_input_preserves_plain_and_empty_text() {
        for source in ["", "plain text"] {
            let (text, entities) = formatted_message_parts(source);
            assert_eq!(text, source);
            assert!(entities.is_empty());
            let _ = formatted_message_input(source);
        }
    }

    #[test]
    fn formatted_message_input_parses_markdown_entities() {
        let (text, entities) = formatted_message_parts("Hello **world**");

        assert_eq!(text, "Hello world");
        assert_eq!(entities.len(), 1);
        let entity = format!("{:?}", entities[0]);
        assert!(entity.contains("Bold"));
        assert!(entity.contains("offset: 6"));
        assert!(entity.contains("length: 5"));
        let _ = formatted_message_input("Hello **world**");
    }

    #[test]
    fn malformed_mention_links_do_not_panic() {
        let (text, _) = formatted_message_parts("[x](tg://user?id=abc) and [y](tg://user?id=42)");
        assert!(text.contains('x') && text.contains('y'));
    }

    #[test]
    fn stored_peer_round_trips() {
        use super::StoredPeer;
        use grammers_session::types::{PeerAuth, PeerId, PeerRef};
        for id in [
            PeerId::user(777).unwrap(),
            PeerId::chat(1234).unwrap(),
            PeerId::channel(1_500_000_000).unwrap(),
            PeerId::self_user(),
        ] {
            let peer = PeerRef {
                id,
                auth: PeerAuth::from_hash(99),
            };
            let stored = StoredPeer::from_ref(peer);
            let back = stored.to_ref().unwrap();
            assert_eq!(back.id, peer.id);
        }
    }

    #[test]
    fn split_text_keeps_short_text() {
        assert_eq!(split_text("hello"), vec!["hello"]);
    }

    #[test]
    fn split_text_preserves_all_characters() {
        let source = "a".repeat(9000);
        let chunks = split_text(&source);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 3900));
        assert_eq!(chunks.join(""), source);
    }
}
