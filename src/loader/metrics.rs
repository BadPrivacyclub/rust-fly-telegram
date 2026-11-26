//! Per-module numbers for the panel: calls, errors, time on the CPU, call duration, and stops
//! by the execution budget, kept for the last hour in minute buckets. Also decides when a
//! module misbehaves often enough to be switched off.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

const MINUTES: u64 = 60;
/// Call durations kept for the 95th percentile.
const DURATIONS: usize = 200;
/// Stops by the budget or the memory limit within [`STRIKE_WINDOW`] that switch a module off.
pub const STRIKES: usize = 5;
pub const STRIKE_WINDOW: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Bucket {
    pub minute: u64,
    pub calls: u64,
    pub errors: u64,
    pub cpu_ms: u64,
}

#[derive(Default)]
struct Stats {
    calls: u64,
    errors: u64,
    stops: u64,
    cpu: Duration,
    buckets: VecDeque<Bucket>,
    durations: VecDeque<Duration>,
    strikes: VecDeque<Instant>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ModuleMetrics {
    pub calls: u64,
    pub errors: u64,
    /// Stops by the execution budget or the memory limit.
    pub stops: u64,
    pub cpu_ms: u64,
    /// Average CPU time per minute over the last 10 minutes.
    pub cpu_ms_per_minute: u64,
    /// 95th percentile of the call duration, waits included.
    pub p95_ms: u64,
    /// Memory of the module's own Lua state; `None` for trusted modules, which share one.
    pub memory_bytes: Option<usize>,
    /// The last hour, oldest first, one bucket per minute with activity.
    pub minutes: Vec<Bucket>,
}

/// How a call ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Error,
    /// Stopped by the execution budget or the memory limit.
    Stopped,
}

impl Outcome {
    pub fn of(error: Option<&str>) -> Self {
        match error {
            None => Self::Ok,
            Some(message)
                if message.contains("ran too long without a pause")
                    || message.contains("not enough memory") =>
            {
                Self::Stopped
            }
            Some(_) => Self::Error,
        }
    }
}

#[derive(Default)]
pub struct Metrics {
    modules: Mutex<HashMap<String, Stats>>,
}

fn minute_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() / 60)
}

impl Metrics {
    /// Records one call. Returns true when the module reached [`STRIKES`] stops within
    /// [`STRIKE_WINDOW`] and should be switched off; the strikes then start over.
    pub fn record(&self, module: &str, cpu: Duration, wall: Duration, outcome: Outcome) -> bool {
        self.record_at(module, cpu, wall, outcome, minute_now(), Instant::now())
    }

    fn record_at(
        &self,
        module: &str,
        cpu: Duration,
        wall: Duration,
        outcome: Outcome,
        minute: u64,
        now: Instant,
    ) -> bool {
        let mut modules = self.modules.lock().unwrap_or_else(|e| e.into_inner());
        let stats = modules.entry(module.to_string()).or_default();
        stats.calls += 1;
        stats.cpu += cpu;
        if outcome != Outcome::Ok {
            stats.errors += 1;
        }
        if stats.buckets.back().is_none_or(|b| b.minute != minute) {
            stats.buckets.push_back(Bucket {
                minute,
                ..Bucket::default()
            });
        }
        while stats
            .buckets
            .front()
            .is_some_and(|b| b.minute + MINUTES <= minute)
        {
            stats.buckets.pop_front();
        }
        if let Some(bucket) = stats.buckets.back_mut() {
            bucket.calls += 1;
            bucket.cpu_ms += cpu.as_millis() as u64;
            if outcome != Outcome::Ok {
                bucket.errors += 1;
            }
        }
        stats.durations.push_back(wall);
        while stats.durations.len() > DURATIONS {
            stats.durations.pop_front();
        }
        if outcome != Outcome::Stopped {
            return false;
        }
        stats.stops += 1;
        stats.strikes.push_back(now);
        while stats
            .strikes
            .front()
            .is_some_and(|at| now.duration_since(*at) > STRIKE_WINDOW)
        {
            stats.strikes.pop_front();
        }
        if stats.strikes.len() >= STRIKES {
            stats.strikes.clear();
            return true;
        }
        false
    }

