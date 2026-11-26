//! Owner notifications: the inbox of the web panel, and optionally a private channel.
//!
//! Every notification lands in the inbox, which the panel polls. An event can carry actions,
//! such as allowing a new private chat, that the owner answers from the panel. With
//! `notify.channel` on, the text is also posted to a private channel of the first account, so
//! the phone rings even when the panel is closed.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use grammers_client::Client;
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, warn};

use crate::core_settings::{self, key};
use crate::database::Database;
use crate::runtime::RuntimeState;
use crate::telegram::StoredPeer;

const DEDUPE_WINDOW: Duration = Duration::from_secs(60);
/// Events the inbox keeps; older ones drop off.
const INBOX_LIMIT: usize = 500;
/// Where the channel copy goes.
const CHANNEL_STORE: &str = "notify.channel.target";
const CHANNEL_TITLE: &str = "fly-telegram";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Errors,
    PmGuard,
    Startup,
    AntiDelete,
    /// Sent by modules through `ctx:notify`.
    Module,
    /// Results of background jobs: exports, cleanup, bulk actions.
    Jobs,
    /// Permission requests of sandboxed modules.
    Permissions,
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
            Topic::Permissions => (key::NOTIFY_PERMISSIONS, true),
        }
    }
}

/// A private chat held while it waits for the owner to allow or block it.
#[derive(Clone)]
pub struct PendingPm {
    pub client: Client,
    pub peer: StoredPeer,
    pub name: String,
}

/// A button on an inbox event. `id` is what the panel sends back, such as `pm.allow:123`.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub danger: bool,
}

impl Action {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            danger: false,
        }
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub id: u64,
    pub topic: Topic,
    /// Unix time in seconds.
    pub time: u64,
    pub text: String,
    pub actions: Vec<Action>,
    /// What the owner chose, once an action was taken.
    pub resolved: Option<String>,
}

#[derive(Default)]
struct Inbox {
    events: VecDeque<Event>,
    next_id: u64,
    read_up_to: u64,
}

#[derive(Serialize)]
pub struct InboxPage {
    pub events: Vec<Event>,
    pub unread: usize,
    pub read_up_to: u64,
}

pub struct Notifier {
    db: Arc<Database>,
    runtime: Arc<RuntimeState>,
    recent: Mutex<HashMap<String, Instant>>,
    inbox: std::sync::Mutex<Inbox>,
    channel_client: RwLock<Option<Client>>,
    pub pm_pending: Mutex<HashMap<i64, PendingPm>>,
}

impl Notifier {
    pub fn new(db: Arc<Database>, runtime: Arc<RuntimeState>) -> Arc<Self> {
        Arc::new(Self {
            db,
            runtime,
            recent: Mutex::new(HashMap::new()),
            inbox: std::sync::Mutex::new(Inbox {
                next_id: 1,
                ..Inbox::default()
            }),
            channel_client: RwLock::new(None),
            pm_pending: Mutex::new(HashMap::new()),
        })
    }

    /// The account whose private channel receives copies. The first account to connect wins.
    pub async fn set_channel_client(&self, client: &Client) {
        let mut slot = self.channel_client.write().await;
        if slot.is_none() {
            *slot = Some(client.clone());
        }
    }

    pub async fn enabled(&self, topic: Topic) -> bool {
        let (key, default) = topic.setting();
        core_settings::flag(&self.db, key, default).await
    }

    /// Adds an event to the inbox, unless its topic is off or the same text arrived within the
    /// last minute. `html` uses the Telegram subset (`<b>`, `<code>`, `<pre>`, `<a>`).
    pub async fn send(&self, topic: Topic, html: &str) {
        self.send_with_actions(topic, html, Vec::new()).await;
    }

    pub async fn send_with_actions(&self, topic: Topic, html: &str, actions: Vec<Action>) {
        if !self.enabled(topic).await || self.is_duplicate(html).await {
            return;
        }
        let text = html_to_text(html);
        self.push(topic, text.clone(), actions);
        if core_settings::flag(&self.db, key::NOTIFY_CHANNEL, false).await {
            self.copy_to_channel(text).await;
        }
    }

    fn push(&self, topic: Topic, text: String, actions: Vec<Action>) -> u64 {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        let id = inbox.next_id;
        inbox.next_id += 1;
        inbox.events.push_back(Event {
            id,
            topic,
            time: unix_now(),
            text,
            actions,
            resolved: None,
        });
        while inbox.events.len() > INBOX_LIMIT {
            inbox.events.pop_front();
        }
        id
    }

