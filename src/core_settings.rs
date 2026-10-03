//! Runtime settings stored in the database, editable from Telegram, the bot, and the panel.

use anyhow::Result;
use serde_json::Value;

use crate::database::Database;
use crate::i18n::Lang;

pub mod key {
    pub const PREFIXES: &str = "core.prefixes";
    pub const LANGUAGE: &str = "core.language";
    pub const DISABLED_MODULES: &str = "core.disabled_modules";
    pub const SUDO_USERS: &str = "core.sudo_users";
    pub const RESTART_NOTICE: &str = "core.restart_notice";
    pub const BOT_TOKEN: &str = "bot_token";
    pub const NOTIFY_ERRORS: &str = "notify.errors";
    pub const NOTIFY_PMGUARD: &str = "notify.pmguard";
    pub const NOTIFY_STARTUP: &str = "notify.startup";
    pub const NOTIFY_ANTIDELETE: &str = "notify.antidelete";
    pub const NOTIFY_MODULES: &str = "notify.modules";
    pub const NOTIFY_JOBS: &str = "notify.jobs";
}

pub const DEFAULT_PREFIX: &str = ".";

/// On/off settings shown in the bot and the panel: database key, i18n label key, default.
pub const TOGGLES: &[(&str, &str, bool)] = &[
    ("handlers.afk.enabled", "set.afk", false),
    ("handlers.autoread.enabled", "set.autoread", false),
    ("handlers.antidelete.enabled", "set.antidelete", false),
    ("pmguard.enabled", "set.pmguard", false),
    ("group.clean_joins.enabled", "set.cleanjoins", false),
    ("group.captcha.enabled", "set.captcha", false),
    (key::NOTIFY_ERRORS, "set.notify_errors", true),
    (key::NOTIFY_PMGUARD, "set.notify_pmguard", true),
    (key::NOTIFY_STARTUP, "set.notify_startup", true),
    (key::NOTIFY_ANTIDELETE, "set.notify_antidelete", false),
    (key::NOTIFY_JOBS, "set.notify_jobs", true),
];

pub fn toggle_default(db_key: &str) -> Option<bool> {
    TOGGLES
        .iter()
        .find(|(key, _, _)| *key == db_key)
        .map(|(_, _, default)| *default)
}

/// Adds or removes an ID from a comma-separated list such as `pmguard.allow`.
pub async fn csv_update(db: &Database, list_key: &str, id: &str, add: bool) -> Result<()> {
    let current = db.get(list_key).await;
    let mut items = current
        .as_str()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty() && *item != id)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if add {
        items.push(id.to_string());
    }
    db.set(list_key, Value::String(items.join(","))).await
}
const MAX_PREFIXES: usize = 5;
const MAX_PREFIX_CHARS: usize = 3;

/// Commands sudo users may never run: they execute code, touch files, change settings,
/// reveal the owner's private data, or spend the owner's API credits.
#[rustfmt::skip]
pub const SUDO_DENYLIST: &[&str] = &[
    // Code, files, and processes.
    "e", "eval", "term", "ytdl", "install", "market", "dl", "sendfile", "urlupload", "rename",
    // Userbot administration.
    "update", "restart", "backup", "restore", "cfg", "sudo", "prefix", "panel", "modules",
    "module", "lang", "afk", "autoread", "antidelete", "pmguard", "approve", "block", "gifts",
    "taskbot",
    // Private data and paid APIs.
    "note", "notes", "alias", "stats", "ai", "ask", "summarize", "translate", "transcribe",
    // Account management.
    "acc", "cleanup", "readall", "archive", "muteall", "export", "jobs", "profile", "auto",
    "away",
];

pub async fn prefixes(db: &Database) -> Vec<String> {
    let parsed = string_list(&db.get(key::PREFIXES).await);
    if parsed.is_empty() {
        vec![DEFAULT_PREFIX.to_string()]
    } else {
        parsed
    }
}

pub fn validate_prefixes(values: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if value.chars().count() > MAX_PREFIX_CHARS {
            anyhow::bail!("prefix '{value}' is longer than {MAX_PREFIX_CHARS} characters");
        }
        if value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_alphanumeric())
        {
            anyhow::bail!("prefix '{value}' must not contain letters, digits, or spaces");
        }
        if !out.iter().any(|existing: &String| existing == value) {
            out.push(value.to_string());
        }
    }
    if out.is_empty() {
        anyhow::bail!("at least one prefix is required");
    }
    if out.len() > MAX_PREFIXES {
        anyhow::bail!("at most {MAX_PREFIXES} prefixes are allowed");
    }
    // Longest first so `..` is matched before `.`.
    out.sort_by_key(|value| std::cmp::Reverse(value.chars().count()));
    Ok(out)
}

