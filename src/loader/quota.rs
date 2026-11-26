//! Limits for sandboxed modules: how often they may send messages or make HTTP requests, and
//! how much they may keep in the database. Trusted modules are not limited.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Largest value a sandboxed module may store under one key, as JSON.
pub const MAX_VALUE_BYTES: usize = 256 * 1024;
/// Most keys a sandboxed module may keep.
pub const MAX_KEYS: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// Sending, editing, scheduling, or reacting to messages, and notifications.
    Message,
    /// HTTP requests and downloads.
    Network,
}

impl Action {
    /// Burst size and the time to earn one more action back.
    fn rate(self) -> (f64, Duration) {
        match self {
            Self::Message => (20.0, Duration::from_secs(3)),
            Self::Network => (60.0, Duration::from_secs(1)),
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Message => "sends messages",
            Self::Network => "makes HTTP requests",
        }
    }
}

struct Bucket {
    tokens: f64,
    updated: Instant,
}

/// One token bucket per module and action.
#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<(String, Action), Bucket>>,
}

impl RateLimiter {
    pub fn take(&self, module: &str, action: Action) -> Result<(), String> {
        self.take_at(module, action, Instant::now())
    }

    fn take_at(&self, module: &str, action: Action, now: Instant) -> Result<(), String> {
        let (burst, refill) = action.rate();
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let bucket = buckets
            .entry((module.to_string(), action))
            .or_insert(Bucket {
                tokens: burst,
                updated: now,
            });
        let earned =
            now.saturating_duration_since(bucket.updated).as_secs_f64() / refill.as_secs_f64();
        bucket.tokens = (bucket.tokens + earned).min(burst);
        bucket.updated = now;
        if bucket.tokens < 1.0 {
            return Err(format!(
                "module '{module}' {} too often, try again later",
                action.describe()
            ));
        }
        bucket.tokens -= 1.0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_is_allowed_then_refused_then_refilled() {
        let limiter = RateLimiter::default();
        let start = Instant::now();
        for _ in 0..20 {
            limiter.take_at("m", Action::Message, start).unwrap();
        }
        assert!(limiter.take_at("m", Action::Message, start).is_err());
        assert!(limiter.take_at("other", Action::Message, start).is_ok());
        assert!(limiter.take_at("m", Action::Network, start).is_ok());
        let later = start + Duration::from_secs(3);
        assert!(limiter.take_at("m", Action::Message, later).is_ok());
        assert!(limiter.take_at("m", Action::Message, later).is_err());
    }
}

/// How often a sandboxed module used each permission, and how often it was refused one it
/// lacks, since the start. Shown in the panel next to the permissions.
#[derive(Default)]
pub struct UsageLog {
    entries: Mutex<HashMap<(String, String, bool), UsageEntry>>,
}

#[derive(Clone, Copy)]
struct UsageEntry {
    count: u64,
    last: u64,
}

#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub struct PermissionUse {
    pub permission: String,
    pub allowed: bool,
    pub count: u64,
    /// Unix time of the last use, in seconds.
    pub last: u64,
}

impl UsageLog {
    pub fn record(&self, module: &str, permission: &str, allowed: bool) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries
            .entry((module.to_string(), permission.to_string(), allowed))
            .or_insert(UsageEntry { count: 0, last: 0 });
        entry.count += 1;
        entry.last = now;
    }

    pub fn for_module(&self, module: &str) -> Vec<PermissionUse> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut uses = entries
            .iter()
            .filter(|((name, _, _), _)| name == module)
            .map(|((_, permission, allowed), entry)| PermissionUse {
                permission: permission.clone(),
                allowed: *allowed,
                count: entry.count,
                last: entry.last,
            })
            .collect::<Vec<_>>();
        uses.sort_by(|a, b| {
            a.permission
                .cmp(&b.permission)
                .then(b.allowed.cmp(&a.allowed))
        });
        uses
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn uses_are_counted_per_module_permission_and_outcome() {
        let log = UsageLog::default();
        log.record("m", "network", true);
        log.record("m", "network", true);
        log.record("m", "shell", false);
        log.record("other", "network", true);
        let uses = log.for_module("m");
        assert_eq!(uses.len(), 2);
        assert_eq!(
            (uses[0].permission.as_str(), uses[0].allowed, uses[0].count),
            ("network", true, 2)
        );
        assert_eq!(
            (uses[1].permission.as_str(), uses[1].allowed, uses[1].count),
            ("shell", false, 1)
        );
        assert!(uses[0].last > 0);
    }
}
