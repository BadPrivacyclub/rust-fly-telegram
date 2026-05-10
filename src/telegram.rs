use anyhow::Result;
use grammers_client::message::InputMessage;
use grammers_client::parsers::parse_markdown_message;
use grammers_client::update::Message;
use grammers_client::Client;
use grammers_session::types::{PeerAuth, PeerId, PeerKind, PeerRef};

use crate::runtime::RuntimeState;

const TELEGRAM_TEXT_LIMIT: usize = 3900;

/// Sends text safely, splitting long payloads into Telegram-sized chunks.
pub async fn send_text(
    client: &Client,
    runtime: &RuntimeState,
    peer_ref: PeerRef,
    text: &str,
) -> Result<()> {
    for chunk in split_text(text) {
        runtime.wait_for_telegram_send().await;
        client
            .send_message(peer_ref, formatted_message(chunk))
            .await?;
    }
    Ok(())
}

/// Sends text as a reply, splitting long payloads into Telegram-sized chunks.
pub async fn reply_text(
    client: &Client,
    runtime: &RuntimeState,
    peer_ref: PeerRef,
    message_id: i32,
    text: &str,
) -> Result<()> {
    for chunk in split_text(text) {
        runtime.wait_for_telegram_send().await;
        client
            .send_message(
                peer_ref,
                formatted_message(chunk).reply_to(Some(message_id)),
            )
            .await?;
    }
    Ok(())
}

/// Edits a message and sends overflow chunks as follow-up messages.
pub async fn edit_or_send_text(
    client: &Client,
    runtime: &RuntimeState,
    peer_ref: PeerRef,
    message_id: i32,
    text: &str,
) -> Result<()> {
    let chunks = split_text(text);
    let Some(first_chunk) = chunks.first() else {
        return Ok(());
    };

    runtime.wait_for_telegram_send().await;
    if client
        .edit_message(peer_ref, message_id, formatted_message(first_chunk.clone()))
        .await
        .is_err()
    {
        runtime.wait_for_telegram_send().await;
        client
            .send_message(peer_ref, formatted_message(first_chunk.clone()))
            .await?;
    }

    for chunk in chunks.into_iter().skip(1) {
        runtime.wait_for_telegram_send().await;
        client
            .send_message(peer_ref, formatted_message(chunk))
            .await?;
    }
    Ok(())
}

/// Edits a message without sending a replacement if the edit fails.
pub async fn edit_text(
    client: &Client,
    runtime: &RuntimeState,
    peer_ref: PeerRef,
    message_id: i32,
    text: &str,
) -> Result<()> {
    let chunks = split_text(text);
    let Some(first_chunk) = chunks.first() else {
        return Ok(());
    };

    runtime.wait_for_telegram_send().await;
    client
        .edit_message(peer_ref, message_id, formatted_message(first_chunk.clone()))
        .await?;
    Ok(())
}

fn formatted_message(markdown: String) -> InputMessage {
    let (text, entities) = parse_markdown_message(&markdown);
    InputMessage::default().text(text).fmt_entities(entities)
}

/// Deletes messages after passing through the shared Telegram call queue.
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

/// Marks a message as read after passing through the shared Telegram call queue.
pub async fn mark_message_as_read(message: &Message, runtime: &RuntimeState) -> Result<()> {
    runtime.wait_for_telegram_send().await;
    message.mark_as_read().await?;
    Ok(())
}

/// Resolves a message peer, including Saved Messages.
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

    let peer_id = msg.peer_id();
    if matches!(
        peer_id.kind(),
        PeerKind::User | PeerKind::UserSelf | PeerKind::Chat
    ) {
        return Ok(PeerRef {
            id: peer_id,
            auth: PeerAuth::default(),
        });
    }

    anyhow::bail!("peer not in cache")
}

/// Splits text into chunks that are below Telegram's 4096 character limit.
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
    use super::split_text;

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
