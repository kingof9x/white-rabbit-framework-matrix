//! Firmware: what each panel has, bootloader mode, and flashing the firmware that comes with White
//! Rabbit or a .uf2 file (`white_rabbit/firmware.py` and the Firmware tab in `tabs.py`).
//!
//! Flashing: the panel restarts into its bootloader and shows up as a drive (RPI-RP2) with an
//! INFO_UF2.TXT on it; copying the .uf2 file onto that drive installs it and the panel restarts.
//!
//! How the Slint callbacks (`FirmwarePage` in `ui/pages/firmware.slint`) map to Rust, connected in
//! `wire`: `refresh()` ("Check again") re-reads every panel's firmware through `Ctx::refresh_modules`;
//! `sleep(on)` puts the ticked panels to sleep or wakes them; `bootloader()` restarts them into the
//! bootloader; `flash(choice)` flashes a bundled firmware (by its index in `choices`) or, for the last
//! choice, a .uf2 file the user picks. The "installed" list is filled by `show_modules`, which runs
//! whenever the panels' firmware information changes.
//!
//! Threads: every panel command goes through `Ctx::act`, which borrows the panels from the monitor and
//! runs on a worker thread. That matters most for flashing: it waits up to 10 seconds for the drive to
//! appear and up to 20 seconds for the panel to restart, per panel. Progress messages are passed back to
//! the UI thread with `Ctx::later`. The confirmation dialogs run on the UI thread, before any work starts.
//!
//! Nothing is saved on the computer. The Settings page's update check (`updates.rs`) uses this file's
//! helpers (`needs_update`, `bundled_build`, `flash_panels`) to offer new firmware too.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::app::{ctx, Ctx, ModuleInfo};
use crate::commands as cmd;
use crate::panels::{Panel, Sender};
use crate::{dialogs, paths, AppWindow, FirmwarePage};

/// (folder under res/releases, label)
/// The firmware that ships with the app, in the order of the page's drop-down. Each folder holds a
/// `ledmatrix.uf2`; entries whose file is missing are left out of the list (`bundled_firmware`).
pub const BUNDLED: [(&str, &str); 2] = [
    ("white-rabbit", "White Rabbit firmware (startup animation setting, extra Game of Life patterns, faster refresh)"),
    ("framework-0.2.0", "Framework's official firmware 0.2.0"),
];
/// The folder of the White Rabbit firmware that comes with the app
pub const WHITE_RABBIT: &str = "white-rabbit";
/// What White Rabbit firmware keeps in its image, followed by its build number (2 bytes, little endian)
/// The firmware build writes these bytes into its image so the build number can be read from a .uf2 file
/// without running it (`uf2_build`).
const BUILD_TAG: &[u8] = b"WRFW-BUILD";
/// A .uf2 file is a list of 512-byte blocks.
const UF2_BLOCK: usize = 512;
/// The two magic numbers at the start of every UF2 block ("UF2\n" and a fixed number from the UF2 spec).
const UF2_MAGIC: [u32; 2] = [0x0A32_4655, 0x9E5D_5157];

/// What firmware a panel has, worked out from what it answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// White Rabbit firmware with this build number (0: built by hand, a test build)
    WhiteRabbit(u16),
    /// White Rabbit firmware from before it reported build numbers (it has the startup setting)
    OlderWhiteRabbit,
    /// Framework's firmware (no startup setting)
    Framework,
    /// The panel didn't answer the version question (in use by another program, or not a known firmware).
    Unknown,
}

/// Works out the firmware from what it answered: a build number means White Rabbit; answering the
/// startup question but no build means an older White Rabbit; a version only means Framework's.
pub fn kind(info: &ModuleInfo) -> Kind {
    match (info.build, info.startup.is_some(), info.version.is_some()) {
        (Some(n), _, _) => Kind::WhiteRabbit(n),
        (None, true, _) => Kind::OlderWhiteRabbit,
        (None, false, true) => Kind::Framework,
        _ => Kind::Unknown,
    }
}

