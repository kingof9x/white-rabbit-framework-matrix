//! --diagnose and --list-apps: reports printed to the console WhiteRabbit.exe was started from.

use std::time::Duration;

use serde_json::json;

pub fn run(args: &[String]) {
    if args.iter().any(|a| a == "--list-apps") {
        list_apps();
    }
    if args.iter().any(|a| a == "--diagnose") {
        diagnose();
    }
}

fn list_apps() {
    println!("Apps you can put on the panels (the name to use in the layout file, then what it shows):");
    for app in crate::pages::monitor::catalog::APPS.iter() {
        println!("  {:<14} {}", app.key, app.help);
    }
}

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