pub async fn set_prefixes(db: &Database, values: &[String]) -> Result<Vec<String>> {
    let values = validate_prefixes(values)?;
    db.set(
        key::PREFIXES,
        Value::Array(values.iter().cloned().map(Value::String).collect()),
    )
    .await?;
    Ok(values)
}

/// Splits `text` into (command, args) when it starts with one of the prefixes.
pub fn parse_command(text: &str, prefixes: &[String]) -> Option<(String, String)> {
    let mut sorted = prefixes.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|value| std::cmp::Reverse(value.len()));
    let body = sorted
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix.as_str()))?;
    let mut parts = body.splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or("").to_lowercase();
    if command.is_empty()
        || !command
            .chars()
            .all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '-')
    {
        return None;
    }
    let args = parts.next().unwrap_or("").trim().to_string();
    Some((command, args))
}

pub async fn language(db: &Database) -> Lang {
    Lang::parse(db.get(key::LANGUAGE).await.as_str().unwrap_or("en"))
}

pub async fn set_language(db: &Database, lang: Lang) -> Result<()> {
    db.set(key::LANGUAGE, Value::String(lang.code().to_string()))
        .await
}

pub async fn disabled_modules(db: &Database) -> Vec<String> {
    string_list(&db.get(key::DISABLED_MODULES).await)
}

pub async fn set_module_enabled(db: &Database, module: &str, enabled: bool) -> Result<()> {
    let mut disabled = disabled_modules(db).await;
    disabled.retain(|name| name != module);
    if !enabled {
        disabled.push(module.to_string());
    }
    disabled.sort();
    db.set(
        key::DISABLED_MODULES,
        Value::Array(disabled.into_iter().map(Value::String).collect()),
    )
    .await
}

pub async fn sudo_users(db: &Database) -> Vec<i64> {
    match db.get(key::SUDO_USERS).await {
        Value::Array(values) => values
            .iter()
            .filter_map(|value| {
                value
                    .as_i64()
                    .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub async fn set_sudo_users(db: &Database, users: &[i64]) -> Result<()> {
    let mut users = users.to_vec();
    users.sort_unstable();
    users.dedup();
    db.set(
        key::SUDO_USERS,
        Value::Array(users.into_iter().map(Value::from).collect()),
    )
    .await
}

pub async fn flag(db: &Database, key: &str, default: bool) -> bool {
    db.get(key).await.as_bool().unwrap_or(default)
}

pub async fn bot_token(db: &Database) -> Option<String> {
    db.get(key::BOT_TOKEN)
        .await
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var(crate::config::env_key::TELOXIDE_TOKEN)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

fn string_list(value: &Value) -> Vec<String> {
    match value {
        Value::Array(values) => values
            .iter()
            .filter_map(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect(),
        Value::String(value) => value
            .split([',', ' '])
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn parses_commands_with_multiple_prefixes() {
        let prefixes = p(&[".", "!", ".."]);
        assert_eq!(
            parse_command(".ping", &prefixes),
            Some(("ping".into(), "".into()))
        );
        assert_eq!(
            parse_command("!Note set  hello world", &prefixes),
            Some(("note".into(), "set  hello world".into()))
        );
        assert_eq!(
            parse_command("..x a", &prefixes),
            Some(("x".into(), "a".into()))
        );
        assert_eq!(parse_command("hello", &prefixes), None);
        assert_eq!(parse_command(". ping", &prefixes), None);
        assert_eq!(parse_command("...", &prefixes), None);
    }

    #[test]
    fn validates_prefixes() {
        assert!(validate_prefixes(&p(&["a"])).is_err());
        assert!(validate_prefixes(&p(&["...."])).is_err());
        assert!(validate_prefixes(&p(&[""])).is_err());
        assert_eq!(
            validate_prefixes(&p(&[".", "!!", "."])).unwrap(),
            p(&["!!", "."])
        );
    }

    #[test]
    fn sudo_denylist_covers_sensitive_commands() {
        for command in ["eval", "term", "cfg", "note", "ask", "restart", "panel"] {
            assert!(SUDO_DENYLIST.contains(&command), "{command}");
        }
        assert!(!SUDO_DENYLIST.contains(&"ping"));
        assert!(!SUDO_DENYLIST.contains(&"weather"));
    }

    #[test]
    fn string_list_accepts_csv() {
        assert_eq!(string_list(&Value::String(". , !".into())), p(&[".", "!"]));
    }
}
