//! Small desktop conveniences for people who start fly-telegram with a double-click.

use std::process::{Command, Stdio};

/// Opens `url` in the default browser. Failures are ignored: on a headless server there is
/// simply no browser, and the URL is printed to the console anyway.
pub fn open_browser(url: &str) {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd");
        // The empty string is the window title argument of `start`.
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

/// True when started without arguments, which is how a double-click launches a program.
pub fn launched_without_arguments() -> bool {
    std::env::args_os().len() <= 1
}

/// Keeps a double-clicked console window open so the user can read the last message.
pub fn pause_before_exit() {
    if cfg!(windows) && launched_without_arguments() {
        use std::io::{BufRead, Write};
        print!("\nPress Enter to close this window...");
        let _ = std::io::stdout().flush();
        let _ = std::io::stdin().lock().lines().next();
    }
}
