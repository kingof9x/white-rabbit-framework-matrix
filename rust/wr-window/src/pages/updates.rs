//! Check for updates, on the Settings page: a newer White Rabbit and newer panel firmware.
//!
//! Nothing is checked until the user clicks "Check for updates". The check asks GitHub for the newest
//! White Rabbit release and the newest firmware release (kingof9x/inputmodule-rs, tagged build-N), and
//! reads each panel's firmware build. The app update downloads the installer, closes White Rabbit and
//! the monitor, and starts the installer, which keeps the user's settings. The firmware update flashes
//! the newest White Rabbit firmware (the one that came with the app, or a newer download) onto the
//! panels that have an older build.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use slint::ComponentHandle;

use crate::app::{read_modules, Ctx, ModuleInfo};
use crate::pages::firmware::{self, bundled_build, needs_update, releases_dir, tag_build};
use crate::panels::Panel;
use crate::{dialogs, AppWindow, UpdatesPage};

pub const REPO: &str = "kingof9x/white-rabbit-framework-matrix";
pub const FIRMWARE_REPO: &str = "kingof9x/inputmodule-rs";
#[cfg_attr(not(windows), allow(dead_code))]
const SETUP_ASSET: &str = "WhiteRabbit-setup.exe";
#[cfg_attr(not(windows), allow(dead_code))]
const FIRMWARE_ASSET: &str = "ledmatrix.uf2";

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
    /// The tag without a leading v: "1.5", "build-15"
    pub version: String,
    pub page: String,
    /// The download link of the asset we want (installer or firmware)
    pub asset_url: Option<String>,
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_release(release: &Value, asset: &str, repo: &str) -> Release {
    let asset_url = release
        .get("assets")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|x| x.get("name").and_then(Value::as_str) == Some(asset)))
        .and_then(|a| a.get("browser_download_url").and_then(Value::as_str))
        .map(String::from);
    Release {
        version: release.get("tag_name").and_then(Value::as_str).unwrap_or("").trim_start_matches('v').to_string(),
        page: release
            .get("html_url")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| format!("https://github.com/{repo}/releases/latest")),
        asset_url,
    }
}

#[cfg(windows)]
fn latest_release(repo: &str, asset: &str) -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let mut reply = ureq::get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "WhiteRabbit")
        .config()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .call()
        .map_err(|e| e.to_string())?;
    let release: Value = reply.body_mut().read_json().map_err(|e| e.to_string())?;
    Ok(parse_release(&release, asset, repo))
}

#[cfg(not(windows))]
fn latest_release(_repo: &str, _asset: &str) -> Result<Release, String> {
    Err("Updates are checked on Windows only".into())
}

/// Downloads a file into a new temporary folder. progress(percent).
#[cfg(windows)]
fn download(url: &str, name: &str, progress: &dyn Fn(u32)) -> Result<PathBuf, String> {
    use std::io::{Read, Write};
    let folder = std::env::temp_dir().join(format!("WhiteRabbit-update-{}", std::process::id()));
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(name);
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
        if let Some(percent) = (done * 100).checked_div(total) {
            progress(percent as u32);
        }
    }
    Ok(path)
}