/// "White Rabbit firmware build 15", for the Firmware page and Settings.
pub fn long_name(info: &ModuleInfo) -> String {
    let version = info.version.clone().unwrap_or_else(|| "unknown".into());
    match kind(info) {
        Kind::WhiteRabbit(0) => format!("White Rabbit firmware, a test build (version {version})"),
        Kind::WhiteRabbit(n) => format!("White Rabbit firmware build {n}"),
        Kind::OlderWhiteRabbit => "White Rabbit firmware, an older build from before build numbers".into(),
        Kind::Framework => format!("Framework's official firmware {version}"),
        Kind::Unknown => "unknown firmware".into(),
    }
}

/// "White Rabbit build 15", for the panel cards.
pub fn short_name(info: &ModuleInfo) -> String {
    match kind(info) {
        Kind::WhiteRabbit(0) => "White Rabbit test build".into(),
        Kind::WhiteRabbit(n) => format!("White Rabbit build {n}"),
        Kind::OlderWhiteRabbit => "White Rabbit, older build".into(),
        Kind::Framework => format!("Framework {}", info.version.clone().unwrap_or_default()),
        Kind::Unknown => "unknown".into(),
    }
}

/// Whether this panel should get White Rabbit firmware build `newest`. Framework's firmware and test
/// builds are left alone: someone put those on on purpose.
pub fn needs_update(info: &ModuleInfo, newest: u16) -> bool {
    !info.in_use
        && match kind(info) {
            Kind::WhiteRabbit(n) => n != 0 && n < newest,
            Kind::OlderWhiteRabbit => true,
            Kind::Framework | Kind::Unknown => false,
        }
}

/// True when the file is a .uf2 firmware file.
/// Checks the size is a whole number of blocks and the first block starts with both magic numbers.
pub fn is_uf2(data: &[u8]) -> bool {
    data.len() >= UF2_BLOCK
        && data.len().is_multiple_of(UF2_BLOCK)
        && uf2_word(data, 0) == UF2_MAGIC[0]
        && uf2_word(data, 4) == UF2_MAGIC[1]
}

/// The little-endian 32-bit number at byte `at`. The caller makes sure there are 4 bytes there.
fn uf2_word(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

/// The White Rabbit build number kept in a .uf2 file, if it has one. The firmware is split into
/// blocks of 512 bytes, each carrying up to 476 bytes of it, so the pieces are put back together first.
pub fn uf2_build(data: &[u8]) -> Option<u16> {
    if !is_uf2(data) {
        return None;
    }
    let mut blocks: Vec<(u32, &[u8])> = data
        .chunks_exact(UF2_BLOCK)
        .filter(|b| uf2_word(b, 0) == UF2_MAGIC[0] && uf2_word(b, 4) == UF2_MAGIC[1])
        // In each block: bytes 12..16 are the flash address, 16..20 the payload size and the payload
        // starts at byte 32. Capped at 476 so a bad size can't read past the block.
        .map(|b| (uf2_word(b, 12), &b[32..32 + (uf2_word(b, 16) as usize).min(476)]))
        .collect();
    // Put the pieces in address order and join them. Gaps between addresses are ignored, which is
    // fine for finding a tag: the tag only has to be contiguous within the joined bytes.
    blocks.sort_by_key(|(addr, _)| *addr);
    let image: Vec<u8> = blocks.iter().flat_map(|(_, d)| d.iter().copied()).collect();
    let at = image.windows(BUILD_TAG.len()).position(|w| w == BUILD_TAG)? + BUILD_TAG.len();
    image.get(at..at + 2).map(|n| u16::from_le_bytes([n[0], n[1]]))
}

/// "build-15" -> 15
/// The release tag of a firmware build to its number. Anything else gives None.
pub fn tag_build(tag: &str) -> Option<u16> {
    tag.trim().strip_prefix("build-").and_then(|n| n.parse().ok())
}

/// The build number of the White Rabbit firmware that comes with the app: from the file itself, or
/// from the release name the app's build wrote next to it.
/// `base` is the releases folder (`releases_dir`). A test build (build 0) in the file falls back to the
/// VERSION file. None when neither says.
pub fn bundled_build(base: &Path) -> Option<u16> {
    let folder = base.join(WHITE_RABBIT);
    std::fs::read(folder.join(FIRMWARE_FILE))
        .ok()
        .and_then(|d| uf2_build(&d))
        .filter(|n| *n != 0)
        .or_else(|| std::fs::read_to_string(folder.join("VERSION")).ok().and_then(|t| tag_build(&t)))
}

/// The firmware choices' labels, with the build number of the White Rabbit firmware.
/// Only the White Rabbit entry gets the number; other labels are used as they are.
fn choice_label(folder: &str, label: &str, base: &Path) -> String {
    match (folder, bundled_build(base)) {
        (WHITE_RABBIT, Some(n)) => {
            label.replacen("White Rabbit firmware", &format!("White Rabbit firmware build {n}"), 1)
        }
        _ => label.to_string(),
    }
}
/// The last drop-down entry, which opens a file picker instead of using a bundled file.
const CHOOSE_FILE: &str = "A .uf2 file on this computer...";
/// The file name of the firmware inside each bundled folder.
const FIRMWARE_FILE: &str = "ledmatrix.uf2";
/// How long to wait for the RPI-RP2 drive to appear after asking a panel to enter its bootloader.
const BOOTLOADER_WAIT: Duration = Duration::from_secs(10);
/// How long a panel may take to restart after flashing, before the next panel is flashed
const RESTART_WAIT: Duration = Duration::from_secs(20);

/// Where the release build puts the firmware: res\releases next to the exe.
/// When run with `cargo run` from the source tree, the repository's res\releases is used instead.
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
        .map(|(folder, label)| (choice_label(folder, label, base), base.join(folder).join(FIRMWARE_FILE)))
        .filter(|(_, p)| p.is_file())
        .collect()
}

