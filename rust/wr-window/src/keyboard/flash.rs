//! Updating a device's firmware from the app, the way White Rabbit does it:
//! get the .uf2 file (the newest build on GitHub, or one on this computer), check it is
//! firmware for that device, restart the device into flashing mode, wait for the RPI-RP2 drive
//! it shows up as, and copy the file onto it. The device installs it and restarts by itself.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use wr_keyboard::protocol::DeviceKind;

/// Where the firmware comes from.
#[derive(Debug, Clone)]
pub enum Source {
    /// The file for this device in the newest firmware build on GitHub.
    Latest,
    /// A .uf2 file on this computer.
    File(PathBuf),
}

/// How to reach flashing mode.
#[derive(Debug, Clone)]
pub enum Target {
    /// The device is running: `restart` asks it to restart into flashing mode.
    Running,
    /// The device is already waiting in flashing mode as this drive.
    Drive(PathBuf),
}

/// What the flashing thread reports back to the window.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Step(String),
    Done(String),
    Failed(String),
}

/// The release file for each device.
pub fn release_file(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Keyboard => "framework_ansi_f16custom.uf2",
        DeviceKind::Macropad => "framework_macropad_f16custom.uf2",
    }
}

const UF2_MAGIC_START0: u32 = 0x0A32_4655;
const UF2_MAGIC_START1: u32 = 0x9E5D_5157;
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;
const UF2_FAMILY_PRESENT: u32 = 0x0000_2000;
const RP2040_FAMILY: u32 = 0xE48B_FF56;

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The bytes a UF2 file puts into the chip's memory (its blocks' payloads, in order).
fn uf2_payload(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.is_empty() || !data.len().is_multiple_of(512) {
        return Err("That is not a .uf2 firmware file.".into());
    }
    let mut out = Vec::with_capacity(data.len() / 2);
    for block in data.chunks(512) {
        if le32(block, 0) != UF2_MAGIC_START0 || le32(block, 4) != UF2_MAGIC_START1 || le32(block, 508) != UF2_MAGIC_END
        {
            return Err("That is not a .uf2 firmware file.".into());
        }
        if le32(block, 8) & UF2_FAMILY_PRESENT == 0 || le32(block, 28) != RP2040_FAMILY {
            return Err("That .uf2 file is not for the Framework 16's chip (RP2040).".into());
        }
        let size = (le32(block, 16) as usize).min(476);
        out.extend_from_slice(&block[32..32 + size]);
    }
    Ok(out)
}

/// The build number in a .uf2 file (None: not a .uf2, or firmware from before build numbers).
pub fn uf2_build(data: &[u8]) -> Option<wr_keyboard::protocol::FirmwareBuild> {
    wr_keyboard::protocol::image_build(&uf2_payload(data).ok()?)
}

fn contains_utf16(hay: &[u8], needle: &str) -> bool {
    let n: Vec<u8> = needle.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    hay.windows(n.len()).any(|w| w == n.as_slice())
}

/// Checks that `data` is RP2040 firmware for `kind`, using the product name the firmware
/// gives itself over USB ("... Keyboard Module - ANSI" or "... RGB Macropad ...").
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
#[cfg(windows)]
pub fn waiting_drives() -> Vec<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i as u8) as char)))
        .filter(|d| d.join("INFO_UF2.TXT").is_file())
        .collect()
}

#[cfg(not(windows))]
pub fn waiting_drives() -> Vec<PathBuf> {
    vec![]
}

/// "E:" for a drive's root.
pub fn drive_name(d: &Path) -> String {
    d.to_string_lossy().trim_end_matches(['\\', '/']).to_string()
}

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
    let (data, what) = if demo {
        std::thread::sleep(Duration::from_millis(800));
        (vec![], "version 9.9.9 (demo)".to_string())
    } else {
        match get_firmware(kind, &source).and_then(|(d, w)| check_uf2(&d, kind).map(|_| (d, w))) {
            Ok(v) => v,
            Err(e) => return report(Progress::Failed(e)),
        }
    };

    let drive = match target {
        Target::Drive(d) => d,
        Target::Running if demo => {
            step(format!("Restarting the {name} into flashing mode…"));
            restart();
            std::thread::sleep(Duration::from_millis(1500));
            PathBuf::from("E:\\")
        }
        Target::Running => {
            let before = waiting_drives();
            step(format!("Restarting the {name} into flashing mode…"));
            restart();
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

    step(format!("Installing on the {name}; it restarts by itself…"));
    if !demo {
        wait_for(Duration::from_secs(15), || (!drive.join("INFO_UF2.TXT").is_file()).then_some(()));
    }
    std::thread::sleep(Duration::from_secs(3));
    report(Progress::Done(format!("The {name} now has {what}.")));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A UF2 file whose payload holds `text` as a USB (UTF-16) string.
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

    #[test]
    fn refuses_other_files() {
        assert!(check_uf2(b"hello", DeviceKind::Keyboard).is_err());
        assert!(check_uf2(&[0u8; 1024], DeviceKind::Keyboard).is_err());
        let other_chip = fake_uf2("Keyboard Module - ANSI", 0x1234_5678);
        assert!(check_uf2(&other_chip, DeviceKind::Keyboard).is_err());
    }

    #[test]
    fn drive_names() {
        assert_eq!(drive_name(Path::new("E:\\")), "E:");
    }
}
