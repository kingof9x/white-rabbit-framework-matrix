//! Updating a device's firmware from the app, the way White Rabbit does it:
//! get the .uf2 file (the newest build on GitHub, or one on this computer), check it is
//! firmware for that device, restart the device into flashing mode, wait for the RPI-RP2 drive
//! it shows up as, and copy the file onto it. The device installs it and restarts by itself.
//!
//! Background for a reader new to this: the keyboard and macropad each run on a Raspberry Pi RP2040
//! chip. The RP2040 has a fixed boot program in its read-only memory. When told to (here: by a USB HID
//! message, see `wr_keyboard::protocol::bootloader_request`, Framework's own "reset into bootloader"
//! command 0x0B 0xFE), the chip restarts into that boot program, which pretends to be a USB memory
//! stick called RPI-RP2 holding an INFO_UF2.TXT file. Copying a .uf2 file onto that drive writes the
//! firmware into the chip's flash; once the last block arrives the chip restarts into the new firmware
//! and the drive disappears. No driver or special tool is needed, which is why this is all plain file
//! copying.
//!
//! Where it runs: `run` is started on its own thread by `State::start_flash` in `keyboard/mod.rs` and
//! blocks (downloads, sleeps, polls drives). It never touches the window directly: it reports through
//! the `report` callback, which pushes `Progress` messages into a mutex-protected inbox and wakes the
//! UI thread, which shows them (`State::flash_answers`). The restart request is also indirect: the
//! `restart` callback sends `Command::Bootloader` to the keyboard engine thread, which owns the USB
//! handles.
//!
//! Safety checks before anything is written: the file must be a well-formed UF2 for the RP2040 family,
//! and the product name compiled into it must match the device (ANSI keyboard vs. macropad), so the
//! keyboard is never given macropad firmware or the ISO keyboard's firmware.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use wr_keyboard::protocol::DeviceKind;

/// Where the firmware comes from.
///
/// Chosen by which Firmware page button was pressed: "Update firmware" uses `Latest`, "Install a
/// file…" uses `File`.
#[derive(Debug, Clone)]
pub enum Source {
    /// The file for this device in the newest firmware build on GitHub.
    Latest,
    /// A .uf2 file on this computer.
    File(PathBuf),
}

/// How to reach flashing mode.
///
/// `Drive` is used when the page has already spotted a device sitting in flashing mode (for example
/// after an earlier attempt stopped half way, or after the device was put into flashing mode by hand).
#[derive(Debug, Clone)]
pub enum Target {
    /// The device is running: `restart` asks it to restart into flashing mode.
    Running,
    /// The device is already waiting in flashing mode as this drive.
    Drive(PathBuf),
}

/// What the flashing thread reports back to the window.
///
/// The thread sends any number of `Step`s, then exactly one `Done` or `Failed`, then stops. The text is
/// already a full sentence for the page's status line.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    /// Something is happening now ("Downloading…", "Copying…"); replaces the previous status text.
    Step(String),
    /// The update finished; the device restarted with the new firmware.
    Done(String),
    /// The update stopped; the text says why and what the person can do.
    Failed(String),
}

/// The release file for each device.
///
/// These names must match the files the keyboard repository's build attaches to its releases
/// (see `update::pick`, which looks them up by exact name).
pub fn release_file(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Keyboard => "framework_ansi_f16custom.uf2",
        DeviceKind::Macropad => "framework_macropad_f16custom.uf2",
    }
}

/// The first start magic number (byte 0 of every block) of the UF2 file format, Microsoft's format
/// for flashing chips by copying a file.
///
/// A UF2 file is a run of 512-byte blocks. Each block, read as little-endian 32-bit numbers, holds:
/// bytes 0 and 4 the two start magics; byte 8 flags; byte 12 the flash address this block is for;
/// byte 16 the payload size; byte 20 the block number; byte 24 the total number of blocks; byte 28 the
/// chip family ID (when the family flag is set); bytes 32 to 507 the payload (up to 476 bytes, in
/// practice 256 for the RP2040); and bytes 508 to 511 the end magic.
const UF2_MAGIC_START0: u32 = 0x0A32_4655;
/// The second start magic number, at byte 4 of every block.
const UF2_MAGIC_START1: u32 = 0x9E5D_5157;
/// The magic number in the last 4 bytes of every block.
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;
/// Flag bit meaning "the word at byte 28 is a chip family ID" (rather than a file size).
const UF2_FAMILY_PRESENT: u32 = 0x0000_2000;
/// The family ID registered for the RP2040; the boot program ignores blocks for other families.
const RP2040_FAMILY: u32 = 0xE48B_FF56;

