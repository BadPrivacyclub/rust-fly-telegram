use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use base64::Engine;
use tokio::sync::RwLock;
use tracing::info;

mod account_tools;
mod anti_delete;
mod app;
mod automations;
mod backup;
mod bot;
mod bundled;
mod client;
mod config;
mod core_settings;
mod crypto;
mod database;
mod desktop;
mod doctor;
mod i18n;
mod jobs;
mod loader;
mod logs;
mod notify;
mod profiles;
mod proxy;
mod restart;
mod runtime;
mod service;
mod session_security;
mod settings;
mod telegram;
mod updater;
mod watcher;
mod web;

use crate::database::Database;
use crate::loader::Loader;
use crate::runtime::RuntimeState;
use crate::settings::{Cli, CliCommand};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("\nerror: {error:#}");
        desktop::pause_before_exit();
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    restart::remember_launch_dir();
    let cli = match Cli::parse(std::env::args()) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("error: {error}\n\n{}", settings::HELP_TEXT);
            std::process::exit(2);
        }
    };

    match &cli.command {
        CliCommand::Help => {
            print!("{}", settings::HELP_TEXT);
            return Ok(());
        }
        CliCommand::Version => {
            println!("fly-telegram {VERSION}");
            return Ok(());
        }
        _ => {}
    }

    settings::enter_data_dir(&cli)?;
    load_env_file(Path::new(service::ENV_FILE));
    let app_config = settings::load(&cli)?;
    let app_config_web_url = app_config.web.public_url();
    let log_buffer = logs::LogBuffer::new();
    init_tracing(&app_config.log.level, std::sync::Arc::clone(&log_buffer))?;

    match &cli.command {
        CliCommand::Keygen => return keygen(),
        CliCommand::Sign(path) => return sign_module(path),
        CliCommand::Doctor => return doctor::run(&app_config).await,
        CliCommand::InitConfig => {
            let path = Path::new(settings::CONFIG_FILE);
            if settings::write_default_config(path)? {
                println!("Wrote {}", path.display());
            } else {
                println!("{} already exists, left unchanged", path.display());
            }
            return Ok(());
        }
        CliCommand::Service(action) => return service::run(action, &cli),
        CliCommand::Run | CliCommand::Help | CliCommand::Version => {}
    }

    info!("fly-telegram {VERSION} starting");
    print_banner(&app_config_web_url);
    if let Ok(cwd) = std::env::current_dir() {
        info!("data directory: {}", cwd.display());
    }

    if let Err(error) = bundled::sync(Path::new(config::MODULES_DIR)) {
        tracing::warn!("could not sync bundled modules: {error}");
    }

    let master_password = read_master_password()?;
    let security_state = Arc::new(RwLock::new(master_password));
    let session_security = session_security::SessionSecurity::new(
        config::DEFAULT_SESSION_FILE,
        Arc::clone(&security_state),
    );
    session_security.prepare().await?;

    let db = Arc::new(
        Database::load_with_state(config::DATABASE_FILE, Arc::clone(&security_state)).await?,
    );
    let runtime = RuntimeState::new();
    let services = Arc::new(app::Services {
        config: app_config,
        notifier: notify::Notifier::new(Arc::clone(&db), Arc::clone(&runtime)),
        db,
        runtime,
        panel_auth: web::auth::PanelAuth::new(),
        logs: log_buffer,
        accounts: account_tools::AccountRegistry::new(),
        jobs: jobs::JobManager::new(),
        automations: automations::Engine::new(),
    });
    automations::spawn(Arc::clone(&services), Arc::clone(&services.automations));

    let loader = Arc::new(Loader::new(Arc::clone(&services), config::MODULES_DIR).await?);
    loader.load_all().await?;

    {
        let loader_w = Arc::clone(&loader);
        tokio::spawn(async move {
            if let Err(e) = watcher::watch(config::MODULES_DIR, loader_w).await {
                tracing::error!("file watcher stopped: {e}");
            }
        });
    }

    // The control bot runs beside the userbot; a missing or bad token only disables it.
    let bot_manager = bot::BotManager::new(Arc::clone(&loader));
    bot_manager.restart().await;

    let run_result = client::run(client::RunOptions {
        services: Arc::clone(&services),
        loader: Arc::clone(&loader),
        bot: Arc::clone(&bot_manager),
        browser_setup: !cli.no_web_auth,
    })
    .await;
    bot_manager.stop().await;
    let seal_result = session_security.seal().await;

    run_result?;
    seal_result?;
    Ok(())
}

fn print_banner(panel_url: &str) {
    let data_dir = std::env::current_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_default();
    println!(
        "\n  \x1b[1m✈ fly-telegram {VERSION}\x1b[0m\n  Data:   {data_dir}\n  Panel:  {panel_url}\n  Stop:   Ctrl+C\n"
    );
}

/// Loads `KEY=VALUE` lines from the data directory env file without overriding the real environment.
fn load_env_file(path: &Path) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches(|ch| ch == '"' || ch == '\'');
        if key.is_empty() || std::env::var_os(key).is_some() {
            continue;
        }
        std::env::set_var(key, value);
    }
}

