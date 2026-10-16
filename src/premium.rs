//! Premium (custom) emoji in userbot replies. I turn markdown such as `[✅](tg://emoji?id=123)`
//! into a custom emoji entity when the sending account has Telegram Premium; otherwise
//! the plain emoji stays, so replies look right either way.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, RwLock};

use anyhow::{bail, Context, Result};
use grammers_client::{tl, Client};
use serde_json::{json, Value};

use crate::database::Database;

pub const EMOJI_URL_PREFIX: &str = "tg://emoji?id=";
const MODE_KEY: &str = "ui.premium.mode";
const PACK_KEY: &str = "ui.premium.pack";
const OVERRIDES_KEY: &str = "ui.premium.icons";
const RESOLVED_KEY: &str = "ui.premium.resolved";

/// Icon names I share with `ui.icons` in Lua, with the plain emoji used without Premium.
pub const ICONS: &[(&str, &str)] = &[
    ("ok", "✅"),
    ("err", "❌"),
    ("warn", "⚠️"),
    ("info", "ℹ️"),
    ("wait", "⏳"),
    ("on", "🟢"),
    ("off", "⚪"),
    ("gear", "⚙️"),
    ("module", "🧩"),
    ("lock", "🔒"),
    ("clock", "⏰"),
    ("chart", "📊"),
    ("note", "📝"),
    ("user", "👤"),
    ("usage", "📖"),
    ("search", "🔍"),
    ("trash", "🗑"),
    ("star", "⭐"),
    ("fire", "🔥"),
    ("link", "🔗"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// I send custom emoji only from accounts that have Premium.
    Auto,
    On,
    Off,
}

impl Mode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

struct State {
    mode: Mode,
    premium: HashMap<i64, bool>,
}

static STATE: LazyLock<RwLock<State>> = LazyLock::new(|| {
    RwLock::new(State {
        mode: Mode::Auto,
        premium: HashMap::new(),
    })
});

pub fn set_account(user_id: i64, premium: bool) {
    if let Ok(mut state) = STATE.write() {
        state.premium.insert(user_id, premium);
    }
}

pub fn remove_account(user_id: i64) {
    if let Ok(mut state) = STATE.write() {
        state.premium.remove(&user_id);
    }
}

pub fn account_premium(user_id: i64) -> Option<bool> {
    STATE.read().ok()?.premium.get(&user_id).copied()
}

/// I decide whether custom emoji may be sent. Without a known sender every connected account
/// must have Premium, because the message may go out from any of them.
pub fn allowed_for(sender: Option<i64>) -> bool {
    let Ok(state) = STATE.read() else {
        return false;
    };
    match state.mode {
        Mode::Off => false,
        Mode::On => true,
        Mode::Auto => match sender.and_then(|id| state.premium.get(&id)) {
            Some(premium) => *premium,
            None => !state.premium.is_empty() && state.premium.values().all(|p| *p),
        },
    }
}

/// I turn `tg://emoji?id=` links into custom emoji entities, or drop them when not allowed.
pub fn apply(
    entities: Vec<tl::enums::MessageEntity>,
    allowed: bool,
) -> Vec<tl::enums::MessageEntity> {
    entities
        .into_iter()
        .filter_map(|entity| match &entity {
            tl::enums::MessageEntity::TextUrl(link) if link.url.starts_with(EMOJI_URL_PREFIX) => {
                let id = link.url[EMOJI_URL_PREFIX.len()..].parse::<i64>().ok()?;
                allowed.then_some({
                    tl::enums::MessageEntity::CustomEmoji(tl::types::MessageEntityCustomEmoji {
                        offset: link.offset,
                        length: link.length,
                        document_id: id,
                    })
                })
            }
            _ => Some(entity),
        })
        .collect()
}

pub fn markdown(id: i64, fallback: &str) -> String {
    format!(
        "[{}]({EMOJI_URL_PREFIX}{id})",
        fallback.replace(['[', ']'], "")
    )
}

/// I return the icon as markdown: a custom emoji link when one is configured, the plain emoji otherwise.
pub fn icon(name: &str) -> String {
    let fallback = ICONS
        .iter()
        .find(|(icon, _)| *icon == name)
        .map(|(_, emoji)| *emoji)
        .unwrap_or("•");
    match ICON_IDS.read().ok().and_then(|ids| ids.get(name).copied()) {
        Some(id) => markdown(id, fallback),
        None => fallback.to_string(),
    }
}

static ICON_IDS: LazyLock<RwLock<HashMap<String, i64>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub async fn mode(db: &Database) -> Mode {
    db.get(MODE_KEY)
        .await
        .as_str()
        .and_then(Mode::parse)
        .unwrap_or(Mode::Auto)
}

/// I compute the icon ids in effect: the resolved pack, then manual overrides on top.
pub async fn icon_ids(db: &Database) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for key in [RESOLVED_KEY, OVERRIDES_KEY] {
        if let Some(map) = db.get(key).await.as_object() {
            for (name, value) in map {
                let id = value
                    .as_i64()
                    .or_else(|| value.as_str().and_then(|s| s.parse().ok()));
                match id {
                    Some(id) if id > 0 => {
                        out.insert(name.clone(), id);
                    }
                    _ => {
                        out.remove(name);
                    }
                }
            }
        }
    }
    out
}

