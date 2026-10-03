//! Automation rules, stored in the database and run for one or several accounts.
//!
//! Time-driven rules run on a 30-second tick: scheduled messages, a clock in the profile
//! name or bio, bio rotation, and the online/offline status. Message-driven rules react to
//! new messages: away replies, forwarding, auto-delete of own messages, and auto-read.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use grammers_client::update::Message;
use grammers_client::{tl, Client};
use grammers_session::types::{PeerKind, PeerRef};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::account_tools::{now_secs, AccountHandle};
use crate::app::Services;
use crate::telegram::{self, StoredPeer};

pub const RULES_KEY: &str = "auto.rules";
const TICK: Duration = Duration::from_secs(30);
const MAX_RULES: usize = 100;
const MAX_TEXT: usize = 4000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleKind {
    /// Send a message on a schedule: every N minutes, or daily at HH:MM on chosen weekdays.
    Schedule {
        chat: String,
        text: String,
        #[serde(default)]
        every_minutes: Option<u32>,
        #[serde(default)]
        at: Option<String>,
        /// ISO weekdays 1 (Monday) .. 7 (Sunday); empty means every day.
        #[serde(default)]
        days: Vec<u8>,
    },
    /// Reply once per sender to private messages inside a time window.
    AwayReply {
        text: String,
        #[serde(default = "default_from")]
        from: String,
        #[serde(default = "default_to")]
        to: String,
        #[serde(default = "default_cooldown")]
        cooldown_minutes: u32,
    },
    /// Forward new messages from one chat to another, optionally only matching keywords.
    Forward {
        from_chat: i64,
        to_chat: String,
        #[serde(default)]
        keywords: Vec<String>,
    },
    /// Delete your own messages in the chosen chats after a delay.
    AutoDelete { chats: Vec<i64>, after_minutes: u32 },
    /// Keep the current time in your name or bio. Template placeholders:
    /// {time} {date} {weekday} {clock}
    ProfileClock { field: String, template: String },
    /// Cycle the bio through phrases.
    BioRotation {
        phrases: Vec<String>,
        every_minutes: u32,
    },
    /// Keep the account shown online, or always offline.
    Presence { online: bool },
    /// Mark new messages in the chosen chats as read.
    AutoRead { chats: Vec<i64> },
}

fn default_from() -> String {
    "23:00".into()
}
fn default_to() -> String {
    "08:00".into()
}
fn default_cooldown() -> u32 {
    60
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    /// Account user IDs; empty means every connected account.
    #[serde(default)]
    pub accounts: Vec<i64>,
    /// Offset from UTC in minutes for time-based rules.
    #[serde(default)]
    pub utc_offset_minutes: i32,
    #[serde(flatten)]
    pub kind: RuleKind,
}

fn enabled_default() -> bool {
    true
}

impl Rule {
    pub fn applies_to(&self, user_id: i64) -> bool {
        self.enabled && (self.accounts.is_empty() || self.accounts.contains(&user_id))
    }

