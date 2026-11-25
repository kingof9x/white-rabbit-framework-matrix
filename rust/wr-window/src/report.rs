//! `WhiteRabbitWindow.exe --diagnose` and `--list-apps`: text reports printed to the console the exe
//! was started from (for a user sending details with a bug report, or for checking a set-up).
//!
//! These run instead of opening the window (see `main.rs`). They only read: nothing is written to
//! settings and the panels are not drawn on.

use std::time::Duration;

use serde_json::json;

/// Prints the reports the command-line arguments ask for. Does nothing when neither flag is given.
pub fn run(args: &[String]) {
    if args.iter().any(|a| a == "--list-apps") {
        list_apps();
    }
    if args.iter().any(|a| a == "--diagnose") {
        diagnose();
    }
}

/// Prints every app that can go on a panel, with the key to use for it in the layout file and a
/// one-line description (both come from the editor's catalog, so this list is always up to date).
fn list_apps() {
    println!("Apps you can put on the panels (the name to use in the layout file, then what it shows):");
    for app in crate::pages::monitor::catalog::APPS.iter() {
        println!("  {:<14} {}", app.key, app.help);
    }
}

/// Prints what a helper needs to understand a user's set-up: the version, where settings and logs
/// live, every serial port Windows reports (device name, USB VID:PID, USB position and description),
/// which of them are LED Matrix panels, and whether the background monitor answers on its control
/// channel (2-second timeout, so a stuck monitor doesn't hang the report).
fn diagnose() {
    println!("White Rabbit {} diagnostics", crate::version());
    println!("Settings folder: {}", crate::paths::config_dir().display());
    println!("Logs: {}", crate::paths::log_dir().display());
    match crate::paths::layout_file() {
        Some(p) => println!("Layout file: {}", p.display()),
        None => println!("Layout file: none yet (using the one that comes with the app)"),
    }
    let ports = crate::panels::list_ports();
    println!("Serial ports ({}):", ports.len());
    for p in &ports {
        println!(
            "  {} {:04x}:{:04x} at {} ({})",
            p.device,
            p.vid.unwrap_or(0),
            p.pid.unwrap_or(0),
            p.location.as_deref().unwrap_or("?"),
            p.description
        );
    }
    let panels = crate::panels::find_panels();
    println!("LED Matrix panels ({}):", panels.len());
    for p in &panels {
        println!("  {}", p.label());
    }
    match crate::link::request(json!({"cmd": "status"}), Duration::from_secs(2)) {
        Some(reply) => println!("The monitor is running: {reply}"),
        None => println!("The monitor isn't running (or didn't answer)."),
    }
}
