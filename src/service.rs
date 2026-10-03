//! `fly-telegram service`: run the userbot in the background on login or boot.
//!
//! - Linux: a systemd unit (user unit, or system unit when run as root).
//! - macOS: a launchd agent.
//! - Windows: a Task Scheduler task that starts at logon.
//!
//! The service always runs with the current data directory. Secrets such as
//! `FLY_MASTER_PASSWORD` go into `fly-telegram.env` inside the data directory, because a
//! background service cannot prompt for a password.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::settings::{Cli, ServiceAction};

const SERVICE_NAME: &str = "fly-telegram";
pub const ENV_FILE: &str = "fly-telegram.env";

pub fn run(action: &ServiceAction, _cli: &Cli) -> Result<()> {
    let exe = std::env::current_exe().context("locating the fly-telegram executable")?;
    let data_dir = std::env::current_dir()?;
    let definition = definition(&exe, &data_dir);

    match action {
        ServiceAction::Print => {
            println!("# {}\n{}", definition.path.display(), definition.content);
            Ok(())
        }
        ServiceAction::Install => install(&definition, &data_dir),
        ServiceAction::Uninstall => uninstall(&definition),
    }
}

struct Definition {
    path: PathBuf,
    content: String,
    #[allow(dead_code)]
    system_wide: bool,
}

#[cfg(target_os = "linux")]
fn definition(exe: &Path, data_dir: &Path) -> Definition {
    let system_wide = is_root();
    let path = if system_wide {
        PathBuf::from(format!("/etc/systemd/system/{SERVICE_NAME}.service"))
    } else {
        home_dir().join(format!(".config/systemd/user/{SERVICE_NAME}.service"))
    };
    let wanted_by = if system_wide {
        "multi-user.target"
    } else {
        "default.target"
    };
    let content = format!(
        "[Unit]
Description=fly-telegram userbot
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
WorkingDirectory={dir}
EnvironmentFile=-{dir}/{ENV_FILE}
ExecStart=\"{exe}\" --data-dir \"{dir}\"
Restart=on-failure
RestartSec=5

[Install]
WantedBy={wanted_by}
",
        dir = data_dir.display(),
        exe = exe.display(),
    );
    Definition {
        path,
        content,
        system_wide,
    }
}

#[cfg(target_os = "macos")]
fn definition(exe: &Path, data_dir: &Path) -> Definition {
    let path = home_dir().join(format!("Library/LaunchAgents/com.{SERVICE_NAME}.plist"));
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.{SERVICE_NAME}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>--data-dir</string>
    <string>{dir}</string>
  </array>
  <key>WorkingDirectory</key><string>{dir}</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>StandardOutPath</key><string>{dir}/fly-telegram.log</string>
  <key>StandardErrorPath</key><string>{dir}/fly-telegram.log</string>
</dict>
</plist>
"#,
        exe = xml_escape(&exe.display().to_string()),
        dir = xml_escape(&data_dir.display().to_string()),
    );
    Definition {
        path,
        content,
        system_wide: false,
    }
}

