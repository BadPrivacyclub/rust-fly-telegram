use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::rules::{ExportRules, MediaKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportMethod {
    /// I use Telegram's takeout session, the same API Telegram Desktop uses.
    Official,
    /// I use regular API requests with my own layout and extra data.
    Custom,
}

impl ExportMethod {
    pub fn title(self) -> &'static str {
        match self {
            Self::Official => "Official export",
            Self::Custom => "Extended export",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportManifest {
    pub id: String,
    pub method: ExportMethod,
    pub account_id: i64,
    pub account_name: String,
    /// I set this when the export covers exactly one chat.
    pub chat_id: Option<i64>,
    pub title: String,
    pub requested_at: i64,
    pub finished_at: Option<i64>,
    pub status: String,
    pub files: Vec<String>,
    pub chats: usize,
    pub messages: usize,
    pub deleted_marked: usize,
    pub media_saved: usize,
    pub media_skipped: usize,
    pub bytes: u64,
    pub error: Option<String>,
    pub rules: ExportRules,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AccountInfo {
    pub id: i64,
    pub first_name: String,
    pub last_name: String,
    pub username: Option<String>,
    pub phone: Option<String>,
    pub about: Option<String>,
    pub birthday: Option<String>,
    pub premium: bool,
    pub avatars: Vec<Avatar>,
    pub sessions: Vec<SessionInfo>,
    pub contacts: Vec<ContactEntry>,
    pub privacy: Vec<PrivacyEntry>,
    pub account_ttl_days: Option<i32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Avatar {
    pub date: i64,
    pub file: Option<String>,
    pub video: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionInfo {
    pub current: bool,
    pub official_app: bool,
    pub app: String,
    pub device: String,
    pub platform: String,
    pub system: String,
    pub ip: String,
    pub location: String,
    pub created: i64,
    pub active: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContactEntry {
    pub id: i64,
    pub name: String,
    pub username: Option<String>,
    pub phone: Option<String>,
    pub mutual: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PrivacyEntry {
    pub key: String,
    pub rules: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChatExport {
    pub id: i64,
    pub title: String,
    pub kind: String,
    pub username: Option<String>,
    pub about: Option<String>,
    pub members: Option<i32>,
    pub avatar: Option<String>,
    pub pinned: bool,
    pub archived: bool,
    pub muted: bool,
    pub unread: i32,
    pub read_inbox_max_id: i32,
    pub read_outbox_max_id: i32,
    pub top_message: i32,
    pub messages: Vec<MessageExport>,
    pub stats: ChatStats,
    pub participants: BTreeMap<i64, Participant>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Participant {
    pub name: String,
    pub username: Option<String>,
    pub avatar: Option<String>,
    pub bot: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChatStats {
    pub total: usize,
    pub outgoing: usize,
    pub incoming: usize,
    pub edited: usize,
    pub deleted: usize,
    pub forwarded: usize,
    pub replies: usize,
    pub service: usize,
    pub first_date: Option<i64>,
    pub last_date: Option<i64>,
    pub media: BTreeMap<String, usize>,
    pub by_hour: [usize; 24],
    pub by_weekday: [usize; 7],
    pub top_senders: Vec<(i64, usize)>,
    pub reactions: BTreeMap<String, i64>,
    pub words: usize,
    /// I store the median seconds before the account owner answered, and the other way round.
    pub my_reply_secs: Option<i64>,
    pub their_reply_secs: Option<i64>,
    pub active_days: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MessageExport {
    pub id: i32,
    pub date: i64,
    pub edited: Option<i64>,
    pub from_id: Option<i64>,
    pub from_name: Option<String>,
    pub out: bool,
    pub text: String,
    pub entities: Vec<TextEntity>,
    pub reply_to: Option<i32>,
    pub forwarded_from: Option<String>,
    pub via_bot: Option<i64>,
    pub service: Option<String>,
    pub media: Option<MediaExport>,
    pub reactions: Vec<ReactionExport>,
    pub views: Option<i32>,
    pub forwards: Option<i32>,
    pub pinned: bool,
    pub grouped_id: Option<i64>,
    /// For outgoing messages I record whether the other side has read them.
    pub read: Option<bool>,
    pub deleted: Option<DeletedMark>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeletedMark {
    pub deleted_at: i64,
    pub restored_from_archive: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextEntity {
    pub kind: String,
    /// I keep UTF-16 offsets, as Telegram sends them.
    pub offset: i32,
    pub length: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReactionExport {
    pub emoji: String,
    pub count: i32,
    pub chosen: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MediaExport {
    /// The media class name; I set `kind` below only for downloadable media.
    pub type_name: String,
    pub kind: Option<MediaKind>,
    pub file: Option<String>,
    pub thumbnail: Option<String>,
    pub name: Option<String>,
    pub mime: Option<String>,
    pub size: Option<u64>,
    pub duration: Option<f64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub emoji: Option<String>,
    pub details: Option<String>,
    /// Why the file is missing; I show it in the placeholder.
    pub skipped: Option<String>,
}

impl MediaExport {
    pub fn is_visual(&self) -> bool {
        matches!(
            self.kind,
            Some(MediaKind::Photo | MediaKind::Sticker | MediaKind::Gif)
        ) || self
            .mime
            .as_deref()
            .is_some_and(|mime| mime.starts_with("image/"))
    }
}

impl ChatStats {
    pub fn build(chat: &ChatExport) -> Self {
        use chrono::{Datelike, TimeZone, Timelike};
        let mut stats = Self::default();
        let mut senders: BTreeMap<i64, usize> = BTreeMap::new();
        for message in &chat.messages {
            stats.total += 1;
            if message.out {
                stats.outgoing += 1;
            } else {
                stats.incoming += 1;
            }
            if message.edited.is_some() {
                stats.edited += 1;
            }
            if message.deleted.is_some() {
                stats.deleted += 1;
            }
            if message.forwarded_from.is_some() {
                stats.forwarded += 1;
            }
            if message.reply_to.is_some() {
                stats.replies += 1;
            }
            if message.service.is_some() {
                stats.service += 1;
            }
            stats.first_date = Some(
                stats
                    .first_date
                    .map_or(message.date, |d| d.min(message.date)),
            );
            stats.last_date = Some(
                stats
                    .last_date
                    .map_or(message.date, |d| d.max(message.date)),
            );
            if let Some(media) = &message.media {
                *stats.media.entry(media.type_name.clone()).or_default() += 1;
            }
            if let Some(time) = chrono::Local.timestamp_opt(message.date, 0).single() {
                stats.by_hour[time.hour() as usize] += 1;
                stats.by_weekday[time.weekday().num_days_from_monday() as usize] += 1;
            }
            if let Some(from) = message.from_id {
                *senders.entry(from).or_default() += 1;
            }
            for reaction in &message.reactions {
                *stats.reactions.entry(reaction.emoji.clone()).or_default() +=
                    i64::from(reaction.count);
            }
            stats.words += message.text.split_whitespace().count();
        }
        let (mine, theirs) = reply_gaps(&chat.messages);
        stats.my_reply_secs = median(mine);
        stats.their_reply_secs = median(theirs);
        stats.active_days = chat
            .messages
            .iter()
            .map(|m| m.date.div_euclid(86_400))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let mut top = senders.into_iter().collect::<Vec<_>>();
        top.sort_by_key(|t| std::cmp::Reverse(t.1));
        top.truncate(10);
        stats.top_senders = top;
        stats
    }
}

/// I measure gaps between the last message of one side and the first answer of the other, under a day.
fn reply_gaps(messages: &[MessageExport]) -> (Vec<i64>, Vec<i64>) {
    let mut mine = Vec::new();
    let mut theirs = Vec::new();
    let mut previous: Option<&MessageExport> = None;
    for message in messages
        .iter()
        .filter(|m| m.service.is_none() && m.deleted.is_none())
    {
        if let Some(prev) = previous {
            let gap = message.date - prev.date;
            if prev.out != message.out && (0..86_400).contains(&gap) {
                if message.out {
                    mine.push(gap);
                } else {
                    theirs.push(gap);
                }
            }
        }
        previous = Some(message);
    }
    (mine, theirs)
}

fn median(mut values: Vec<i64>) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[values.len() / 2])
}
