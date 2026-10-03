//! In-memory ring buffer of recent log records for the web panel and the bot.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

const CAPACITY: usize = 2000;

#[derive(Clone, Debug, Serialize)]
pub struct LogRecord {
    pub id: u64,
    pub ts_ms: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    records: VecDeque<LogRecord>,
}

#[derive(Default)]
pub struct LogBuffer {
    inner: Mutex<Inner>,
}

impl LogBuffer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn push(&self, level: &Level, target: &str, message: String) {
        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.next_id += 1;
        let record = LogRecord {
            id: inner.next_id,
            ts_ms,
            level: level.to_string(),
            target: target.to_string(),
            message,
        };
        inner.records.push_back(record);
        while inner.records.len() > CAPACITY {
            inner.records.pop_front();
        }
    }

    /// Records newer than `after_id`, filtered by minimum level, at most `limit` newest ones.
    pub fn since(&self, after_id: u64, min_level: Option<Level>, limit: usize) -> Vec<LogRecord> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        let mut records = inner
            .records
            .iter()
            .filter(|record| record.id > after_id)
            .filter(|record| match (&min_level, record.level.parse::<Level>()) {
                // tracing orders levels by verbosity: ERROR < WARN < INFO.
                (Some(min), Ok(level)) => level <= *min,
                _ => true,
            })
            .cloned()
            .collect::<Vec<_>>();
        if records.len() > limit {
            records.drain(..records.len() - limit);
        }
        records
    }

    pub fn recent_errors(&self, limit: usize) -> Vec<LogRecord> {
        self.since(0, Some(Level::WARN), limit)
    }
}

pub struct BufferLayer {
    buffer: Arc<LogBuffer>,
}

impl BufferLayer {
    pub fn new(buffer: Arc<LogBuffer>) -> Self {
        Self { buffer }
    }
}

impl<S: Subscriber> Layer<S> for BufferLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        // Dependency chatter at DEBUG/TRACE would evict useful records.
        if *metadata.level() > Level::INFO && !metadata.target().starts_with("fly_telegram") {
            return;
        }
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        self.buffer
            .push(metadata.level(), metadata.target(), visitor.finish());
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl MessageVisitor {
    fn finish(self) -> String {
        if self.fields.is_empty() {
            self.message
        } else {
            format!("{}{}", self.message, self.fields)
        }
    }
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            let _ = write!(self.fields, " {}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={value:?}", field.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_filters_and_limits() {
        let buffer = LogBuffer::default();
        buffer.push(&Level::INFO, "fly_telegram", "one".into());
        buffer.push(&Level::ERROR, "fly_telegram", "two".into());
        buffer.push(&Level::WARN, "fly_telegram", "three".into());

        assert_eq!(buffer.since(0, None, 10).len(), 3);
        let errors = buffer.since(0, Some(Level::WARN), 10);
        assert_eq!(
            errors
                .iter()
                .map(|r| r.message.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "three"]
        );
        let newest = buffer.since(0, None, 1);
        assert_eq!(newest[0].message, "three");
        assert_eq!(buffer.since(newest[0].id, None, 10).len(), 0);
    }
}
