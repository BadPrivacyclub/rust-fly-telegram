use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use grammers_client::media::{Document, Downloadable, Photo};
use grammers_client::{tl, Client, InvocationError};

use super::model::*;
use super::rules::{ExportRules, MediaKind};
use crate::jobs::Job;

const PAGE: i32 = 100;
const FLOOD_RETRIES: u32 = 6;
const MAX_FLOOD_WAIT_SECS: u32 = 600;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(900);
const REQUEST_PAUSE: Duration = Duration::from_millis(350);
const PARTICIPANT_AVATARS: usize = 60;
const ARCHIVE_FOLDER: i32 = 1;

/// I send requests either directly or inside a takeout session.
pub struct Fetcher {
    pub client: Client,
    pub takeout: Option<i64>,
    pub job: Job,
    pub root: PathBuf,
    pub self_id: i64,
    downloaded: AtomicU64,
}

#[derive(Clone, Debug)]
pub struct PeerEntry {
    pub id: i64,
    pub title: String,
    pub username: Option<String>,
    pub kind: &'static str,
    pub bot: bool,
    pub input: tl::enums::InputPeer,
    pub photo_id: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct DialogEntry {
    pub peer: PeerEntry,
    pub top_message: i32,
    pub read_inbox_max_id: i32,
    pub read_outbox_max_id: i32,
    pub unread: i32,
    pub pinned: bool,
    pub archived: bool,
    pub muted: bool,
}

#[derive(Default)]
pub struct PeerIndex {
    peers: HashMap<i64, PeerEntry>,
}

impl PeerIndex {
    pub fn get(&self, id: i64) -> Option<&PeerEntry> {
        self.peers.get(&id)
    }

    pub fn absorb(&mut self, users: &[tl::enums::User], chats: &[tl::enums::Chat], self_id: i64) {
        for user in users {
            if let Some(entry) = user_entry(user, self_id) {
                self.peers.insert(entry.id, entry);
            }
        }
        for chat in chats {
            if let Some(entry) = chat_entry(chat) {
                self.peers.insert(entry.id, entry);
            }
        }
    }

