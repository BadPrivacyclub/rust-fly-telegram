//! My in-memory ring buffer of recent log records for the web panel and the bot.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

const CAPACITY: usize = 2000;
const MASK: &str = "[hidden]";

/// What I cut out of every log line, because each of these opens the account or the panel:
/// bot tokens, proxy logins, named secrets, login links, API hashes and session ids, and
/// string sessions (long base64).
static SECRETS: LazyLock<[(Regex, String); 6]> = LazyLock::new(|| {
    let rule = |pattern: &str, replacement: String| {
        (
            Regex::new(pattern).expect("log masks are valid patterns"),
            replacement,
        )
    };
    [
        rule(r"\d{6,12}:[A-Za-z0-9_-]{30,}", MASK.to_string()),
        rule(r"://[^/\s:@]+:[^/\s@]+@", format!("://{MASK}@")),
        rule(
            r#"(?i)\b(password|passwd|passphrase|secret|api_hash|auth_key|bot_token|token|fly_session)("?\s*[=:]\s*"?)[^\s",;&}]+"#,
            format!("${{1}}${{2}}{MASK}"),
        ),
        rule(r"([?&#]t=)[0-9a-fA-F]+", format!("${{1}}{MASK}")),
        rule(r"\b[0-9a-fA-F]{32,}\b", MASK.to_string()),
        rule(r"[A-Za-z0-9+/_-]{200,}={0,2}", MASK.to_string()),
    ]
});

/// I hide credentials in `text`, leaving the rest of the line alone.
pub fn redact(text: &str) -> Cow<'_, str> {
    let mut out = Cow::Borrowed(text);
    for (pattern, replacement) in SECRETS.iter() {
        if let Cow::Owned(changed) = pattern.replace_all(&out, replacement.as_str()) {
            out = Cow::Owned(changed);
        }
    }
    out
}

/// The console side of the same rule: I mask a formatted line before it reaches stdout, so it
/// also stays out of journald and any file the output is sent to.
#[derive(Clone, Copy, Default)]
pub struct RedactingStdout;

impl<'a> MakeWriter<'a> for RedactingStdout {
    type Writer = RedactingStdout;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingStdout
    }
}

impl std::io::Write for RedactingStdout {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        std::io::stdout().write_all(redact(&text).as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().flush()
    }
}

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
            message: redact(&message).into_owned(),
        };
        inner.records.push_back(record);
        while inner.records.len() > CAPACITY {
            inner.records.pop_front();
        }
    }

    /// I return records newer than `after_id`, filtered by minimum level, at most `limit` newest ones.
    pub fn since(&self, after_id: u64, min_level: Option<Level>, limit: usize) -> Vec<LogRecord> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        let mut records = inner
            .records
            .iter()
            .filter(|record| record.id > after_id)
            .filter(|record| match (&min_level, record.level.parse::<Level>()) {
                // tracing orders levels by verbosity (ERROR < WARN < INFO), so I compare that way.
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
        // I skip dependency chatter at DEBUG and TRACE, which would evict useful records.
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
    fn secrets_are_hidden_and_other_text_is_kept() {
        for (line, secret) in [
            ("GET https://api.telegram.org/bot123456789:AAFabcdefghijklmnopqrstuvwxyz012345/getMe", "AAFabc"),
            ("proxy socks5://user:hunter2@10.0.0.1:1080 failed", "hunter2"),
            ("api_hash=0123456789abcdef0123456789abcdef", "0123456789abcdef"),
            (r#"body {"password":"hunter2","x":1}"#, "hunter2"),
            ("token: abc.def", "abc.def"),
            ("open http://127.0.0.1:8080/auth#t=deadbeef", "deadbeef"),
            ("cookie fly_session=abcabc; Path=/", "abcabc"),
            ("session 0123456789abcdef0123456789abcdef0123456789abcdef", "0123456789abcdef"),
        ] {
            let masked = redact(line);
            assert!(!masked.contains(secret), "{masked}");
            assert!(masked.contains(MASK), "{masked}");
        }
        let long = format!("1{}", "A".repeat(350));
        assert_eq!(redact(&long), MASK);

        for line in [
            "module 'ai' handler 'ask': boom",
            "userbot account 'Alex' connected, starting update loop",
            "session of Alex was lost: 401",
            "2FA password required",
        ] {
            assert_eq!(redact(line), line);
        }
    }

    #[test]
    fn the_ring_buffer_never_stores_a_secret() {
        let buffer = LogBuffer::default();
        buffer.push(
            &Level::ERROR,
            "fly_telegram",
            "bot 123456789:AAFabcdefghijklmnopqrstuvwxyz012345 failed".into(),
        );
        assert!(!buffer.since(0, None, 1)[0].message.contains("AAFabc"));
    }

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
