//! Scheduled handlers: `M.every = { ["10m"] = "tick" }` and `M.at = { ["09:00"] = "morning" }`.
//!
//! A handler is called with `ctx` once per connected account, outside any chat. `every` waits a
//! full interval after the start before its first run. `at` runs once a day at that local time;
//! a run missed while the userbot was off is not made up. The days of `at` runs are stored, so a
//! restart within the same minute does not run them twice.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mlua::{Function, Table};
use tokio::sync::{Mutex, OnceCell};
use tracing::warn;

use super::{budget, clean_lua_error, context::Ctx, Account, Loader};

const TICK: Duration = Duration::from_secs(20);
/// The shortest interval of `every`.
const MIN_INTERVAL: Duration = Duration::from_secs(60);
/// How late an `at` run may start after its minute, for a tick that came late.
const AT_GRACE_MINUTES: u32 = 3;
/// Days of the last `at` runs, by `module:handler:session`.
const AT_RUNS_KEY: &str = "core.schedule_at_runs";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum When {
    Every(Duration),
    /// Minutes after local midnight.
    At(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub when: When,
    pub handler: String,
}

pub fn parse_interval(text: &str) -> Option<Duration> {
    let text = text.trim();
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().ok()?;
    let seconds = match unit.trim() {
        "s" => number,
        "m" => number * 60,
        "h" => number * 3600,
        "d" => number * 86_400,
        _ => return None,
    };
    Some(Duration::from_secs(seconds))
}

pub fn parse_time(text: &str) -> Option<u32> {
    let (hours, minutes) = text.trim().split_once(':')?;
    let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

/// Reads `M.every` and `M.at`. Entries that do not parse are skipped with a warning.
pub fn read(module: &str, table: &Table) -> Vec<Schedule> {
    let mut schedules = Vec::new();
    if let Ok(every) = table.get::<Table>("every") {
        for (key, handler) in every.pairs::<String, String>().flatten() {
            match parse_interval(&key) {
                Some(interval) if interval >= MIN_INTERVAL => schedules.push(Schedule {
                    when: When::Every(interval),
                    handler,
                }),
                _ => warn!("module '{module}': every['{key}'] needs an interval of at least 1m, such as 10m or 2h"),
            }
        }
    }
    if let Ok(at) = table.get::<Table>("at") {
        for (key, handler) in at.pairs::<String, String>().flatten() {
            match parse_time(&key) {
                Some(minutes) => schedules.push(Schedule {
                    when: When::At(minutes),
                    handler,
                }),
                None => warn!("module '{module}': at['{key}'] needs a time such as 09:30"),
            }
        }
    }
    schedules.sort_by(|a, b| a.handler.cmp(&b.handler));
    schedules
}

/// Whether an `at` run is due at `now_minutes` past midnight, given the day of the last run.
fn at_due(at: u32, now_minutes: u32, today: &str, last_day: Option<&str>) -> bool {
    now_minutes >= at && now_minutes < at + AT_GRACE_MINUTES && last_day != Some(today)
}

pub fn spawn(loader: Arc<Loader>) {
    tokio::spawn(async move {
        let started = Instant::now();
        let last_every: Mutex<HashMap<String, Instant>> = Mutex::new(HashMap::new());
        let mut interval = tokio::time::interval(TICK);
        loop {
            interval.tick().await;
            tick(&loader, started, &last_every).await;
        }
    });
}

async fn tick(
    loader: &Arc<Loader>,
    started: Instant,
    last_every: &Mutex<HashMap<String, Instant>>,
) {
    let db = &loader.services.db;
    let disabled = crate::core_settings::disabled_modules(db).await;
    let due_modules = loader.scheduled_modules(&disabled).await;
    if due_modules.is_empty() {
        return;
    }
    let accounts = loader.services.accounts.list().await;
    if accounts.is_empty() {
        return;
    }
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let now_minutes = now.format("%H").to_string().parse::<u32>().unwrap_or(0) * 60
        + now.format("%M").to_string().parse::<u32>().unwrap_or(0);
    let mut at_runs = db
        .get(AT_RUNS_KEY)
        .await
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut at_changed = false;

    for (manifest, table, schedules) in due_modules {
        for schedule in schedules {
            for handle in &accounts {
                let key = format!(
                    "{}:{}:{}",
                    manifest.name, schedule.handler, handle.session_file
                );
                let due = match schedule.when {
                    When::Every(every) => {
                        let mut last = last_every.lock().await;
                        let previous = *last.entry(key.clone()).or_insert(started);
                        if previous.elapsed() >= every {
                            last.insert(key.clone(), Instant::now());
                            true
                        } else {
                            false
                        }
                    }
                    When::At(at) => {
                        let last_day = at_runs.get(&key).and_then(|v| v.as_str());
                        if at_due(at, now_minutes, &today, last_day) {
                            at_runs.insert(key.clone(), serde_json::Value::String(today.clone()));
                            at_changed = true;
                            true
                        } else {
                            false
                        }
                    }
                };
                if !due {
                    continue;
                }
                let Ok(function) = table.get::<Function>(schedule.handler.as_str()) else {
                    warn!(
                        "module '{}': scheduled handler '{}' is not a function",
                        manifest.name, schedule.handler
                    );
                    continue;
                };
                let account = Account {
                    client: handle.client.clone(),
                    session_file: handle.session_file.clone(),
                    own_user_id: Arc::new(OnceCell::new()),
                };
                let ctx = Ctx::new(Arc::clone(loader), &account, &manifest);
                let loader = Arc::clone(loader);
                let manifest = manifest.clone();
                let handler = schedule.handler.clone();
                tokio::spawn(async move {
                    let call = function.call_async::<()>(ctx);
                    let started = Instant::now();
                    let (result, cpu) =
                        budget::measured(budget::limit_for(manifest.trusted), call).await;
                    let error = result.err().map(|e| clean_lua_error(&e));
                    loader
                        .after_call(&manifest.name, cpu, started.elapsed(), error.as_deref())
                        .await;
                    if let Some(message) = error {
                        loader.services.runtime.record_error();
                        warn!("module '{}' scheduled {handler}: {message}", manifest.name);
                    }
                });
            }
        }
    }
    if at_changed {
        if let Err(error) = db
            .set(AT_RUNS_KEY, serde_json::Value::Object(at_runs))
            .await
        {
            warn!("could not store scheduled runs: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_and_times_parse() {
        assert_eq!(parse_interval("10m"), Some(Duration::from_secs(600)));
        assert_eq!(parse_interval("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_interval("1d"), Some(Duration::from_secs(86_400)));
        assert_eq!(parse_interval("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_interval("m"), None);
        assert_eq!(parse_interval("5x"), None);
        assert_eq!(parse_time("09:30"), Some(570));
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("7:5"), Some(425));
    }

    #[test]
    fn at_runs_once_a_day_and_does_not_make_up_missed_runs() {
        assert!(at_due(540, 540, "2026-11-26", None));
        assert!(at_due(540, 542, "2026-11-26", Some("2026-11-25")));
        assert!(!at_due(540, 542, "2026-11-26", Some("2026-11-26")));
        assert!(
            !at_due(540, 600, "2026-11-26", None),
            "an hour late is a missed run"
        );
        assert!(!at_due(540, 539, "2026-11-26", None));
    }

    #[test]
    fn module_tables_become_schedules() {
        let lua = mlua::Lua::new();
        let table: Table = lua
            .load(r#"return { every = { ["10m"] = "tick", ["10s"] = "fast" }, at = { ["09:00"] = "morning", ["99:00"] = "bad" } }"#)
            .eval()
            .unwrap();
        let schedules = read("m", &table);
        assert_eq!(
            schedules,
            vec![
                Schedule {
                    when: When::At(540),
                    handler: "morning".into()
                },
                Schedule {
                    when: When::Every(Duration::from_secs(600)),
                    handler: "tick".into()
                },
            ]
        );
    }
}
