//! HID access to the keyboard and macropad (hidapi).
//!
//! This is the layer between the message bytes of [`crate::protocol`] and the USB devices. It finds
//! the devices (by Framework's vendor id and the VIA raw HID usage page), opens them, sends 32 byte
//! reports and waits for the matching answers. It is used from the lighting engine's thread
//! (`hid-engine`, see [`crate::engine`]), which owns every [`Device`]; nothing here spawns threads or
//! keeps global state.
//!
//! The transport is hidden behind the [`Port`] trait so the same code drives either a real USB
//! device ([`HidPort`]) or the pretend devices of [`crate::sim`] (`--demo` and the tests).
//!
//! Errors: any USB error, a timeout or a refused command comes back as [`DeviceError`]. The engine
//! treats any error from a device as "it was unplugged": it closes it and scans for it again.
//!
//! Also here: [`set_fn_lock`], the "Top row" setting, which first repairs the top row's keymap so
//! Fn lock really swaps media keys and F1-F12 (see [`Keymap::top_row_fixes`]).

use crate::keycodes::{Board, Keymap};
use crate::protocol::{self as proto, DeviceInfo, DeviceKind, LedPoint, ProtocolError, Report, REPORT_LEN};
use hidapi::{HidApi, HidDevice};
use std::fmt;

/// How long to wait for one answer, in milliseconds. The devices answer within a few
/// milliseconds; 250 ms is long enough for a busy USB bus and short enough that an unplugged
/// device is noticed quickly (the engine's thread blocks while it waits).
const READ_TIMEOUT_MS: i32 = 250;

/// Why talking to a device failed. `Display` gives a short sentence for the window's status line.
#[derive(Debug)]
pub enum DeviceError {
    /// The USB layer failed (unplugged, access denied, write error).
    Hid(hidapi::HidError),
    /// The device answered, but with something unusable (see [`ProtocolError`]).
    Protocol(ProtocolError),
    /// No matching answer arrived in time.
    Timeout,
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hid(e) => write!(f, "USB error: {e}"),
            Self::Protocol(e) => write!(f, "{e}"),
            Self::Timeout => write!(f, "device did not answer"),
        }
    }
}

impl From<hidapi::HidError> for DeviceError {
    fn from(e: hidapi::HidError) -> Self {
        Self::Hid(e)
    }
}
impl From<ProtocolError> for DeviceError {
    fn from(e: ProtocolError) -> Self {
        Self::Protocol(e)
    }
}

/// Shorthand used by every function in this file.
pub type Result<T> = std::result::Result<T, DeviceError>;

/// Something that carries 32 byte reports to a device and back:
/// the real USB connection, or the simulator used by `--demo`.
pub trait Port {
    /// Sends one 32 byte report. Does not wait for an answer.
    fn write(&self, report: &Report) -> Result<()>;
    /// `None` = nothing arrived within the timeout.
    fn read(&self, timeout_ms: i32) -> Result<Option<Report>>;
}

/// A real device opened through hidapi.
struct HidPort(HidDevice);

impl Port for HidPort {
    fn write(&self, report: &Report) -> Result<()> {
        // hidapi wants the report id (0 = none) in front
        let mut buf = [0u8; REPORT_LEN + 1];
        buf[1..].copy_from_slice(report);
        self.0.write(&buf)?;
        Ok(())
    }

    // hidapi returns the number of bytes read; 0 means the timeout passed with nothing to read.
    fn read(&self, timeout_ms: i32) -> Result<Option<Report>> {
        let mut resp = [0u8; REPORT_LEN];
        let n = self.0.read_timeout(&mut resp, timeout_ms)?;
        Ok((n > 0).then_some(resp))
    }
}

/// A connected keyboard or macropad (VIA raw HID interface).
pub struct Device {
    /// Keyboard or macropad (from the USB product id; INFO confirms it).
    pub kind: DeviceKind,
    /// USB serial number (Framework writes one into each module at the factory).
    pub serial: Option<String>,
    /// Where the reports go: real USB or the simulator.
    port: Box<dyn Port>,
}

/// What `scan` found for one device type.
pub enum Found {
    /// Running our firmware.
    /// The open device, what INFO said, and its LED positions.
    Ready(Device, DeviceInfo, Vec<LedPoint>),
    /// Present but not running f16custom (stock firmware or other).
    Stock(DeviceKind),
}

impl Device {
    /// Wraps an already open port. The serial number is filled in by [`scan`] when hidapi knows it.
    pub fn new(kind: DeviceKind, port: Box<dyn Port>) -> Self {
        Self { kind, serial: None, port }
    }

    /// Sends a report without reading the answer. Used for the bootloader command (the device resets
    /// and never answers) and for clock sync, where answers are collected later.
    pub fn send(&self, req: &Report) -> Result<()> {
        self.port.write(req)
    }

