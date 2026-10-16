//! Small desktop conveniences I offer people who start fly-telegram with a double-click.

use std::process::{Command, Stdio};

/// I open `url` in the default browser and ignore failures: a headless server has
/// no browser, and the URL is printed to the console anyway.
pub fn open_browser(url: &str) {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd");
        // I pass an empty window title, which `start` expects before the URL.
        command.args(["/C", "start", "", url]);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else {
        let has_display =
            std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
        if !has_display {
            return;
        }
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    let _ = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// I take a start without arguments as a double-click launch.
pub fn launched_without_arguments() -> bool {
    std::env::args_os().len() <= 1
}

/// I keep a double-clicked console window open so the user can read the last message.
pub fn pause_before_exit() {
    if cfg!(windows) && launched_without_arguments() {
        use std::io::{BufRead, Write};
        print!("\nPress Enter to close this window...");
        let _ = std::io::stdout().flush();
        let _ = std::io::stdin().lock().lines().next();
    }
}