    pub fn snapshot(&self, module: &str, memory_bytes: Option<usize>) -> ModuleMetrics {
        self.snapshot_at(module, memory_bytes, minute_now())
    }

    fn snapshot_at(&self, module: &str, memory_bytes: Option<usize>, minute: u64) -> ModuleMetrics {
        let modules = self.modules.lock().unwrap_or_else(|e| e.into_inner());
        let Some(stats) = modules.get(module) else {
            return ModuleMetrics {
                memory_bytes,
                ..ModuleMetrics::default()
            };
        };
        let minutes = stats
            .buckets
            .iter()
            .filter(|b| b.minute + MINUTES > minute)
            .copied()
            .collect::<Vec<_>>();
        let recent_cpu: u64 = minutes
            .iter()
            .filter(|b| b.minute + 10 > minute)
            .map(|b| b.cpu_ms)
            .sum();
        let mut durations = stats.durations.iter().copied().collect::<Vec<_>>();
        durations.sort_unstable();
        let p95 = durations
            .get((durations.len() * 95).div_ceil(100).saturating_sub(1))
            .copied()
            .unwrap_or_default();
        ModuleMetrics {
            calls: stats.calls,
            errors: stats.errors,
            stops: stats.stops,
            cpu_ms: stats.cpu.as_millis() as u64,
            cpu_ms_per_minute: recent_cpu / 10,
            p95_ms: p95.as_millis() as u64,
            memory_bytes,
            minutes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_are_counted_in_minute_buckets() {
        let metrics = Metrics::default();
        let now = Instant::now();
        let ms = Duration::from_millis;
        metrics.record_at("m", ms(30), ms(40), Outcome::Ok, 100, now);
        metrics.record_at("m", ms(10), ms(500), Outcome::Error, 100, now);
        metrics.record_at("m", ms(5), ms(5), Outcome::Ok, 101, now);
        metrics.record_at("m", ms(1), ms(1), Outcome::Ok, 30, now);
        let snapshot = metrics.snapshot_at("m", Some(1024), 101);
        assert_eq!(snapshot.calls, 4);
        assert_eq!(snapshot.errors, 1);
        assert_eq!(snapshot.cpu_ms, 46);
        assert_eq!(snapshot.p95_ms, 500);
        assert_eq!(snapshot.memory_bytes, Some(1024));
        assert_eq!(snapshot.minutes.len(), 2, "minute 30 is older than an hour");
        assert_eq!(snapshot.minutes[0].calls, 2);
        assert_eq!(snapshot.cpu_ms_per_minute, 4);
        assert_eq!(metrics.snapshot_at("other", None, 101).calls, 0);
    }

    #[test]
    fn repeated_stops_switch_a_module_off() {
        let metrics = Metrics::default();
        let start = Instant::now();
        let zero = Duration::ZERO;
        for i in 0..(STRIKES as u64 - 1) {
            assert!(!metrics.record_at(
                "m",
                zero,
                zero,
                Outcome::Stopped,
                1,
                start + Duration::from_secs(i)
            ));
        }
        assert!(!metrics.record_at("m", zero, zero, Outcome::Error, 1, start));
        let late = start + STRIKE_WINDOW + Duration::from_secs(60);
        assert!(
            !metrics.record_at("m", zero, zero, Outcome::Stopped, 1, late),
            "old strikes expire"
        );
        let mut tripped = false;
        for i in 0..STRIKES as u64 {
            tripped |= metrics.record_at(
                "m",
                zero,
                zero,
                Outcome::Stopped,
                1,
                late + Duration::from_secs(i),
            );
        }
        assert!(tripped);
        assert_eq!(
            Outcome::of(Some(
                "the module ran too long without a pause and was stopped"
            )),
            Outcome::Stopped
        );
        assert_eq!(Outcome::of(Some("boom")), Outcome::Error);
        assert_eq!(Outcome::of(None), Outcome::Ok);
    }
}