    /// Waits for the answer to `req`: reads up to 8 reports, skipping answers that belong to other
    /// requests. A refused command (0xFF) is returned as an error at once; nothing at all within
    /// [`READ_TIMEOUT_MS`] is a timeout.
    fn recv_matching(&self, req: &Report) -> Result<Report> {
        // Skip stale answers (e.g. left over after a timeout) until ours shows up.
        for _ in 0..8 {
            let Some(resp) = self.port.read(READ_TIMEOUT_MS)? else {
                return Err(DeviceError::Timeout);
            };
            match proto::check_response(req, &resp) {
                Err(ProtocolError::Mismatch { .. }) => continue,
                Err(e) => return Err(e.into()),
                Ok(_) => return Ok(resp),
            }
        }
        Err(DeviceError::Timeout)
    }

    /// Sends one request and returns the 29 byte answer payload.
    /// Blocks the calling thread for up to 250 ms per read.
    pub fn transact(&self, req: &Report) -> Result<[u8; REPORT_LEN - 3]> {
        self.send(req)?;
        let resp = self.recv_matching(req)?;
        let mut payload = [0u8; REPORT_LEN - 3];
        payload.copy_from_slice(&resp[3..]);
        Ok(payload)
    }

    /// Sends several requests back to back, then collects the answers.
    /// Much faster than one round trip per message for streaming frames.
    /// The device queues its answers in order, so they are then read back one by one. Answers'
    /// payloads are thrown away; only "every message was accepted" matters. Stops at the first error.
    pub fn transact_many(&self, reqs: &[Report]) -> Result<()> {
        for r in reqs {
            self.send(r)?;
        }
        for r in reqs {
            self.recv_matching(r)?;
        }
        Ok(())
    }

    /// Reads the device's keymap (VIA dynamic keymap, the layers the app edits).
    /// About 37 round trips for the keyboard (4 layers x 256 bytes in 28 byte chunks), 5 for the
    /// macropad. Fails if any read fails or the buffer comes back too short.
    pub fn keymap(&self) -> Result<Keymap> {
        let board = match self.kind {
            DeviceKind::Keyboard => Board::Keyboard,
            DeviceKind::Macropad => Board::Macropad,
        };
        // Read the whole buffer (every layer the app edits, back to back) in 28 byte chunks.
        let mut buf = Vec::new();
        for r in proto::keymap_buffer_requests(board.layer_bytes() * board.layers() as usize) {
            let p = self.transact(&r)?;
            buf.extend_from_slice(proto::parse_keymap_buffer(&p));
        }
        Keymap::from_buffer(board, &buf).ok_or(DeviceError::Protocol(ProtocolError::Invalid("short keymap")))
    }