/// The RPI-RP2 drives: the ones with INFO_UF2.TXT on them.
/// On Windows it checks every drive letter A: to Z: for INFO_UF2.TXT (a quick file check each).
/// Elsewhere (for development) it looks in the usual Linux mount folders under /media and /run/media.
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

/// Polls the drives every half second until there are some (`want_drives` true) or none (false), or
/// `wait` runs out. Returns the drives found at the end either way. Runs on a worker thread.
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
/// Runs on a worker thread (through `Ctx::act`). `status` reports progress to the page. If an RPI-RP2
/// drive is already there (the user pressed Enter bootloader first) the panel isn't asked again. The
/// firmware is copied to every RPI-RP2 drive present, so the result counts drives, not panels. Errors
/// come back as text for the status line.
pub fn flash(sender: &Sender, panel: &Panel, path: &Path, status: &dyn Fn(String)) -> Result<String, String> {
    let firmware = std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let mut drives = get_drives();
    if drives.is_empty() {
        status(format!("Putting the {} into bootloader mode...", panel.called()));
        // The panel disappears straight away, so it may not answer
        let _ = sender.write(&panel.device, &[cmd::bootloader()]);
        // Without a monitor the window has the port open itself; close it now that the panel has
        // gone, so the port is free when the panel comes back with the new firmware.
        sender.close_direct();
        drives = wait_for(true, BOOTLOADER_WAIT);
    }
    if drives.is_empty() {
        return Err("No RPI-RP2 drive appeared. Try Enter bootloader, then flash again.".into());
    }
    for drive in &drives {
        status(format!("Copying the firmware to {drive}..."));
        wr_core::info!("Flashing {} onto {drive}", path.display());
        // The file name doesn't matter to the bootloader; any .uf2 written to the drive is flashed.
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

/// Sets the status line on the Firmware page.
fn show(app: &AppWindow, text: impl Into<SharedString>) {
    app.global::<FirmwarePage>().set_status(text.into());
}

/// `show` in the form `Ctx::act` takes for reporting its result.
fn status_fn() -> crate::app::Status {
    Arc::new(|app: &AppWindow, text: String| show(app, text))
}

/// What the panels said about themselves.
/// Registered in `ctx.on_modules`, so it runs on the UI thread each time the firmware information is
/// read again. One line per panel; a panel that didn't answer counts as unknown firmware.
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
                format!("{}: {}", p.label(), long_name(&info))
            }
            .into(),
        );
    }
    app.global::<FirmwarePage>().set_installed(ModelRc::new(VecModel::from(lines)));
}

