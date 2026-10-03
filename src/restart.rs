//! In-place process restart, so `.restart` and `.update` work without an external supervisor.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use grammers_client::Client;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::core_settings::key;
use crate::database::Database;
use crate::i18n::{self, Lang};
use crate::runtime::RuntimeState;
use crate::telegram::{self, StoredPeer};

static LAUNCH_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Must run before the data directory changes the working directory, so relative
/// command-line paths resolve the same way after a restart.
pub fn remember_launch_dir() {
    if let Ok(dir) = std::env::current_dir() {
        let _ = LAUNCH_DIR.set(dir);
    }
}

/// The message to edit with "Restarted in N s" once the account reconnects.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestartNotice {
    pub session_file: String,
    pub peer: StoredPeer,
    pub message_id: i32,
    pub requested_at_ms: u64,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub async fn save_notice(db: &Database, notice: &RestartNotice) {
    if let Ok(value) = serde_json::to_value(notice) {
        let _ = db.set(key::RESTART_NOTICE, value).await;
    }
}

/// Replaces the current process with a fresh copy of the (possibly updated) executable.
pub fn restart_now() -> ! {
    info!("restarting process");
    if let Some(dir) = LAUNCH_DIR.get() {
        let _ = std::env::set_current_dir(dir);
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            warn!("cannot locate executable for restart ({error}); exiting for the supervisor");
            std::process::exit(0);
        }
    };
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // On Linux a replaced binary shows up as "<path> (deleted)"; strip that suffix.
        let exe = exe
            .to_str()
            .and_then(|path| path.strip_suffix(" (deleted)"))
            .map(PathBuf::from)
            .unwrap_or(exe);
        let error = std::process::Command::new(&exe).args(&args).exec();
        warn!("exec failed ({error}); exiting for the supervisor");
        std::process::exit(0);
    }

    #[cfg(not(unix))]
    {
        match std::process::Command::new(&exe).args(&args).spawn() {
            Ok(_) => std::process::exit(0),
            Err(error) => {
                warn!("spawn failed ({error}); exiting for the supervisor");
                std::process::exit(0);
            }
        }
    }
}

/// Edits the message that requested a restart once its account is back online.
pub async fn complete_notice(
    db: &Database,
    runtime: &RuntimeState,
    client: &Client,
    session_file: &str,
    lang: Lang,
) {
    let value = db.get(key::RESTART_NOTICE).await;
    let Ok(notice) = serde_json::from_value::<RestartNotice>(value) else {
        return;
    };
    if notice.session_file != session_file {
        return;
    }
    let _ = db.remove(key::RESTART_NOTICE).await;
    let Some(peer) = notice.peer.to_ref() else {
        return;
    };
    let elapsed = now_ms().saturating_sub(notice.requested_at_ms) as f64 / 1000.0;
    let text = format!(
        "✅ **{}** `{elapsed:.1} s`  \n`fly-telegram {}`",
        i18n::tr(lang, "restart.done"),
        crate::VERSION
    );
    runtime.wait_for_telegram_send().await;
    if let Err(error) = client
        .edit_message(
            peer,
            notice.message_id,
            telegram::formatted_message_input(&text),
        )
        .await
    {
        warn!("could not edit restart message: {error}");
    }
}
