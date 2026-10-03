//! Owner notifications delivered by the control bot.
//!
//! The userbot cannot notify itself in a way that rings the phone, but a bot DM can. Each
//! notification topic can be switched off from Telegram, the bot, or the panel.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use teloxide::prelude::*;
use teloxide::types::{ChatId, InlineKeyboardMarkup, ParseMode};
use tokio::sync::{Mutex, RwLock};
use tracing::debug;

use crate::core_settings::{self, key};
use crate::database::Database;
use crate::runtime::RuntimeState;
use crate::telegram::StoredPeer;

const DEDUPE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug)]
pub enum Topic {
    Errors,
    PmGuard,
    Startup,
    AntiDelete,
    /// Sent by modules through `ctx:notify`.
    Module,
    /// Background job results (cleanup, bulk actions).
    Jobs,
}

impl Topic {
    fn setting(self) -> (&'static str, bool) {
        match self {
            Topic::Errors => (key::NOTIFY_ERRORS, true),
            Topic::PmGuard => (key::NOTIFY_PMGUARD, true),
            Topic::Startup => (key::NOTIFY_STARTUP, true),
            Topic::AntiDelete => (key::NOTIFY_ANTIDELETE, false),
            Topic::Module => (key::NOTIFY_MODULES, true),
            Topic::Jobs => (key::NOTIFY_JOBS, true),
        }
    }
}

/// A private chat waiting for an allow/deny decision from the bot.
#[derive(Clone)]
pub struct PendingPm {
    pub client: grammers_client::Client,
    pub peer: StoredPeer,
    pub name: String,
}

pub struct Notifier {
    bot: RwLock<Option<Bot>>,
    bot_username: RwLock<Option<String>>,
    db: Arc<Database>,
    runtime: Arc<RuntimeState>,
    recent: Mutex<HashMap<String, Instant>>,
    pub pm_pending: Mutex<HashMap<i64, PendingPm>>,
}

impl Notifier {
    pub fn new(db: Arc<Database>, runtime: Arc<RuntimeState>) -> Arc<Self> {
        Arc::new(Self {
            bot: RwLock::new(None),
            bot_username: RwLock::new(None),
            db,
            runtime,
            recent: Mutex::new(HashMap::new()),
            pm_pending: Mutex::new(HashMap::new()),
        })
    }

    pub async fn set_bot(&self, bot: Option<Bot>, username: Option<String>) {
        *self.bot.write().await = bot;
        *self.bot_username.write().await = username;
    }

    pub async fn bot_username(&self) -> Option<String> {
        self.bot_username.read().await.clone()
    }

    pub async fn bot(&self) -> Option<Bot> {
        self.bot.read().await.clone()
    }

    pub async fn enabled(&self, topic: Topic) -> bool {
        let (key, default) = topic.setting();
        core_settings::flag(&self.db, key, default).await
    }

    /// Sends an HTML message to every owner account. Silently skipped without a bot,
    /// when the topic is off, or when the same text was sent within the last minute.
    pub async fn send(&self, topic: Topic, html: &str) {
        self.send_with_keyboard(topic, html, None).await;
    }

    pub async fn send_with_keyboard(
        &self,
        topic: Topic,
        html: &str,
        keyboard: Option<InlineKeyboardMarkup>,
    ) {
        let Some(bot) = self.bot().await else {
            return;
        };
        if !self.enabled(topic).await || self.is_duplicate(html).await {
            return;
        }
        for owner in self.runtime.owner_ids().await {
            let mut request = bot
                .send_message(ChatId(owner), html)
                .parse_mode(ParseMode::Html);
            if let Some(keyboard) = keyboard.clone() {
                request = request.reply_markup(keyboard);
            }
            if let Err(error) = request.await {
                // The owner may not have pressed /start yet; that is not worth a warning.
                debug!("bot notification to {owner} failed: {error}");
            }
        }
    }

    async fn is_duplicate(&self, text: &str) -> bool {
        let mut recent = self.recent.lock().await;
        let now = Instant::now();
        recent.retain(|_, at| now.duration_since(*at) < DEDUPE_WINDOW);
        if recent.contains_key(text) {
            return true;
        }
        recent.insert(text.to_string(), now);
        false
    }
}

pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn clip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text.chars().take(max_chars).collect::<String>();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_clips() {
        assert_eq!(escape_html("<b>&"), "&lt;b&gt;&amp;");
        assert_eq!(clip("hello", 3), "hel…");
        assert_eq!(clip("hi", 3), "hi");
    }
}
