//! Firmware: what each panel has, bootloader mode, and flashing the firmware that comes with White
//! Rabbit or a .uf2 file (`white_rabbit/firmware.py` and the Firmware tab in `tabs.py`).
//!
//! Flashing: the panel restarts into its bootloader and shows up as a drive (RPI-RP2) with an
//! INFO_UF2.TXT on it; copying the .uf2 file onto that drive installs it and the panel restarts.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::app::{ctx, Ctx};
use crate::commands as cmd;
use crate::panels::{Panel, Sender};
use crate::{dialogs, paths, AppWindow, FirmwarePage};

/// (folder under res/releases, label)
pub const BUNDLED: [(&str, &str); 2] = [
    ("white-rabbit", "White Rabbit firmware (startup animation setting, extra Game of Life patterns, faster refresh)"),
    ("framework-0.2.0", "Framework's official firmware 0.2.0"),
];
const CHOOSE_FILE: &str = "A .uf2 file on this computer...";
const FIRMWARE_FILE: &str = "ledmatrix.uf2";
const BOOTLOADER_WAIT: Duration = Duration::from_secs(10);
/// How long a panel may take to restart after flashing, before the next panel is flashed
const RESTART_WAIT: Duration = Duration::from_secs(20);

/// Where the release build puts the firmware: res\releases next to the exe.
pub fn releases_dir() -> PathBuf {
    let next_to_exe = paths::app_dir().join("res").join("releases");
    if next_to_exe.is_dir() {
        return next_to_exe;
    }
    // Run from the source tree
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("res").join("releases")
}

/// [(label, path)] of the firmware files that are actually there.
pub fn bundled_firmware(base: &Path) -> Vec<(String, PathBuf)> {
    BUNDLED
        .iter()
        .map(|(folder, label)| (label.to_string(), base.join(folder).join(FIRMWARE_FILE)))
        .filter(|(_, p)| p.is_file())
        .collect()
}

/// The RPI-RP2 drives: the ones with INFO_UF2.TXT on them.
pub fn get_drives() -> Vec<String> {
    let mut drives = Vec::new();
    #[cfg(windows)]
    {
        for letter in b'A'..=b'Z' {
            let drive = format!("{}:", letter as char);
            if Path::new(&format!("{drive}/INFO_UF2.TXT")).is_file() {
                drives.push(drive);
            }
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(user) = std::env::var("USER") {
            for root in [format!("/media/{user}"), format!("/run/media/{user}")] {
                for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
                    if e.path().join("INFO_UF2.TXT").is_file() {
                        drives.push(e.path().display().to_string());
                    }
                }
            }
        }
    }
    drives
}