#[cfg(not(windows))]
fn download(_url: &str, _name: &str, _progress: &dyn Fn(u32)) -> Result<PathBuf, String> {
    Err("Updates download on Windows only".into())
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

/// Where the newest White Rabbit firmware is.
#[derive(Clone, Debug, PartialEq)]
pub enum FirmwareSource {
    /// The file that came with the app
    Bundled(PathBuf),
    /// A newer release on GitHub, downloaded when it's flashed
    Download(String),
}

#[derive(Clone, Debug)]
pub struct FirmwareOffer {
    pub build: u16,
    pub source: FirmwareSource,
    /// The panels that have an older build
    pub panels: Vec<Panel>,
}

/// What the check found out about the firmware: the text for Settings, and what to flash if anything.
pub fn firmware_report(
    panels: &[Panel],
    modules: &std::collections::HashMap<String, ModuleInfo>,
    bundled: Option<(u16, PathBuf)>,
    release: Option<&Release>,
) -> (String, Option<FirmwareOffer>) {
    let mut lines = Vec::new();
    if panels.is_empty() {
        lines.push("No LED Matrix panels found.".to_string());
    }
    for p in panels {
        let info = modules.get(&p.device).cloned().unwrap_or_default();
        lines.push(if info.in_use {
            format!("{}: in use by another program, so its firmware couldn't be read.", p.label())
        } else {
            format!("{}: {}.", p.label(), firmware::long_name(&info))
        });
    }
    let released = release.and_then(|r| Some((tag_build(&r.version)?, r.asset_url.clone()?)));
    let newest = match (bundled, released) {
        (Some((b, _)), Some((r, url))) if r > b => Some((r, FirmwareSource::Download(url))),
        (Some((b, path)), _) => Some((b, FirmwareSource::Bundled(path))),
        (None, Some((r, url))) => Some((r, FirmwareSource::Download(url))),
        (None, None) => None,
    };
    let Some((build, source)) = newest else {
        lines.push("Couldn't find out which firmware is the newest.".into());
        return (lines.join("\n"), None);
    };
    let from = match source {
        FirmwareSource::Bundled(_) => "comes with this app",
        FirmwareSource::Download(_) => "on GitHub",
    };
    lines.push(format!("Newest: White Rabbit firmware build {build} ({from})."));
    let outdated: Vec<Panel> =
        panels.iter().filter(|p| modules.get(&p.device).is_some_and(|m| needs_update(m, build))).cloned().collect();
    let framework = panels
        .iter()
        .any(|p| modules.get(&p.device).is_some_and(|m| firmware::kind(m) == firmware::Kind::Framework && !m.in_use));
    if outdated.is_empty() {
        if !panels.is_empty() {
            lines.push(if framework {
                "Panels with Framework's firmware are left as they are. You can flash White Rabbit firmware on the Firmware page.".into()
            } else {
                "Your panels are up to date.".into()
            });
        }
        return (lines.join("\n"), None);
    }
    (lines.join("\n"), Some(FirmwareOffer { build, source, panels: outdated }))
}

fn show_app(app: &AppWindow, text: impl Into<slint::SharedString>) {
    app.global::<UpdatesPage>().set_app_status(text.into());
}

fn show_firmware(app: &AppWindow, text: impl Into<slint::SharedString>) {
    app.global::<UpdatesPage>().set_firmware_status(text.into());
}

thread_local! {
    static RELEASE: std::cell::RefCell<Option<Release>> = const { std::cell::RefCell::new(None) };
    static OFFER: std::cell::RefCell<Option<FirmwareOffer>> = const { std::cell::RefCell::new(None) };
}

fn panel_names(panels: &[Panel]) -> String {
    let sides: Vec<String> = panels.iter().map(|p| p.side.clone()).collect();
    format!("{} panel{}", sides.join(" and "), if panels.len() > 1 { "s" } else { "" })
}

/// Shows what the check found. Also used by the screenshot tool.
pub fn show_results(
    app: &AppWindow,
    current: &str,
    app_release: Result<Release, String>,
    fw_text: String,
    offer: Option<FirmwareOffer>,
) {
    let p = app.global::<UpdatesPage>();
    p.set_checking(false);
    p.set_can_install(false);
    p.set_show_page(false);
    match app_release {
        Err(e) => {
            wr_core::warn!("Update check failed: {e}");
            show_app(
                app,
                "Couldn't reach GitHub to check for a newer White Rabbit. Check the internet connection and try again.",
            );
        }
        Ok(release) => {
            let latest = if release.version.is_empty() { "unknown".to_string() } else { release.version.clone() };
            if !is_newer(&release.version, current) {
                show_app(app, format!("You have the newest version ({current})."));
            } else if release.asset_url.is_some() {
                show_app(app, format!("Version {latest} is available (you have {current}). Your settings are kept."));
                p.set_can_install(true);
                p.set_show_page(true);
            } else {
                show_app(app, format!("Version {latest} is available (you have {current})."));
                p.set_show_page(true);
            }
            RELEASE.with(|r| *r.borrow_mut() = Some(release));
        }
    }
    show_firmware(app, fw_text);
    p.set_can_flash(offer.is_some());
    p.set_flash_label(match &offer {
        Some(o) => format!("Flash build {}", o.build).into(),
        None => "".into(),
    });
    OFFER.with(|o| *o.borrow_mut() = offer);
}

pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let page = app.global::<UpdatesPage>();
    page.on_release_page(|| {
        let page = RELEASE.with(|r| r.borrow().as_ref().map(|r| r.page.clone()));
        crate::app::open_url(&page.unwrap_or_else(|| format!("https://github.com/{REPO}/releases/latest")));
    });
    {
        let ctx = ctx.clone();
        page.on_check(move || {
            let Some(app) = ctx.ui.upgrade() else { return };
            let p = app.global::<UpdatesPage>();
            p.set_checking(true);
            p.set_can_install(false);
            p.set_show_page(false);
            p.set_can_flash(false);
            show_app(&app, "Asking GitHub for the newest version...");
            show_firmware(&app, "Reading the panels and asking GitHub for the newest firmware...");
            let (sender, panels, modules) = (ctx.sender.clone(), ctx.panels(), ctx.modules.clone());
            ctx.in_background(
                move || {
                    let found = read_modules(&sender, &panels);
                    *modules.lock().unwrap() = found.clone();
                    let app_release = latest_release(REPO, SETUP_ASSET);
                    let fw_release = latest_release(FIRMWARE_REPO, FIRMWARE_ASSET);
                    if let Err(e) = &fw_release {
                        wr_core::warn!("Firmware update check failed: {e}");
                    }
                    let base = releases_dir();
                    let bundled =
                        bundled_build(&base).map(|n| (n, base.join(firmware::WHITE_RABBIT).join(FIRMWARE_ASSET)));
                    let (mut text, offer) = firmware_report(&panels, &found, bundled, fw_release.as_ref().ok());
                    if fw_release.is_err() {
                        text.push_str(
                            "\n(Couldn't reach GitHub, so this compares with the firmware that came with the app.)",
                        );
                    }
                    (app_release, text, offer)
                },
                |app, (app_release, text, offer)| {
                    show_results(app, &crate::version(), app_release, text, offer);
                    crate::app::ctx().modules_changed(app);
                },
            );
        });
    }
    {
        let ctx = ctx.clone();
        page.on_install(move || {
            let Some(release) = RELEASE.with(|r| r.borrow().clone()) else { return };
            let Some(url) = release.asset_url.clone() else { return };
            let Some(app) = ctx.ui.upgrade() else { return };
            app.global::<UpdatesPage>().set_checking(true);
            show_app(&app, format!("Downloading version {}...", release.version));
            let ui = ctx.ui.clone();
            let version = release.version.clone();
            ctx.in_background(
                move || {
                    let v = version.clone();
                    download(&url, SETUP_ASSET, &move |pct| {
                        let v = v.clone();
                        Ctx::later(&ui, move |app| show_app(app, format!("Downloading version {v}... {pct}%")));
                    })
                },
                move |app, result| {
                    app.global::<UpdatesPage>().set_checking(false);
                    match result {
                        Err(e) => {
                            wr_core::warn!("Update download failed: {e}");
                            show_app(app, "The download didn't work. Try again, or get it from the download page.");
                        }
                        Ok(path) => {
                            if !dialogs::confirm(
                                "Install the update",
                                &format!(
                                    "Version {} is downloaded.\n\nWhite Rabbit will close (the panels too) and the installer will start. Your settings are kept.",
                                    release.version
                                ),
                            ) {
                                show_app(app, "Downloaded, not installed yet. Click \"Download and install\" when you're ready.");
                                return;
                            }
                            show_app(app, "Closing White Rabbit and starting the installer...");
                            quit_for_update(path);
                        }
                    }
                },
            );
        });
    }
    {
        let ctx = ctx.clone();
        page.on_flash(move || {
            let Some(offer) = OFFER.with(|o| o.borrow().clone()) else { return };
            let Some(app) = ctx.ui.upgrade() else { return };
            let names = panel_names(&offer.panels);
            let from = match &offer.source {
                FirmwareSource::Bundled(_) => "It came with this app.",
                FirmwareSource::Download(_) => "It's downloaded from GitHub first.",
            };
            if !dialogs::confirm(
                "Update the panel firmware",
                &format!(
                    "Flash White Rabbit firmware build {} onto the {names}?\n\n{from} Close other programs that use the LED panels first. Each panel goes dark for a few seconds while it updates.",
                    offer.build
                ),
            ) {
                return;
            }
            let p = app.global::<UpdatesPage>();
            p.set_checking(true);
            p.set_can_flash(false);
            let build = offer.build;
            let ui = ctx.ui.clone();
            ctx.in_background(
                move || -> Result<PathBuf, String> {
                    match offer.source {
                        FirmwareSource::Bundled(path) => Ok(path),
                        FirmwareSource::Download(url) => {
                            let path = download(&url, FIRMWARE_ASSET, &move |pct| {
                                Ctx::later(&ui, move |app| {
                                    show_firmware(app, format!("Downloading firmware build {build}... {pct}%"))
                                });
                            })?;
                            let data = std::fs::read(&path).map_err(|e| e.to_string())?;
                            if !firmware::is_uf2(&data) {
                                return Err("the download isn't a firmware file".into());
                            }
                            Ok(path)
                        }
                    }
                },
                move |app, result| {
                    let path = match result {
                        Ok(path) => path,
                        Err(e) => {
                            wr_core::warn!("Firmware download failed: {e}");
                            app.global::<UpdatesPage>().set_checking(false);
                            show_firmware(app, "The firmware download didn't work. Try again, or use the Firmware page.");
                            return;
                        }
                    };
                    show_firmware(app, format!("Flashing firmware build {build} onto the {names}..."));
                    let done_text = format!(
                        "Flashed firmware build {build} onto the {names}. Click \"Check for updates\" again in a few seconds to see the new version."
                    );
                    firmware::flash_panels(
                        &crate::app::ctx(),
                        offer.panels.clone(),
                        path,
                        Arc::new(|app: &AppWindow, text: String| show_firmware(app, text)),
                        Arc::new(move |app: &AppWindow, text: String| {
                            app.global::<UpdatesPage>().set_checking(false);
                            show_firmware(app, if text == "Done." { done_text.clone() } else { text });
                        }),
                    );
                },
            );
        });
    }
}