/// I reload the mode and icon ids from the database into memory.
pub async fn load(db: &Database) -> BTreeMap<String, i64> {
    let mode = mode(db).await;
    if let Ok(mut state) = STATE.write() {
        state.mode = mode;
    }
    let ids = icon_ids(db).await;
    if let Ok(mut map) = ICON_IDS.write() {
        *map = ids.iter().map(|(k, v)| (k.clone(), *v)).collect();
    }
    ids
}

pub async fn status(db: &Database) -> Value {
    let ids = icon_ids(db).await;
    let premium = STATE
        .read()
        .map(|state| {
            state
                .premium
                .iter()
                .map(|(k, v)| (k.to_string(), json!(v)))
                .collect::<serde_json::Map<_, _>>()
        })
        .unwrap_or_default();
    json!({
        "mode": mode(db).await.as_str(),
        "pack": db.get(PACK_KEY).await,
        "icons": ICONS.iter().map(|(name, emoji)| json!({
            "name": name,
            "fallback": emoji,
            "id": ids.get(*name).map(|id| id.to_string()),
        })).collect::<Vec<_>>(),
        "accounts": premium,
    })
}

pub async fn set_mode(db: &Database, value: &str) -> Result<()> {
    let mode = Mode::parse(value).context("mode must be auto, on, or off")?;
    db.set(MODE_KEY, Value::String(mode.as_str().into())).await
}

pub async fn set_icon(db: &Database, name: &str, id: Option<i64>) -> Result<()> {
    if !ICONS.iter().any(|(icon, _)| *icon == name) {
        bail!("unknown icon {name}");
    }
    let mut overrides = db.get(OVERRIDES_KEY).await;
    if !overrides.is_object() {
        overrides = json!({});
    }
    overrides[name] = match id {
        Some(id) if id > 0 => Value::String(id.to_string()),
        _ => Value::Null,
    };
    if let Some(map) = overrides.as_object_mut() {
        map.retain(|_, v| !v.is_null());
    }
    db.set(OVERRIDES_KEY, overrides).await
}

pub async fn use_pack_offline_clear(db: &Database) -> Result<usize> {
    db.remove(PACK_KEY).await?;
    db.remove(RESOLVED_KEY).await?;
    Ok(0)
}

/// I accept `name`, `t.me/addemoji/name`, or the full https link.
pub fn pack_name(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_end_matches('/');
    let name = trimmed
        .rsplit_once("/addemoji/")
        .map(|(_, name)| name)
        .unwrap_or(trimmed);
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    valid.then(|| name.to_string())
}

fn strip_variation(text: &str) -> String {
    text.chars()
        .filter(|ch| *ch != '\u{fe0f}' && *ch != '\u{fe0e}')
        .collect()
}