fn wait_for(want_drives: bool, wait: Duration) -> Vec<String> {
    let end = Instant::now() + wait;
    loop {
        let drives = get_drives();
        if drives.is_empty() != want_drives || Instant::now() >= end {
            return drives;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Puts the panel into bootloader mode and copies the firmware onto its drive.
pub fn flash(sender: &Sender, panel: &Panel, path: &Path, status: &dyn Fn(String)) -> Result<String, String> {
    let firmware = std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let mut drives = get_drives();
    if drives.is_empty() {
        status(format!("Putting the {} into bootloader mode...", panel.called()));
        // The panel disappears straight away, so it may not answer
        let _ = sender.write(&panel.device, &[cmd::bootloader()]);
        sender.close_direct();
        drives = wait_for(true, BOOTLOADER_WAIT);
    }
    if drives.is_empty() {
        return Err("No RPI-RP2 drive appeared. Try Enter bootloader, then flash again.".into());
    }
    for drive in &drives {
        status(format!("Copying the firmware to {drive}..."));
        wr_core::info!("Flashing {} onto {drive}", path.display());
        std::fs::write(format!("{drive}/NEW.UF2"), &firmware).map_err(|e| format!("Couldn't copy to {drive}: {e}"))?;
    }
    // Let the panel restart before anything else happens, so flashing the next panel doesn't find this
    // one's drive still there and copy onto it again
    status(format!("Waiting for the {} to restart...", panel.called()));
    wait_for(false, RESTART_WAIT);
    Ok(format!(
        "Flashed {} panel{}. It restarts with the new firmware in a few seconds.",
        drives.len(),
        if drives.len() > 1 { "s" } else { "" }
    ))
}

fn show(app: &AppWindow, text: impl Into<SharedString>) {
    app.global::<FirmwarePage>().set_status(text.into());
}

fn status_fn() -> crate::app::Status {
    Arc::new(|app: &AppWindow, text: String| show(app, text))
}

/// What the panels said about themselves.
fn show_modules(app: &AppWindow) {
    let ctx = ctx();
    let modules = ctx.modules.lock().unwrap().clone();
    let panels = ctx.panels();
    let mut lines: Vec<SharedString> = Vec::new();
    if panels.is_empty() {
        lines.push("No LED Matrix panels found.".into());
    }
    for p in &panels {
        let info = modules.get(&p.device).cloned().unwrap_or_default();
        lines.push(
            if info.in_use {
                format!("{}: in use by another program. Close it, then click Check again.", p.label())
            } else {
                format!(
                    "{}: version {}, {} the startup animation setting",
                    p.label(),
                    info.version.unwrap_or_else(|| "unknown".into()),
                    if info.startup.is_some() { "has" } else { "doesn't have" }
                )
            }
            .into(),
        );
    }
    app.global::<FirmwarePage>().set_installed(ModelRc::new(VecModel::from(lines)));
}

pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let page = app.global::<FirmwarePage>();
    let bundled = bundled_firmware(&releases_dir());
    let mut choices: Vec<SharedString> = bundled.iter().map(|(l, _)| l.as_str().into()).collect();
    choices.push(CHOOSE_FILE.into());
    page.set_choices(ModelRc::new(VecModel::from(choices)));
    ctx.on_modules.lock().unwrap().push(Box::new(show_modules));
    {
        let ctx = ctx.clone();
        page.on_refresh(move || {
            if let Some(app) = ctx.ui.upgrade() {
                app.global::<FirmwarePage>()
                    .set_installed(ModelRc::new(VecModel::from(vec![SharedString::from("Reading the panels...")])));
            }
            ctx.refresh_modules();
        });
    }
    {
        let ctx = ctx.clone();
        page.on_sleep(move |on| {
            ctx.act(move |s, p| s.write(&p.device, &[cmd::sleep(on)]), status_fn(), None, true, None, false);
        });
    }
    {
        let ctx = ctx.clone();
        page.on_bootloader(move || {
            if !dialogs::confirm(
                "Enter bootloader",
                "The ticked panels go dark and show up as an RPI-RP2 drive, ready for new firmware. Continue?",
            ) {
                return;
            }
            let status: crate::app::Status = Arc::new(|app: &AppWindow, text: String| {
                show(
                    app,
                    if text == "Done." {
                        "Copy a .uf2 file onto the RPI-RP2 drive, or use Flash.".into()
                    } else {
                        text
                    },
                )
            });
            ctx.act(
                |s, p| {
                    let _ = s.write(&p.device, &[cmd::bootloader()]);
                    s.close_direct();
                    Ok(())
                },
                status,
                None,
                true,
                None,
                false,
            );
        });
    }
    {
        let ctx = ctx.clone();
        page.on_flash(move |choice| {
            let Some(app) = ctx.ui.upgrade() else { return };
            // Check the panels first: a message after the file picker was easy to miss
            let panels = ctx.selected_panels();
            if panels.is_empty() {
                dialogs::info(
                    "Flash firmware",
                    "Tick the panels to flash under \"Controls act on\", bottom left, first.",
                );
                return;
            }
            let (label, path) = match bundled.get(choice as usize) {
                Some((label, path)) => (label.clone(), path.clone()),
                None => match dialogs::open_file("Choose LED Matrix firmware", &[("Firmware", &["uf2"])]) {
                    Some(p) => (p.display().to_string(), p),
                    None => {
                        show(&app, "No firmware file chosen.");
                        return;
                    }
                },
            };
            let file = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            let where_ = panels.iter().map(|p| p.side.clone()).collect::<Vec<_>>().join(" and ");
            let plural = if panels.len() > 1 { "s" } else { "" };
            if !dialogs::confirm("Flash firmware", &format!("Flash {file} onto the {where_} panel{plural}?\n\n{label}"))
            {
                return;
            }
            show(&app, format!("Flashing {file}..."));
            app.global::<FirmwarePage>().set_busy(true);
            let ui = ctx.ui.clone();
            let done: crate::app::Status = Arc::new(move |app: &AppWindow, text: String| {
                let text =
                    if text == "Done." { format!("Flashed {file} onto the {where_} panel{plural}.") } else { text };
                show(app, text);
                app.global::<FirmwarePage>().set_busy(false);
                // Give the panels back once they have restarted, then read their new versions
                let ctx = crate::app::ctx();
                let panels = ctx.selected_panels();
                ctx.after(6.0, move |_| {
                    let ctx = crate::app::ctx();
                    ctx.clone_parts().give_back(panels, None);
                    ctx.refresh_modules();
                });
            });
            ctx.act(
                move |s, p| {
                    let ui = ui.clone();
                    let status = move |text: String| Ctx::later(&ui, move |app| show(app, text));
                    flash(s, p, &path, &status).map(|_| ())
                },
                done,
                Some(panels),
                true,
                None,
                false,
            );
        });
    }
}