/// Reads the little-endian 32-bit number at byte `at` of `b`. Panics if fewer than 4 bytes are left,
/// so callers only use it on whole 512-byte blocks.
fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The bytes a UF2 file puts into the chip's memory (its blocks' payloads, in order).
///
/// The payloads are joined in file order without looking at their flash addresses. That is enough
/// for what callers do with the result (search it for text and the build marker); it is not a
/// faithful memory image if a file had gaps or blocks out of order. Any bad block (wrong length,
/// wrong magic, no family or the wrong family) rejects the whole file with a sentence for the page.
fn uf2_payload(data: &[u8]) -> Result<Vec<u8>, String> {
    // A real UF2 is always a whole number of 512-byte blocks; this also guarantees every `le32` below
    // stays inside its block.
    if data.is_empty() || !data.len().is_multiple_of(512) {
        return Err("That is not a .uf2 firmware file.".into());
    }
    // Each 512-byte block normally carries 256 bytes of payload, so half the file size is a good guess.
    let mut out = Vec::with_capacity(data.len() / 2);
    for block in data.chunks(512) {
        if le32(block, 0) != UF2_MAGIC_START0 || le32(block, 4) != UF2_MAGIC_START1 || le32(block, 508) != UF2_MAGIC_END
        {
            return Err("That is not a .uf2 firmware file.".into());
        }
        if le32(block, 8) & UF2_FAMILY_PRESENT == 0 || le32(block, 28) != RP2040_FAMILY {
            return Err("That .uf2 file is not for the Framework 16's chip (RP2040).".into());
        }
        // Clamp the payload size to the 476 bytes a block has room for, so a corrupt size field can't
        // make the slice run past the block.
        let size = (le32(block, 16) as usize).min(476);
        out.extend_from_slice(&block[32..32 + size]);
    }
    Ok(out)
}

/// The build number in a .uf2 file (None: not a .uf2, or firmware from before build numbers).
///
/// The firmware embeds the text `F16FW-BUILD` followed by a 2-byte little-endian number (0 means a
/// test build); `protocol::image_build` searches the payload for it. Used by `update::newest` to read
/// the build of the file on GitHub.
pub fn uf2_build(data: &[u8]) -> Option<wr_keyboard::protocol::FirmwareBuild> {
    wr_keyboard::protocol::image_build(&uf2_payload(data).ok()?)
}

/// Whether `hay` contains `needle` written as UTF-16 little-endian (2 bytes per character), which is
/// how USB descriptor strings such as the product name are stored inside the firmware. A plain byte
/// search for "ANSI" would miss it because each letter is followed by a zero byte.
fn contains_utf16(hay: &[u8], needle: &str) -> bool {
    let n: Vec<u8> = needle.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    hay.windows(n.len()).any(|w| w == n.as_slice())
}

/// Checks that `data` is RP2040 firmware for `kind`, using the product name the firmware
/// gives itself over USB ("... Keyboard Module - ANSI" or "... RGB Macropad ...").
///
/// Returns a sentence for the page on failure. Order matters in the match: a file that names the
/// other device is rejected with "that's the other device's firmware" before the more general "not for
/// this device" messages. The ISO keyboard's firmware says "ISO" instead of "ANSI", so it is refused
/// for the ANSI keyboard this app supports.
pub fn check_uf2(data: &[u8], kind: DeviceKind) -> Result<(), String> {
    let payload = uf2_payload(data)?;
    let ansi = contains_utf16(&payload, "ANSI");
    let macropad = contains_utf16(&payload, "Macropad");
    match kind {
        DeviceKind::Keyboard if macropad => Err("That file is macropad firmware, not keyboard firmware.".into()),
        DeviceKind::Keyboard if !ansi => Err("That file is not firmware for the ANSI (US layout) keyboard.".into()),
        DeviceKind::Macropad if ansi => Err("That file is keyboard firmware, not macropad firmware.".into()),
        DeviceKind::Macropad if !macropad => Err("That file is not firmware for the macropad.".into()),
        _ => Ok(()),
    }
}

