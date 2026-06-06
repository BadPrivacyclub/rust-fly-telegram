use anyhow::Result;
use grammers_client::message::InputMessage;
use grammers_client::parsers::parse_markdown_message;
use grammers_client::update::Message;
use grammers_client::Client;
use grammers_session::types::{PeerAuth, PeerId, PeerKind, PeerRef};

use crate::runtime::RuntimeState;

const TELEGRAM_TEXT_LIMIT: usize = 3900;


pub fn formatted_message_input(markdown: &str) -> InputMessage {
    let (text, entities) = parse_markdown_message(markdown);
    InputMessage::default().text(text).fmt_entities(entities)
}


/// Edits a message using grammers' own peer resolution, falling back to respond on failure.
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

/// Edits a message using grammers' own peer resolution, no fallback.
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

/// Sends a new message to the same chat using grammers' own peer resolution.
pub async fn msg_respond(runtime: &RuntimeState, msg: &Message, text: &str) -> Result<()> {
    for chunk in split_text(text) {
        runtime.wait_for_telegram_send().await;
        msg.respond(formatted_message_input(&chunk))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    Ok(())
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

    // Fall back to the peer ID without an access hash. For regular users,
    // chats, and self this is always valid. For channels/supergroups it may
    // fail at the Telegram API level if the hash is missing, but that gives
    // a clear API error rather than a silent "peer not in cache" crash.
    Ok(PeerRef {
        id: msg.peer_id(),
        auth: PeerAuth::default(),
    })
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