    pub fn name(&self, id: i64) -> Option<String> {
        self.peers.get(&id).map(|peer| peer.title.clone())
    }
}

pub fn peer_id(peer: &tl::enums::Peer) -> i64 {
    match peer {
        tl::enums::Peer::User(user) => user.user_id,
        tl::enums::Peer::Chat(chat) => -chat.chat_id,
        tl::enums::Peer::Channel(channel) => -1_000_000_000_000 - channel.channel_id,
    }
}

fn user_entry(user: &tl::enums::User, self_id: i64) -> Option<PeerEntry> {
    let tl::enums::User::User(user) = user else {
        return None;
    };
    let name = [user.first_name.as_deref(), user.last_name.as_deref()]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let is_self = user.is_self || user.id == self_id;
    let title = if is_self {
        "Saved Messages".to_string()
    } else if user.deleted || name.trim().is_empty() {
        "Deleted account".to_string()
    } else {
        name
    };
    let input = if is_self {
        tl::enums::InputPeer::PeerSelf
    } else {
        tl::enums::InputPeer::User(tl::types::InputPeerUser {
            user_id: user.id,
            access_hash: user.access_hash.unwrap_or_default(),
        })
    };
    let photo_id = match &user.photo {
        Some(tl::enums::UserProfilePhoto::Photo(photo)) => Some(photo.photo_id),
        _ => None,
    };
    Some(PeerEntry {
        id: user.id,
        title,
        username: user.username.clone(),
        kind: if is_self {
            "saved"
        } else if user.bot {
            "bot"
        } else {
            "private"
        },
        bot: user.bot,
        input,
        photo_id,
    })
}

fn chat_entry(chat: &tl::enums::Chat) -> Option<PeerEntry> {
    match chat {
        tl::enums::Chat::Chat(chat) => Some(PeerEntry {
            id: -chat.id,
            title: chat.title.clone(),
            username: None,
            kind: "group",
            bot: false,
            input: tl::enums::InputPeer::Chat(tl::types::InputPeerChat { chat_id: chat.id }),
            photo_id: match &chat.photo {
                tl::enums::ChatPhoto::Photo(photo) => Some(photo.photo_id),
                tl::enums::ChatPhoto::Empty => None,
            },
        }),
        tl::enums::Chat::Forbidden(chat) => Some(PeerEntry {
            id: -chat.id,
            title: chat.title.clone(),
            username: None,
            kind: "group",
            bot: false,
            input: tl::enums::InputPeer::Chat(tl::types::InputPeerChat { chat_id: chat.id }),
            photo_id: None,
        }),
        tl::enums::Chat::Channel(channel) => Some(PeerEntry {
            id: -1_000_000_000_000 - channel.id,
            title: channel.title.clone(),
            username: channel.username.clone(),
            kind: if channel.broadcast {
                "channel"
            } else {
                "supergroup"
            },
            bot: false,
            input: tl::enums::InputPeer::Channel(tl::types::InputPeerChannel {
                channel_id: channel.id,
                access_hash: channel.access_hash.unwrap_or_default(),
            }),
            photo_id: match &channel.photo {
                tl::enums::ChatPhoto::Photo(photo) => Some(photo.photo_id),
                tl::enums::ChatPhoto::Empty => None,
            },
        }),
        tl::enums::Chat::ChannelForbidden(channel) => Some(PeerEntry {
            id: -1_000_000_000_000 - channel.id,
            title: channel.title.clone(),
            username: None,
            kind: if channel.broadcast {
                "channel"
            } else {
                "supergroup"
            },
            bot: false,
            input: tl::enums::InputPeer::Channel(tl::types::InputPeerChannel {
                channel_id: channel.id,
                access_hash: channel.access_hash,
            }),
            photo_id: None,
        }),
        tl::enums::Chat::Empty(_) => None,
    }
}

/// A profile or chat photo I address through its peer, which needs no file reference.
struct PeerPhoto {
    peer: tl::enums::InputPeer,
    photo_id: i64,
    big: bool,
}

impl Downloadable for PeerPhoto {
    fn to_raw_input_location(&self) -> Option<tl::enums::InputFileLocation> {
        Some(tl::enums::InputFileLocation::InputPeerPhotoFileLocation(
            tl::types::InputPeerPhotoFileLocation {
                big: self.big,
                peer: self.peer.clone(),
                photo_id: self.photo_id,
            },
        ))
    }
}

struct HistoryPhoto(tl::types::Photo);

impl Downloadable for HistoryPhoto {
    fn to_raw_input_location(&self) -> Option<tl::enums::InputFileLocation> {
        let largest = self
            .0
            .sizes
            .iter()
            .filter_map(|size| match size {
                tl::enums::PhotoSize::Size(size) => Some((size.r#type.clone(), size.size)),
                tl::enums::PhotoSize::Progressive(size) => {
                    size.sizes.last().map(|bytes| (size.r#type.clone(), *bytes))
                }
                _ => None,
            })
            .max_by_key(|(_, bytes)| *bytes)?;
        Some(tl::enums::InputFileLocation::InputPhotoFileLocation(
            tl::types::InputPhotoFileLocation {
                id: self.0.id,
                access_hash: self.0.access_hash,
                file_reference: self.0.file_reference.clone(),
                thumb_size: largest.0,
            },
        ))
    }
}

impl Fetcher {
    pub fn new(
        client: Client,
        takeout: Option<i64>,
        job: Job,
        root: PathBuf,
        self_id: i64,
    ) -> Self {
        Self {
            client,
            takeout,
            job,
            root,
            self_id,
            downloaded: AtomicU64::new(0),
        }
    }

    pub fn downloaded_bytes(&self) -> u64 {
        self.downloaded.load(Ordering::Relaxed)
    }

    /// I wrap the request in the takeout session when there is one and wait out FLOOD_WAIT.
    pub async fn call<R>(&self, request: R) -> Result<R::Return, InvocationError>
    where
        R: tl::RemoteCall + Clone,
    {
        let mut attempt = 0;
        loop {
            let result = match self.takeout {
                Some(takeout_id) => {
                    self.client
                        .invoke(&tl::functions::InvokeWithTakeout {
                            takeout_id,
                            query: request.clone(),
                        })
                        .await
                }
                None => self.client.invoke(&request).await,
            };
            match result {
                Err(InvocationError::Rpc(error))
                    if (error.name.starts_with("FLOOD")
                        || error.name == "TAKEOUT_INVALID_RETRY")
                        && attempt < FLOOD_RETRIES =>
                {
                    attempt += 1;
                    let wait = error.value.unwrap_or(5).min(MAX_FLOOD_WAIT_SECS);
                    self.job
                        .log(format!("Telegram asked to wait {wait} s"))
                        .await;
                    for _ in 0..wait {
                        if self.job.is_cancelled() {
                            return Err(InvocationError::Rpc(error));
                        }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
                other => return other,
            }
        }
    }

    pub async fn dialogs(&self, index: &mut PeerIndex) -> Result<Vec<DialogEntry>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut offset_date = 0;
        let mut offset_id = 0;
        let mut offset_peer = tl::enums::InputPeer::Empty;
        let now = crate::account_tools::now_secs() as i32;
        loop {
            let response = self
                .call(tl::functions::messages::GetDialogs {
                    exclude_pinned: false,
                    folder_id: None,
                    offset_date,
                    offset_id,
                    offset_peer: offset_peer.clone(),
                    limit: PAGE,
                    hash: 0,
                })
                .await?;
            let (dialogs, messages, chats, users, complete) = match response {
                tl::enums::messages::Dialogs::Dialogs(d) => {
                    (d.dialogs, d.messages, d.chats, d.users, true)
                }
                tl::enums::messages::Dialogs::Slice(d) => {
                    let complete = d.dialogs.len() < PAGE as usize;
                    (d.dialogs, d.messages, d.chats, d.users, complete)
                }
                tl::enums::messages::Dialogs::NotModified(_) => break,
            };
            index.absorb(&users, &chats, self.self_id);
            let dates = messages
                .iter()
                .filter_map(|message| match message {
                    tl::enums::Message::Message(m) => Some(((peer_id(&m.peer_id), m.id), m.date)),
                    tl::enums::Message::Service(m) => Some(((peer_id(&m.peer_id), m.id), m.date)),
                    tl::enums::Message::Empty(_) => None,
                })
                .collect::<HashMap<_, _>>();
            let mut last = None;
            let mut added = 0;
            for dialog in dialogs {
                let tl::enums::Dialog::Dialog(dialog) = dialog else {
                    continue;
                };
                let id = peer_id(&dialog.peer);
                last = Some((id, dialog.top_message));
                if !seen.insert(id) {
                    continue;
                }
                let Some(peer) = index.get(id).cloned() else {
                    continue;
                };
                added += 1;
                let tl::enums::PeerNotifySettings::Settings(notify) = &dialog.notify_settings;
                out.push(DialogEntry {
                    peer,
                    top_message: dialog.top_message,
                    read_inbox_max_id: dialog.read_inbox_max_id,
                    read_outbox_max_id: dialog.read_outbox_max_id,
                    unread: dialog.unread_count,
                    pinned: dialog.pinned,
                    archived: dialog.folder_id == Some(ARCHIVE_FOLDER),
                    muted: notify.mute_until.is_some_and(|until| until > now),
                });
            }
            let Some((last_id, last_top)) = last else {
                break;
            };
            if complete || added == 0 {
                break;
            }
            offset_id = last_top;
            offset_date = dates.get(&(last_id, last_top)).copied().unwrap_or(0);
            offset_peer = index
                .get(last_id)
                .map(|peer| peer.input.clone())
                .unwrap_or(tl::enums::InputPeer::Empty);
            self.job
                .log(format!("Found {} chats so far", out.len()))
                .await;
            tokio::time::sleep(REQUEST_PAUSE).await;
        }
        Ok(out)
    }

    /// I return raw messages of one chat, newest first, within the date bounds and the per-chat limit.
    pub async fn history(
        &self,
        dialog: &DialogEntry,
        rules: &ExportRules,
        index: &mut PeerIndex,
    ) -> Result<Vec<tl::enums::Message>> {
        let (from, to) = rules.date_bounds();
        let limit = rules.messages.max_per_chat as usize;
        let mut out = Vec::new();
        let mut offset_id = 0;
        let offset_date = to.map(|to| (to + 1) as i32).unwrap_or(0);
        loop {
            if self.job.is_cancelled() {
                break;
            }
            let response = self
                .call(tl::functions::messages::GetHistory {
                    peer: dialog.peer.input.clone(),
                    offset_id,
                    offset_date: if offset_id == 0 { offset_date } else { 0 },
                    add_offset: 0,
                    limit: PAGE,
                    max_id: 0,
                    min_id: 0,
                    hash: 0,
                })
                .await?;
            let (messages, chats, users) = match response {
                tl::enums::messages::Messages::Messages(m) => (m.messages, m.chats, m.users),
                tl::enums::messages::Messages::Slice(m) => (m.messages, m.chats, m.users),
                tl::enums::messages::Messages::ChannelMessages(m) => (m.messages, m.chats, m.users),
                tl::enums::messages::Messages::NotModified(_) => break,
            };
            index.absorb(&users, &chats, self.self_id);
            if messages.is_empty() {
                break;
            }
            let mut reached_start = false;
            for message in messages {
                let (id, date) = match &message {
                    tl::enums::Message::Message(m) => (m.id, m.date),
                    tl::enums::Message::Service(m) => (m.id, m.date),
                    tl::enums::Message::Empty(m) => (m.id, 0),
                };
                offset_id = id;
                if from.is_some_and(|from| i64::from(date) < from) && date != 0 {
                    reached_start = true;
                    break;
                }
                if !matches!(message, tl::enums::Message::Empty(_)) {
                    out.push(message);
                }
                if limit > 0 && out.len() >= limit {
                    reached_start = true;
                    break;
                }
            }
            if reached_start || offset_id <= 1 {
                break;
            }
            if out.len() % 1000 < PAGE as usize {
                self.job
                    .log(format!(
                        "{}: {} messages read",
                        dialog.peer.title,
                        out.len()
                    ))
                    .await;
            }
            tokio::time::sleep(REQUEST_PAUSE).await;
        }
        Ok(out)
    }

    pub async fn chat_details(&self, peer: &PeerEntry) -> (Option<String>, Option<i32>) {
        let result = match &peer.input {
            tl::enums::InputPeer::Channel(channel) => self
                .call(tl::functions::channels::GetFullChannel {
                    channel: tl::enums::InputChannel::Channel(tl::types::InputChannel {
                        channel_id: channel.channel_id,
                        access_hash: channel.access_hash,
                    }),
                })
                .await
                .ok()
                .map(|full| {
                    let tl::enums::messages::ChatFull::Full(full) = full;
                    match full.full_chat {
                        tl::enums::ChatFull::ChannelFull(full) => {
                            (Some(full.about), full.participants_count)
                        }
                        tl::enums::ChatFull::Full(full) => (Some(full.about), None),
                    }
                }),
            tl::enums::InputPeer::Chat(chat) => self
                .call(tl::functions::messages::GetFullChat {
                    chat_id: chat.chat_id,
                })
                .await
                .ok()
                .map(|full| {
                    let tl::enums::messages::ChatFull::Full(full) = full;
                    let members = full.chats.iter().find_map(|chat| match chat {
                        tl::enums::Chat::Chat(chat) => Some(chat.participants_count),
                        _ => None,
                    });
                    match full.full_chat {
                        tl::enums::ChatFull::Full(full) => (Some(full.about), members),
                        tl::enums::ChatFull::ChannelFull(full) => (Some(full.about), members),
                    }
                }),
            tl::enums::InputPeer::User(user) => self
                .call(tl::functions::users::GetFullUser {
                    id: tl::enums::InputUser::User(tl::types::InputUser {
                        user_id: user.user_id,
                        access_hash: user.access_hash,
                    }),
                })
                .await
                .ok()
                .map(|full| {
                    let tl::enums::users::UserFull::Full(full) = full;
                    let tl::enums::UserFull::Full(full) = full.full_user;
                    (full.about, None)
                }),
            _ => None,
        };
        let (about, members) = result.unwrap_or((None, None));
        (about.filter(|text| !text.trim().is_empty()), members)
    }

    /// I make sure my own user is in the index, so my messages get a name and a photo.
    pub async fn absorb_self(&self, index: &mut PeerIndex) {
        if index.get(self.self_id).is_some() {
            return;
        }
        if let Ok(users) = self
            .call(tl::functions::users::GetUsers {
                id: vec![tl::enums::InputUser::UserSelf],
            })
            .await
        {
            index.absorb(&users, &[], self.self_id);
        }
    }

    /// I save the current photo of a chat or user and return its path relative to the export root.
    pub async fn avatar(&self, peer: &PeerEntry, dir: &str) -> Option<String> {
        let photo_id = peer.photo_id?;
        let relative = format!("{dir}/{}.jpg", safe_name(&peer.id.to_string()));
        let path = self.root.join(&relative);
        if path.exists() {
            return Some(relative);
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.ok()?;
        }
        let photo = PeerPhoto {
            peer: peer.input.clone(),
            photo_id,
            big: true,
        };
        match tokio::time::timeout(
            Duration::from_secs(60),
            self.client.download_media(&photo, &path),
        )
        .await
        {
            Ok(Ok(())) => Some(relative),
            _ => {
                let _ = tokio::fs::remove_file(&path).await;
                None
            }
        }
    }

    pub async fn account_info(
        &self,
        rules: &ExportRules,
        index: &mut PeerIndex,
    ) -> Result<AccountInfo> {
        let sections = &rules.account;
        let mut info = AccountInfo::default();
        let full = self
            .call(tl::functions::users::GetFullUser {
                id: tl::enums::InputUser::UserSelf,
            })
            .await
            .context("reading the account profile")?;
        let tl::enums::users::UserFull::Full(full) = full;
        index.absorb(&full.users, &full.chats, self.self_id);
        let tl::enums::UserFull::Full(full_user) = full.full_user;
        if let Some(tl::enums::User::User(me)) = full.users.iter().find(|user| match user {
            tl::enums::User::User(user) => user.is_self || user.id == self.self_id,
            _ => false,
        }) {
            info.id = me.id;
            info.first_name = me.first_name.clone().unwrap_or_default();
            info.last_name = me.last_name.clone().unwrap_or_default();
            info.username = me.username.clone();
            info.premium = me.premium;
            if sections.phone {
                info.phone = me.phone.clone().map(|phone| format!("+{phone}"));
            }
        }
        if sections.profile {
            info.about = full_user.about.clone();
            info.birthday = full_user.birthday.as_ref().map(|birthday| {
                let tl::enums::Birthday::Birthday(b) = birthday;
                match b.year {
                    Some(year) => format!("{year:04}-{:02}-{:02}", b.month, b.day),
                    None => format!("{:02}-{:02}", b.month, b.day),
                }
            });
        }
        if sections.avatars {
            self.job.log("Saving profile photos").await;
            info.avatars = self.profile_photos().await;
        }
        if sections.sessions {
            if let Ok(tl::enums::account::Authorizations::Authorizations(list)) = self
                .call(tl::functions::account::GetAuthorizations {})
                .await
            {
                info.sessions = list
                    .authorizations
                    .into_iter()
                    .map(|auth| {
                        let tl::enums::Authorization::Authorization(a) = auth;
                        SessionInfo {
                            current: a.current,
                            official_app: a.official_app,
                            app: format!("{} {}", a.app_name, a.app_version)
                                .trim()
                                .to_string(),
                            device: a.device_model,
                            platform: a.platform,
                            system: a.system_version,
                            ip: a.ip,
                            location: [a.region, a.country]
                                .into_iter()
                                .filter(|part| !part.is_empty())
                                .collect::<Vec<_>>()
                                .join(", "),
                            created: i64::from(a.date_created),
                            active: i64::from(a.date_active),
                        }
                    })
                    .collect();
            }
        }
        if sections.contacts {
            if let Ok(tl::enums::contacts::Contacts::Contacts(list)) = self
                .call(tl::functions::contacts::GetContacts { hash: 0 })
                .await
            {
                index.absorb(&list.users, &[], self.self_id);
                info.contacts = list
                    .users
                    .iter()
                    .filter_map(|user| match user {
                        tl::enums::User::User(user) => Some(ContactEntry {
                            id: user.id,
                            name: [user.first_name.as_deref(), user.last_name.as_deref()]
                                .into_iter()
                                .flatten()
                                .collect::<Vec<_>>()
                                .join(" "),
                            username: user.username.clone(),
                            phone: sections
                                .phone
                                .then(|| user.phone.clone().map(|p| format!("+{p}")))
                                .flatten(),
                            mutual: user.mutual_contact,
                        }),
                        _ => None,
                    })
                    .collect();
            }
        }
        if sections.settings {
            info.privacy = self.privacy(index).await;
            if let Ok(tl::enums::AccountDaysTtl::Ttl(ttl)) =
                self.call(tl::functions::account::GetAccountTtl {}).await
            {
                info.account_ttl_days = Some(ttl.days);
            }
        }
        Ok(info)
    }

    async fn profile_photos(&self) -> Vec<Avatar> {
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            let Ok(response) = self
                .call(tl::functions::photos::GetUserPhotos {
                    user_id: tl::enums::InputUser::UserSelf,
                    offset,
                    max_id: 0,
                    limit: PAGE,
                })
                .await
            else {
                break;
            };
            let photos = match response {
                tl::enums::photos::Photos::Photos(p) => p.photos,
                tl::enums::photos::Photos::Slice(p) => p.photos,
            };
            if photos.is_empty() {
                break;
            }
            let count = photos.len();
            for photo in photos {
                let tl::enums::Photo::Photo(photo) = photo else {
                    continue;
                };
                let relative = format!("account/avatars/{}.jpg", photo.id);
                let path = self.root.join(&relative);
                if let Some(parent) = path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                let date = i64::from(photo.date);
                let video = photo
                    .video_sizes
                    .as_ref()
                    .is_some_and(|sizes| !sizes.is_empty());
                let saved = tokio::time::timeout(
                    Duration::from_secs(60),
                    self.client.download_media(&HistoryPhoto(photo), &path),
                )
                .await
                .is_ok_and(|result| result.is_ok());
                out.push(Avatar {
                    date,
                    file: saved.then_some(relative),
                    video,
                });
            }
            offset += count as i32;
            if count < PAGE as usize {
                break;
            }
        }
        out
    }

    async fn privacy(&self, index: &PeerIndex) -> Vec<PrivacyEntry> {
        use tl::enums::InputPrivacyKey as K;
        let keys = [
            ("Last seen and online", K::StatusTimestamp),
            ("Phone number", K::PhoneNumber),
            ("Find me by phone", K::AddedByPhone),
            ("Profile photos", K::ProfilePhoto),
            ("Bio", K::About),
            ("Birthday", K::Birthday),
            ("Forwarded messages", K::Forwards),
            ("Calls", K::PhoneCall),
            ("Peer-to-peer calls", K::PhoneP2P),
            ("Voice messages", K::VoiceMessages),
            ("Invites to groups", K::ChatInvite),
        ];
        let mut out = Vec::new();
        for (name, key) in keys {
            let Ok(tl::enums::account::PrivacyRules::Rules(rules)) =
                self.call(tl::functions::account::GetPrivacy { key }).await
            else {
                continue;
            };
            out.push(PrivacyEntry {
                key: name.to_string(),
                rules: rules
                    .rules
                    .iter()
                    .map(|rule| describe_privacy(rule, index))
                    .collect(),
            });
            tokio::time::sleep(REQUEST_PAUSE).await;
        }
        out
    }

    /// I download one attachment when the rules and limits allow it.
    pub async fn save_media(
        &self,
        raw: &tl::enums::MessageMedia,
        media: &mut MediaExport,
        rules: &ExportRules,
        chat_dir: &str,
        message_id: i32,
    ) {
        let Some(kind) = media.kind else {
            return;
        };
        if !rules.media.allows(kind) {
            media.skipped = Some(format!(
                "{} excluded by export rules",
                capitalize(kind.label())
            ));
            return;
        }
        let size = media.size.unwrap_or(0);
        if let Some(max) = rules.media.max_file_bytes() {
            if size > max {
                media.skipped = Some(format!("Larger than the {} limit", human_bytes(max)));
                return;
            }
        }
        if let Some(total) = rules.media.max_total_bytes() {
            if self.downloaded_bytes() + size > total {
                media.skipped = Some(format!(
                    "Total export size limit of {} reached",
                    human_bytes(total)
                ));
                return;
            }
        }
        let extension = extension_for(media);
        let file_name = match &media.name {
            Some(name) => format!("{message_id}_{}", safe_name(name)),
            None => format!("{message_id}.{extension}"),
        };
        let relative = format!("{chat_dir}/media/{file_name}");
        let path = self.root.join(&relative);
        if let Some(parent) = path.parent() {
            if tokio::fs::create_dir_all(parent).await.is_err() {
                media.skipped = Some("Could not create the media folder".into());
                return;
            }
        }
        let result = match raw {
            tl::enums::MessageMedia::Photo(photo) => {
                download(&self.client, &Photo { raw: photo.clone() }, &path).await
            }
            tl::enums::MessageMedia::Document(document) => {
                download(
                    &self.client,
                    &Document {
                        raw: document.clone(),
                    },
                    &path,
                )
                .await
            }
            _ => Err("nothing to download".to_string()),
        };
        match result {
            Ok(()) => {
                let written = tokio::fs::metadata(&path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(size);
                self.downloaded.fetch_add(written, Ordering::Relaxed);
                if media.size.is_none() {
                    media.size = Some(written);
                }
                media.file = Some(relative);
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&path).await;
                media.skipped = Some(format!("Download failed: {error}"));
            }
        }
    }

    /// I fetch small avatars of the most active senders for group exports.
    pub async fn participant_avatars(&self, chat: &mut ChatExport, index: &PeerIndex) {
        let mut ids = chat
            .stats
            .top_senders
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in chat.participants.keys() {
            if ids.len() >= PARTICIPANT_AVATARS {
                break;
            }
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        for id in ids.into_iter().take(PARTICIPANT_AVATARS) {
            if self.job.is_cancelled() {
                return;
            }
            let Some(peer) = index.get(id) else {
                continue;
            };
            if let Some(path) = self.avatar(peer, "avatars").await {
                if let Some(participant) = chat.participants.get_mut(&id) {
                    participant.avatar = Some(path);
                }
            }
        }
    }
}

async fn download<D: Downloadable>(client: &Client, item: &D, path: &Path) -> Result<(), String> {
    match tokio::time::timeout(DOWNLOAD_TIMEOUT, client.download_media(item, path)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("timed out".to_string()),
    }
}

fn describe_privacy(rule: &tl::enums::PrivacyRule, index: &PeerIndex) -> String {
    use tl::enums::PrivacyRule as R;
    let names = |ids: &[i64]| {
        ids.iter()
            .map(|id| index.name(*id).unwrap_or_else(|| id.to_string()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match rule {
        R::PrivacyValueAllowAll => "Everybody".into(),
        R::PrivacyValueAllowContacts => "My contacts".into(),
        R::PrivacyValueDisallowAll => "Nobody".into(),
        R::PrivacyValueDisallowContacts => "Not my contacts".into(),
        R::PrivacyValueAllowCloseFriends => "Close friends".into(),
        R::PrivacyValueAllowPremium => "Premium users".into(),
        R::PrivacyValueAllowBots => "Bots allowed".into(),
        R::PrivacyValueDisallowBots => "Bots not allowed".into(),
        R::PrivacyValueAllowUsers(u) => format!("Always allow: {}", names(&u.users)),
        R::PrivacyValueDisallowUsers(u) => format!("Never allow: {}", names(&u.users)),
        R::PrivacyValueAllowChatParticipants(c) => {
            format!("Allow members of {} chats", c.chats.len())
        }
        R::PrivacyValueDisallowChatParticipants(c) => {
            format!("Block members of {} chats", c.chats.len())
        }
    }
}

/// I convert a raw message into the export model; I describe media here and download it later.
pub fn convert_message(
    raw: &tl::enums::Message,
    dialog: &DialogEntry,
    index: &PeerIndex,
    self_id: i64,
) -> Option<MessageExport> {
    match raw {
        tl::enums::Message::Message(m) => {
            let from_id = m.from_id.as_ref().map(peer_id).or_else(|| {
                (dialog.peer.kind == "private"
                    || dialog.peer.kind == "bot"
                    || dialog.peer.kind == "saved")
                    .then_some(if m.out { self_id } else { dialog.peer.id })
            });
            let from_id =
                from_id.or_else(|| (dialog.peer.kind == "channel").then_some(dialog.peer.id));
            Some(MessageExport {
                id: m.id,
                date: i64::from(m.date),
                edited: m.edit_date.filter(|_| !m.edit_hide).map(i64::from),
                from_id,
                from_name: from_id
                    .and_then(|id| index.name(id))
                    .or_else(|| m.post_author.clone()),
                out: m.out,
                text: m.message.clone(),
                entities: m
                    .entities
                    .as_deref()
                    .map(convert_entities)
                    .unwrap_or_default(),
                reply_to: reply_to(m.reply_to.as_ref()),
                forwarded_from: m
                    .fwd_from
                    .as_ref()
                    .map(|header| forward_name(header, index)),
                via_bot: m.via_bot_id,
                service: None,
                media: m.media.as_ref().and_then(describe_media),
                reactions: m
                    .reactions
                    .as_ref()
                    .map(convert_reactions)
                    .unwrap_or_default(),
                views: m.views,
                forwards: m.forwards,
                pinned: m.pinned,
                grouped_id: m.grouped_id,
                read: m.out.then_some(m.id <= dialog.read_outbox_max_id),
                deleted: None,
            })
        }
        tl::enums::Message::Service(m) => {
            let from_id = m.from_id.as_ref().map(peer_id);
            Some(MessageExport {
                id: m.id,
                date: i64::from(m.date),
                from_id,
                from_name: from_id.and_then(|id| index.name(id)),
                out: m.out,
                reply_to: reply_to(m.reply_to.as_ref()),
                service: Some(describe_action(&m.action, index)),
                reactions: m
                    .reactions
                    .as_ref()
                    .map(convert_reactions)
                    .unwrap_or_default(),
                ..MessageExport::default()
            })
        }
        tl::enums::Message::Empty(_) => None,
    }
}

fn reply_to(header: Option<&tl::enums::MessageReplyHeader>) -> Option<i32> {
    match header? {
        tl::enums::MessageReplyHeader::Header(header) => header.reply_to_msg_id,
        tl::enums::MessageReplyHeader::MessageReplyStoryHeader(_) => None,
    }
}

fn forward_name(header: &tl::enums::MessageFwdHeader, index: &PeerIndex) -> String {
    let tl::enums::MessageFwdHeader::Header(header) = header;
    header
        .from_name
        .clone()
        .or_else(|| {
            header
                .from_id
                .as_ref()
                .and_then(|peer| index.name(peer_id(peer)))
        })
        .or_else(|| header.saved_from_name.clone())
        .or_else(|| header.post_author.clone())
        .unwrap_or_else(|| "Hidden sender".to_string())
}

pub fn convert_entities(entities: &[tl::enums::MessageEntity]) -> Vec<TextEntity> {
    use tl::enums::MessageEntity as E;
    entities
        .iter()
        .filter_map(|entity| {
            let entry = |kind: &str, offset: i32, length: i32| TextEntity {
                kind: kind.to_string(),
                offset,
                length,
                url: None,
                user_id: None,
                language: None,
                document_id: None,
            };
            Some(match entity {
                E::Bold(e) => entry("bold", e.offset, e.length),
                E::Italic(e) => entry("italic", e.offset, e.length),
                E::Underline(e) => entry("underline", e.offset, e.length),
                E::Strike(e) => entry("strike", e.offset, e.length),
                E::Spoiler(e) => entry("spoiler", e.offset, e.length),
                E::Code(e) => entry("code", e.offset, e.length),
                E::Pre(e) => TextEntity {
                    language: Some(e.language.clone()).filter(|l| !l.is_empty()),
                    ..entry("pre", e.offset, e.length)
                },
                E::Blockquote(e) => entry("blockquote", e.offset, e.length),
                E::Url(e) => entry("url", e.offset, e.length),
                E::TextUrl(e) => TextEntity {
                    url: Some(e.url.clone()),
                    ..entry("text_link", e.offset, e.length)
                },
                E::Email(e) => entry("email", e.offset, e.length),
                E::Phone(e) => entry("phone", e.offset, e.length),
                E::Mention(e) => entry("mention", e.offset, e.length),
                E::MentionName(e) => TextEntity {
                    user_id: Some(e.user_id),
                    ..entry("mention_name", e.offset, e.length)
                },
                E::Hashtag(e) => entry("hashtag", e.offset, e.length),
                E::Cashtag(e) => entry("cashtag", e.offset, e.length),
                E::BotCommand(e) => entry("bot_command", e.offset, e.length),
                E::BankCard(e) => entry("bank_card", e.offset, e.length),
                E::CustomEmoji(e) => TextEntity {
                    document_id: Some(e.document_id),
                    ..entry("custom_emoji", e.offset, e.length)
                },
                E::Unknown(_) | E::InputMessageEntityMentionName(_) => return None,
            })
        })
        .collect()
}

fn convert_reactions(reactions: &tl::enums::MessageReactions) -> Vec<ReactionExport> {
    let tl::enums::MessageReactions::Reactions(reactions) = reactions;
    reactions
        .results
        .iter()
        .map(|result| {
            let tl::enums::ReactionCount::Count(count) = result;
            let emoji = match &count.reaction {
                tl::enums::Reaction::Emoji(e) => e.emoticon.clone(),
                tl::enums::Reaction::CustomEmoji(_) => "custom emoji".to_string(),
                tl::enums::Reaction::Paid => "star".to_string(),
                tl::enums::Reaction::Empty => "reaction".to_string(),
            };
            ReactionExport {
                emoji,
                count: count.count,
                chosen: count.chosen_order.is_some(),
            }
        })
        .collect()
}

pub fn describe_media(media: &tl::enums::MessageMedia) -> Option<MediaExport> {
    use tl::enums::MessageMedia as M;
    let typed = |type_name: &str, details: Option<String>| MediaExport {
        type_name: type_name.to_string(),
        details,
        ..MediaExport::default()
    };
    Some(match media {
        M::Photo(photo) => {
            let (size, width, height) = match &photo.photo {
                Some(tl::enums::Photo::Photo(photo)) => largest_photo_size(photo),
                _ => (None, None, None),
            };
            if photo.photo.is_none() {
                return Some(typed("photo", Some("Self-destructing photo".into())));
            }
            MediaExport {
                type_name: "photo".into(),
                kind: Some(MediaKind::Photo),
                mime: Some("image/jpeg".into()),
                size,
                width,
                height,
                ..MediaExport::default()
            }
        }
        M::Document(document) => {
            let Some(tl::enums::Document::Document(doc)) = &document.document else {
                return Some(typed(
                    "file",
                    Some("Self-destructing or unavailable file".into()),
                ));
            };
            describe_document(doc, document.round, document.voice)
        }
        M::Contact(c) => typed(
            "contact",
            Some(format!(
                "{} {} +{}",
                c.first_name,
                c.last_name,
                c.phone_number.trim_start_matches('+')
            )),
        ),
        M::Geo(g) => typed("location", geo_text(&g.geo)),
        M::GeoLive(g) => typed("live location", geo_text(&g.geo)),
        M::Venue(v) => typed("venue", Some(format!("{}, {}", v.title, v.address))),
        M::Poll(p) => {
            let tl::enums::Poll::Poll(poll) = &p.poll;
            let tl::enums::TextWithEntities::Entities(question) = &poll.question;
            let answers = poll
                .answers
                .iter()
                .map(|answer| {
                    let tl::enums::PollAnswer::Answer(answer) = answer;
                    let tl::enums::TextWithEntities::Entities(text) = &answer.text;
                    format!("• {}", text.text)
                })
                .collect::<Vec<_>>()
                .join("\n");
            typed("poll", Some(format!("{}\n{answers}", question.text)))
        }
        M::Dice(d) => typed("dice", Some(format!("{} {}", d.emoticon, d.value))),
        M::WebPage(page) => match &page.webpage {
            tl::enums::WebPage::Page(page) => typed(
                "link preview",
                Some(
                    [
                        page.site_name.as_deref(),
                        page.title.as_deref(),
                        Some(page.url.as_str()),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · "),
                ),
            ),
            _ => return None,
        },
        M::Game(_) => typed("game", None),
        M::Invoice(i) => typed("invoice", Some(i.title.clone())),
        M::Story(_) => typed("story", None),
        M::Giveaway(_) | M::GiveawayResults(_) => typed("giveaway", None),
        M::PaidMedia(p) => typed("paid media", Some(format!("{} stars", p.stars_amount))),
        M::ToDo(_) => typed("checklist", None),
        M::VideoStream(_) => typed("video stream", None),
        M::Empty | M::Unsupported => return None,
    })
}

fn describe_document(doc: &tl::types::Document, round: bool, voice: bool) -> MediaExport {
    let mut out = MediaExport {
        type_name: "file".into(),
        kind: Some(MediaKind::File),
        mime: Some(doc.mime_type.clone()).filter(|m| !m.is_empty()),
        size: u64::try_from(doc.size).ok(),
        ..MediaExport::default()
    };
    let mut animated = false;
    let mut sticker = false;
    for attribute in &doc.attributes {
        match attribute {
            tl::enums::DocumentAttribute::Filename(f) => out.name = Some(f.file_name.clone()),
            tl::enums::DocumentAttribute::Animated => animated = true,
            tl::enums::DocumentAttribute::Sticker(s) => {
                sticker = true;
                out.emoji = Some(s.alt.clone()).filter(|alt| !alt.is_empty());
            }
            tl::enums::DocumentAttribute::Video(v) => {
                out.duration = Some(v.duration);
                out.width = Some(v.w);
                out.height = Some(v.h);
                out.kind = Some(if v.round_message || round {
                    MediaKind::VideoNote
                } else {
                    MediaKind::Video
                });
            }
            tl::enums::DocumentAttribute::Audio(a) => {
                out.duration = Some(f64::from(a.duration));
                out.kind = Some(if a.voice || voice {
                    MediaKind::Voice
                } else {
                    MediaKind::Audio
                });
                if !a.voice {
                    let title = [a.performer.as_deref(), a.title.as_deref()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" - ");
                    if !title.is_empty() {
                        out.details = Some(title);
                    }
                }
            }
            tl::enums::DocumentAttribute::ImageSize(i) => {
                out.width.get_or_insert(i.w);
                out.height.get_or_insert(i.h);
            }
            _ => {}
        }
    }
    if sticker {
        out.kind = Some(MediaKind::Sticker);
    } else if animated {
        out.kind = Some(MediaKind::Gif);
    }
    out.type_name = match out.kind {
        Some(MediaKind::Sticker) => "sticker",
        Some(MediaKind::Gif) => "gif",
        Some(MediaKind::Video) => "video",
        Some(MediaKind::VideoNote) => "video message",
        Some(MediaKind::Voice) => "voice message",
        Some(MediaKind::Audio) => "audio",
        _ if out.mime.as_deref().is_some_and(|m| m.starts_with("image/")) => "image",
        _ => "file",
    }
    .to_string();
    out
}

fn largest_photo_size(photo: &tl::types::Photo) -> (Option<u64>, Option<i32>, Option<i32>) {
    photo
        .sizes
        .iter()
        .filter_map(|size| match size {
            tl::enums::PhotoSize::Size(s) => Some((s.size, s.w, s.h)),
            tl::enums::PhotoSize::Progressive(s) => s.sizes.last().map(|bytes| (*bytes, s.w, s.h)),
            _ => None,
        })
        .max_by_key(|(bytes, _, _)| *bytes)
        .map(|(bytes, w, h)| (u64::try_from(bytes).ok(), Some(w), Some(h)))
        .unwrap_or((None, None, None))
}

fn geo_text(geo: &tl::enums::GeoPoint) -> Option<String> {
    match geo {
        tl::enums::GeoPoint::Point(point) => Some(format!("{:.5}, {:.5}", point.lat, point.long)),
        tl::enums::GeoPoint::Empty => None,
    }
}

pub fn describe_action(action: &tl::enums::MessageAction, index: &PeerIndex) -> String {
    use tl::enums::MessageAction as A;
    let names = |ids: &[i64]| {
        ids.iter()
            .map(|id| index.name(*id).unwrap_or_else(|| id.to_string()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match action {
        A::ChatCreate(a) => format!("Created the group «{}»", a.title),
        A::ChannelCreate(a) => format!("Created the channel «{}»", a.title),
        A::ChatEditTitle(a) => format!("Changed the title to «{}»", a.title),
        A::ChatEditPhoto(_) => "Changed the photo".into(),
        A::ChatDeletePhoto => "Removed the photo".into(),
        A::ChatAddUser(a) => format!("Added {}", names(&a.users)),
        A::ChatDeleteUser(a) => format!("Removed {}", names(&[a.user_id])),
        A::ChatJoinedByLink(_) => "Joined by invite link".into(),
        A::ChatJoinedByRequest => "Joined after a request was approved".into(),
        A::ChatMigrateTo(_) => "Converted to a supergroup".into(),
        A::ChannelMigrateFrom(a) => format!("Converted from the group «{}»", a.title),
        A::PinMessage => "Pinned a message".into(),
        A::HistoryClear => "Cleared the history".into(),
        A::PhoneCall(call) => {
            let kind = if call.video { "Video call" } else { "Call" };
            match call.duration {
                Some(seconds) => format!("{kind}, {}", human_duration(f64::from(seconds))),
                None => format!("{kind}, missed or declined"),
            }
        }
        A::GroupCall(call) => match call.duration {
            Some(seconds) => format!("Voice chat, {}", human_duration(f64::from(seconds))),
            None => "Started a voice chat".into(),
        },
        A::ScreenshotTaken => "Took a screenshot".into(),
        A::ContactSignUp => "Joined Telegram".into(),
        A::CustomAction(a) => a.message.clone(),
        A::SetMessagesTtl(a) => {
            if a.period == 0 {
                "Turned off auto-delete".into()
            } else {
                format!("Set auto-delete to {}", human_duration(f64::from(a.period)))
            }
        }
        A::TopicCreate(a) => format!("Created the topic «{}»", a.title),
        A::GiftPremium(_)
        | A::GiftCode(_)
        | A::GiftStars(_)
        | A::StarGift(_)
        | A::StarGiftUnique(_)
        | A::GiftTon(_) => "Sent a gift".into(),
        A::BoostApply(_) => "Boosted the chat".into(),
        A::Empty => "Service message".into(),
        other => {
            let debug = format!("{other:?}");
            let name = debug.split(['(', ' ']).next().unwrap_or("Service message");
            split_camel(name)
        }
    }
}

fn split_camel(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push(' ');
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// I apply the message rules that need no network.
pub fn keep_message(message: &MessageExport, rules: &ExportRules, skip_words: &[String]) -> bool {
    let (from, to) = rules.date_bounds();
    if from.is_some_and(|from| message.date < from) || to.is_some_and(|to| message.date > to) {
        return false;
    }
    let m = &rules.messages;
    if m.skip_service && message.service.is_some() {
        return false;
    }
    if m.skip_forwarded && message.forwarded_from.is_some() {
        return false;
    }
    if m.only_mine && !message.out {
        return false;
    }
    if m.skip_links
        && (message
            .entities
            .iter()
            .any(|e| matches!(e.kind.as_str(), "url" | "text_link"))
            || message.text.contains("http://")
            || message.text.contains("https://"))
    {
        return false;
    }
    if !skip_words.is_empty() {
        let text = message.text.to_lowercase();
        let caption = message
            .media
            .as_ref()
            .and_then(|media| media.name.as_deref().or(media.details.as_deref()))
            .unwrap_or("")
            .to_lowercase();
        if skip_words
            .iter()
            .any(|word| text.contains(word) || caption.contains(word))
        {
            return false;
        }
    }
    true
}

pub fn chat_allowed(dialog: &DialogEntry, rules: &ExportRules) -> bool {
    let chats = &rules.chats;
    if chats.exclude.contains(&dialog.peer.id) {
        return false;
    }
    if !chats.only.is_empty() {
        return chats.only.contains(&dialog.peer.id);
    }
    if dialog.archived && !chats.archived {
        return false;
    }
    match dialog.peer.kind {
        "saved" => chats.saved,
        "bot" => chats.bots,
        "private" => chats.private,
        "group" | "supergroup" => chats.groups,
        "channel" => chats.channels,
        _ => false,
    }
}

/// I fold deleted messages from the anti-delete archive into the chat timeline.
pub async fn merge_deleted(
    chat: &mut ChatExport,
    archived: Vec<crate::anti_delete::ArchivedDeletion>,
    rules: &ExportRules,
    skip_words: &[String],
    root: &Path,
    chat_dir: &str,
) -> usize {
    let present = chat.messages.iter().map(|m| m.id).collect::<HashSet<_>>();
    let mut added = 0;
    for record in archived {
        let mark = DeletedMark {
            deleted_at: record.deleted_at,
            restored_from_archive: true,
        };
        if present.contains(&record.message_id) {
            if let Some(message) = chat.messages.iter_mut().find(|m| m.id == record.message_id) {
                message.deleted = Some(mark);
                added += 1;
            }
            continue;
        }
        let from_id = record.sender_id;
        let mut message = MessageExport {
            id: record.message_id,
            date: record.sent_at,
            from_id,
            from_name: from_id.and_then(|id| chat.participants.get(&id).map(|p| p.name.clone())),
            out: false,
            text: record.text.clone(),
            deleted: Some(mark),
            ..MessageExport::default()
        };
        if let Some(type_name) = record.media_type.clone() {
            let kind = archive_kind(&type_name);
            let mut media = MediaExport {
                type_name: type_name.clone(),
                kind,
                name: record.media_name.clone(),
                size: record.media_size.and_then(|s| u64::try_from(s).ok()),
                ..MediaExport::default()
            };
            match (record.media_path.as_deref(), kind) {
                (Some(source), Some(kind)) if rules.media.allows(kind) => {
                    let name = Path::new(source)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| format!("{}.bin", record.message_id));
                    let relative = format!("{chat_dir}/deleted_media/{}", safe_name(&name));
                    let target = root.join(&relative);
                    if let Some(parent) = target.parent() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                    if tokio::fs::copy(source, &target).await.is_ok() {
                        media.file = Some(relative);
                    } else {
                        media.skipped = Some("The archived copy is missing".into());
                    }
                }
                (_, Some(kind)) if !rules.media.allows(kind) => {
                    media.skipped = Some(format!(
                        "{} excluded by export rules",
                        capitalize(kind.label())
                    ));
                }
                _ => media.skipped = Some("The file was not archived".into()),
            }
            message.media = Some(media);
        }
        if keep_message(&message, rules, skip_words) {
            chat.messages.push(message);
            added += 1;
        }
    }
    chat.messages
        .sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)));
    added
}

fn archive_kind(type_name: &str) -> Option<MediaKind> {
    Some(match type_name {
        "photo" | "image" => MediaKind::Photo,
        "video" => MediaKind::Video,
        "audio" => MediaKind::Audio,
        "sticker" => MediaKind::Sticker,
        "file" | "document" => MediaKind::File,
        _ => return None,
    })
}

fn extension_for(media: &MediaExport) -> &'static str {
    match media.mime.as_deref().unwrap_or("") {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "application/x-tgsticker" => "tgs",
        "application/pdf" => "pdf",
        _ => match media.kind {
            Some(MediaKind::Photo) => "jpg",
            Some(MediaKind::Video | MediaKind::VideoNote | MediaKind::Gif) => "mp4",
            Some(MediaKind::Voice) => "ogg",
            _ => "bin",
        },
    }
}

pub fn safe_name(value: &str) -> String {
    let cleaned = value
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let trimmed = cleaned.trim_matches('.').to_string();
    let mut out = if trimmed.is_empty() {
        "file".to_string()
    } else {
        trimmed
    };
    if out.len() > 120 {
        let cut = out
            .char_indices()
            .nth(100)
            .map(|(i, _)| i)
            .unwrap_or(out.len());
        let extension = Path::new(&out)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        out = format!("{}{extension}", &out[..cut]);
    }
    out
}

pub fn human_bytes(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

pub fn human_duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub fn chat_shell(dialog: &DialogEntry) -> ChatExport {
    ChatExport {
        id: dialog.peer.id,
        title: dialog.peer.title.clone(),
        kind: dialog.peer.kind.to_string(),
        username: dialog.peer.username.clone(),
        pinned: dialog.pinned,
        archived: dialog.archived,
        muted: dialog.muted,
        unread: dialog.unread,
        read_inbox_max_id: dialog.read_inbox_max_id,
        read_outbox_max_id: dialog.read_outbox_max_id,
        top_message: dialog.top_message,
        participants: BTreeMap::new(),
        ..ChatExport::default()
    }
}

pub fn register_participants(chat: &mut ChatExport, index: &PeerIndex) {
    let ids = chat
        .messages
        .iter()
        .filter_map(|m| m.from_id)
        .collect::<HashSet<_>>();
    for id in ids {
        if let Some(peer) = index.get(id) {
            chat.participants.entry(id).or_insert_with(|| Participant {
                name: peer.title.clone(),
                username: peer.username.clone(),
                avatar: None,
                bot: peer.bot,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_on_disk() {
        assert_eq!(safe_name("../report?.pdf"), "_report_.pdf");
        assert_eq!(safe_name(""), "file");
        assert!(safe_name(&"x".repeat(300)).len() <= 120);
    }

    #[test]
    fn rules_filter_messages() {
        let mut rules = ExportRules::default();
        rules.messages.skip_words = vec!["Secret".into()];
        rules.messages.skip_service = true;
        let words = rules.skip_words_lower();
        let text = |t: &str| MessageExport {
            text: t.into(),
            date: 1_700_000_000,
            ..MessageExport::default()
        };
        assert!(keep_message(&text("hello"), &rules, &words));
        assert!(!keep_message(&text("my SECRET plan"), &rules, &words));
        let service = MessageExport {
            service: Some("Pinned a message".into()),
            ..text("")
        };
        assert!(!keep_message(&service, &rules, &words));
        rules.messages.from_date = Some("2030-01-01".into());
        assert!(!keep_message(&text("old"), &rules, &[]));
    }

    #[test]
    fn camel_case_actions_read_well() {
        assert_eq!(split_camel("GeoProximityReached"), "Geo proximity reached");
    }
}
