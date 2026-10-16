use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::database::Database;

const DEFAULT_RULES_KEY: &str = "export.rules.default";
const ACCOUNT_RULES_PREFIX: &str = "export.rules.account.";

/// Media classes I can keep in an export or replace with a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Photo,
    Video,
    VideoNote,
    Voice,
    Audio,
    Gif,
    Sticker,
    File,
}

impl MediaKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Photo => "photo",
            Self::Video => "video",
            Self::VideoNote => "video message",
            Self::Voice => "voice message",
            Self::Audio => "audio",
            Self::Gif => "GIF",
            Self::Sticker => "sticker",
            Self::File => "file",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaRules {
    pub photos: bool,
    pub videos: bool,
    pub video_notes: bool,
    pub voice: bool,
    pub audio: bool,
    pub gifs: bool,
    pub stickers: bool,
    pub files: bool,
    /// With 0 I keep files of any size.
    pub max_file_mb: f64,
    /// With 0 I keep downloading until every allowed file is saved.
    pub max_total_mb: f64,
}

impl Default for MediaRules {
    fn default() -> Self {
        Self {
            photos: true,
            videos: true,
            video_notes: true,
            voice: true,
            audio: true,
            gifs: true,
            stickers: true,
            files: true,
            max_file_mb: 50.0,
            max_total_mb: 0.0,
        }
    }
}

impl MediaRules {
    pub fn allows(&self, kind: MediaKind) -> bool {
        match kind {
            MediaKind::Photo => self.photos,
            MediaKind::Video => self.videos,
            MediaKind::VideoNote => self.video_notes,
            MediaKind::Voice => self.voice,
            MediaKind::Audio => self.audio,
            MediaKind::Gif => self.gifs,
            MediaKind::Sticker => self.stickers,
            MediaKind::File => self.files,
        }
    }

    pub fn max_file_bytes(&self) -> Option<u64> {
        mb_to_bytes(self.max_file_mb)
    }

    pub fn max_total_bytes(&self) -> Option<u64> {
        mb_to_bytes(self.max_total_mb)
    }

    pub fn any(&self) -> bool {
        self.photos
            || self.videos
            || self.video_notes
            || self.voice
            || self.audio
            || self.gifs
            || self.stickers
            || self.files
    }
}