/// I read a custom emoji pack and map each icon to the first emoji in it with the same base.
pub async fn use_pack(db: &Database, client: &Client, input: &str) -> Result<usize> {
    if input.trim().is_empty() {
        return use_pack_offline_clear(db).await;
    }
    let name = pack_name(input).context("send a pack name or a t.me/addemoji/ link")?;
    let set = client
        .invoke(&tl::functions::messages::GetStickerSet {
            stickerset: tl::enums::InputStickerSet::ShortName(
                tl::types::InputStickerSetShortName {
                    short_name: name.clone(),
                },
            ),
            hash: 0,
        })
        .await
        .with_context(|| format!("Telegram has no emoji pack named {name}"))?;
    let documents = match set {
        tl::enums::messages::StickerSet::Set(set) => set.documents,
        tl::enums::messages::StickerSet::NotModified => Vec::new(),
    };
    let mut by_emoji: HashMap<String, i64> = HashMap::new();
    for document in documents {
        let tl::enums::Document::Document(document) = document else {
            continue;
        };
        for attribute in &document.attributes {
            if let tl::enums::DocumentAttribute::CustomEmoji(custom) = attribute {
                by_emoji
                    .entry(strip_variation(&custom.alt))
                    .or_insert(document.id);
            }
        }
    }
    if by_emoji.is_empty() {
        bail!("{name} is not a custom emoji pack");
    }
    let mut resolved = serde_json::Map::new();
    for (icon, fallback) in ICONS {
        if let Some(id) = by_emoji.get(&strip_variation(fallback)) {
            resolved.insert(icon.to_string(), Value::String(id.to_string()));
        }
    }
    let count = resolved.len();
    db.set(PACK_KEY, Value::String(name)).await?;
    db.set(RESOLVED_KEY, Value::Object(resolved)).await?;
    Ok(count)
}

/// I list custom emoji in a received message as `(document id, emoji text)`.
pub fn custom_emoji_in(
    text: &str,
    entities: Option<&Vec<tl::enums::MessageEntity>>,
) -> Vec<(i64, String)> {
    let units = text.encode_utf16().collect::<Vec<_>>();
    entities
        .into_iter()
        .flatten()
        .filter_map(|entity| match entity {
            tl::enums::MessageEntity::CustomEmoji(e) => {
                let start = usize::try_from(e.offset).ok()?;
                let end = start.checked_add(usize::try_from(e.length).ok()?)?;
                let piece = units.get(start..end).map(String::from_utf16_lossy)?;
                Some((e.document_id, piece))
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str) -> tl::enums::MessageEntity {
        tl::enums::MessageEntity::TextUrl(tl::types::MessageEntityTextUrl {
            offset: 0,
            length: 1,
            url: url.into(),
        })
    }

    #[test]
    fn emoji_links_become_entities_only_when_allowed() {
        let entities = vec![
            link("tg://emoji?id=5368324170671202286"),
            link("https://example.com"),
        ];
        let allowed = apply(entities.clone(), true);
        assert!(
            matches!(&allowed[0], tl::enums::MessageEntity::CustomEmoji(e) if e.document_id == 5368324170671202286)
        );
        assert!(matches!(&allowed[1], tl::enums::MessageEntity::TextUrl(_)));
        let plain = apply(entities, false);
        assert_eq!(plain.len(), 1);
    }

    #[test]
    fn pack_links_are_parsed() {
        assert_eq!(
            pack_name("https://t.me/addemoji/IconsPack/").as_deref(),
            Some("IconsPack")
        );
        assert_eq!(pack_name("IconsPack").as_deref(), Some("IconsPack"));
        assert_eq!(pack_name("../evil"), None);
    }

    #[test]
    fn markdown_link_parses_into_custom_emoji() {
        let (text, entities) = grammers_client::parsers::parse_markdown_message(&format!(
            "{} done",
            markdown(42, "✅")
        ));
        assert_eq!(text, "✅ done");
        let entities = apply(entities, true);
        assert!(
            matches!(&entities[0], tl::enums::MessageEntity::CustomEmoji(e) if e.document_id == 42 && e.length == 1)
        );
    }

    #[test]
    fn custom_emoji_are_read_back() {
        let entities = vec![tl::enums::MessageEntity::CustomEmoji(
            tl::types::MessageEntityCustomEmoji {
                offset: 2,
                length: 2,
                document_id: 9,
            },
        )];
        assert_eq!(
            custom_emoji_in("a 😀", Some(&entities)),
            vec![(9, "😀".to_string())]
        );
    }
}
