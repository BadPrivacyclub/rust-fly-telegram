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
