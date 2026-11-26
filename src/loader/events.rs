//! Events modules can handle besides commands: `on_edit`, `on_delete`, `on_join`, `on_leave`,
//! and `on_reaction`, next to `on_message`. Each needs `telegram.read` in a sandboxed module
//! and honours its chat and account scopes.

use grammers_client::tl;
use grammers_client::update::Message;
use mlua::{Lua, Table};

/// Every event handler a module can define.
pub const HANDLERS: &[&str] = &[
    "on_message",
    "on_edit",
    "on_delete",
    "on_join",
    "on_leave",
    "on_reaction",
];

/// The text of deleted messages, which anti-delete keeps, needs its own permission.
pub const DELETED_PERMISSION: &str = "telegram.deleted";

/// A chat id in the Bot API format from a raw peer.
pub fn peer_chat_id(peer: &tl::enums::Peer) -> i64 {
    match peer {
        tl::enums::Peer::User(user) => user.user_id,
        tl::enums::Peer::Chat(chat) => -chat.chat_id,
        tl::enums::Peer::Channel(channel) => -1_000_000_000_000 - channel.channel_id,
    }
}

/// Members added to or removed from a group, read from a service message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Membership {
    pub joined: bool,
    pub user_ids: Vec<i64>,
    pub by_link: bool,
}

pub fn membership(msg: &Message) -> Option<Membership> {
    let sender = msg
        .sender_id()
        .map(|id| id.bot_api_dialog_id())
        .into_iter()
        .collect::<Vec<_>>();
    let (joined, user_ids, by_link) = match msg.action()? {
        tl::enums::MessageAction::ChatAddUser(add) => (true, add.users.clone(), false),
        tl::enums::MessageAction::ChatJoinedByLink(_) => (true, sender, true),
        tl::enums::MessageAction::ChatJoinedByRequest => (true, sender, true),
        tl::enums::MessageAction::ChatDeleteUser(delete) => (false, vec![delete.user_id], false),
        _ => return None,
    };
    Some(Membership {
        joined,
        user_ids,
        by_link,
    })
}

pub fn membership_table(lua: &Lua, chat_id: i64, membership: &Membership) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("chat_id", chat_id)?;
    table.set("user_ids", membership.user_ids.clone())?;
    table.set("by_link", membership.by_link)?;
    Ok(table)
}

/// Reactions on a message, read from a raw update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reactions {
    pub chat_id: i64,
    pub message_id: i32,
    /// Emoji (or `custom:<id>`, or `paid`) and how many chose it.
    pub counts: Vec<(String, i32)>,
}

pub fn reactions(update: &tl::enums::Update) -> Option<Reactions> {
    let tl::enums::Update::MessageReactions(update) = update else {
        return None;
    };
    let tl::enums::MessageReactions::Reactions(reactions) = &update.reactions;
    let counts = reactions
        .results
        .iter()
        .map(|result| {
            let tl::enums::ReactionCount::Count(count) = result;
            let name = match &count.reaction {
                tl::enums::Reaction::Emoji(emoji) => emoji.emoticon.clone(),
                tl::enums::Reaction::CustomEmoji(custom) => {
                    format!("custom:{}", custom.document_id)
                }
                tl::enums::Reaction::Paid => "paid".to_string(),
                tl::enums::Reaction::Empty => String::new(),
            };
            (name, count.count)
        })
        .filter(|(name, _)| !name.is_empty())
        .collect();
    Some(Reactions {
        chat_id: peer_chat_id(&update.peer),
        message_id: update.msg_id,
        counts,
    })
}

pub fn reactions_table(lua: &Lua, reactions: &Reactions) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("chat_id", reactions.chat_id)?;
    table.set("message_id", reactions.message_id)?;
    let counts = lua.create_table()?;
    for (index, (emoji, count)) in reactions.counts.iter().enumerate() {
        let item = lua.create_table()?;
        item.set("emoji", emoji.as_str())?;
        item.set("count", *count)?;
        counts.set(index + 1, item)?;
    }
    table.set("reactions", counts)?;
    Ok(table)
}

/// Deleted messages: their ids, the chat when Telegram names it (channels and supergroups),
/// and what anti-delete kept of them.
#[derive(Clone, Debug, Default)]
pub struct Deletion {
    pub chat_id: Option<i64>,
    pub ids: Vec<i32>,
    pub kept: Vec<KeptMessage>,
}

#[derive(Clone, Debug)]
pub struct KeptMessage {
    pub id: i32,
    pub chat_id: String,
    pub sender_id: Option<String>,
    pub text: String,
}

/// The `on_delete` table. `with_text` adds what anti-delete kept, for modules with
/// [`DELETED_PERMISSION`].
pub fn deletion_table(lua: &Lua, deletion: &Deletion, with_text: bool) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    if let Some(chat_id) = deletion.chat_id {
        table.set("chat_id", chat_id)?;
    }
    table.set("ids", deletion.ids.clone())?;
    if with_text {
        let kept = lua.create_table()?;
        for (index, message) in deletion.kept.iter().enumerate() {
            let item = lua.create_table()?;
            item.set("id", message.id)?;
            item.set("chat_id", message.chat_id.as_str())?;
            if let Some(sender) = &message.sender_id {
                item.set("sender_id", sender.as_str())?;
            }
            item.set("text", message.text.as_str())?;
            kept.set(index + 1, item)?;
        }
        table.set("messages", kept)?;
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_peers_become_bot_api_ids() {
        let user = tl::enums::Peer::User(tl::types::PeerUser { user_id: 5 });
        let chat = tl::enums::Peer::Chat(tl::types::PeerChat { chat_id: 7 });
        let channel = tl::enums::Peer::Channel(tl::types::PeerChannel { channel_id: 123 });
        assert_eq!(peer_chat_id(&user), 5);
        assert_eq!(peer_chat_id(&chat), -7);
        assert_eq!(peer_chat_id(&channel), -1_000_000_000_123);
    }

    #[test]
    fn deletion_text_is_only_given_with_the_permission() {
        let lua = Lua::new();
        let deletion = Deletion {
            chat_id: Some(-100),
            ids: vec![1, 2],
            kept: vec![KeptMessage {
                id: 1,
                chat_id: "-100".into(),
                sender_id: Some("9".into()),
                text: "secret".into(),
            }],
        };
        let plain = deletion_table(&lua, &deletion, false).unwrap();
        assert!(plain.get::<Option<Table>>("messages").unwrap().is_none());
        assert_eq!(plain.get::<Vec<i32>>("ids").unwrap(), vec![1, 2]);
        let full = deletion_table(&lua, &deletion, true).unwrap();
        let messages = full.get::<Table>("messages").unwrap();
        assert_eq!(
            messages
                .get::<Table>(1)
                .unwrap()
                .get::<String>("text")
                .unwrap(),
            "secret"
        );
    }
}
