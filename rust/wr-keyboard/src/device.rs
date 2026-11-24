//! HID access to the keyboard and macropad (hidapi).

use crate::keycodes::{Board, Keymap};
use crate::protocol::{self as proto, DeviceInfo, DeviceKind, LedPoint, ProtocolError, Report, REPORT_LEN};
use hidapi::{HidApi, HidDevice};
use std::fmt;

const READ_TIMEOUT_MS: i32 = 250;

#[derive(Debug)]
pub enum DeviceError {
    Hid(hidapi::HidError),
    Protocol(ProtocolError),
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

pub type Result<T> = std::result::Result<T, DeviceError>;

/// Something that carries 32 byte reports to a device and back:
/// the real USB connection, or the simulator used by `--demo`.
pub trait Port {
    fn write(&self, report: &Report) -> Result<()>;
    /// `None` = nothing arrived within the timeout.
    fn read(&self, timeout_ms: i32) -> Result<Option<Report>>;
}

struct HidPort(HidDevice);

impl Port for HidPort {
    fn write(&self, report: &Report) -> Result<()> {
        // hidapi wants the report id (0 = none) in front
        let mut buf = [0u8; REPORT_LEN + 1];
        buf[1..].copy_from_slice(report);
        self.0.write(&buf)?;
        Ok(())
    }

    fn read(&self, timeout_ms: i32) -> Result<Option<Report>> {
        let mut resp = [0u8; REPORT_LEN];
        let n = self.0.read_timeout(&mut resp, timeout_ms)?;
        Ok((n > 0).then_some(resp))
    }
}

/// A connected keyboard or macropad (VIA raw HID interface).
pub struct Device {
    pub kind: DeviceKind,
    /// USB serial number (Framework writes one into each module at the factory).
    pub serial: Option<String>,
    port: Box<dyn Port>,
}

/// What `scan` found for one device type.
pub enum Found {
    /// Running our firmware.
    Ready(Device, DeviceInfo, Vec<LedPoint>),
    /// Present but not running f16custom (stock firmware or other).
    Stock(DeviceKind),
}

impl Device {
    pub fn new(kind: DeviceKind, port: Box<dyn Port>) -> Self {
        Self { kind, serial: None, port }
    }

    pub fn send(&self, req: &Report) -> Result<()> {
        self.port.write(req)
    }

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
    pub fn transact(&self, req: &Report) -> Result<[u8; REPORT_LEN - 3]> {
        self.send(req)?;
        let resp = self.recv_matching(req)?;
        let mut payload = [0u8; REPORT_LEN - 3];
        payload.copy_from_slice(&resp[3..]);
        Ok(payload)
    }

    /// Sends several requests back to back, then collects the answers.
    /// Much faster than one round trip per message for streaming frames.
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
    pub fn keymap(&self) -> Result<Keymap> {
        let board = match self.kind {
            DeviceKind::Keyboard => Board::Keyboard,
            DeviceKind::Macropad => Board::Macropad,
        };
        let mut buf = Vec::new();
        for r in proto::keymap_buffer_requests(board.layer_bytes() * board.layers() as usize) {
            let p = self.transact(&r)?;
            buf.extend_from_slice(proto::parse_keymap_buffer(&p));
        }
        Keymap::from_buffer(board, &buf).ok_or(DeviceError::Protocol(ProtocolError::Invalid("short keymap")))
    }

    /// Macropad corner combos; None on firmware without them.
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

    pub fn info(&self) -> Result<DeviceInfo> {
        let p = self.transact(&proto::info_request())?;
        Ok(proto::parse_info(&p)?)
    }

