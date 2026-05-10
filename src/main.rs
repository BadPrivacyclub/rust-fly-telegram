use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::info;

mod anti_delete;
mod bot;
mod client;
mod config;
mod crypto;
mod database;
mod loader;
mod runtime;
mod session_security;
mod telegram;
mod watcher;
mod web;

use crate::database::Database;
use crate::loader::Loader;
use crate::runtime::RuntimeState;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("fly_telegram=info".parse()?),
        )
        .init();

    info!("fly-telegram starting");

    let use_web = !std::env::args().any(|a| a == "--no-web");
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

    let loader =
        Arc::new(Loader::new(Arc::clone(&db), Arc::clone(&runtime), config::MODULES_DIR).await?);
    loader.load_all().await?;

    {
        let loader_w = Arc::clone(&loader);
        tokio::spawn(async move {
            if let Err(e) = watcher::watch(config::MODULES_DIR, loader_w).await {
                tracing::error!("file watcher stopped: {e}");
            }
        });
    }

    // Inline bot runs as a background task so it never races with the userbot
    // in tokio::select!. If the token is absent it exits immediately (Ok),
    // leaving the userbot unaffected.
    tokio::spawn(bot::run(Arc::clone(&db)));

    // Block until the userbot exits (Ctrl-C or fatal error).
    let run_result = client::run(
        Arc::clone(&db),
        Arc::clone(&loader),
        Arc::clone(&runtime),
        use_web,
    )
    .await;
    let seal_result = session_security.seal().await;

    run_result?;
    seal_result?;
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