/// Sample answers for screenshots: "older" (panels from before build numbers, newer firmware and app on
/// GitHub) or "current" (everything up to date).
pub fn demo(app: &AppWindow, ctx: &Rc<Ctx>, case: &str) {
    let panels = ctx.panels();
    let wr = |b| ModuleInfo { version: Some("0.2.0".into()), build: b, startup: Some((1, 0, 0)), in_use: false };
    let build = if case == "current" { Some(15) } else { None };
    let modules: std::collections::HashMap<String, ModuleInfo> =
        panels.iter().map(|p| (p.device.clone(), wr(build))).collect();
    *ctx.modules.lock().unwrap() = modules.clone();
    let fw = Release {
        version: "build-15".into(),
        page: String::new(),
        asset_url: Some("https://example.invalid/ledmatrix.uf2".into()),
    };
    let (text, offer) = firmware_report(&panels, &modules, Some((14, PathBuf::from("bundled.uf2"))), Some(&fw));
    let app_release = Release {
        version: if case == "current" { crate::version() } else { "1.6".into() },
        page: String::new(),
        asset_url: Some("https://example.invalid/WhiteRabbit-setup.exe".into()),
    };
    show_results(app, &crate::version(), Ok(app_release), text, offer);
    ctx.modules_changed(app);
}