/// Drives that are a device waiting in flashing mode (they hold an INFO_UF2.TXT file).
///
/// Looks at every drive letter A: to Z: for an INFO_UF2.TXT file in its root, which is the RP2040
/// boot program's signature. Cheap enough to call every couple of seconds (the Firmware page does,
/// while it is open). The order is by drive letter.
#[cfg(windows)]
pub fn waiting_drives() -> Vec<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;
    // GetLogicalDrives returns a bitmask: bit 0 set means drive A: exists, bit 1 B:, and so on.
    // It takes no pointers and only reads system state, so there is nothing that could be misused;
    // on failure it returns 0, which simply means no drives are listed.
    // SAFETY: plain call with no arguments.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i as u8) as char)))
        // A drive that is present but empty (a card reader with no card) just fails this check.
        .filter(|d| d.join("INFO_UF2.TXT").is_file())
        .collect()
}

/// Away from Windows there are no drive letters to look at, so no device is ever found waiting.
#[cfg(not(windows))]
pub fn waiting_drives() -> Vec<PathBuf> {
    vec![]
}

/// "E:" for a drive's root.
///
/// Drops the trailing backslash or slash so the name reads naturally in a sentence.
pub fn drive_name(d: &Path) -> String {
    d.to_string_lossy().trim_end_matches(['\\', '/']).to_string()
}

/// Gets the firmware bytes and a name for them to use in sentences: the file name for a local file, or
/// the release label ("the newest firmware (…)") for a download. Blocking (reads disk or network).
fn get_firmware(kind: DeviceKind, source: &Source) -> Result<(Vec<u8>, String), String> {
    match source {
        Source::File(p) => {
            let data = std::fs::read(p).map_err(|e| format!("Could not read that file: {e}"))?;
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            Ok((data, name))
        }
        Source::Latest => {
            let (label, data) = super::update::latest_asset(release_file(kind))?;
            Ok((data, label))
        }
    }
}

