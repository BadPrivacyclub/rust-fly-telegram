//! `fly-telegram doctor`: environment and configuration self-check.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Result;

use crate::config;
use crate::settings::{AppConfig, CONFIG_FILE};

enum Status {
    Ok,
    Warn,
    Fail,
}

struct Report {
    failures: usize,
    warnings: usize,
}

impl Report {
    fn line(&mut self, status: Status, label: &str, detail: impl AsRef<str>) {
        let (mark, color) = match status {
            Status::Ok => ("✓", "\x1b[32m"),
            Status::Warn => {
                self.warnings += 1;
                ("!", "\x1b[33m")
            }
            Status::Fail => {
                self.failures += 1;
                ("✗", "\x1b[31m")
            }
        };
        println!("  {color}{mark}\x1b[0m {label:<22} {}", detail.as_ref());
    }
}

pub async fn run(app: &AppConfig) -> Result<()> {
    let mut report = Report {
        failures: 0,
        warnings: 0,
    };
    println!(
        "fly-telegram {} · {} {}\n",
        crate::VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    println!("Files");
    let cwd = std::env::current_dir()?;
    let probe = cwd.join(".fly-write-test");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            report.line(Status::Ok, "data directory", cwd.display().to_string());
        }
        Err(e) => report.line(
            Status::Fail,
            "data directory",
            format!("{} is not writable: {e}", cwd.display()),
        ),
    }

    if Path::new(CONFIG_FILE).exists() {
        report.line(Status::Ok, "config.toml", "loaded");
    } else {
        report.line(
            Status::Ok,
            "config.toml",
            "not present, using defaults (create one with `fly-telegram init`)",
        );
    }

    let encrypted = Path::new(config::DATABASE_ENCRYPTED_FILE).exists();
    if encrypted {
        report.line(Status::Ok, "database", "encrypted (database.json.enc)");
    } else if Path::new(config::DATABASE_FILE).exists() {
        report.line(
            Status::Warn,
            "database",
            "stored in plain text; set a master password in the panel to encrypt it",
        );
    } else {
        report.line(
            Status::Ok,
            "database",
            "not created yet; the setup wizard creates it",
        );
    }

    let has_session = Path::new(config::DEFAULT_SESSION_FILE).exists()
        || Path::new(config::DEFAULT_SESSION_ENCRYPTED_FILE).exists()
        || std::fs::read_dir(config::SESSIONS_DIR)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
    if has_session {
        report.line(Status::Ok, "telegram session", "found");
    } else {
        report.line(
            Status::Warn,
            "telegram session",
            format!(
                "none yet; run fly-telegram and open {}",
                app.web.public_url()
            ),
        );
    }

    let module_count = std::fs::read_dir(config::MODULES_DIR)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "lua"))
                .count()
        })
        .unwrap_or(0);
    report.line(
        Status::Ok,
        "modules",
        format!(
            "{module_count} in {}/ (+{} bundled installed on start)",
            config::MODULES_DIR,
            crate::bundled::BUNDLED_FILES
                .iter()
                .filter(|(name, _)| name.ends_with(".lua"))
                .count()
        ),
    );
    if Path::new(config::SIGNING_PUB_KEY_FILE).exists() {
        report.line(Status::Ok, "signing key", "public key present");
    } else {
        report.line(
            Status::Ok,
            "signing key",
            "none (third-party modules run sandboxed; `fly-telegram keygen` to sign your own)",
        );
    }

    println!("\nWeb panel");
    match tokio::net::TcpListener::bind(app.web.bind_addr()).await {
        Ok(listener) => {
            drop(listener);
            report.line(
                Status::Ok,
                "port",
                format!("{} is free", app.web.bind_addr()),
            );
        }
        Err(e) => report.line(
            Status::Warn,
            "port",
            format!(
                "{} unavailable ({e}); is fly-telegram already running? Change with --port",
                app.web.bind_addr()
            ),
        ),
    }

    println!("\nNetwork");
    match tokio::time::timeout(
        Duration::from_secs(6),
        tokio::net::TcpStream::connect("149.154.167.51:443"),
    )
    .await
    {
        Ok(Ok(_)) => report.line(Status::Ok, "telegram MTProto", "DC2 reachable"),
        Ok(Err(e)) => report.line(
            Status::Fail,
            "telegram MTProto",
            format!("cannot connect ({e}); configure a SOCKS5 proxy in the setup wizard"),
        ),
        Err(_) => report.line(
            Status::Fail,
            "telegram MTProto",
            "timed out; configure a SOCKS5 proxy in the setup wizard",
        ),
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()?;
    match client.get("https://api.telegram.org/").send().await {
        Ok(_) => report.line(Status::Ok, "telegram Bot API", "reachable"),
        Err(e) => report.line(
            Status::Warn,
            "telegram Bot API",
            format!("unreachable ({e}); the control bot will not work"),
        ),
    }

    println!("\nOptional tools");
    for (tool, purpose) in [
        ("git", ".update"),
        ("cargo", ".update rebuilds from source"),
        ("yt-dlp", ".ytdl and music"),
        ("ffmpeg", "music worker and media conversion"),
    ] {
        match tool_version(tool).await {
            Some(version) => report.line(Status::Ok, tool, version),
            None => report.line(
                Status::Warn,
                tool,
                format!("not found (needed for {purpose})"),
            ),
        }
    }

    println!(
        "\n{} problem(s), {} warning(s)",
        report.failures, report.warnings
    );
    if report.failures > 0 {
        std::process::exit(1);
    }
    Ok(())
}

async fn tool_version(tool: &str) -> Option<String> {
    let arg = if tool == "ffmpeg" {
        "-version"
    } else {
        "--version"
    };
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(tool)
            .arg(arg)
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(|line| line.chars().take(60).collect())
}