    /// Events newer than `after`, oldest first.
    pub fn page(&self, after: u64) -> InboxPage {
        let inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        InboxPage {
            events: inbox
                .events
                .iter()
                .filter(|event| event.id > after)
                .cloned()
                .collect(),
            unread: inbox
                .events
                .iter()
                .filter(|event| event.id > inbox.read_up_to)
                .count(),
            read_up_to: inbox.read_up_to,
        }
    }

    pub fn mark_read(&self, up_to: u64) {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        inbox.read_up_to = inbox.read_up_to.max(up_to.min(inbox.next_id - 1));
    }

    /// The action `action_id` of event `event_id`, if the event still offers it.
    pub fn find_action(&self, event_id: u64, action_id: &str) -> Option<Action> {
        let inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        inbox
            .events
            .iter()
            .find(|event| event.id == event_id && event.resolved.is_none())
            .and_then(|event| event.actions.iter().find(|a| a.id == action_id).cloned())
    }

    /// Marks every open event that offers `action_id` as resolved with `outcome`, so the same
    /// question asked twice is answered once.
    pub fn resolve(&self, action_id: &str, outcome: &str) {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        for event in inbox.events.iter_mut() {
            if event.resolved.is_none() && event.actions.iter().any(|a| a.id == action_id) {
                event.resolved = Some(outcome.to_string());
            }
        }
    }

    async fn copy_to_channel(&self, text: String) {
        let Some(client) = self.channel_client.read().await.clone() else {
            return;
        };
        let db = Arc::clone(&self.db);
        let runtime = Arc::clone(&self.runtime);
        tokio::spawn(async move {
            let about = "fly-telegram notifications";
            let channel = match crate::private_channel::ensure(
                &client,
                &db,
                CHANNEL_STORE,
                CHANNEL_TITLE,
                about,
                false,
            )
            .await
            {
                Ok(channel) => channel,
                Err(error) => {
                    warn!("could not open the notification channel: {error:#}");
                    return;
                }
            };
            let Some(peer) = channel.peer_ref() else {
                return;
            };
            for part in crate::telegram::split_text(&text) {
                runtime.wait_for_telegram_send().await;
                let message = grammers_client::message::InputMessage::new().text(part);
                if let Err(error) = client.send_message(peer, message).await {
                    debug!("notification channel message failed: {error}");
                    return;
                }
            }
        });
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

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Plain text from the Telegram HTML subset: tags dropped, entities decoded, pictographs
/// removed, and blank lines collapsed. The panel shows the result as text, so no markup from
/// a message or a module can reach the page.
pub fn html_to_text(html: &str) -> String {
    static TAGS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static PICTOGRAPHS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let tags = TAGS.get_or_init(|| regex::Regex::new(r"<[^>]*>").expect("valid pattern"));
    let pictographs = PICTOGRAPHS.get_or_init(|| {
        regex::Regex::new(r"[\p{Extended_Pictographic}\u{FE0F}\u{200D}]").expect("valid pattern")
    });
    let text = tags.replace_all(html, "");
    let text = text
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let text = pictographs.replace_all(&text, "");
    text.lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
        .split("\n\n\n")
        .collect::<Vec<_>>()
        .join("\n\n")
        .trim()
        .to_string()
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

    #[test]
    fn html_becomes_safe_text() {
        assert_eq!(
            html_to_text(
                "❌ <b>Module error</b> <code>ai.ask</code>\n<pre>a &lt;b&gt; &amp; c</pre>"
            ),
            "Module error ai.ask\na <b> & c"
        );
        assert_eq!(html_to_text("<a href=\"tg://user?id=1\">Bob</a>"), "Bob");
    }

    #[tokio::test]
    async fn inbox_keeps_events_and_actions() {
        let services = crate::app::test_services().await;
        let notifier = &services.notifier;
        notifier
            .send_with_actions(
                Topic::PmGuard,
                "<b>New PM</b>",
                vec![
                    Action::new("pm.allow:5", "Allow"),
                    Action::new("pm.block:5", "Block").danger(),
                ],
            )
            .await;
        notifier.send(Topic::PmGuard, "<b>New PM</b>").await;
        let page = notifier.page(0);
        assert_eq!(
            page.events.len(),
            1,
            "duplicates within a minute are dropped"
        );
        assert_eq!(page.unread, 1);
        let id = page.events[0].id;
        assert!(notifier.find_action(id, "pm.allow:5").is_some());
        notifier.resolve("pm.allow:5", "Allowed");
        assert!(notifier.find_action(id, "pm.allow:5").is_none());
        notifier.mark_read(id);
        assert_eq!(notifier.page(0).unread, 0);
        assert!(notifier.page(id).events.is_empty());
    }
}