/// Fills the firmware choices and the "comes with this app" note, registers `show_modules`, and connects
/// the callbacks (see the file header). Runs once at start-up on the UI thread.
pub fn wire(ctx: &Rc<Ctx>, app: &AppWindow) {
    let page = app.global::<FirmwarePage>();
    let bundled = bundled_firmware(&releases_dir());
    page.set_bundled(
        match bundled_build(&releases_dir()) {
            Some(n) => format!("White Rabbit firmware build {n} comes with this app. To look for newer firmware, use Check for updates on the Settings page."),
            None => "To look for newer firmware, use Check for updates on the Settings page.".into(),
        }
        .into(),
    );
    let mut choices: Vec<SharedString> = bundled.iter().map(|(l, _)| l.as_str().into()).collect();
    choices.push(CHOOSE_FILE.into());
    page.set_choices(ModelRc::new(VecModel::from(choices)));
    ctx.on_modules.lock().unwrap().push(Box::new(show_modules));
    {
        let ctx = ctx.clone();
        // `refresh()`: show "Reading..." at once, then read the panels on a worker thread.
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
        // `sleep(on)`: the firmware turns its LEDs off (on = true) or back on.
        page.on_sleep(move |on| {
            ctx.act(move |s, p| s.write(&p.device, &[cmd::sleep(on)]), status_fn(), None, true, None, false);
        });
    }
    {
        let ctx = ctx.clone();
        // `bootloader()`: asks first, as the panels go dark until they are flashed or replugged.
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
                    // The panel restarts as a drive at once and usually can't answer, so a failed
                    // write is expected and ignored.
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
        // `flash(choice)`: check panels, pick the file, ask, then flash on a worker thread.
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
            // For a file the user chose, say in the question which White Rabbit build it holds, if any.
            let label = match (bundled.get(choice as usize), std::fs::read(&path).ok().and_then(|d| uf2_build(&d))) {
                (None, Some(n)) if n != 0 => format!("{label}\n(White Rabbit firmware build {n})"),
                _ => label,
            };
            let file = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            let where_ = panels.iter().map(|p| p.side.clone()).collect::<Vec<_>>().join(" and ");
            let plural = if panels.len() > 1 { "s" } else { "" };
            if !dialogs::confirm("Flash firmware", &format!("Flash {file} onto the {where_} panel{plural}?\n\n{label}"))
            {
                return;
            }
            show(&app, format!("Flashing {file}..."));
            // `busy` disables the buttons until the flashing is done, so it can't be started twice.
            app.global::<FirmwarePage>().set_busy(true);
            let done_text = format!("Flashed {file} onto the {where_} panel{plural}.");
            flash_panels(
                &ctx,
                panels,
                path,
                Arc::new(show_text),
                Arc::new(move |app: &AppWindow, text: String| {
                    show(app, if text == "Done." { done_text.clone() } else { text });
                    app.global::<FirmwarePage>().set_busy(false);
                }),
            );
        });
    }
}

/// `show` as a plain function, to pass as the progress callback.
fn show_text(app: &AppWindow, text: String) {
    show(app, text)
}

