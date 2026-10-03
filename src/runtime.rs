use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::{Mutex, RwLock};

const TELEGRAM_SEND_INTERVAL: Duration = Duration::from_millis(850);
const ACTIVITY_MINUTES: usize = 60;

/// Updates and commands counted per wall-clock minute, for the panel activity chart.
#[derive(Clone, Copy, Serialize, Default)]
pub struct ActivityBucket {
    pub minute: u64,
    pub updates: u64,
    pub commands: u64,
}

#[derive(Clone, Serialize)]
pub struct CommandCount {
    pub command: String,
    pub count: u64,
}

struct CpuSample {
    sampled_at: Instant,
    process_time: Duration,
}

#[derive(Clone, Serialize)]
pub struct AccountRuntime {
    pub session_file: String,
    pub account_id: String,
    pub name: String,
    pub connected: bool,
    pub updates_seen: u64,
    pub commands_seen: u64,
}

struct AccountRuntimeEntry {
    account_id: String,
    name: String,
    connected: bool,
    updates_seen: u64,
    commands_seen: u64,
}

pub struct RuntimeState {
    started_at: Instant,
    connected: AtomicBool,
    updates_seen: AtomicU64,
    commands_seen: AtomicU64,
    cpu_sample: Mutex<Option<CpuSample>>,
    telegram_send_at: Mutex<Instant>,
    account_name: RwLock<Option<String>>,
    accounts: RwLock<HashMap<String, AccountRuntimeEntry>>,
    command_counts: std::sync::Mutex<HashMap<String, u64>>,
    activity: std::sync::Mutex<VecDeque<ActivityBucket>>,
    errors_seen: AtomicU64,
}

impl RuntimeState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            connected: AtomicBool::new(false),
            updates_seen: AtomicU64::new(0),
            commands_seen: AtomicU64::new(0),
            cpu_sample: Mutex::new(None),
            telegram_send_at: Mutex::new(Instant::now()),
            account_name: RwLock::new(None),
            accounts: RwLock::new(HashMap::new()),
            command_counts: std::sync::Mutex::new(HashMap::new()),
            activity: std::sync::Mutex::new(VecDeque::new()),
            errors_seen: AtomicU64::new(0),
        }
    }
}

impl RuntimeState {
    pub async fn set_connected(&self, account_name: Option<String>) {
        self.connected.store(true, Ordering::Relaxed);
        *self.account_name.write().await = account_name;
    }

    pub async fn set_account_connected(
        &self,
        session_file: String,
        account_id: String,
        name: String,
    ) {
        let mut accounts = self.accounts.write().await;
        accounts.insert(
            session_file,
            AccountRuntimeEntry {
                account_id,
                name,
                connected: true,
                updates_seen: 0,
                commands_seen: 0,
            },
        );
        self.connected.store(true, Ordering::Relaxed);
    }

    pub fn record_update(&self) {
        self.updates_seen.fetch_add(1, Ordering::Relaxed);
        self.bump_activity(|bucket| bucket.updates += 1);
    }

    fn bump_activity(&self, apply: impl FnOnce(&mut ActivityBucket)) {
        let minute = unix_minute();
        let Ok(mut activity) = self.activity.lock() else {
            return;
        };
        if activity.back().map(|bucket| bucket.minute) != Some(minute) {
            activity.push_back(ActivityBucket {
                minute,
                ..ActivityBucket::default()
            });
            while activity.len() > ACTIVITY_MINUTES {
                activity.pop_front();
            }
        }
        if let Some(bucket) = activity.back_mut() {
            apply(bucket);
        }
    }