/// Waits up to `limit` for `f` to give something.
///
/// Polls `f` every 250 ms and returns its first `Some`, or None once more than `limit` has passed.
/// `f` is always tried at least once. Blocks the calling thread, so only the flashing thread uses it.
fn wait_for<T>(limit: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if start.elapsed() > limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Does the whole update; runs on its own thread. `restart` asks the running device to restart
/// into flashing mode. `report` is called with each step and once with Done or Failed.
///
/// `demo` (the app's --demo mode, used for screenshots) goes through the same steps with pauses and
/// no real file, device or network. Nothing is written to a device until the file has passed
/// `check_uf2`. Every way out of this function calls `report` with `Done` or `Failed` exactly once.
pub fn run(
    kind: DeviceKind,
    source: Source,
    target: Target,
    demo: bool,
    restart: impl Fn(),
    report: impl Fn(Progress),
) {
    let name = kind.name().to_lowercase();
    let step = |s: String| report(Progress::Step(s));

    step(match source {
        Source::Latest => "Downloading the latest firmware from GitHub…".into(),
        Source::File(_) => "Reading the file…".into(),
    });
    // Step 1: get the file and check it is firmware for this device.
    let (data, what) = if demo {
        std::thread::sleep(Duration::from_millis(800));
        (vec![], "version 9.9.9 (demo)".to_string())
    } else {
        match get_firmware(kind, &source).and_then(|(d, w)| check_uf2(&d, kind).map(|_| (d, w))) {
            Ok(v) => v,
            Err(e) => return report(Progress::Failed(e)),
        }
    };

    // Step 2: find the RPI-RP2 drive to copy to.
    let drive = match target {
        Target::Drive(d) => d,
        Target::Running if demo => {
            step(format!("Restarting the {name} into flashing mode…"));
            restart();
            std::thread::sleep(Duration::from_millis(1500));
            PathBuf::from("E:\\")
        }
        Target::Running => {
            // Remember drives that were already waiting, so a different device left in flashing mode
            // isn't mistaken for this one: only a drive that appears after the restart counts.
            let before = waiting_drives();
            step(format!("Restarting the {name} into flashing mode…"));
            restart();
            // Windows usually mounts the drive within a few seconds; 20 s leaves room for a slow USB
            // re-enumeration (and for Windows asking what to do with the new drive).
            match wait_for(Duration::from_secs(20), || waiting_drives().into_iter().find(|d| !before.contains(d))) {
                Some(d) => d,
                None => {
                    return report(Progress::Failed(format!(
                        "The {name} did not show up as an RPI-RP2 drive. If it did restart, press \
                         \"Search again\" below, or copy the .uf2 file onto the drive yourself."
                    )))
                }
            }
        }
    };

    // Step 3: copy the file. The boot program accepts any file name ending in .uf2.
    step(format!("Copying {what} to the {name} (drive {})…", drive_name(&drive)));
    if demo {
        std::thread::sleep(Duration::from_millis(1500));
    } else if let Err(e) = std::fs::write(drive.join("NEW.UF2"), &data) {
        // The device restarts as soon as it has the last block, which can cut the copy's
        // last step short: that is fine if the drive has gone.
        std::thread::sleep(Duration::from_secs(1));
        if drive.join("INFO_UF2.TXT").is_file() {
            return report(Progress::Failed(format!(
                "Could not copy the firmware to drive {}: {e}",
                drive_name(&drive)
            )));
        }
    }

    // Step 4: wait for the drive to vanish, which means the chip has restarted into the new firmware.
    // The result of this wait is ignored: if the drive is still there after 15 s, Done is reported
    // anyway. The 3 s pause after that gives the device time to come back on USB before the window
    // rescans for it.
    step(format!("Installing on the {name}; it restarts by itself…"));
    if !demo {
        wait_for(Duration::from_secs(15), || (!drive.join("INFO_UF2.TXT").is_file()).then_some(()));
    }
    std::thread::sleep(Duration::from_secs(3));
    report(Progress::Done(format!("The {name} now has {what}.")));
}

/// Tests for the UF2 checks, using small UF2 files built in memory.
#[cfg(test)]
mod tests {
    use super::*;

    /// A UF2 file whose payload holds `text` as a USB (UTF-16) string.
    ///
    /// It is 512 bytes of payload (300 zero bytes, the text, then zeros) split into two 256-byte blocks
    /// with every header field filled in the way a real RP2040 UF2 does (flash starts at 0x1000_0000).
    fn fake_uf2(text: &str, family: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 300];
        payload.extend(text.encode_utf16().flat_map(|c| c.to_le_bytes()));
        payload.resize(512, 0);
        let mut out = vec![];
        for (i, chunk) in payload.chunks(256).enumerate() {
            let mut b = vec![0u8; 512];
            for (at, v) in [
                (0, UF2_MAGIC_START0),
                (4, UF2_MAGIC_START1),
                (8, UF2_FAMILY_PRESENT),
                (12, 0x1000_0000 + 256 * i as u32),
                (16, 256),
                (20, i as u32),
                (24, 2),
                (28, family),
                (508, UF2_MAGIC_END),
            ] {
                b[at..at + 4].copy_from_slice(&v.to_le_bytes());
            }
            b[32..32 + chunk.len()].copy_from_slice(chunk);
            out.extend(b);
        }
        out
    }

    /// Keyboard, macropad and ISO keyboard firmware are each accepted only for the right device.
    #[test]
    fn tells_the_devices_apart() {
        let kb = fake_uf2("Laptop 16 Keyboard Module - ANSI", RP2040_FAMILY);
        let pad = fake_uf2("Laptop 16 RGB Macropad Module", RP2040_FAMILY);
        let iso = fake_uf2("Laptop 16 Keyboard Module - ISO", RP2040_FAMILY);
        assert!(check_uf2(&kb, DeviceKind::Keyboard).is_ok());
        assert!(check_uf2(&pad, DeviceKind::Macropad).is_ok());
        assert!(check_uf2(&kb, DeviceKind::Macropad).is_err());
        assert!(check_uf2(&pad, DeviceKind::Keyboard).is_err());
        assert!(check_uf2(&iso, DeviceKind::Keyboard).is_err());
    }

    /// Text, a zero-filled file of the right length, and a UF2 for another chip family are all refused.
    #[test]
    fn refuses_other_files() {
        assert!(check_uf2(b"hello", DeviceKind::Keyboard).is_err());
        assert!(check_uf2(&[0u8; 1024], DeviceKind::Keyboard).is_err());
        let other_chip = fake_uf2("Keyboard Module - ANSI", 0x1234_5678);
        assert!(check_uf2(&other_chip, DeviceKind::Keyboard).is_err());
    }

    /// A drive root "E:\" is shown as "E:".
    #[test]
    fn drive_names() {
        assert_eq!(drive_name(Path::new("E:\\")), "E:");
    }
}
