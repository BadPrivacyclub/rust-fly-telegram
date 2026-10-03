//! Process-level configuration: `config.toml`, environment variables, and CLI flags.
//!
//! Precedence (highest first): CLI flags, environment variables, `config.toml`, defaults.
//! Runtime state that users change at runtime (prefixes, proxy, bot token) lives in the
//! database instead, so it can be edited from Telegram, the bot, or the web panel.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = "config.toml";

pub const DEFAULT_WEB_HOST: &str = "127.0.0.1";

pub const DEFAULT_WEB_PORT: u16 = 8080;

pub mod env_key {
    pub const DATA_DIR: &str = "FLY_DATA_DIR";
    pub const WEB_HOST: &str = "FLY_WEB_HOST";
    pub const WEB_PORT: &str = "FLY_WEB_PORT";
    pub const LOG: &str = "FLY_LOG";
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub web: WebConfig,
    pub log: LogConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct WebConfig {
    /// Starts the panel after the account connects.
    pub enabled: bool,
    /// Bind address. The panel only accepts requests addressed to a loopback host name,
    /// so binding to `0.0.0.0` is only useful inside a container with a loopback port mapping.
    pub host: String,
    pub port: u16,
    /// Opens the setup wizard in the default browser on first run.
    pub open_browser: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct LogConfig {
    /// `tracing` filter directive, e.g. `info` or `fly_telegram=debug`.
    pub level: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            host: DEFAULT_WEB_HOST.to_string(),
            port: DEFAULT_WEB_PORT,
            open_browser: true,
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
        }
    }
}