/// Flashes the firmware file onto these panels, one after the other. `progress` shows what's going
/// on, `done` gets "Done." or what went wrong. Afterwards the panels go back to the monitor and their
/// firmware is read again.
pub fn flash_panels(
    ctx: &Rc<Ctx>,
    panels: Vec<Panel>,
    path: PathBuf,
    progress: crate::app::Status,
    done: crate::app::Status,
) {
    let ui = ctx.ui.clone();
    let flashed = panels.clone();
    let done: crate::app::Status = Arc::new(move |app: &AppWindow, text: String| {
        done(app, text);
        // Give the panels back once they have restarted, then read their new versions
        let panels = flashed.clone();
        // 6 seconds after the last panel was flashed (each one already waited for its restart), give
        // the panels back and read their firmware again; this runs on a UI timer.
        crate::app::ctx().after(6.0, move |_| {
            let ctx = crate::app::ctx();
            ctx.clone_parts().give_back(panels, None);
            ctx.refresh_modules();
        });
    });
    ctx.act(
        move |s, p| {
            let ui = ui.clone();
            let progress = progress.clone();
            // `flash` runs on the worker thread; each progress text is sent to the UI thread to show.
            let status = move |text: String| {
                let progress = progress.clone();
                Ctx::later(&ui, move |app| progress(app, text))
            };
            flash(s, p, &path, &status).map(|_| ())
        },
        done,
        Some(panels),
        true,
        None,
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A .uf2 file holding `image` at 0x10000000, 256 bytes per block like elf2uf2 writes.
    /// The block fields written: both start magic numbers, the address, the payload size (256), the block
    /// number, the block count, the payload, and the end magic number at byte 508.
    fn uf2(image: &[u8]) -> Vec<u8> {
        let chunks: Vec<&[u8]> = image.chunks(256).collect();
        let mut out = Vec::new();
        for (i, c) in chunks.iter().enumerate() {
            let mut b = vec![0u8; UF2_BLOCK];
            b[0..4].copy_from_slice(&UF2_MAGIC[0].to_le_bytes());
            b[4..8].copy_from_slice(&UF2_MAGIC[1].to_le_bytes());
            b[12..16].copy_from_slice(&(0x1000_0000u32 + 256 * i as u32).to_le_bytes());
            b[16..20].copy_from_slice(&256u32.to_le_bytes());
            b[20..24].copy_from_slice(&(i as u32).to_le_bytes());
            b[24..28].copy_from_slice(&(chunks.len() as u32).to_le_bytes());
            b[32..32 + c.len()].copy_from_slice(c);
            b[508..512].copy_from_slice(&0x0AB1_6F30u32.to_le_bytes());
            out.extend(b);
        }
        out
    }

    /// The build number is found even when the tag spans two blocks; files without the tag, and data that
    /// isn't UF2, give None.
    #[test]
    fn build_in_uf2() {
        // The tag split across two blocks
        let mut image = vec![7u8; 250];
        image.extend_from_slice(b"WRFW-BUILD");
        image.extend_from_slice(&15u16.to_le_bytes());
        image.extend(vec![1u8; 300]);
        assert_eq!(uf2_build(&uf2(&image)), Some(15));
        assert_eq!(uf2_build(&uf2(&[3u8; 600])), None);
        assert_eq!(uf2_build(b"not a uf2 file"), None);
    }

    /// The firmware kinds get the right names, and only White Rabbit builds older than the newest (or older
    /// than build numbers) are offered an update; test builds and Framework's firmware are left alone.
    #[test]
    fn names_and_updates() {
        let wr = |b| ModuleInfo { version: Some("0.2.0".into()), build: b, startup: Some((0, 0, 0)), in_use: false };
        let framework = ModuleInfo { version: Some("0.2.0".into()), ..Default::default() };
        assert_eq!(long_name(&wr(Some(15))), "White Rabbit firmware build 15");
        assert_eq!(kind(&wr(None)), Kind::OlderWhiteRabbit);
        assert_eq!(long_name(&framework), "Framework's official firmware 0.2.0");
        assert!(needs_update(&wr(None), 15));
        assert!(needs_update(&wr(Some(14)), 15));
        assert!(!needs_update(&wr(Some(15)), 15));
        assert!(!needs_update(&wr(Some(0)), 15));
        assert!(!needs_update(&framework, 15));
        assert_eq!(tag_build("build-14"), Some(14));
        assert_eq!(tag_build("v1.2"), None);
    }
}