fn init_tracing(level: &str, buffer: Arc<logs::LogBuffer>) -> Result<()> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::EnvFilter;

    let filter = match std::env::var("RUST_LOG") {
        Ok(value) if !value.trim().is_empty() => EnvFilter::new(value),
        _ => {
            let level = level.trim();
            if level.contains('=') || level.contains(',') {
                EnvFilter::new(level)
            } else {
                // A bare level applies to this crate; dependencies stay at warn to keep logs readable.
                EnvFilter::new(format!("warn,fly_telegram={level}"))
            }
        }
    };
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(logs::BufferLayer::new(buffer))
        .try_init()
        .map_err(|e| anyhow::anyhow!(e.to_string()))
}

fn keygen() -> Result<()> {
    let password = read_masked_password("Signing key password: ")?;
    let (sk, vk) = crypto::generate_keypair();
    crypto::save_keypair(
        &sk,
        &vk,
        &password,
        Path::new(config::SIGNING_KEY_ENC_FILE),
        Path::new(config::SIGNING_PUB_KEY_FILE),
    )?;
    println!(
        "Key pair saved:\n  private: {}\n  public:  {}",
        config::SIGNING_KEY_ENC_FILE,
        config::SIGNING_PUB_KEY_FILE
    );
    Ok(())
}

fn sign_module(lua_path: &Path) -> Result<()> {
    let source =
        std::fs::read_to_string(lua_path).with_context(|| format!("reading {lua_path:?}"))?;

    let manifest_path = loader::manifest::manifest_path(lua_path);
    let mut manifest: loader::manifest::ModuleManifest = if manifest_path.exists() {
        let raw = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading {manifest_path:?}"))?;
        serde_json::from_str(&raw).with_context(|| format!("parsing {manifest_path:?}"))?
    } else {
        loader::manifest::ModuleManifest::inferred(
            lua_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("module")
                .to_string(),
            loader::manifest::module_commands(&source),
            &source,
        )
    };

    let password = read_masked_password("Signing key password: ")?;
    let sk = crypto::load_signing_key(Path::new(config::SIGNING_KEY_ENC_FILE), &password)?;

    let sha256 = loader::manifest::source_sha256(&source);
    let payload = loader::manifest::signing_payload(&manifest, &sha256);
    let sig_bytes = crypto::sign_bytes(&payload, &sk);

    manifest.signature = Some(base64::engine::general_purpose::STANDARD.encode(sig_bytes));
    manifest.checksum = Some(sha256);
    manifest.trusted = true;

    let json = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(&manifest_path, json).with_context(|| format!("writing {manifest_path:?}"))?;
    println!("Signed: {manifest_path:?}");
    Ok(())
}

fn read_master_password() -> Result<Option<String>> {
    if let Some(password) = std::env::var(config::env_key::FLY_MASTER_PASSWORD)
        .ok()
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(password));
    }

    if encrypted_state_exists() {
        return read_masked_password("Master password: ").map(Some);
    }

    Ok(None)
}

fn encrypted_state_exists() -> bool {
    Path::new(config::DATABASE_ENCRYPTED_FILE).exists()
        || Path::new(config::DEFAULT_SESSION_ENCRYPTED_FILE).exists()
}

fn prompt(message: &str) -> Result<String> {
    use std::io::Write;
    print!("{message}");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

#[cfg(windows)]
fn read_masked_password(message: &str) -> Result<String> {
    use std::io::Write;
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, ReadConsoleW, SetConsoleMode, ENABLE_ECHO_INPUT,
        ENABLE_LINE_INPUT, STD_INPUT_HANDLE,
    };

    print!("{message}");
    std::io::stdout().flush()?;

    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if handle.is_null() {
        return prompt("");
    }
    let mut original_mode = 0;
    let mode_read = unsafe { GetConsoleMode(handle, &mut original_mode) };
    if mode_read == 0 {
        return prompt("");
    }

    let masked_mode = original_mode & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT);
    if unsafe { SetConsoleMode(handle, masked_mode) } == 0 {
        return prompt("");
    }

    let mut password = String::new();
    loop {
        let mut buffer = [0_u16; 1];
        let mut read = 0;
        let ok = unsafe {
            ReadConsoleW(
                handle,
                buffer.as_mut_ptr().cast(),
                1,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            break;
        }

        let ch = char::from_u32(buffer[0] as u32).unwrap_or_default();
        match ch {
            '\r' | '\n' => {
                println!();
                break;
            }
            '\u{3}' => {
                let _ = unsafe { SetConsoleMode(handle, original_mode) };
                anyhow::bail!("password input cancelled");
            }
            '\u{8}' => {
                if password.pop().is_some() {
                    print!("\u{8} \u{8}");
                    std::io::stdout().flush()?;
                }
            }
            _ => {
                password.push(ch);
                print!("*");
                std::io::stdout().flush()?;
            }
        }
    }

    let _ = unsafe { SetConsoleMode(handle, original_mode) };
    Ok(password)
}

#[cfg(not(windows))]
fn read_masked_password(message: &str) -> Result<String> {
    prompt(message)
}
