use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::{Mutex, RwLock};

const TELEGRAM_SEND_INTERVAL: Duration = Duration::from_millis(850);

struct CpuSample {
    sampled_at: Instant,
    process_time: Duration,
}

/// Runtime information for one connected Telegram account.
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

/// Runtime metrics shown in the local dashboard.
pub struct RuntimeState {
    started_at: Instant,
    connected: AtomicBool,
    updates_seen: AtomicU64,
    commands_seen: AtomicU64,
    cpu_sample: Mutex<Option<CpuSample>>,
    telegram_send_at: Mutex<Instant>,
    account_name: RwLock<Option<String>>,
    accounts: RwLock<HashMap<String, AccountRuntimeEntry>>,
}

impl RuntimeState {
    /// Creates an empty runtime metrics state.
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
        }
    }
}

impl RuntimeState {
    /// Marks the userbot connection as active.
    pub async fn set_connected(&self, account_name: Option<String>) {
        self.connected.store(true, Ordering::Relaxed);
        *self.account_name.write().await = account_name;
    }

    /// Marks one Telegram account as connected.
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

    /// Records one incoming update.
    pub fn record_update(&self) {
        self.updates_seen.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one incoming update for a specific account.
    pub async fn record_account_update(&self, session_file: &str) {
        self.record_update();
        if let Some(account) = self.accounts.write().await.get_mut(session_file) {
            account.updates_seen = account.updates_seen.saturating_add(1);
        }
    }

    /// Records one command dispatch attempt.
    pub fn record_command(&self) {
        self.commands_seen.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one command dispatch attempt for a specific account.
    pub async fn record_account_command(&self, session_file: &str) {
        self.record_command();
        if let Some(account) = self.accounts.write().await.get_mut(session_file) {
            account.commands_seen = account.commands_seen.saturating_add(1);
        }
    }

    /// Returns uptime in whole seconds.
    pub fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    /// Returns whether the userbot is connected.
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Returns the number of updates seen by the userbot.
    pub fn updates_seen(&self) -> u64 {
        self.updates_seen.load(Ordering::Relaxed)
    }

    /// Returns the number of command messages seen by the dispatcher.
    pub fn commands_seen(&self) -> u64 {
        self.commands_seen.load(Ordering::Relaxed)
    }

    /// Returns the connected account display name, if known.
    pub async fn account_name(&self) -> Option<String> {
        self.account_name.read().await.clone()
    }

    /// Returns all known account runtime entries.
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

    /// Returns current process CPU usage percent when the platform supports it.
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

    /// Serializes outgoing Telegram calls to reduce flood-wait risk.
    pub async fn wait_for_telegram_send(&self) {
        let mut next_send_at = self.telegram_send_at.lock().await;
        let now = Instant::now();
        if *next_send_at > now {
            tokio::time::sleep_until((*next_send_at).into()).await;
        }
        *next_send_at = Instant::now() + TELEGRAM_SEND_INTERVAL;
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
