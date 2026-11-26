//! Private broadcast channels the userbot creates on an account for its own use: the
//! anti-delete archive, notifications, and backups.

use anyhow::{Context, Result};
use grammers_client::tl;
use grammers_client::Client;
use grammers_session::types::{PeerAuth, PeerId, PeerRef};
use serde::{Deserialize, Serialize};

use crate::database::Database;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateChannel {
    pub channel_id: i64,
    pub access_hash: i64,
}

impl PrivateChannel {
    pub fn peer_ref(&self) -> Option<PeerRef> {
        Some(PeerRef {
            id: PeerId::channel(self.channel_id)?,
            auth: PeerAuth::from_hash(self.access_hash),
        })
    }

    pub fn input_peer(&self) -> tl::enums::InputPeer {
        tl::enums::InputPeer::Channel(tl::types::InputPeerChannel {
            channel_id: self.channel_id,
            access_hash: self.access_hash,
        })
    }

    pub fn link(channel_id: i64, message_id: i64) -> String {
        format!("https://t.me/c/{channel_id}/{message_id}")
    }
}

pub async fn stored(db: &Database, key: &str) -> Option<PrivateChannel> {
    serde_json::from_value(db.get(key).await).ok()
}

/// The channel stored under `key`, or a new private channel saved there. A `quiet` channel is
/// moved to the archive folder and muted, for storage nobody needs to see arrive.
pub async fn ensure(
    client: &Client,
    db: &Database,
    key: &str,
    title: &str,
    about: &str,
    quiet: bool,
) -> Result<PrivateChannel> {
    if let Some(channel) = stored(db, key).await {
        return Ok(channel);
    }
    let channel = create(client, title, about).await?;
    db.set(key, serde_json::to_value(channel)?).await?;
    if quiet {
        tuck_away(client, channel).await;
    }
    Ok(channel)
}

async fn create(client: &Client, title: &str, about: &str) -> Result<PrivateChannel> {
    let updates = client
        .invoke(&tl::functions::channels::CreateChannel {
            broadcast: true,
            megagroup: false,
            for_import: false,
            forum: false,
            title: title.into(),
            about: about.into(),
            geo_point: None,
            address: None,
            ttl_period: None,
        })
        .await
        .with_context(|| format!("creating the channel '{title}'"))?;
    let chats = match &updates {
        tl::enums::Updates::Updates(u) => u.chats.clone(),
        tl::enums::Updates::Combined(u) => u.chats.clone(),
        _ => Vec::new(),
    };
    chats
        .iter()
        .find_map(|chat| match chat {
            tl::enums::Chat::Channel(c) => c.access_hash.map(|hash| PrivateChannel {
                channel_id: c.id,
                access_hash: hash,
            }),
            _ => None,
        })
        .with_context(|| format!("Telegram did not return the channel '{title}'"))
}

/// Moves the channel to the archive folder and mutes it. Failures are ignored: the channel
/// works either way.
async fn tuck_away(client: &Client, channel: PrivateChannel) {
    let peer = channel.input_peer();
    let _ = client
        .invoke(&tl::functions::folders::EditPeerFolders {
            folder_peers: vec![tl::enums::InputFolderPeer::Peer(
                tl::types::InputFolderPeer {
                    peer: peer.clone(),
                    folder_id: 1,
                },
            )],
        })
        .await;
    let _ = client
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