    /// Dense per-minute series for the last hour, oldest first, with empty minutes filled in.
    pub fn activity(&self) -> Vec<ActivityBucket> {
        let now = unix_minute();
        let recorded = self
            .activity
            .lock()
            .map(|activity| activity.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        (0..ACTIVITY_MINUTES as u64)
            .rev()
            .map(|ago| {
                let minute = now.saturating_sub(ago);
                recorded
                    .iter()
                    .find(|bucket| bucket.minute == minute)
                    .copied()
                    .unwrap_or(ActivityBucket {
                        minute,
                        ..ActivityBucket::default()
                    })
            })
            .collect()
    }

    pub fn record_command_name(&self, command: &str) {
        if let Ok(mut counts) = self.command_counts.lock() {
            *counts.entry(command.to_string()).or_insert(0) += 1;
        }
    }

    pub fn top_commands(&self, limit: usize) -> Vec<CommandCount> {
        let mut counts = self
            .command_counts
            .lock()
            .map(|counts| {
                counts
                    .iter()
                    .map(|(command, count)| CommandCount {
                        command: command.clone(),
                        count: *count,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        counts.sort_by(|a, b| b.count.cmp(&a.count).then(a.command.cmp(&b.command)));
        counts.truncate(limit);
        counts
    }

    pub fn record_error(&self) {
        self.errors_seen.fetch_add(1, Ordering::Relaxed);
    }

    pub fn errors_seen(&self) -> u64 {
        self.errors_seen.load(Ordering::Relaxed)
    }

    pub async fn record_account_update(&self, session_file: &str) {
        self.record_update();
        if let Some(account) = self.accounts.write().await.get_mut(session_file) {
            account.updates_seen = account.updates_seen.saturating_add(1);
        }
    }

    pub fn record_command(&self) {
        self.commands_seen.fetch_add(1, Ordering::Relaxed);
        self.bump_activity(|bucket| bucket.commands += 1);
    }

    pub async fn record_account_command(&self, session_file: &str) {
        self.record_command();
        if let Some(account) = self.accounts.write().await.get_mut(session_file) {
            account.commands_seen = account.commands_seen.saturating_add(1);
        }
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn updates_seen(&self) -> u64 {
        self.updates_seen.load(Ordering::Relaxed)
    }

    pub fn commands_seen(&self) -> u64 {
        self.commands_seen.load(Ordering::Relaxed)
    }

    pub async fn account_name(&self) -> Option<String> {
        self.account_name.read().await.clone()
    }

    pub async fn accounts(&self) -> Vec<AccountRuntime> {
        let mut accounts = self
            .accounts
            .read()
            .await
            .iter()
            .map(|(session_file, account)| AccountRuntime {
                session_file: session_file.clone(),
                account_id: account.account_id.clone(),
                name: account.name.clone(),
                connected: account.connected,
                updates_seen: account.updates_seen,
                commands_seen: account.commands_seen,
            })
            .collect::<Vec<_>>();
        accounts.sort_by(|left, right| left.name.cmp(&right.name));
        accounts
    }

    pub async fn process_cpu_percent(&self) -> Option<f64> {
        let process_time = process_cpu_time()?;
        let mut sample = self.cpu_sample.lock().await;
        let Some(previous) = sample.replace(CpuSample {
            sampled_at: Instant::now(),
            process_time,
        }) else {
            return Some(0.0);
        };

        let elapsed = previous.sampled_at.elapsed().as_secs_f64();
        if elapsed <= f64::EPSILON {
            return Some(0.0);
        }

        let cpu_count = std::thread::available_parallelism()
            .map(|count| count.get() as f64)
            .unwrap_or(1.0);
        let cpu_time = process_time.saturating_sub(previous.process_time);
        Some((cpu_time.as_secs_f64() / elapsed / cpu_count) * 100.0)
    }

    pub async fn set_account_disconnected(&self, session_file: &str) {
        let mut accounts = self.accounts.write().await;
        if let Some(account) = accounts.get_mut(session_file) {
            account.connected = false;
        }
        let any_connected = accounts.values().any(|account| account.connected);
        self.connected.store(any_connected, Ordering::Relaxed);
    }

    /// Telegram user IDs of every connected account; the bot treats them as owners.
    pub async fn owner_ids(&self) -> Vec<i64> {
        self.accounts
            .read()
            .await
            .values()
            .filter_map(|account| account.account_id.parse().ok())
            .collect()
    }

    pub async fn wait_for_telegram_send(&self) {
        let mut next_send_at = self.telegram_send_at.lock().await;
        let now = Instant::now();
        if *next_send_at > now {
            tokio::time::sleep_until((*next_send_at).into()).await;
        }
        *next_send_at = Instant::now() + TELEGRAM_SEND_INTERVAL;
    }
}

fn unix_minute() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0)
}

/// Resident memory of this process in bytes, where the platform exposes it cheaply.
pub fn process_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
        let kib = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
        Some(kib * 1024)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(windows)]
fn process_cpu_time() -> Option<Duration> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    if ok == 0 {
        return None;
    }

    let ticks = filetime_to_u64(kernel) + filetime_to_u64(user);
    Some(Duration::from_nanos(ticks.saturating_mul(100)))
}

#[cfg(windows)]
fn filetime_to_u64(time: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64
}

#[cfg(target_os = "linux")]
fn process_cpu_time() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let after_name = stat.rsplit_once(") ")?.1;
    let fields = after_name.split_whitespace().collect::<Vec<_>>();
    let user_ticks = fields.get(11)?.parse::<u64>().ok()?;
    let system_ticks = fields.get(12)?.parse::<u64>().ok()?;
    let ticks = user_ticks.saturating_add(system_ticks);
    Some(Duration::from_secs_f64(ticks as f64 / linux_clock_ticks()))
}

#[cfg(target_os = "linux")]
fn linux_clock_ticks() -> f64 {
    100.0
}

#[cfg(not(any(windows, target_os = "linux")))]
fn process_cpu_time() -> Option<Duration> {
    None
}

#[cfg(test)]
mod tests {
    use super::RuntimeState;

    #[test]
    fn record_update_increments_updates_seen() {
        let runtime = RuntimeState::default();
        assert_eq!(runtime.updates_seen(), 0);

        runtime.record_update();
        runtime.record_update();

        assert_eq!(runtime.updates_seen(), 2);
        let activity = runtime.activity();
        assert_eq!(activity.len(), 60);
        assert_eq!(activity.last().map(|bucket| bucket.updates), Some(2));
    }

    #[test]
    fn top_commands_are_sorted() {
        let runtime = RuntimeState::default();
        for command in ["ping", "help", "ping"] {
            runtime.record_command_name(command);
        }
        let top = runtime.top_commands(5);
        assert_eq!(top[0].command, "ping");
        assert_eq!(top[0].count, 2);
        assert_eq!(top.len(), 2);
    }
}
