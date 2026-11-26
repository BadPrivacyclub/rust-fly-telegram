//! Permission requests: a sandboxed module that calls something it may not use asks the owner
//! through the inbox instead of only failing.
//!
//! A command waits up to [`WAIT`] for the answer, since the owner just sent it. Background
//! calls (`on_message`, schedules) are refused at once and leave the request in the inbox.
//! The owner can allow once (for [`ONCE_FOR`]), allow within a scope (this chat, this host,
//! or this account), allow everywhere, or refuse for good. Identical open requests share one
//! inbox event.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::watch;

/// How long a command waits for the owner.
pub const WAIT: Duration = Duration::from_secs(60);
/// How long "allow once" lasts.
pub const ONCE_FOR: Duration = Duration::from_secs(10 * 60);
/// Module and permission pairs refused for good, as `module:permission`.
pub const DENIED_KEY: &str = "core.permission_denied";
/// Permissions that are never granted for good from a request: they hand over the computer
/// or the account, so they are granted on the module page after a look at the source.
pub const MANUAL_ONLY: &[&str] = &["shell", "secrets", "modules.install", "core.admin"];

/// What the request is about beyond the permission: the host of a request, or the chat and
/// account of the call.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Subject {
    pub module: String,
    pub permission: String,
    pub host: Option<String>,
    pub chat: Option<i64>,
    pub account: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Pending,
    Allowed,
    Denied,
}

struct Open {
    id: u64,
    subject: Subject,
    answer: watch::Sender<Decision>,
}

/// A temporary grant: module, permission, and host (`None` for every host).
type OnceKey = (String, String, Option<String>);

#[derive(Default)]
pub struct Requests {
    open: Mutex<Vec<Open>>,
    next_id: Mutex<u64>,
    /// Temporary grants and when they end.
    once: Mutex<HashMap<OnceKey, Instant>>,
}

pub enum Opened {
    /// A new request; the caller posts it to the inbox.
    New(u64, watch::Receiver<Decision>),
    /// The same request is already open.
    Existing(u64, watch::Receiver<Decision>),
}

impl Requests {
    /// Opens a request, or joins the open one about the same subject.
    pub fn open(&self, subject: Subject) -> Opened {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = open.iter().find(|o| o.subject == subject) {
            return Opened::Existing(existing.id, existing.answer.subscribe());
        }
        let id = {
            let mut next = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
            *next += 1;
            *next
        };
        let (answer, receiver) = watch::channel(Decision::Pending);
        open.push(Open {
            id,
            subject,
            answer,
        });
        Opened::New(id, receiver)
    }

    pub fn subject(&self, id: u64) -> Option<Subject> {
        let open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        open.iter().find(|o| o.id == id).map(|o| o.subject.clone())
    }

    /// Closes a request and wakes every call waiting for it.
    pub fn close(&self, id: u64, decision: Decision) {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(index) = open.iter().position(|o| o.id == id) {
            let closed = open.remove(index);
            let _ = closed.answer.send(decision);
        }
    }

    pub fn allow_once(&self, subject: &Subject) {
        let mut once = self.once.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        once.retain(|_, until| *until > now);
        once.insert(
            (
                subject.module.clone(),
                subject.permission.clone(),
                subject.host.clone(),
            ),
            now + ONCE_FOR,
        );
    }

    /// Whether a temporary grant covers the call. A grant without a host covers every host.
    pub fn allowed_once(&self, module: &str, permission: &str, host: Option<&str>) -> bool {
        let once = self.once.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let live = |host: Option<String>| {
            once.get(&(module.to_string(), permission.to_string(), host))
                .is_some_and(|until| *until > now)
        };
        live(None) || host.is_some_and(|host| live(Some(host.to_string())))
    }

    /// Hosts with a live temporary `network` grant for the module. `None` when a grant covers
    /// every host.
    pub fn once_hosts(&self, module: &str) -> Option<Vec<String>> {
        let once = self.once.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let mut hosts = Vec::new();
        for ((name, permission, host), until) in once.iter() {
            if name != module || permission != "network" || *until <= now {
                continue;
            }
            hosts.push(host.clone()?);
        }
        Some(hosts)
    }
}

pub fn denied_entry(module: &str, permission: &str) -> String {
    format!("{module}:{permission}")
}

pub fn denied_set(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(host: Option<&str>) -> Subject {
        Subject {
            module: "weather".into(),
            permission: "network".into(),
            host: host.map(str::to_string),
            chat: Some(5),
            account: "main.session".into(),
        }
    }

    #[tokio::test]
    async fn identical_requests_share_one_answer() {
        let requests = Requests::default();
        let Opened::New(id, mut first) = requests.open(subject(Some("a.com"))) else {
            panic!("the first request is new");
        };
        let Opened::Existing(same, mut second) = requests.open(subject(Some("a.com"))) else {
            panic!("the second request joins the first");
        };
        assert_eq!(id, same);
        assert!(matches!(
            requests.open(subject(Some("b.com"))),
            Opened::New(..)
        ));
        requests.close(id, Decision::Allowed);
        first.changed().await.unwrap();
        second.changed().await.unwrap();
        assert_eq!(*first.borrow(), Decision::Allowed);
        assert_eq!(*second.borrow(), Decision::Allowed);
        assert!(requests.subject(id).is_none());
    }

    #[test]
    fn allow_once_covers_the_host_or_everything() {
        let requests = Requests::default();
        requests.allow_once(&subject(Some("a.com")));
        assert!(requests.allowed_once("weather", "network", Some("a.com")));
        assert!(!requests.allowed_once("weather", "network", Some("b.com")));
        requests.allow_once(&subject(None));
        assert!(requests.allowed_once("weather", "network", Some("b.com")));
        assert!(!requests.allowed_once("other", "network", Some("a.com")));
    }
}