/// Closes the window and the monitor, then starts the installer.
fn quit_for_update(path: PathBuf) {
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
            Err(e) => show_app(app, format!("Couldn't start the installer: {e}")),
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
        let r = parse_release(
            &json!({"tag_name": "v1.5", "html_url": "https://x/1.5",
                "assets": [{"name": "WhiteRabbit.exe", "browser_download_url": "a"},
                           {"name": "WhiteRabbit-setup.exe", "browser_download_url": "b"}]}),
            SETUP_ASSET,
            REPO,
        );
        assert_eq!(r.version, "1.5");
        assert_eq!(r.asset_url.as_deref(), Some("b"));
    }

    fn panel(side: &str) -> Panel {
        Panel { device: format!("COM-{side}"), side: side.into(), location: None, nickname: String::new() }
    }

    #[test]
    fn firmware_offers() {
        let panels = vec![panel("left"), panel("right")];
        let wr = |b| ModuleInfo { version: Some("0.2.0".into()), build: b, startup: Some((0, 0, 0)), in_use: false };
        let mut modules = std::collections::HashMap::new();
        modules.insert("COM-left".to_string(), wr(None));
        modules.insert("COM-right".to_string(), wr(Some(15)));
        let release = Release { version: "build-16".into(), page: String::new(), asset_url: Some("u".into()) };
        // A newer release on GitHub: both panels are older
        let (text, offer) = firmware_report(&panels, &modules, Some((15, PathBuf::from("b"))), Some(&release));
        let offer = offer.unwrap();
        assert_eq!(offer.build, 16);
        assert_eq!(offer.source, FirmwareSource::Download("u".into()));
        assert_eq!(offer.panels.len(), 2);
        assert!(text.contains("build 16 (on GitHub)"), "{text}");
        // Offline: the bundled build 15 only updates the older panel
        let (_, offer) = firmware_report(&panels, &modules, Some((15, PathBuf::from("b"))), None);
        let offer = offer.unwrap();
        assert_eq!(offer.source, FirmwareSource::Bundled(PathBuf::from("b")));
        assert_eq!(offer.panels.len(), 1);
        // Up to date
        modules.insert("COM-left".to_string(), wr(Some(16)));
        modules.insert("COM-right".to_string(), wr(Some(16)));
        let (text, offer) = firmware_report(&panels, &modules, Some((15, PathBuf::from("b"))), Some(&release));
        assert!(offer.is_none());
        assert!(text.contains("up to date"), "{text}");
    }
}