impl WebConfig {
    pub fn bind_addr(&self) -> String {
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// The URL printed for the operator. Wildcard binds are shown as loopback.
    pub fn public_url(&self) -> String {
        let host = match self.host.as_str() {
            "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
            other => other,
        };
        format!("http://{host}:{}", self.port)
    }
}

/// Parsed command line. Unknown flags are rejected so typos do not silently change behavior.
#[derive(Clone, Debug, Default)]
pub struct Cli {
    pub command: CliCommand,
    pub no_web_auth: bool,
    pub no_panel: bool,
    pub no_browser: bool,
    pub data_dir: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub host: Option<String>,
    pub port: Option<u16>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CliCommand {
    #[default]
    Run,
    Help,
    Version,
    Keygen,
    Sign(PathBuf),
    Doctor,
    InitConfig,
    Service(ServiceAction),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceAction {
    Install,
    Uninstall,
    Print,
}

impl Cli {
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self> {
        let mut cli = Cli::default();
        let mut args = args.into_iter().skip(1).peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" | "help" => cli.command = CliCommand::Help,
                "-V" | "--version" | "version" => cli.command = CliCommand::Version,
                "--keygen" | "keygen" => cli.command = CliCommand::Keygen,
                "--sign" | "sign" => {
                    let path = args.next().context("--sign requires a module path")?;
                    cli.command = CliCommand::Sign(PathBuf::from(path));
                }
                "doctor" | "--doctor" => cli.command = CliCommand::Doctor,
                "init" | "--init-config" => cli.command = CliCommand::InitConfig,
                "service" => {
                    let action = match args.next().as_deref() {
                        Some("install") => ServiceAction::Install,
                        Some("uninstall") | Some("remove") => ServiceAction::Uninstall,
                        Some("print") | None => ServiceAction::Print,
                        Some(other) => anyhow::bail!("unknown service action '{other}'"),
                    };
                    cli.command = CliCommand::Service(action);
                }
                "--no-web" => cli.no_web_auth = true,
                "--no-panel" => cli.no_panel = true,
                "--no-browser" => cli.no_browser = true,
                "--data-dir" => {
                    cli.data_dir = Some(PathBuf::from(
                        args.next().context("--data-dir requires a path")?,
                    ));
                }
                "--config" => {
                    cli.config = Some(PathBuf::from(
                        args.next().context("--config requires a path")?,
                    ));
                }
                "--host" => cli.host = Some(args.next().context("--host requires a value")?),
                "--port" => {
                    let raw = args.next().context("--port requires a value")?;
                    cli.port = Some(raw.parse().context("--port must be a number")?);
                }
                other => {
                    if let Some(value) = other.strip_prefix("--data-dir=") {
                        cli.data_dir = Some(PathBuf::from(value));
                    } else if let Some(value) = other.strip_prefix("--port=") {
                        cli.port = Some(value.parse().context("--port must be a number")?);
                    } else if let Some(value) = other.strip_prefix("--host=") {
                        cli.host = Some(value.to_string());
                    } else if let Some(value) = other.strip_prefix("--config=") {
                        cli.config = Some(PathBuf::from(value));
                    } else {
                        anyhow::bail!("unknown argument '{other}', see --help");
                    }
                }
            }
        }
        Ok(cli)
    }
}

pub const HELP_TEXT: &str = "\
fly-telegram: Telegram userbot with Lua modules

USAGE:
    fly-telegram [OPTIONS] [COMMAND]

COMMANDS:
    (none)              Run the userbot
    doctor              Check the environment and configuration
    init                Write a default config.toml into the data directory
    service install     Install and enable a background service (systemd/launchd/Task Scheduler)
    service uninstall   Remove the background service
    service print       Print the service definition without installing it
    keygen              Generate an Ed25519 module signing key pair
    sign <module.lua>   Sign a module manifest with the operator key
    version             Print the version
    help                Print this help

OPTIONS:
    --data-dir <path>   Directory for database, sessions, modules, and data (env FLY_DATA_DIR)
    --config <path>     Path to config.toml (default: <data-dir>/config.toml)
    --host <addr>       Web panel bind address (env FLY_WEB_HOST, default 127.0.0.1)
    --port <port>       Web panel port (env FLY_WEB_PORT, default 8080)
    --no-web            Authorize in the terminal instead of the browser setup wizard
    --no-panel          Do not start the web panel after connecting
    --no-browser        Do not open the browser for first-run setup

Started without --data-dir, fly-telegram keeps its data next to the executable
(portable mode) or in ~/fly-telegram when the executable sits in a system folder.
";

/// Resolves the data directory from CLI or environment and makes it the working directory.
/// All runtime paths are relative, so this keeps every file in one place.
pub fn enter_data_dir(cli: &Cli) -> Result<Option<PathBuf>> {
    let dir = cli
        .data_dir
        .clone()
        .or_else(|| std::env::var_os(env_key::DATA_DIR).map(PathBuf::from))
        .filter(|path| !path.as_os_str().is_empty());
    let dir = match dir {
        Some(dir) => dir,
        None => match default_data_dir() {
            Some(dir) => dir,
            None => return Ok(None),
        },
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("creating data dir {dir:?}"))?;
    std::env::set_current_dir(&dir).with_context(|| format!("entering data dir {dir:?}"))?;
    Ok(Some(dir))
}

/// Files that mark a directory as an existing fly-telegram data directory.
const DATA_MARKERS: &[&str] = &[
    "database.json",
    "database.json.enc",
    "fly-telegram.session",
    "fly-telegram.session.enc",
    "config.toml",
    "modules",
];

fn has_data(dir: &Path) -> bool {
    DATA_MARKERS.iter().any(|marker| dir.join(marker).exists())
}

/// Picks a data directory when none is given, so a downloaded binary works when started
/// with a double-click:
/// 1. the current directory if it already holds a fly-telegram installation (source checkouts);
/// 2. the executable's directory if it holds one, or if it is a writable, non-system folder
///    (portable use: unpack the archive anywhere and run);
/// 3. `~/fly-telegram` otherwise (e.g. the binary lives in ~/.local/bin).
fn default_data_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok();
    if let Some(cwd) = &cwd {
        if has_data(cwd) {
            return None;
        }
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    if let Some(exe_dir) = &exe_dir {
        if has_data(exe_dir) || is_portable_dir(exe_dir) {
            return Some(exe_dir.clone());
        }
    }
    home_dir().map(|home| home.join("fly-telegram"))
}

fn is_portable_dir(dir: &Path) -> bool {
    let lower = dir.to_string_lossy().to_lowercase().replace('\\', "/");
    let system = lower.ends_with("/bin")
        || lower.contains("/program files")
        || lower.starts_with("/usr")
        || lower.starts_with("/opt")
        || lower.contains("/target/release")
        || lower.contains("/target/debug");
    if system {
        return false;
    }
    let probe = dir.join(".fly-write-test");
    let writable = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    writable
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

pub fn load(cli: &Cli) -> Result<AppConfig> {
    let path = cli
        .config
        .clone()
        .unwrap_or_else(|| PathBuf::from(CONFIG_FILE));
    let mut config = read_file(&path)?;
    apply_env(&mut config);
    if let Some(host) = &cli.host {
        config.web.host = host.clone();
    }
    if let Some(port) = cli.port {
        config.web.port = port;
    }
    if cli.no_panel {
        config.web.enabled = false;
    }
    if cli.no_browser {
        config.web.open_browser = false;
    }
    Ok(config)
}

fn read_file(path: &Path) -> Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let raw = std::fs::read_to_string(path).with_context(|| format!("reading {path:?}"))?;
    toml::from_str(&raw).with_context(|| format!("parsing {path:?}"))
}

fn apply_env(config: &mut AppConfig) {
    if let Some(host) = non_empty_env(env_key::WEB_HOST) {
        config.web.host = host;
    }
    if let Some(port) = non_empty_env(env_key::WEB_PORT).and_then(|value| value.parse().ok()) {
        config.web.port = port;
    }
    if let Some(level) = non_empty_env(env_key::LOG) {
        config.log.level = level;
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub fn default_config_toml() -> String {
    "\
# fly-telegram configuration.
# Values here can be overridden by environment variables and CLI flags.
# Settings that change at runtime (command prefixes, proxy, bot token, language)
# are stored in the database and edited from the web panel or Telegram.

[web]
# Start the local web panel after the account connects.
enabled = true
# The panel only answers requests addressed to localhost / 127.0.0.1 / [::1].
host = \"127.0.0.1\"
port = 8080
# Open the setup wizard in your browser on first run.
open_browser = true

[log]
# tracing filter, e.g. \"info\", \"debug\", or \"fly_telegram=debug,info\"
level = \"info\"
"
    .to_string()
}

pub fn write_default_config(path: &Path) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    std::fs::write(path, default_config_toml()).with_context(|| format!("writing {path:?}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        std::iter::once("fly-telegram")
            .chain(values.iter().copied())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn parses_flags_and_commands() {
        let cli = Cli::parse(args(&["--port", "9000", "--no-web", "doctor"])).unwrap();
        assert_eq!(cli.port, Some(9000));
        assert!(cli.no_web_auth);
        assert_eq!(cli.command, CliCommand::Doctor);

        let cli = Cli::parse(args(&["--sign", "modules/core.lua"])).unwrap();
        assert_eq!(
            cli.command,
            CliCommand::Sign(PathBuf::from("modules/core.lua"))
        );

        let cli = Cli::parse(args(&["service", "install", "--data-dir=/tmp/x"])).unwrap();
        assert_eq!(cli.command, CliCommand::Service(ServiceAction::Install));
        assert_eq!(cli.data_dir, Some(PathBuf::from("/tmp/x")));
    }

    #[test]
    fn portable_dir_rules() {
        assert!(!is_portable_dir(Path::new("/usr/local/bin")));
        assert!(!is_portable_dir(Path::new("/home/me/.local/bin")));
        assert!(!is_portable_dir(Path::new("C:\\Program Files\\fly")));
        let dir = std::env::temp_dir().join(format!("fly_portable_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(is_portable_dir(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_unknown_flags() {
        assert!(Cli::parse(args(&["--bogus"])).is_err());
    }

    #[test]
    fn default_config_round_trips() {
        let parsed: AppConfig = toml::from_str(&default_config_toml()).unwrap();
        assert_eq!(parsed.web.port, DEFAULT_WEB_PORT);
        assert_eq!(parsed.web.host, DEFAULT_WEB_HOST);
        assert!(parsed.web.enabled);
    }

    #[test]
    fn bind_addr_brackets_ipv6() {
        let web = WebConfig {
            host: "::1".into(),
            ..WebConfig::default()
        };
        assert_eq!(web.bind_addr(), "[::1]:8080");
    }
}