#[cfg(windows)]
fn definition(exe: &Path, data_dir: &Path) -> Definition {
    let content = format!(
        "schtasks /Create /TN {SERVICE_NAME} /SC ONLOGON /RL LIMITED /F /TR \"\\\"{}\\\" --data-dir \\\"{}\\\"\"",
        exe.display(),
        data_dir.display()
    );
    Definition {
        path: PathBuf::from("Task Scheduler"),
        content,
        system_wide: false,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn definition(exe: &Path, data_dir: &Path) -> Definition {
    Definition {
        path: PathBuf::from("(unsupported platform)"),
        content: format!(
            "\"{}\" --data-dir \"{}\"",
            exe.display(),
            data_dir.display()
        ),
        system_wide: false,
    }
}

fn install(definition: &Definition, data_dir: &Path) -> Result<()> {
    ensure_env_file(data_dir)?;
    install_platform(definition)?;
    println!(
        "\nSecrets for the service (FLY_MASTER_PASSWORD, TELOXIDE_TOKEN) go into {}",
        data_dir.join(ENV_FILE).display()
    );
    Ok(())
}

fn ensure_env_file(data_dir: &Path) -> Result<()> {
    let path = data_dir.join(ENV_FILE);
    if path.exists() {
        return Ok(());
    }
    std::fs::write(
        &path,
        "# Environment for the fly-telegram service. Keep this file private.\n\
         # FLY_MASTER_PASSWORD=change-me\n\
         # TELOXIDE_TOKEN=123456:ABC\n",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_platform(definition: &Definition) -> Result<()> {
    if !std::path::Path::new("/run/systemd/system").exists() {
        anyhow::bail!(
            "systemd is not running on this system (containers and some distros use another init).\n\
             Use `fly-telegram service print` to see the command line for your init system, \
             or start fly-telegram from your desktop session."
        );
    }
    if let Some(parent) = definition.path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&definition.path, &definition.content)?;
    println!("Wrote {}", definition.path.display());
    let scope: &[&str] = if definition.system_wide {
        &[]
    } else {
        &["--user"]
    };
    run_command("systemctl", &[scope, &["daemon-reload"]].concat())?;
    run_command(
        "systemctl",
        &[scope, &["enable", "--now", SERVICE_NAME]].concat(),
    )?;
    println!(
        "Service started. Logs: journalctl {} -u {SERVICE_NAME} -f",
        scope.join(" ")
    );
    if !definition.system_wide {
        println!(
            "To keep it running after you log out: sudo loginctl enable-linger {}",
            std::env::var("USER").unwrap_or_else(|_| "$USER".into())
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_platform(definition: &Definition) -> Result<()> {
    if let Some(parent) = definition.path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&definition.path, &definition.content)?;
    println!("Wrote {}", definition.path.display());
    run_command(
        "launchctl",
        &["load", "-w", &definition.path.display().to_string()],
    )?;
    println!("Agent loaded. Logs are written to fly-telegram.log in the data directory.");
    Ok(())
}

#[cfg(windows)]
fn install_platform(_definition: &Definition) -> Result<()> {
    let exe = std::env::current_exe()?;
    let dir = std::env::current_dir()?;
    let task = format!("\"{}\" --data-dir \"{}\"", exe.display(), dir.display());
    run_command(
        "schtasks",
        &[
            "/Create",
            "/TN",
            SERVICE_NAME,
            "/SC",
            "ONLOGON",
            "/RL",
            "LIMITED",
            "/F",
            "/TR",
            &task,
        ],
    )?;
    println!("Scheduled task created; it starts at your next logon.");
    println!("Start it now with: schtasks /Run /TN {SERVICE_NAME}");
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn install_platform(_definition: &Definition) -> Result<()> {
    anyhow::bail!("service installation is not supported on this platform")
}

#[cfg(target_os = "linux")]
fn uninstall(definition: &Definition) -> Result<()> {
    let scope: &[&str] = if definition.system_wide {
        &[]
    } else {
        &["--user"]
    };
    let _ = run_command(
        "systemctl",
        &[scope, &["disable", "--now", SERVICE_NAME]].concat(),
    );
    if definition.path.exists() {
        std::fs::remove_file(&definition.path)?;
        println!("Removed {}", definition.path.display());
    }
    let _ = run_command("systemctl", &[scope, &["daemon-reload"]].concat());
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall(definition: &Definition) -> Result<()> {
    let _ = run_command(
        "launchctl",
        &["unload", "-w", &definition.path.display().to_string()],
    );
    if definition.path.exists() {
        std::fs::remove_file(&definition.path)?;
        println!("Removed {}", definition.path.display());
    }
    Ok(())
}

#[cfg(windows)]
fn uninstall(_definition: &Definition) -> Result<()> {
    run_command("schtasks", &["/Delete", "/TN", SERVICE_NAME, "/F"])
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn uninstall(_definition: &Definition) -> Result<()> {
    anyhow::bail!("service removal is not supported on this platform")
}

#[allow(dead_code)]
fn run_command(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("running {program}"))?;
    if !status.success() {
        anyhow::bail!("{program} {} failed with {status}", args.join(" "));
    }
    Ok(())
}

#[allow(dead_code)]
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(target_os = "linux")]
fn is_root() -> bool {
    std::env::var("USER").is_ok_and(|user| user == "root")
        || std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status
                    .lines()
                    .find(|line| line.starts_with("Uid:"))
                    .and_then(|line| line.split_whitespace().nth(1).map(|uid| uid == "0"))
            })
            .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
