//! About White Rabbit: the version, the projects it comes from, and updates (`white_rabbit/info_tab.py`).
//!
//! Checking for updates asks GitHub for the newest release. The app can download that release's
//! installer, close itself and the monitor, and start the installer, which keeps the user's settings.

use std::rc::Rc;
use std::time::Duration;

use serde_json::{json, Value};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::Ctx;
use crate::{dialogs, AboutPage, AppWindow, Credit};

const REPO: &str = "kingof9x/white-rabbit-framework-matrix";
const SETUP_ASSET: &str = "WhiteRabbit-setup.exe";

/// The projects White Rabbit is based on or inspired by: (name, url, what came from it)
pub const CREDITS: [(&str, &str, &str); 8] = [
    ("led-matrix by MidnightJava", "https://github.com/MidnightJava/led-matrix",
     "The system monitor: apps on the panels, plugins and the layout file."),
    ("LED System Monitor by Jeremy", "https://code.karsttech.com/jeremy/FW_LED_System_Monitor",
     "The original system monitor that led-matrix grew from."),
    ("inputmodule-rs by Framework", "https://github.com/FrameworkComputer/inputmodule-rs",
     "The LED Matrix firmware and the LED Matrix Control app: patterns, games and firmware updates."),
    ("Framework-LED-Matrix by mariobx", "https://github.com/mariobx/Framework-LED-Matrix",
     "The ideas for the Math page (written from scratch, no code copied)."),
    ("pixeled by patate (slowpaste)", "https://github.com/slowpaste/pixeled",
     "The ideas for the Effects page, the sand battery and now playing apps (written from scratch)."),
    ("Framework16 LED Matrix Manager by Deo", "https://github.com/Deovandski/Deo-s-Framework16-LED-Matrix-Manager",
     "The ideas for the animations, the old-TV effect, mirror mode, the keyboard gap, panel names and the Draw page (our own code)."),
    ("Slint", "https://slint.dev",
     "The toolkit this window is built with (royalty-free licence)."),
    ("Fluent UI System Icons by Microsoft", "https://github.com/microsoft/fluentui-system-icons",
     "The icons in this window (MIT licence)."),
];

/// "v1.2" -> [1, 2, 0, 0], so "1" and "1.0.0" count as the same version.
pub fn version_numbers(text: &str) -> [u64; 4] {
    let mut out = [0u64; 4];
    let nums = text.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok());
    for (slot, n) in out.iter_mut().zip(nums) {
        *slot = n;
    }
    out
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    current.is_empty() || version_numbers(latest) > version_numbers(current)
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub setup_url: Option<String>,
}

pub fn parse_release(release: &Value) -> Release {
    let setup_url = release
        .get("assets")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|x| x.get("name").and_then(Value::as_str) == Some(SETUP_ASSET)))
        .and_then(|a| a.get("browser_download_url").and_then(Value::as_str))
        .map(String::from);
    Release {
        version: release.get("tag_name").and_then(Value::as_str).unwrap_or("").trim_start_matches('v').to_string(),
        page: release
            .get("html_url")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| format!("https://github.com/{REPO}/releases/latest")),
        setup_url,
    }
}

#[cfg(windows)]
fn latest_release() -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut reply = ureq::get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "WhiteRabbit")
        .config()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .call()
        .map_err(|e| e.to_string())?;
    let release: Value = reply.body_mut().read_json().map_err(|e| e.to_string())?;
    Ok(parse_release(&release))
}

#[cfg(not(windows))]
fn latest_release() -> Result<Release, String> {
    Err("Updates are checked on Windows only".into())
}

/// Downloads the installer into a new temporary folder. progress(percent).
#[cfg(windows)]
fn download(url: &str, progress: &dyn Fn(u32)) -> Result<std::path::PathBuf, String> {
    use std::io::{Read, Write};
    let folder = std::env::temp_dir().join(format!("WhiteRabbit-update-{}", std::process::id()));
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(SETUP_ASSET);
    let mut reply = ureq::get(url)
        .header("User-Agent", "WhiteRabbit")
        .config()
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .call()
        .map_err(|e| e.to_string())?;
    let total: u64 =
        reply.headers().get("Content-Length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = reply.body_mut().with_config().limit(500 * 1024 * 1024).reader();
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    loop {
        let n = body.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if total > 0 {
            progress((done * 100 / total) as u32);
        }
    }
    Ok(path)
}

#[cfg(not(windows))]
fn download(_url: &str, _progress: &dyn Fn(u32)) -> Result<std::path::PathBuf, String> {
    Err("Updates install on Windows only".into())
}

/// Starts the installer a couple of seconds from now, once White Rabbit has closed. The installer
/// refuses to start while White Rabbit runs, so a hidden cmd waits first.
fn start_installer_after_exit(path: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let line = format!("/c ping -n 3 127.0.0.1 >nul & start \"\" \"{}\"", path.display());
        std::process::Command::new("cmd")
            .raw_arg(line)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("Updates install on Windows only".into())
    }
}

fn show(app: &AppWindow, text: impl Into<slint::SharedString>) {
    app.global::<AboutPage>().set_update_status(text.into());
}

thread_local! {
    static RELEASE: std::cell::RefCell<Option<Release>> = const { std::cell::RefCell::new(None) };
}

pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let page = app.global::<AboutPage>();
    page.set_version(crate::version().into());
    let credits: Vec<Credit> =
        CREDITS.iter().map(|(n, u, w)| Credit { name: (*n).into(), url: (*u).into(), what: (*w).into() }).collect();
    page.set_credits(ModelRc::new(VecModel::from(credits)));
    page.on_release_page(|| {
        let page = RELEASE.with(|r| r.borrow().as_ref().map(|r| r.page.clone()));
        crate::app::open_url(&page.unwrap_or_else(|| format!("https://github.com/{REPO}/releases/latest")));
    });
    {
        let ctx = ctx.clone();
        page.on_check(move || {
            let Some(app) = ctx.ui.upgrade() else { return };
            let p = app.global::<AboutPage>();
            p.set_checking(true);
            p.set_can_install(false);
            p.set_show_page(false);
            show(&app, "Asking GitHub for the newest version...");
            ctx.in_background(latest_release, |app, result| {
                let p = app.global::<AboutPage>();
                p.set_checking(false);
                let current = crate::version();
                match result {
                    Err(e) => {
                        wr_core::warn!("Update check failed: {e}");
                        show(app, "Couldn't reach GitHub to check for updates. Check the internet connection and try again.");
                        p.set_show_page(true);
                    }
                    Ok(release) => {
                        let latest = if release.version.is_empty() { "unknown".to_string() } else { release.version.clone() };
                        if !is_newer(&release.version, &current) {
                            show(app, format!("You have the newest version ({current})."));
                        } else if cfg!(windows) && release.setup_url.is_some() {
                            show(
                                app,
                                format!(
                                    "Version {latest} is available (you have {current}). \"Download and install\" gets the installer, closes White Rabbit and starts the installer. Your settings are kept."
                                ),
                            );
                            p.set_can_install(true);
                            p.set_show_page(true);
                        } else {
                            show(app, format!("Version {latest} is available (you have {current})."));
                            p.set_show_page(true);
                        }
                        RELEASE.with(|r| *r.borrow_mut() = Some(release));
                    }
                }
            });
        });
    }
    {
        let ctx = ctx.clone();
        page.on_install(move || {
            let Some(release) = RELEASE.with(|r| r.borrow().clone()) else { return };
            let Some(url) = release.setup_url.clone() else { return };
            let Some(app) = ctx.ui.upgrade() else { return };
            app.global::<AboutPage>().set_checking(true);
            show(&app, format!("Downloading version {}...", release.version));
            let ui = ctx.ui.clone();
            let version = release.version.clone();
            ctx.in_background(
                move || {
                    let v = version.clone();
                    download(&url, &move |pct| {
                        let v = v.clone();
                        Ctx::later(&ui, move |app| show(app, format!("Downloading version {v}... {pct}%")));
                    })
                },
                move |app, result| {
                    app.global::<AboutPage>().set_checking(false);
                    match result {
                        Err(e) => {
                            wr_core::warn!("Update download failed: {e}");
                            show(app, "The download didn't work. Try again, or get it from the download page.");
                        }
                        Ok(path) => {
                            if !dialogs::confirm(
                                "Install the update",
                                &format!(
                                    "Version {} is downloaded.\n\nWhite Rabbit will close (the panels too) and the installer will start. Your settings are kept.",
                                    release.version
                                ),
                            ) {
                                show(app, "Downloaded, not installed yet. Click \"Download and install\" when you're ready.");
                                return;
                            }
                            show(app, "Closing White Rabbit and starting the installer...");
                            quit_for_update(path);
                        }
                    }
                },
            );
        });
    }
}

/// Closes the window and the monitor, then starts the installer.
fn quit_for_update(path: std::path::PathBuf) {
    let ctx = crate::app::ctx();
    let parts = ctx.clone_parts();
    ctx.in_background(
        move || {
            crate::jobs::stop_all();
            let panels = parts.panels.lock().unwrap().clone();
            parts.lender.give_back(&parts.sender, &panels, None);
            if parts.link.running(true) {
                crate::link::request(json!({"cmd": "quit"}), Duration::from_secs(5));
                let end = std::time::Instant::now() + Duration::from_secs(15);
                while std::time::Instant::now() < end && parts.link.running(true) {
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
            start_installer_after_exit(&path)
        },
        |app, result| match result {
            Ok(()) => {
                wr_core::info!("Closing for the update");
                let _ = app.hide();
                let _ = slint::quit_event_loop();
            }
            Err(e) => show(app, format!("Couldn't start the installer: {e}")),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(version_numbers("v1.2"), [1, 2, 0, 0]);
        assert!(is_newer("1.5", "1.4.0"));
        assert!(!is_newer("1.4", "1.4.0"));
        assert!(is_newer("1.4", ""));
    }

    #[test]
    fn release_reply() {
        let r = parse_release(&json!({"tag_name": "v1.5", "html_url": "https://x/1.5",
            "assets": [{"name": "WhiteRabbit.exe", "browser_download_url": "a"},
                       {"name": "WhiteRabbit-setup.exe", "browser_download_url": "b"}]}));
        assert_eq!(r.version, "1.5");
        assert_eq!(r.setup_url.as_deref(), Some("b"));
    }
}