    /// Macropad corner combos; None on firmware without them.
    /// "Unsupported" is a normal answer here (a keyboard, or older firmware), not an error.
    pub fn combos(&self) -> Result<Option<proto::Combos>> {
        match self.transact(&proto::combos_request()) {
            Ok(p) => Ok(Some(proto::parse_combos(&p))),
            Err(DeviceError::Protocol(ProtocolError::Unsupported)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Whether the keyboard's F12 shortcuts are on; None if the firmware has none.
    pub fn shortcuts(&self) -> Result<Option<bool>> {
        match self.transact(&proto::shortcuts_request()) {
            Ok(p) => Ok(Some(p[0] != 0)),
            Err(DeviceError::Protocol(ProtocolError::Unsupported)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// LED calibration; None on firmware without it.
    pub fn led_cal(&self) -> Result<Option<proto::LedCal>> {
        match self.transact(&proto::led_cal_request()) {
            Ok(p) => Ok(Some(proto::parse_led_cal(&p))),
            Err(DeviceError::Protocol(ProtocolError::Unsupported)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Asks the device for INFO and decodes it. Fails on stock firmware (no "FX" magic or 0xFF).
    pub fn info(&self) -> Result<DeviceInfo> {
        let p = self.transact(&proto::info_request())?;
        Ok(proto::parse_info(&p)?)
    }

    /// LED positions. For the keyboard and macropad these come from Framework's
    /// source (built into the app) rather than the device: while the devices are
    /// joined, the firmware squeezes its positions into its slice of the shared
    /// surface, and matching keys against those put painted keys on the wrong LEDs.
    ///
    /// When the device reports an LED count that the built-in tables don't have (a different
    /// firmware), the positions are read over USB with LED_INFO instead.
    pub fn led_points(&self, count: u8) -> Result<Vec<LedPoint>> {
        let known: &[(u8, u8)] = match self.kind {
            DeviceKind::Keyboard => &crate::keys::KEYBOARD_LEDS,
            DeviceKind::Macropad => &crate::keys::MACROPAD_LEDS,
        };
        // The count check guards against firmware with a different LED table: only use the built-in
        // positions when they cover exactly the LEDs the device says it has.
        if known.len() == count as usize {
            return Ok(crate::keys::firmware_leds(known));
        }
        let mut out = Vec::with_capacity(count as usize);
        for req in proto::led_info_requests(count) {
            out.extend(proto::parse_led_info(&self.transact(&req)?));
        }
        // The last LED_INFO answer may carry fewer LEDs; truncate in case a device reports extras.
        out.truncate(count as usize);
        Ok(out)
    }
}

/// Opens the raw HID interface of one device type without talking to it.
/// Used to send the bootloader command to a device that is not open (for example one running
/// stock firmware, which [`scan`] reports but does not keep open). None if it isn't plugged in or
/// can't be opened.
pub fn open_raw(api: &mut HidApi, want: DeviceKind) -> Option<Device> {
    // A failed refresh just leaves the old list; opening below then fails or finds nothing.
    let _ = api.refresh_devices();
    api.device_list()
        .filter(|d| {
            d.vendor_id() == proto::VID
                && d.usage_page() == proto::RAW_USAGE_PAGE
                && d.usage() == proto::RAW_USAGE
                && DeviceKind::from_pid(d.product_id()) == Some(want)
        })
        // On Windows one physical device shows up as several HID interfaces; the filter above keeps
        // only the raw HID one, and the first that opens wins.
        .find_map(|d| d.open_device(api).ok())
        .map(|hid| Device::new(want, Box::new(HidPort(hid))))
}

/// Finds the keyboard and macropad raw HID interfaces, skipping kinds in `skip`
/// (devices that are already open).
///
/// Each device found is opened and asked for INFO ([`identify`]), so this blocks for a few round
/// trips per device. A device that can't be opened (another program holds it, or Windows refuses
/// access) is silently skipped and will be tried again on the next scan.
pub fn scan(api: &mut HidApi, skip: &[DeviceKind]) -> Vec<Found> {
    if let Err(e) = api.refresh_devices() {
        eprintln!("HID refresh failed: {e}");
    }
    let mut found = Vec::new();
    let mut seen = Vec::new();
    for d in api.device_list() {
        // Only Framework devices, and only their raw HID (VIA) interface.
        if d.vendor_id() != proto::VID || d.usage_page() != proto::RAW_USAGE_PAGE || d.usage() != proto::RAW_USAGE {
            continue;
        }
        let Some(kind) = DeviceKind::from_pid(d.product_id()) else {
            continue;
        };
        if seen.contains(&kind) || skip.contains(&kind) {
            continue; // only one of each
        }
        let Ok(hid) = d.open_device(api) else {
            continue;
        };
        seen.push(kind);
        let mut dev = Device::new(kind, Box::new(HidPort(hid)));
        dev.serial = d.serial_number().map(str::to_string).filter(|s| !s.is_empty());
        found.push(identify(dev));
    }
    found
}

/// Framework's numpad (white backlight). The app doesn't talk to it, it only
/// notices that it is plugged in.
pub const PID_NUMPAD: u16 = 0x0014;

/// Whether a Framework numpad is plugged in.
/// Uses hidapi's device list as of the last refresh (call after [`scan`]).
pub fn numpad_present(api: &HidApi) -> bool {
    api.device_list().any(|d| d.vendor_id() == proto::VID && d.product_id() == PID_NUMPAD)
}

/// Whether a Framework keyboard, macropad or numpad is plugged in (any firmware), without opening it.
/// Creates its own hidapi context, so it can be called from any thread, but it is slow-ish
/// (enumerates every HID device); callers use it for occasional checks only.
pub fn any_present() -> bool {
    let Ok(api) = HidApi::new() else {
        return false;
    };
    // Any interface of the right product counts; nothing is opened.
    let present = api.device_list().any(|d| {
        d.vendor_id() == proto::VID && (DeviceKind::from_pid(d.product_id()).is_some() || d.product_id() == PID_NUMPAD)
    });
    present
}

/// What happened when the Top row setting was changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FnLockResult {
    /// The keyboard now reports the Fn lock that was asked for.
    /// False when the firmware answered "unsupported" or reads back a different value afterwards.
    pub took: bool,
    /// Top-row keys put back the right way round first (see `Keymap::top_row_fixes`).
    pub fixed_keys: usize,
}

/// Sets the keyboard's Fn lock: `true` = the top row sends F1-F12 on its own (shortcuts with
/// Fn), `false` = shortcuts on their own (F1-F12 with Fn). Same as Fn + Esc, and saved on the
/// keyboard. First puts the top row's two jobs where Fn lock expects them, so the choice works
/// even when the keymap got mixed up. Reads the keyboard back to check it took.
///
/// How the keyboard does Fn lock: the firmware's keymap has four layers (0 base, 1 Fn, 2 Fn lock,
/// 3 Fn while locked). Fn lock makes layer 2 the default layer instead of 0, and layer 2 holds the
/// Fn layer's top row, so the top row's two jobs swap. That only works if layer 0 holds the
/// shortcut and layer 1 the F-key; if a key has them the wrong way round, both settings send the
/// same thing. Hence the repair before the switch.
///
/// `keymap` is the app's copy of the keyboard's keymap; it is updated to match what was written.
/// Without it (keymap could not be read) only the switch is made. Errors from USB are returned;
/// "unsupported" (old firmware) is reported as `took: false` instead.
pub fn set_fn_lock(dev: &Device, keymap: Option<&mut Keymap>, on: bool) -> Result<FnLockResult> {
    let mut fixed_keys = 0;
    // Step 1: repair the top row in the keymap if needed, so the switch has the right effect.
    if let Some(km) = keymap {
        let fixes = km.top_row_fixes();
        if !fixes.is_empty() {
            let reqs: Vec<Report> = fixes.iter().map(|&(l, r, c, code)| proto::set_keycode(l, r, c, code)).collect();
            dev.transact_many(&reqs)?;
            // Keep the app's copy in step with what the keyboard now holds.
            km.apply(&fixes);
            // One key usually needs writes on several layers; count keys, not writes, for the message.
            let mut keys: Vec<(u8, u8)> = fixes.iter().map(|&(_, r, c, _)| (r, c)).collect();
            keys.sort_unstable();
            keys.dedup();
            fixed_keys = keys.len();
        }
    }
    // Step 2: flip Fn lock (LAYER 0/1), exactly what Fn + Esc does on the keyboard.
    match dev.transact(&proto::set_layer(on as u8)) {
        Ok(_) => {}
        // Keyboard firmware from before Fn lock could be set from the app
        Err(DeviceError::Protocol(ProtocolError::Unsupported)) => return Ok(FnLockResult { took: false, fixed_keys }),
        Err(e) => return Err(e),
    }
    // Step 3: read it back. A keyboard that accepted the command but kept the old value says
    // "didn't take", which the window turns into "update the firmware".
    let took = dev.info()?.layer == Some(on as u8);
    Ok(FnLockResult { took, fixed_keys })
}

/// Asks a freshly opened device whether it runs our firmware.
/// Any failure (no answer, 0xFF, wrong magic or version, LED positions unreadable) counts as
/// "not our firmware": the window then offers to flash it. Consumes the device; a stock device
/// is dropped (closed) here.
pub fn identify(dev: Device) -> Found {
    let kind = dev.kind;
    match dev.info() {
        Ok(info) => match dev.led_points(info.led_count) {
            Ok(leds) => Found::Ready(dev, info, leds),
            Err(_) => Found::Stock(kind),
        },
        Err(_) => Found::Stock(kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keycodes::app_code;
    use crate::sim::SimPort;

    /// Keymap reads and writes, Fn lock and the macropad combos all work end to end through
    /// `Device` against the simulator.
    #[test]
    fn keymap_and_combos_through_the_pretend_devices() {
        let kb = Device::new(DeviceKind::Keyboard, Box::new(SimPort::keyboard()));
        let km = kb.keymap().unwrap();
        assert_eq!(km, Keymap::defaults(Board::Keyboard));
        // Fn + F12 opens app 1: written to the Fn and Fn-lock layers
        let writes = km.writes(1, 12, Some(app_code(0)));
        let reqs: Vec<Report> = writes.iter().map(|&(l, r, c, code)| proto::set_keycode(l, r, c, code)).collect();
        kb.transact_many(&reqs).unwrap();
        let km = kb.keymap().unwrap();
        assert_eq!(km.code(1, 12), app_code(0));
        assert_eq!(km.layers[2][12], app_code(0));
        assert_eq!(km.code(0, 12), Keymap::defaults(Board::Keyboard).code(0, 12));
        assert_eq!(kb.combos().unwrap(), None);
        // Fn lock: read and set through the LAYER command, like Fn + Esc
        assert_eq!(kb.info().unwrap().layer, Some(0));
        kb.transact(&proto::set_layer(1)).unwrap();
        assert_eq!(kb.info().unwrap().layer, Some(1));

        let pad = Device::new(DeviceKind::Macropad, Box::new(SimPort::macropad()));
        assert_eq!(pad.keymap().unwrap(), Keymap::defaults(Board::Macropad));
        assert_eq!(pad.combos().unwrap(), Some(proto::Combos::default()));
        let c = proto::Combos {
            top_on: false,
            bottom_on: true,
            top_action: 0,
            bottom_action: app_code(3),
            corner_lights: true,
            top_row: true,
        };
        pad.transact(&proto::set_combos(&c)).unwrap();
        assert_eq!(pad.combos().unwrap(), Some(c));
    }
}