    pub fn type_name(&self) -> &'static str {
        match self.kind {
            RuleKind::Schedule { .. } => "schedule",
            RuleKind::AwayReply { .. } => "away_reply",
            RuleKind::Forward { .. } => "forward",
            RuleKind::AutoDelete { .. } => "auto_delete",
            RuleKind::ProfileClock { .. } => "profile_clock",
            RuleKind::BioRotation { .. } => "bio_rotation",
            RuleKind::Presence { .. } => "presence",
            RuleKind::AutoRead { .. } => "auto_read",
        }
    }

    /// Checks the rule and fills in defaults. Returns a readable error for the editor.
    pub fn validate(&mut self) -> Result<()> {
        if self.id.trim().is_empty() {
            self.id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        }
        if self.name.trim().is_empty() {
            self.name = self.type_name().replace('_', " ");
        }
        if !(-14 * 60..=14 * 60).contains(&self.utc_offset_minutes) {
            anyhow::bail!("UTC offset must be between -14 and +14 hours");
        }
        let check_text = |text: &str| -> Result<()> {
            if text.trim().is_empty() {
                anyhow::bail!("text cannot be empty");
            }
            if text.chars().count() > MAX_TEXT {
                anyhow::bail!("text is longer than {MAX_TEXT} characters");
            }
            Ok(())
        };
        match &mut self.kind {
            RuleKind::Schedule {
                chat,
                text,
                every_minutes,
                at,
                days,
            } => {
                check_text(text)?;
                if chat.trim().is_empty() {
                    anyhow::bail!("choose a chat: me, @username, or a chat ID");
                }
                match (every_minutes.as_ref(), at.as_deref()) {
                    (Some(minutes), None) if *minutes >= 1 => {}
                    (None, Some(time)) => {
                        parse_hhmm(time).context("time must look like 09:30")?;
                    }
                    _ => anyhow::bail!("set either an interval in minutes or a time of day"),
                }
                if days.iter().any(|d| !(1..=7).contains(d)) {
                    anyhow::bail!("weekdays are numbered 1 (Monday) to 7 (Sunday)");
                }
            }
            RuleKind::AwayReply { text, from, to, .. } => {
                check_text(text)?;
                parse_hhmm(from).context("start time must look like 23:00")?;
                parse_hhmm(to).context("end time must look like 08:00")?;
            }
            RuleKind::Forward { to_chat, .. } => {
                if to_chat.trim().is_empty() {
                    anyhow::bail!("choose a destination chat");
                }
            }
            RuleKind::AutoDelete {
                chats,
                after_minutes,
            } => {
                if chats.is_empty() {
                    anyhow::bail!("add at least one chat ID");
                }
                if !(1..=7 * 24 * 60).contains(after_minutes) {
                    anyhow::bail!("delay must be between 1 minute and 7 days");
                }
            }
            RuleKind::ProfileClock { field, template } => {
                if field != "name" && field != "bio" {
                    anyhow::bail!("field must be name or bio");
                }
                check_text(template)?;
                let limit = if field == "name" { 64 } else { 70 };
                if render_template(template, 0).chars().count() > limit {
                    anyhow::bail!("the rendered text is longer than Telegram allows ({limit})");
                }
            }
            RuleKind::BioRotation {
                phrases,
                every_minutes,
            } => {
                phrases.retain(|p| !p.trim().is_empty());
                if phrases.is_empty() {
                    anyhow::bail!("add at least one phrase");
                }
                if *every_minutes < 1 {
                    anyhow::bail!("interval must be at least 1 minute");
                }
            }
            RuleKind::AutoRead { chats } => {
                if chats.is_empty() {
                    anyhow::bail!("add at least one chat ID");
                }
            }
            RuleKind::Presence { .. } => {}
        }
        Ok(())
    }
}