fn mb_to_bytes(value: f64) -> Option<u64> {
    (value.is_finite() && value > 0.0).then_some((value * 1024.0 * 1024.0) as u64)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatRules {
    pub private: bool,
    pub bots: bool,
    pub groups: bool,
    pub channels: bool,
    pub saved: bool,
    pub archived: bool,
    /// When set, I export only these chats (Bot API ids).
    pub only: Vec<i64>,
    pub exclude: Vec<i64>,
}

impl Default for ChatRules {
    fn default() -> Self {
        Self {
            private: true,
            bots: false,
            groups: true,
            channels: false,
            saved: true,
            archived: true,
            only: Vec::new(),
            exclude: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageRules {
    /// I treat these `YYYY-MM-DD` bounds as inclusive.
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    /// With 0 I export the whole history.
    pub max_per_chat: u32,
    /// I leave out messages containing any of these words, ignoring case.
    pub skip_words: Vec<String>,
    pub skip_service: bool,
    pub skip_forwarded: bool,
    pub skip_links: bool,
    pub only_mine: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountSections {
    pub profile: bool,
    pub avatars: bool,
    pub phone: bool,
    pub contacts: bool,
    pub sessions: bool,
    pub settings: bool,
}

impl Default for AccountSections {
    fn default() -> Self {
        Self {
            profile: true,
            avatars: true,
            phone: true,
            contacts: true,
            sessions: true,
            settings: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Formats {
    pub json: bool,
    pub html: bool,
    pub pdf: bool,
}

impl Default for Formats {
    fn default() -> Self {
        Self {
            json: true,
            html: true,
            pdf: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportRules {
    pub media: MediaRules,
    pub chats: ChatRules,
    pub messages: MessageRules,
    pub account: AccountSections,
    pub formats: Formats,
    /// I mark messages from the anti-delete archive inside the export.
    pub include_deleted: bool,
}

impl ExportRules {
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("max_file_mb", self.media.max_file_mb),
            ("max_total_mb", self.media.max_total_mb),
        ] {
            if !value.is_finite() || value < 0.0 {
                bail!("{name} must be zero or a positive number");
            }
        }
        for date in [&self.messages.from_date, &self.messages.to_date]
            .into_iter()
            .flatten()
        {
            if parse_day(date).is_none() {
                bail!("dates use the YYYY-MM-DD format, got {date:?}");
            }
        }
        if !self.formats.json && !self.formats.html && !self.formats.pdf {
            bail!("choose at least one format");
        }
        Ok(())
    }

    /// I return Unix bounds for the date filter, with the end day inclusive.
    pub fn date_bounds(&self) -> (Option<i64>, Option<i64>) {
        let from = self.messages.from_date.as_deref().and_then(parse_day);
        let to = self
            .messages
            .to_date
            .as_deref()
            .and_then(parse_day)
            .map(|day| day + 86_399);
        (from, to)
    }

    pub fn skip_words_lower(&self) -> Vec<String> {
        self.messages
            .skip_words
            .iter()
            .map(|word| word.trim().to_lowercase())
            .filter(|word| !word.is_empty())
            .collect()
    }
}

fn parse_day(value: &str) -> Option<i64> {
    let date = chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok()?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp())
}

pub async fn default_rules(db: &Database) -> ExportRules {
    serde_json::from_value(db.get(DEFAULT_RULES_KEY).await).unwrap_or_default()
}

pub async fn save_default_rules(db: &Database, rules: &ExportRules) -> Result<()> {
    rules.validate()?;
    db.set(DEFAULT_RULES_KEY, serde_json::to_value(rules)?)
        .await
}

/// I return the partial override stored for one account; only the fields it sets differ from the defaults.
pub async fn account_override(db: &Database, account_id: i64) -> Option<Value> {
    let value = db.get(&format!("{ACCOUNT_RULES_PREFIX}{account_id}")).await;
    value.is_object().then_some(value)
}

pub async fn save_account_override(
    db: &Database,
    account_id: i64,
    patch: Option<Value>,
) -> Result<()> {
    let key = format!("{ACCOUNT_RULES_PREFIX}{account_id}");
    match patch.filter(|value| value.as_object().is_some_and(|map| !map.is_empty())) {
        Some(patch) => {
            let defaults = serde_json::to_value(default_rules(db).await)?;
            let merged: ExportRules = serde_json::from_value(merge(defaults, &patch))?;
            merged.validate()?;
            db.set(key, patch).await
        }
        None => db.remove(&key).await,
    }
}

pub async fn account_overrides(db: &Database) -> Map<String, Value> {
    let mut out = Map::new();
    for key in db.keys_with_prefix(ACCOUNT_RULES_PREFIX).await {
        let id = key.trim_start_matches(ACCOUNT_RULES_PREFIX).to_string();
        out.insert(id, db.get(&key).await);
    }
    out
}

/// I layer the defaults, then the account override, then the one-off patch of a single request.
pub async fn effective_rules(
    db: &Database,
    account_id: i64,
    request_patch: Option<&Value>,
) -> Result<ExportRules> {
    let mut value = serde_json::to_value(default_rules(db).await)?;
    if let Some(patch) = account_override(db, account_id).await {
        value = merge(value, &patch);
    }
    if let Some(patch) = request_patch {
        value = merge(value, patch);
    }
    let rules: ExportRules = serde_json::from_value(value)?;
    rules.validate()?;
    Ok(rules)
}

pub fn merge(mut base: Value, patch: &Value) -> Value {
    match (&mut base, patch) {
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (key, value) in patch_map {
                let merged = merge(base_map.remove(key).unwrap_or(Value::Null), value);
                base_map.insert(key.clone(), merged);
            }
            base
        }
        (_, patch) => patch.clone(),
    }
}

pub async fn remove_account(db: &Database, account_id: i64) -> Result<()> {
    db.remove(&format!("{ACCOUNT_RULES_PREFIX}{account_id}"))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_keeps_unpatched_fields() {
        let base = serde_json::to_value(ExportRules::default()).unwrap();
        let merged = merge(
            base,
            &json!({ "media": { "stickers": false, "max_file_mb": 5 } }),
        );
        let rules: ExportRules = serde_json::from_value(merged).unwrap();
        assert!(!rules.media.stickers);
        assert!(rules.media.photos);
        assert_eq!(rules.media.max_file_bytes(), Some(5 * 1024 * 1024));
    }

    #[test]
    fn validation_rejects_bad_input() {
        let mut rules = ExportRules::default();
        rules.messages.from_date = Some("15.10.2026".into());
        assert!(rules.validate().is_err());
        rules.messages.from_date = Some("2026-10-01".into());
        rules.messages.to_date = Some("2026-10-02".into());
        assert!(rules.validate().is_ok());
        let (from, to) = rules.date_bounds();
        assert_eq!(to.unwrap() - from.unwrap(), 2 * 86_400 - 1);
        rules.formats = Formats {
            json: false,
            html: false,
            pdf: false,
        };
        assert!(rules.validate().is_err());
    }

    #[tokio::test]
    async fn account_override_layers_over_defaults() {
        let path = std::env::temp_dir().join(format!("fly_rules_{}.json", uuid::Uuid::new_v4()));
        let db = Database::load(&path).await.unwrap();
        let mut defaults = ExportRules::default();
        defaults.media.videos = false;
        save_default_rules(&db, &defaults).await.unwrap();
        save_account_override(&db, 7, Some(json!({ "media": { "gifs": false } })))
            .await
            .unwrap();
        let rules = effective_rules(&db, 7, Some(&json!({ "formats": { "pdf": true } })))
            .await
            .unwrap();
        assert!(!rules.media.videos && !rules.media.gifs && rules.formats.pdf);
        let other = effective_rules(&db, 8, None).await.unwrap();
        assert!(other.media.gifs);
        let _ = tokio::fs::remove_file(path).await;
    }
}