    /// LED positions. For the keyboard and macropad these come from Framework's
    /// source (built into the app) rather than the device: while the devices are
    /// joined, the firmware squeezes its positions into its slice of the shared
    /// surface, and matching keys against those put painted keys on the wrong LEDs.
    pub fn led_points(&self, count: u8) -> Result<Vec<LedPoint>> {
        let known: &[(u8, u8)] = match self.kind {
            DeviceKind::Keyboard => &crate::keys::KEYBOARD_LEDS,
            DeviceKind::Macropad => &crate::keys::MACROPAD_LEDS,
        };
        if known.len() == count as usize {
            return Ok(crate::keys::firmware_leds(known));
        }
        let mut out = Vec::with_capacity(count as usize);
        for req in proto::led_info_requests(count) {
            out.extend(proto::parse_led_info(&self.transact(&req)?));
        }
        out.truncate(count as usize);
        Ok(out)
    }
}

/// Opens the raw HID interface of one device type without talking to it.
pub fn open_raw(api: &mut HidApi, want: DeviceKind) -> Option<Device> {
    let _ = api.refresh_devices();
    api.device_list()
        .filter(|d| {
            d.vendor_id() == proto::VID
                && d.usage_page() == proto::RAW_USAGE_PAGE
                && d.usage() == proto::RAW_USAGE
                && DeviceKind::from_pid(d.product_id()) == Some(want)
        })
        .find_map(|d| d.open_device(api).ok())
        .map(|hid| Device::new(want, Box::new(HidPort(hid))))
}

/// Finds the keyboard and macropad raw HID interfaces, skipping kinds in `skip`
/// (devices that are already open).
pub fn scan(api: &mut HidApi, skip: &[DeviceKind]) -> Vec<Found> {
    if let Err(e) = api.refresh_devices() {
        eprintln!("HID refresh failed: {e}");
    }
    let mut found = Vec::new();
    let mut seen = Vec::new();
    for d in api.device_list() {
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
pub fn numpad_present(api: &HidApi) -> bool {
    api.device_list().any(|d| d.vendor_id() == proto::VID && d.product_id() == PID_NUMPAD)
}

/// Whether a Framework keyboard, macropad or numpad is plugged in (any firmware), without opening it.
pub fn any_present() -> bool {
    let Ok(api) = HidApi::new() else {
        return false;
    };
    let present = api.device_list().any(|d| {
        d.vendor_id() == proto::VID && (DeviceKind::from_pid(d.product_id()).is_some() || d.product_id() == PID_NUMPAD)
    });
    present
}

/// What happened when the Top row setting was changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FnLockResult {
    /// The keyboard now reports the Fn lock that was asked for.
    pub took: bool,
    /// Top-row keys put back the right way round first (see `Keymap::top_row_fixes`).
    pub fixed_keys: usize,
}

/// Sets the keyboard's Fn lock: `true` = the top row sends F1-F12 on its own (shortcuts with
/// Fn), `false` = shortcuts on their own (F1-F12 with Fn). Same as Fn + Esc, and saved on the
/// keyboard. First puts the top row's two jobs where Fn lock expects them, so the choice works
/// even when the keymap got mixed up. Reads the keyboard back to check it took.
pub fn set_fn_lock(dev: &Device, keymap: Option<&mut Keymap>, on: bool) -> Result<FnLockResult> {
    let mut fixed_keys = 0;
    if let Some(km) = keymap {
        let fixes = km.top_row_fixes();
        if !fixes.is_empty() {
            let reqs: Vec<Report> = fixes.iter().map(|&(l, r, c, code)| proto::set_keycode(l, r, c, code)).collect();
            dev.transact_many(&reqs)?;
            km.apply(&fixes);
            let mut keys: Vec<(u8, u8)> = fixes.iter().map(|&(_, r, c, _)| (r, c)).collect();
            keys.sort_unstable();
            keys.dedup();
            fixed_keys = keys.len();
        }
    }
    match dev.transact(&proto::set_layer(on as u8)) {
        Ok(_) => {}
        // Keyboard firmware from before Fn lock could be set from the app
        Err(DeviceError::Protocol(ProtocolError::Unsupported)) => return Ok(FnLockResult { took: false, fixed_keys }),
        Err(e) => return Err(e),
    }
    let took = dev.info()?.layer == Some(on as u8);
    Ok(FnLockResult { took, fixed_keys })
}

/// Asks a freshly opened device whether it runs our firmware.
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