pub fn parse_hhmm(text: &str) -> Option<u32> {
    let (h, m) = text.trim().split_once(':')?;
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Minute of the local day and ISO weekday (1 = Monday) for a UNIX time and UTC offset.
pub fn local_minute_and_weekday(unix: i64, offset_minutes: i32) -> (u32, u8) {
    let local = unix + i64::from(offset_minutes) * 60;
    let minute = local.rem_euclid(86_400) / 60;
    // 1970-01-01 was a Thursday (ISO weekday 4).
    let weekday = ((local.div_euclid(86_400) + 3).rem_euclid(7) + 1) as u8;
    (minute as u32, weekday)
}

/// True when `minute` falls inside a window that may wrap past midnight.
pub fn in_window(minute: u32, from: u32, to: u32) -> bool {
    if from <= to {
        minute >= from && minute < to
    } else {
        minute >= from || minute < to
    }
}

pub fn render_template(template: &str, local_unix: i64) -> String {
    const CLOCKS: [&str; 12] = [
        "🕛", "🕐", "🕑", "🕒", "🕓", "🕔", "🕕", "🕖", "🕗", "🕘", "🕙", "🕚",
    ];
    const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let minute_of_day = local_unix.rem_euclid(86_400) / 60;
    let (hour, minute) = (minute_of_day / 60, minute_of_day % 60);
    let days = local_unix.div_euclid(86_400);
    let (_, month, day) = civil(days);
    let weekday = WEEKDAYS[((days + 3).rem_euclid(7)) as usize];
    template
        .replace("{time}", &format!("{hour:02}:{minute:02}"))
        .replace("{date}", &format!("{day:02}.{month:02}"))
        .replace("{weekday}", weekday)
        .replace("{clock}", CLOCKS[(hour % 12) as usize])
}

fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

pub async fn load_rules(services: &Services) -> Vec<Rule> {
    match services.db.get(RULES_KEY).await {
        Value::Array(values) => values
            .into_iter()
            .filter_map(|value| match serde_json::from_value::<Rule>(value) {
                Ok(rule) => Some(rule),
                Err(error) => {
                    warn!("skipping invalid automation rule: {error}");
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

async fn save_rules(services: &Services, rules: &[Rule]) -> Result<()> {
    services
        .db
        .set(RULES_KEY, serde_json::to_value(rules)?)
        .await
}

pub async fn upsert_rule(services: &Services, mut rule: Rule) -> Result<Rule> {
    rule.validate()?;
    let mut rules = load_rules(services).await;
    match rules.iter_mut().find(|r| r.id == rule.id) {
        Some(existing) => *existing = rule.clone(),
        None => {
            if rules.len() >= MAX_RULES {
                anyhow::bail!("at most {MAX_RULES} rules are allowed");
            }
            rules.push(rule.clone());
        }
    }
    save_rules(services, &rules).await?;
    Ok(rule)
}

pub async fn set_enabled(services: &Services, id: &str, enabled: bool) -> Result<()> {
    let mut rules = load_rules(services).await;
    let rule = rules
        .iter_mut()
        .find(|r| r.id == id)
        .with_context(|| format!("no rule with id {id}"))?;
    rule.enabled = enabled;
    save_rules(services, &rules).await
}

pub async fn delete_rule(services: &Services, id: &str) -> Result<()> {
    let mut rules = load_rules(services).await;
    let before = rules.len();
    rules.retain(|r| r.id != id);
    if rules.len() == before {
        anyhow::bail!("no rule with id {id}");
    }
    save_rules(services, &rules).await
}

/// In-memory state: last runs, rendered profile text, reply cooldowns, resolved chats.
#[derive(Default)]
pub struct Engine {
    last_run: Mutex<HashMap<(String, i64), i64>>,
    last_profile: Mutex<HashMap<(String, i64), String>>,
    replied: Mutex<HashMap<(String, i64, i64), i64>>,
    peers: Mutex<HashMap<(i64, String), StoredPeer>>,
}

impl Engine {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Resolves `me`, `@username`, or a numeric chat ID to a peer this account can use.
    pub async fn resolve_chat(&self, handle: &AccountHandle, spec: &str) -> Result<PeerRef> {
        let spec = spec.trim();
        if spec.eq_ignore_ascii_case("me") || spec.eq_ignore_ascii_case("saved") {
            return Ok(telegram::saved_messages());
        }
        let cache_key = (handle.user_id, spec.to_lowercase());
        if let Some(peer) = self.peers.lock().await.get(&cache_key).copied() {
            if let Some(peer) = peer.to_ref() {
                return Ok(peer);
            }
        }
        let peer = if let Some(username) = spec.strip_prefix('@') {
            let resolved = handle
                .client
                .resolve_username(username)
                .await?
                .with_context(|| format!("@{username} not found"))?;
            resolved
                .to_ref()
                .await
                .with_context(|| format!("cannot access @{username}"))?
        } else {
            let id: i64 = spec
                .parse()
                .with_context(|| format!("'{spec}' is not me, @username, or a chat ID"))?;
            find_dialog_peer(&handle.client, id)
                .await?
                .with_context(|| format!("chat {id} is not in this account's chat list"))?
        };
        self.peers
            .lock()
            .await
            .insert(cache_key, StoredPeer::from_ref(peer));
        Ok(peer)
    }
}

async fn find_dialog_peer(client: &Client, id: i64) -> Result<Option<PeerRef>> {
    let mut dialogs = client.iter_dialogs();
    while let Some(dialog) = dialogs.next().await? {
        if dialog.peer_id().bot_api_dialog_id() == id {
            return Ok(Some(dialog.peer_ref()));
        }
    }
    Ok(None)
}

pub fn spawn(services: Arc<Services>, engine: Arc<Engine>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let rules = load_rules(&services).await;
            if rules.iter().all(|r| !r.enabled) {
                continue;
            }
            for handle in services.accounts.list().await {
                for rule in rules.iter().filter(|r| r.applies_to(handle.user_id)) {
                    if let Err(error) = tick_rule(&services, &engine, &handle, rule).await {
                        warn!("automation '{}' for {}: {error:#}", rule.name, handle.name);
                    }
                }
            }
        }
    });
    info!("automation engine started");
}

async fn tick_rule(
    services: &Services,
    engine: &Engine,
    handle: &AccountHandle,
    rule: &Rule,
) -> Result<()> {
    let now = now_secs();
    let key = (rule.id.clone(), handle.user_id);
    let (minute, weekday) = local_minute_and_weekday(now, rule.utc_offset_minutes);
    match &rule.kind {
        RuleKind::Schedule {
            chat,
            text,
            every_minutes,
            at,
            days,
        } => {
            if !days.is_empty() && !days.contains(&weekday) {
                return Ok(());
            }
            let last = engine.last_run.lock().await.get(&key).copied();
            let due = match (every_minutes, at.as_deref().and_then(parse_hhmm)) {
                (Some(every), _) => match last {
                    Some(last) => now - last >= i64::from(*every) * 60,
                    // First tick after start or creation: wait one interval instead of
                    // firing immediately on every restart.
                    None => {
                        engine.last_run.lock().await.insert(key.clone(), now);
                        false
                    }
                },
                (None, Some(at)) => minute == at && last.is_none_or(|last| now - last > 120),
                _ => false,
            };
            if due {
                engine.last_run.lock().await.insert(key, now);
                let peer = engine.resolve_chat(handle, chat).await?;
                let local = now + i64::from(rule.utc_offset_minutes) * 60;
                telegram::send_markdown(
                    &handle.client,
                    &services.runtime,
                    peer,
                    &render_template(text, local),
                )
                .await?;
            }
        }
        RuleKind::ProfileClock { field, template } => {
            let local = now + i64::from(rule.utc_offset_minutes) * 60;
            let text = render_template(template, local);
            let mut last = engine.last_profile.lock().await;
            if last.get(&key) == Some(&text) {
                return Ok(());
            }
            set_profile_field(&handle.client, field, &text).await?;
            last.insert(key, text);
        }
        RuleKind::BioRotation {
            phrases,
            every_minutes,
        } => {
            let slot = now / (i64::from((*every_minutes).max(1)) * 60);
            let phrase = &phrases[(slot as usize) % phrases.len().max(1)];
            let mut last = engine.last_profile.lock().await;
            if last.get(&key) == Some(phrase) {
                return Ok(());
            }
            set_profile_field(&handle.client, "bio", phrase).await?;
            last.insert(key, phrase.clone());
        }
        RuleKind::Presence { online } => {
            handle
                .client
                .invoke(&tl::functions::account::UpdateStatus { offline: !online })
                .await?;
        }
        _ => {}
    }
    Ok(())
}

async fn set_profile_field(client: &Client, field: &str, text: &str) -> Result<()> {
    let request = if field == "name" {
        tl::functions::account::UpdateProfile {
            first_name: Some(text.to_string()),
            last_name: None,
            about: None,
        }
    } else {
        tl::functions::account::UpdateProfile {
            first_name: None,
            last_name: None,
            about: Some(text.to_string()),
        }
    };
    client.invoke(&request).await?;
    Ok(())
}

/// Runs message-driven rules for a new message on `handle`'s account.
pub async fn on_message(
    services: Arc<Services>,
    engine: Arc<Engine>,
    handle: AccountHandle,
    msg: Message,
) {
    let rules = load_rules(&services).await;
    for rule in rules.iter().filter(|r| r.applies_to(handle.user_id)) {
        if let Err(error) = handle_message_rule(&services, &engine, &handle, rule, &msg).await {
            warn!("automation '{}' for {}: {error:#}", rule.name, handle.name);
        }
    }
}

async fn handle_message_rule(
    services: &Arc<Services>,
    engine: &Engine,
    handle: &AccountHandle,
    rule: &Rule,
    msg: &Message,
) -> Result<()> {
    let chat_id = msg.peer_id().bot_api_dialog_id();
    match &rule.kind {
        RuleKind::AwayReply {
            text,
            from,
            to,
            cooldown_minutes,
        } => {
            if msg.outgoing() || !matches!(msg.peer_id().kind(), PeerKind::User) {
                return Ok(());
            }
            if msg.sender().is_some_and(
                |peer| matches!(peer, grammers_client::peer::Peer::User(u) if u.is_bot()),
            ) {
                return Ok(());
            }
            let (minute, _) = local_minute_and_weekday(now_secs(), rule.utc_offset_minutes);
            let (Some(from), Some(to)) = (parse_hhmm(from), parse_hhmm(to)) else {
                return Ok(());
            };
            if !in_window(minute, from, to) {
                return Ok(());
            }
            let now = now_secs();
            let key = (rule.id.clone(), handle.user_id, chat_id);
            {
                let mut replied = engine.replied.lock().await;
                if replied
                    .get(&key)
                    .is_some_and(|last| now - last < i64::from(*cooldown_minutes) * 60)
                {
                    return Ok(());
                }
                replied.insert(key, now);
            }
            let local = now + i64::from(rule.utc_offset_minutes) * 60;
            telegram::msg_respond(&services.runtime, msg, &render_template(text, local)).await?;
        }
        RuleKind::Forward {
            from_chat,
            to_chat,
            keywords,
        } => {
            if chat_id != *from_chat {
                return Ok(());
            }
            if !keywords.is_empty() {
                let text = msg.text().to_lowercase();
                if !keywords.iter().any(|k| text.contains(&k.to_lowercase())) {
                    return Ok(());
                }
            }
            let source = telegram::resolve_message_peer(&handle.client, msg).await?;
            let destination = engine.resolve_chat(handle, to_chat).await?;
            services.runtime.wait_for_telegram_send().await;
            handle
                .client
                .forward_messages(destination, &[msg.id()], source)
                .await?;
        }
        RuleKind::AutoDelete {
            chats,
            after_minutes,
        } => {
            if !msg.outgoing() || !chats.contains(&chat_id) {
                return Ok(());
            }
            let client = handle.client.clone();
            let runtime = Arc::clone(&services.runtime);
            let peer = telegram::resolve_message_peer(&client, msg).await?;
            let id = msg.id();
            let delay = Duration::from_secs(u64::from(*after_minutes) * 60);
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                if let Err(error) = telegram::delete_messages(&client, &runtime, peer, &[id]).await
                {
                    warn!("auto-delete failed: {error}");
                }
            });
        }
        RuleKind::AutoRead { chats } if !msg.outgoing() && chats.contains(&chat_id) => {
            telegram::mark_message_as_read(msg, &services.runtime).await?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(kind: RuleKind) -> Rule {
        Rule {
            id: String::new(),
            name: String::new(),
            enabled: true,
            accounts: Vec::new(),
            utc_offset_minutes: 0,
            kind,
        }
    }

    #[test]
    fn parses_times_and_windows() {
        assert_eq!(parse_hhmm("09:30"), Some(570));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("nope"), None);
        assert!(in_window(23 * 60 + 30, 23 * 60, 8 * 60));
        assert!(in_window(60, 23 * 60, 8 * 60));
        assert!(!in_window(12 * 60, 23 * 60, 8 * 60));
        assert!(in_window(10 * 60, 9 * 60, 18 * 60));
    }

    #[test]
    fn local_time_and_weekday() {
        // 2024-01-01 00:00 UTC was a Monday.
        let monday = 1_704_067_200;
        assert_eq!(local_minute_and_weekday(monday, 0), (0, 1));
        assert_eq!(local_minute_and_weekday(monday, 180), (180, 1));
        assert_eq!(local_minute_and_weekday(monday, -60), (23 * 60, 7));
    }

    #[test]
    fn renders_templates() {
        let text = render_template(
            "{clock} {time} {date} {weekday}",
            1_704_067_200 + 13 * 3600 + 5 * 60,
        );
        assert_eq!(text, "🕐 13:05 01.01 Mon");
    }

    #[test]
    fn validates_rules() {
        let mut ok = rule(RuleKind::Schedule {
            chat: "me".into(),
            text: "hi".into(),
            every_minutes: Some(60),
            at: None,
            days: vec![],
        });
        ok.validate().unwrap();
        assert_eq!(ok.id.len(), 8);
        assert_eq!(ok.name, "schedule");

        let mut both = rule(RuleKind::Schedule {
            chat: "me".into(),
            text: "hi".into(),
            every_minutes: Some(5),
            at: Some("10:00".into()),
            days: vec![],
        });
        assert!(both.validate().is_err());

        let mut long_name = rule(RuleKind::ProfileClock {
            field: "name".into(),
            template: "x".repeat(80),
        });
        assert!(long_name.validate().is_err());

        let mut bad_field = rule(RuleKind::ProfileClock {
            field: "photo".into(),
            template: "{time}".into(),
        });
        assert!(bad_field.validate().is_err());

        let mut away = rule(RuleKind::AwayReply {
            text: "sleeping".into(),
            from: "23:00".into(),
            to: "7:5".into(),
            cooldown_minutes: 60,
        });
        away.validate().unwrap();

        let mut rotation = rule(RuleKind::BioRotation {
            phrases: vec!["".into(), "  ".into()],
            every_minutes: 10,
        });
        assert!(rotation.validate().is_err());
    }

    #[test]
    fn rules_round_trip_as_flat_json() {
        let json = serde_json::json!({
            "id": "abc", "name": "Night", "type": "away_reply", "text": "zz",
            "accounts": [1], "utc_offset_minutes": 180
        });
        let parsed: Rule = serde_json::from_value(json).unwrap();
        assert!(parsed.enabled);
        assert!(
            matches!(parsed.kind, RuleKind::AwayReply { ref from, cooldown_minutes: 60, .. } if from == "23:00")
        );
        assert!(parsed.applies_to(1) && !parsed.applies_to(2));
        let back = serde_json::to_value(&parsed).unwrap();
        assert_eq!(back["type"], "away_reply");
    }

    #[tokio::test]
    async fn storage_crud() {
        let services = crate::app::test_services().await;
        let saved = upsert_rule(&services, rule(RuleKind::Presence { online: true }))
            .await
            .unwrap();
        assert_eq!(load_rules(&services).await.len(), 1);
        set_enabled(&services, &saved.id, false).await.unwrap();
        assert!(!load_rules(&services).await[0].enabled);
        assert!(set_enabled(&services, "missing", true).await.is_err());
        delete_rule(&services, &saved.id).await.unwrap();
        assert!(load_rules(&services).await.is_empty());
    }
}
