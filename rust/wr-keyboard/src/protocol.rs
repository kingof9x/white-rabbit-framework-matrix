//! Raw HID message encoding for the `f16custom` firmware.
//!
//! Must stay in sync with `firmware/users/f16custom/hostfx_protocol.h`.
//! Every message is a 32 byte report on the VIA raw HID interface
//! (usage page 0xFF60, usage 0x61):
//!
//! ```text
//! byte 0   0x07 = set, 0x08 = get   (VIA id_custom_set_value / get_value)
//! byte 1   0x00                     (VIA id_custom_channel)
//! byte 2   value id                 (ValueId)
//! byte 3.. payload
//! ```
//!
//! The device echoes bytes 0..=2 in its answer, or sets byte 0 to 0xFF when it
//! does not understand the command (for example on stock firmware).

use std::fmt;

/// Framework Computer USB vendor id.
pub const VID: u16 = 0x32AC;
/// Framework Laptop 16 ANSI keyboard (the RGB and white versions share this id).
pub const PID_KEYBOARD: u16 = 0x0012;
/// Framework Laptop 16 RGB macropad.
pub const PID_MACROPAD: u16 = 0x0013;
/// VIA raw HID interface.
pub const RAW_USAGE_PAGE: u16 = 0xFF60;
pub const RAW_USAGE: u16 = 0x61;

pub const REPORT_LEN: usize = 32;
pub type Report = [u8; REPORT_LEN];

pub const CMD_SET: u8 = 0x07;
pub const CMD_GET: u8 = 0x08;
pub const CHANNEL: u8 = 0x00;
pub const UNHANDLED: u8 = 0xFF;

pub const MAGIC: [u8; 2] = *b"FX";
pub const PROTOCOL_VERSION: u8 = 2;

/// Max LEDs per FRAME message. Host->device messages stay within 31 bytes
/// (3 + 2 + 8 * 3 = 29) because the RP2040 sometimes only receives 31 of 32.
pub const FRAME_LEDS_PER_MSG: usize = 8;
/// Max LEDs per LED_INFO answer (device->host, 3 + 2 + 9 * 3 = 32).
pub const INFO_LEDS_PER_MSG: usize = 9;
/// Max key presses returned per KEY_EVENTS poll.
pub const KEYS_PER_MSG: usize = 28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ValueId {
    Info = 0x01,
    State = 0x02,
    HotkeyLock = 0x03,
    LedInfo = 0x04,
    Frame = 0x05,
    HostMode = 0x06,
    KeyEvents = 0x07,
    Layer = 0x08,
    Span = 0x09,
    Clock = 0x0A,
    Paint = 0x0B,
    PaintCtrl = 0x0C,
    Combos = 0x0D,
    LedCal = 0x0E,
    Shortcuts = 0x0F,
}

/// Painted keys: what PAINT_CTRL can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PaintOp {
    Clear = 0,
    Save = 1,
    FollowOff = 2,
    FollowOn = 3,
    Reload = 4,
    Tint = 5,
}

/// How painted keys look while an effect runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaintMode {
    /// A steady color.
    Steady,
    /// Their own color, brightening and dimming with the effect.
    #[default]
    Follow,
    /// The effect's color, turned round the color wheel by the painted hue.
    Tint,
}

impl PaintMode {
    pub fn op(self) -> PaintOp {
        match self {
            PaintMode::Steady => PaintOp::FollowOff,
            PaintMode::Follow => PaintOp::FollowOn,
            PaintMode::Tint => PaintOp::Tint,
        }
    }
}

/// Painted LEDs per PAINT set message (3 + 2 + 8 * 3 = 29 bytes).
pub const PAINT_PER_SET_MSG: usize = 8;
pub const PAINT_PER_GET_MSG: usize = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    Keyboard,
    Macropad,
}

impl DeviceKind {
    pub fn from_pid(pid: u16) -> Option<Self> {
        match pid {
            PID_KEYBOARD => Some(Self::Keyboard),
            PID_MACROPAD => Some(Self::Macropad),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Keyboard => "Keyboard",
            Self::Macropad => "Macropad",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// Device answered 0xFF: the command is unknown (most likely stock firmware).
    Unsupported,
    /// Answer does not belong to the request we sent.
    Mismatch { expected: [u8; 3], got: [u8; 3] },
    /// INFO answer has no "FX" magic: not our firmware.
    NotCustomFirmware,
    /// Firmware speaks a newer/older protocol.
    VersionMismatch(u8),
    /// Bad argument (e.g. invalid effect id).
    Invalid(&'static str),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(f, "command not supported (is the f16custom firmware flashed?)"),
            Self::Mismatch { expected, got } => {
                write!(f, "unexpected answer {got:02X?}, expected {expected:02X?}")
            }
            Self::NotCustomFirmware => write!(f, "device is not running the f16custom firmware"),
            Self::VersionMismatch(v) => {
                write!(f, "firmware protocol v{v}, app expects v{PROTOCOL_VERSION}")
            }
            Self::Invalid(why) => write!(f, "invalid value: {why}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

fn request(cmd: u8, id: ValueId, payload: &[u8]) -> Report {
    assert!(payload.len() <= REPORT_LEN - 3, "payload too long");
    let mut r = [0u8; REPORT_LEN];
    r[0] = cmd;
    r[1] = CHANNEL;
    r[2] = id as u8;
    r[3..3 + payload.len()].copy_from_slice(payload);
    r
}

pub fn get(id: ValueId, payload: &[u8]) -> Report {
    request(CMD_GET, id, payload)
}

pub fn set(id: ValueId, payload: &[u8]) -> Report {
    request(CMD_SET, id, payload)
}

/// Checks that `resp` answers `req` and returns its 29 byte payload.
pub fn check_response<'a>(req: &Report, resp: &'a Report) -> Result<&'a [u8], ProtocolError> {
    if resp[0] == UNHANDLED {
        return Err(ProtocolError::Unsupported);
    }
    if resp[..3] != req[..3] {
        return Err(ProtocolError::Mismatch { expected: [req[0], req[1], req[2]], got: [resp[0], resp[1], resp[2]] });
    }
    Ok(&resp[3..])
}

// ---------------------------------------------------------------------------
// INFO
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub protocol_version: u8,
    pub led_count: u8,
    /// RGB_MATRIX_EFFECT_MAX: valid effect ids are 1..effect_count
    pub effect_count: u8,
    /// Effect id of the host-driven ("PC controlled") effect.
    pub host_effect: u8,
    pub kind: DeviceKind,
    pub hotkey_lock: bool,
    /// Macropad layer (0 = MIDI, 1 = F-keys), or on the keyboard Fn lock (0 = off: the top row
    /// is media keys, 1 = on: the top row is F1-F12). None on keyboard firmware too old to say.
    pub layer: Option<u8>,
    pub max_brightness: u8,
    /// Effect id of "off, painted keys only".
    pub paint_only_effect: u8,
    /// Which build of the firmware this is.
    pub build: FirmwareBuild,
}

/// The firmware's build number, from INFO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareBuild {
    /// Firmware from before build numbers (it doesn't say).
    Older,
    /// A test build (a pull request, or made by hand).
    Test,
    /// Built from main or a release, numbered by the build that made it.
    Build(u16),
}

impl FirmwareBuild {
    /// "build 12", "a test build", "an older build from before build numbers"
    pub fn describe(self) -> String {
        match self {
            Self::Build(n) => format!("build {n}"),
            Self::Test => "a test build".into(),
            Self::Older => "an older build from before build numbers".into(),
        }
    }
}

/// The build number a .uf2 image holds (the firmware keeps `F16FW-BUILD` + u16 in it), from the
/// bytes the image puts into the chip. None when it has no number (older firmware).
pub fn image_build(image: &[u8]) -> Option<FirmwareBuild> {
    const TAG: &[u8] = b"F16FW-BUILD";
    let at = image.windows(TAG.len()).position(|w| w == TAG)? + TAG.len();
    let n = u16::from_le_bytes([*image.get(at)?, *image.get(at + 1)?]);
    Some(if n == 0 { FirmwareBuild::Test } else { FirmwareBuild::Build(n) })
}

pub fn info_request() -> Report {
    get(ValueId::Info, &[])
}

pub fn parse_info(payload: &[u8]) -> Result<DeviceInfo, ProtocolError> {
    if payload[0..2] != MAGIC {
        return Err(ProtocolError::NotCustomFirmware);
    }
    if payload[2] != PROTOCOL_VERSION {
        return Err(ProtocolError::VersionMismatch(payload[2]));
    }
    let kind = match payload[6] {
        0 => DeviceKind::Keyboard,
        1 => DeviceKind::Macropad,
        _ => return Err(ProtocolError::Invalid("device kind")),
    };
    Ok(DeviceInfo {
        protocol_version: payload[2],
        led_count: payload[3],
        effect_count: payload[4],
        host_effect: payload[5],
        kind,
        hotkey_lock: payload[7] != 0,
        layer: if payload[8] == 0xFF { None } else { Some(payload[8]) },
        max_brightness: payload[9],
        paint_only_effect: payload[10],
        build: match payload[11] {
            b'B' => match u16::from_le_bytes([payload[12], payload[13]]) {
                0 => FirmwareBuild::Test,
                n => FirmwareBuild::Build(n),
            },
            _ => FirmwareBuild::Older,
        },
    })
}

// ---------------------------------------------------------------------------
// STATE
// ---------------------------------------------------------------------------

/// Built-in effect state. Hue/sat/val/speed use QMK's 0..=255 scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightState {
    pub enabled: bool,
    pub mode: u8,
    pub hue: u8,
    pub sat: u8,
    pub val: u8,
    pub speed: u8,
}

impl Default for LightState {
    fn default() -> Self {
        Self { enabled: true, mode: 1, hue: 0, sat: 0, val: 255, speed: 127 }
    }
}

pub fn state_request() -> Report {
    get(ValueId::State, &[])
}

/// Returns the state plus whether the PC is currently streaming frames.
pub fn parse_state(payload: &[u8]) -> (LightState, bool) {
    (
        LightState {
            enabled: payload[0] != 0,
            mode: payload[1],
            hue: payload[2],
            sat: payload[3],
            val: payload[4],
            speed: payload[5],
        },
        payload[6] != 0,
    )
}

/// `save = true` also writes it to the device's EEPROM (survives unplugging).
pub fn set_state(state: &LightState, save: bool, effect_count: u8) -> Result<Report, ProtocolError> {
    if state.mode == 0 || state.mode >= effect_count {
        return Err(ProtocolError::Invalid("effect id"));
    }
    Ok(set(
        ValueId::State,
        &[state.enabled as u8, state.mode, state.hue, state.sat, state.val, state.speed, save as u8],
    ))
}

// ---------------------------------------------------------------------------
// Hotkey lock / host mode / layer
// ---------------------------------------------------------------------------

pub fn set_hotkey_lock(on: bool) -> Report {
    set(ValueId::HotkeyLock, &[on as u8])
}

pub fn hotkey_lock_request() -> Report {
    get(ValueId::HotkeyLock, &[])
}

pub fn set_host_mode(on: bool) -> Report {
    set(ValueId::HostMode, &[on as u8])
}

pub fn set_layer(layer: u8) -> Report {
    set(ValueId::Layer, &[layer])
}

pub fn layer_request() -> Report {
    get(ValueId::Layer, &[])
}

/// Restarts the device into its RP2040 bootloader ("RPI-RP2" USB drive) so a
/// new UF2 can be dropped on it. This is Framework's own factory command
/// (VIA command 0x0B, sub command 0xFE), so it also works on stock firmware.
/// The device resets immediately and sends no answer.
pub fn bootloader_request() -> Report {
    let mut r = [0u8; REPORT_LEN];
    r[0] = 0x0B;
    r[1] = 0xFE;
    r
}

// ---------------------------------------------------------------------------
// Keymap (VIA's own dynamic keymap commands, saved on the device)
// ---------------------------------------------------------------------------

pub const VIA_GET_KEYCODE: u8 = 0x04;
pub const VIA_SET_KEYCODE: u8 = 0x05;
pub const VIA_GET_BUFFER: u8 = 0x12;
/// Keymap bytes per VIA buffer read (4 header bytes + 28).
pub const KEYMAP_BYTES_PER_MSG: usize = 28;

/// Reads `size` bytes of the keymap buffer starting at `offset`.
pub fn keymap_buffer_request(offset: u16, size: u8) -> Report {
    let mut r = [0u8; REPORT_LEN];
    let o = offset.to_be_bytes();
    r[..4].copy_from_slice(&[VIA_GET_BUFFER, o[0], o[1], size.min(KEYMAP_BYTES_PER_MSG as u8)]);
    r
}

/// Requests that read the first `len` bytes of the keymap buffer.
pub fn keymap_buffer_requests(len: usize) -> Vec<Report> {
    (0..len)
        .step_by(KEYMAP_BYTES_PER_MSG)
        .map(|o| keymap_buffer_request(o as u16, (len - o).min(KEYMAP_BYTES_PER_MSG) as u8))
        .collect()
}

/// The data part of a keymap buffer answer (its payload starts at byte 3 = size).
pub fn parse_keymap_buffer(payload: &[u8]) -> &[u8] {
    let n = (payload[0] as usize).min(KEYMAP_BYTES_PER_MSG);
    &payload[1..1 + n]
}

pub fn set_keycode(layer: u8, row: u8, col: u8, code: u16) -> Report {
    let mut r = [0u8; REPORT_LEN];
    let c = code.to_be_bytes();
    r[..6].copy_from_slice(&[VIA_SET_KEYCODE, layer, row, col, c[0], c[1]]);
    r
}

// ---------------------------------------------------------------------------
// Macropad corner combos
// ---------------------------------------------------------------------------

/// The macropad's two corner-key combos. An action of 0 means the built-in job
/// (top: switch layer A/B, bottom: lighting controls).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Combos {
    pub top_on: bool,
    pub bottom_on: bool,
    pub top_action: u16,
    pub bottom_action: u16,
    /// The top corner keys keep the active layer's color (cyan A, orange B).
    pub corner_lights: bool,
    /// The top combo is the whole top row (four keys) instead of the two corners.
    pub top_row: bool,
}

impl Default for Combos {
    fn default() -> Self {
        Self { top_on: true, bottom_on: true, top_action: 0, bottom_action: 0, corner_lights: false, top_row: false }
    }
}

pub fn combos_request() -> Report {
    get(ValueId::Combos, &[])
}

pub fn parse_combos(p: &[u8]) -> Combos {
    Combos {
        top_on: p[0] != 0,
        bottom_on: p[1] != 0,
        top_action: u16::from_be_bytes([p[2], p[3]]),
        bottom_action: u16::from_be_bytes([p[4], p[5]]),
        corner_lights: p[6] != 0,
        top_row: p[7] != 0,
    }
}

pub fn set_combos(c: &Combos) -> Report {
    let t = c.top_action.to_be_bytes();
    let b = c.bottom_action.to_be_bytes();
    set(
        ValueId::Combos,
        &[c.top_on as u8, c.bottom_on as u8, t[0], t[1], b[0], b[1], c.corner_lights as u8, c.top_row as u8],
    )
}

// ---------------------------------------------------------------------------
// Keyboard shortcuts (F12 + arrows)
// ---------------------------------------------------------------------------

pub fn shortcuts_request() -> Report {
    get(ValueId::Shortcuts, &[])
}

pub fn set_shortcuts(on: bool) -> Report {
    set(ValueId::Shortcuts, &[on as u8])
}

// ---------------------------------------------------------------------------
// LED calibration: the LED driver's current and per-color gains
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedCal {
    /// LED driver current (the chip's global current setting).
    pub current: u8,
    /// Red, green, blue gains, 255 = full.
    pub gains: [u8; 3],
    /// The firmware's own current, for "Reset".
    pub default_current: u8,
}

pub fn led_cal_request() -> Report {
    get(ValueId::LedCal, &[])
}

pub fn parse_led_cal(p: &[u8]) -> LedCal {
    LedCal { current: p[0], gains: [p[1], p[2], p[3]], default_current: p[4] }
}

/// `save` keeps it after a restart.
pub fn set_led_cal(c: &LedCal, save: bool) -> Report {
    set(ValueId::LedCal, &[c.current, c.gains[0], c.gains[1], c.gains[2], save as u8])
}

// ---------------------------------------------------------------------------
// Joined surface, effect clock, painted keys
// ---------------------------------------------------------------------------

/// Joined surface: this device takes `x_start..=x_end` of the shared 0..=224 space.
pub fn set_span(on: bool, x_start: u8, x_end: u8) -> Report {
    set(ValueId::Span, &[on as u8, x_start, x_end])
}

pub fn span_request() -> Report {
    get(ValueId::Span, &[])
}

pub fn parse_span(payload: &[u8]) -> (bool, u8, u8) {
    (payload[0] != 0, payload[1], payload[2])
}

/// Sets the effect clock to `ms`. Sent to both joined devices back to back.
pub fn set_clock(ms: u32) -> Report {
    set(ValueId::Clock, &ms.to_le_bytes())
}

/// One LED's paint: `None` = not painted.
pub type Paint = Option<(u8, u8)>;

/// PAINT set messages for every LED (index order).
pub fn encode_paint(leds: &[Paint]) -> Vec<Report> {
    leds.chunks(PAINT_PER_SET_MSG)
        .enumerate()
        .map(|(i, chunk)| {
            let mut p = vec![(i * PAINT_PER_SET_MSG) as u8, chunk.len() as u8];
            for led in chunk {
                match led {
                    Some((h, s)) => p.extend_from_slice(&[*h, *s, 1]),
                    None => p.extend_from_slice(&[0, 0, 0]),
                }
            }
            set(ValueId::Paint, &p)
        })
        .collect()
}

pub fn paint_requests(led_count: u8) -> Vec<Report> {
    (0..led_count as usize).step_by(PAINT_PER_GET_MSG).map(|s| get(ValueId::Paint, &[s as u8])).collect()
}

pub fn parse_paint(payload: &[u8]) -> Vec<Paint> {
    let n = (payload[1] as usize).min(PAINT_PER_GET_MSG);
    (0..n)
        .map(|i| {
            let b = &payload[2 + i * 3..5 + i * 3];
            (b[2] != 0).then_some((b[0], b[1]))
        })
        .collect()
}

pub fn paint_ctrl(op: PaintOp) -> Report {
    set(ValueId::PaintCtrl, &[op as u8])
}

pub fn paint_ctrl_request() -> Report {
    get(ValueId::PaintCtrl, &[])
}

/// (follow the effect, number of painted LEDs)
pub fn parse_paint_ctrl(payload: &[u8]) -> (bool, u8) {
    (payload[0] != 0, payload[1])
}

// ---------------------------------------------------------------------------
// LED positions
// ---------------------------------------------------------------------------

/// LED position in QMK's coordinate space (x 0..=224, y 0..=64) + LED flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedPoint {
    pub x: u8,
    pub y: u8,
    pub flags: u8,
}

pub fn led_info_request(start: u8) -> Report {
    get(ValueId::LedInfo, &[start])
}

/// All LED_INFO requests needed to read `led_count` LEDs.
pub fn led_info_requests(led_count: u8) -> Vec<Report> {
    (0..led_count as usize).step_by(INFO_LEDS_PER_MSG).map(|start| led_info_request(start as u8)).collect()
}

pub fn parse_led_info(payload: &[u8]) -> Vec<LedPoint> {
    let count = (payload[1] as usize).min(INFO_LEDS_PER_MSG);
    (0..count)
        .map(|i| {
            let b = &payload[2 + i * 3..2 + i * 3 + 3];
            LedPoint { x: b[0], y: b[1], flags: b[2] }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Frames and key events
// ---------------------------------------------------------------------------

/// Splits one frame (one color per LED, in LED index order) into FRAME messages.
pub fn encode_frame(colors: &[[u8; 3]]) -> Vec<Report> {
    colors
        .chunks(FRAME_LEDS_PER_MSG)
        .enumerate()
        .map(|(i, chunk)| {
            let mut payload = Vec::with_capacity(2 + chunk.len() * 3);
            payload.push((i * FRAME_LEDS_PER_MSG) as u8);
            payload.push(chunk.len() as u8);
            for c in chunk {
                payload.extend_from_slice(c);
            }
            set(ValueId::Frame, &payload)
        })
        .collect()
}

pub fn key_events_request() -> Report {
    get(ValueId::KeyEvents, &[])
}

/// LED indices of keys pressed since the last poll.
pub fn parse_key_events(payload: &[u8]) -> Vec<u8> {
    let n = (payload[0] as usize).min(KEYS_PER_MSG);
    payload[1..1 + n].to_vec()
}

// ---------------------------------------------------------------------------
// Effect names
// ---------------------------------------------------------------------------

/// Names of QMK's built-in effects in the order Framework's firmware enables
/// them (keyboards/framework/config.h, Framework QMK v0.3.1). Index = effect id.
pub const EFFECT_NAMES: &[&str] = &[
    "Off",
    "Solid color",
    "Alphas / mods",
    "Gradient up-down",
    "Gradient left-right",
    "Breathing",
    "Band (saturation)",
    "Band (brightness)",
    "Pinwheel band (saturation)",
    "Pinwheel band (brightness)",
    "Spiral band (saturation)",
    "Spiral band (brightness)",
    "Cycle all",
    "Cycle left-right",
    "Cycle up-down",
    "Rainbow chevron",
    "Cycle out-in",
    "Cycle out-in dual",
    "Cycle pinwheel",
    "Cycle spiral",
    "Dual beacon",
    "Rainbow beacon",
    "Rainbow pinwheels",
    "Raindrops",
    "Jellybean raindrops",
    "Hue breathing",
    "Hue pendulum",
    "Hue wave",
    "Pixel rain",
    "Pixel flow",
    "Pixel fractal",
    "Typing heatmap",
    "Digital rain",
    "Reactive (simple)",
    "Reactive",
    "Reactive wide",
    "Reactive multi-wide",
    "Reactive cross",
    "Reactive multi-cross",
    "Reactive nexus",
    "Reactive multi-nexus",
    "Splash",
    "Multi-splash",
    "Solid splash",
    "Solid multi-splash",
];

pub fn effect_name(mode: u8, host_effect: u8) -> String {
    if mode == host_effect {
        return "PC controlled (Linked)".to_string();
    }
    EFFECT_NAMES.get(mode as usize).map(|s| s.to_string()).unwrap_or_else(|| format!("Effect {mode}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_have_via_custom_header() {
        let r = get(ValueId::Info, &[]);
        assert_eq!(&r[..3], &[0x08, 0x00, 0x01]);
        let r = set_hotkey_lock(true);
        assert_eq!(&r[..4], &[0x07, 0x00, 0x03, 0x01]);
        assert_eq!(r.len(), 32);
    }

    #[test]
    fn bootloader_is_framework_factory_command() {
        let r = bootloader_request();
        assert_eq!(&r[..2], &[0x0B, 0xFE]);
        assert!(r[2..].iter().all(|&b| b == 0));
    }

    #[test]
    fn span_clock_and_paint_messages() {
        assert_eq!(&set_span(true, 0, 180)[..6], &[0x07, 0, 0x09, 1, 0, 180]);
        assert_eq!(parse_span(&[1, 186, 224]), (true, 186, 224));
        assert_eq!(&set_clock(0x0102_0304)[..7], &[0x07, 0, 0x0A, 4, 3, 2, 1]);
        let mut leds: Vec<Paint> = vec![None; 97];
        leds[0] = Some((10, 20));
        leds[96] = Some((30, 40));
        let msgs = encode_paint(&leds);
        assert_eq!(msgs.len(), 13);
        assert_eq!(&msgs[0][..8], &[0x07, 0, 0x0B, 0, 8, 10, 20, 1]);
        assert_eq!(&msgs[0][8..11], &[0, 0, 0]);
        let last = msgs.last().unwrap();
        assert_eq!(&last[3..8], &[96, 1, 30, 40, 1]);
        assert!(msgs.iter().all(|m| m[29..].iter().all(|&b| b == 0)));
        let mut pl = [0u8; 29];
        pl[..8].copy_from_slice(&[9, 2, 5, 6, 1, 0, 0, 0]);
        assert_eq!(parse_paint(&pl), vec![Some((5, 6)), None]);
        assert_eq!(&paint_ctrl(PaintOp::Save)[..4], &[0x07, 0, 0x0C, 1]);
        assert_eq!(parse_paint_ctrl(&[1, 12]), (true, 12));
        assert_eq!(paint_requests(97).len(), 11);
    }

    #[test]
    fn response_checks() {
        let req = info_request();
        let mut ok = req;
        ok[3] = b'F';
        assert_eq!(check_response(&req, &ok).unwrap()[0], b'F');

        let mut unhandled = req;
        unhandled[0] = 0xFF;
        assert_eq!(check_response(&req, &unhandled), Err(ProtocolError::Unsupported));

        let other = state_request();
        assert!(matches!(check_response(&req, &other), Err(ProtocolError::Mismatch { .. })));
    }

    #[test]
    fn parses_info() {
        let mut p = [0u8; 29];
        p[..11].copy_from_slice(&[b'F', b'X', 2, 97, 47, 45, 1, 1, 0, 255, 46]);
        let info = parse_info(&p).unwrap();
        assert_eq!(info.led_count, 97);
        assert_eq!(info.effect_count, 47);
        assert_eq!(info.paint_only_effect, 46);
        assert_eq!(info.host_effect, 45);
        assert_eq!(info.kind, DeviceKind::Macropad);
        assert!(info.hotkey_lock);
        assert_eq!(info.layer, Some(0));
        assert_eq!(info.build, FirmwareBuild::Older, "older firmware leaves the build bytes 0");
        p[11..14].copy_from_slice(&[b'B', 0x2C, 0x01]);
        assert_eq!(parse_info(&p).unwrap().build, FirmwareBuild::Build(300));
        p[12] = 0;
        p[13] = 0;
        assert_eq!(parse_info(&p).unwrap().build, FirmwareBuild::Test);

        p[6] = 0;
        p[8] = 0xFF;
        let info = parse_info(&p).unwrap();
        assert_eq!(info.kind, DeviceKind::Keyboard);
        assert_eq!(info.layer, None);

        p[0] = 0;
        assert_eq!(parse_info(&p), Err(ProtocolError::NotCustomFirmware));
        p[0] = b'F';
        p[2] = 9;
        assert_eq!(parse_info(&p), Err(ProtocolError::VersionMismatch(9)));
    }

    #[test]
    fn reads_the_build_from_an_image() {
        let mut img = vec![0u8; 64];
        img[20..31].copy_from_slice(b"F16FW-BUILD");
        img[31] = 7;
        assert_eq!(image_build(&img), Some(FirmwareBuild::Build(7)));
        img[31] = 0;
        assert_eq!(image_build(&img), Some(FirmwareBuild::Test));
        assert_eq!(image_build(&[0u8; 64]), None);
    }

    #[test]
    fn keymap_buffer_reads_cover_the_layers() {
        let reqs = keymap_buffer_requests(4 * 256);
        assert_eq!(reqs.len(), 37);
        assert_eq!(&reqs[1][..4], &[0x12, 0, 28, 28]);
        assert_eq!(reqs[36][3], (1024 - 36 * 28) as u8);
        let r = set_keycode(1, 2, 3, 0x7E40);
        assert_eq!(&r[..6], &[0x05, 1, 2, 3, 0x7E, 0x40]);
        let c = Combos {
            top_on: false,
            bottom_on: true,
            top_action: 0x0704,
            bottom_action: 0,
            corner_lights: true,
            top_row: true,
        };
        let r = set_combos(&c);
        assert_eq!(parse_combos(&r[3..]), c);
        let cal = LedCal { current: 90, gains: [255, 240, 200], default_current: 0 };
        let r = set_led_cal(&cal, true);
        assert_eq!(&r[3..8], &[90, 255, 240, 200, 1]);
    }

    #[test]
    fn state_round_trip() {
        let s = LightState { enabled: true, mode: 12, hue: 10, sat: 20, val: 30, speed: 40 };
        let r = set_state(&s, true, 46).unwrap();
        assert_eq!(&r[..10], &[0x07, 0, 0x02, 1, 12, 10, 20, 30, 40, 1]);
        // The device answers a GET with the same field layout (+ streaming flag)
        let mut payload = [0u8; 29];
        payload[..7].copy_from_slice(&r[3..10]);
        let (parsed, streaming) = parse_state(&payload);
        assert_eq!(parsed, s);
        assert!(streaming); // the save byte sits where "streaming" is reported
    }

    #[test]
    fn set_state_rejects_bad_effect() {
        let mut s = LightState { mode: 0, ..Default::default() };
        assert!(set_state(&s, false, 46).is_err());
        s.mode = 46;
        assert!(set_state(&s, false, 46).is_err());
        s.mode = 45;
        assert!(set_state(&s, false, 46).is_ok());
    }

    #[test]
    fn frame_is_chunked_in_eights() {
        let colors: Vec<[u8; 3]> = (0..97u8).map(|i| [i, i.wrapping_add(1), i.wrapping_add(2)]).collect();
        let msgs = encode_frame(&colors);
        assert_eq!(msgs.len(), 13); // ceil(97 / 8)
        assert_eq!(&msgs[0][..5], &[0x07, 0, 0x05, 0, 8]);
        assert_eq!(&msgs[0][5..8], &[0, 1, 2]);
        assert_eq!(&msgs[1][3..5], &[8, 8]);
        assert_eq!(&msgs[1][5..8], &[8, 9, 10]);
        let last = msgs.last().unwrap();
        assert_eq!(&last[3..5], &[96, 1]);
        assert_eq!(&last[5..8], &[96, 97, 98]);
        // the last byte is never used (the RP2040 may not receive it)
        assert!(msgs.iter().all(|m| m.len() == REPORT_LEN && m[29..].iter().all(|&b| b == 0)));
        // the macropad's 24 LEDs fit in 3 messages
        assert_eq!(encode_frame(&[[0; 3]; 24]).len(), 3);
    }

    #[test]
    fn led_info_requests_cover_all_leds() {
        let reqs = led_info_requests(97);
        assert_eq!(reqs.len(), 11);
        assert_eq!(reqs[10][3], 90);
        let mut payload = [0u8; 29];
        payload[0] = 90;
        payload[1] = 2;
        payload[2..8].copy_from_slice(&[10, 20, 4, 30, 40, 1]);
        assert_eq!(
            parse_led_info(&payload),
            vec![LedPoint { x: 10, y: 20, flags: 4 }, LedPoint { x: 30, y: 40, flags: 1 }]
        );
    }

    #[test]
    fn key_events_are_clamped() {
        let mut p = [0u8; 29];
        p[0] = 3;
        p[1..4].copy_from_slice(&[5, 6, 7]);
        assert_eq!(parse_key_events(&p), vec![5, 6, 7]);
        p[0] = 200; // garbage count must not panic
        assert_eq!(parse_key_events(&p).len(), KEYS_PER_MSG);
    }

    #[test]
    fn effect_names_match_firmware_numbering() {
        // firmware: RGB_MATRIX_TYPING_HEATMAP == 31, SOLID_MULTISPLASH == 44, host effect == 45
        assert_eq!(EFFECT_NAMES[31], "Typing heatmap");
        assert_eq!(EFFECT_NAMES.len(), 45);
        assert_eq!(effect_name(45, 45), "PC controlled (Linked)");
        assert_eq!(effect_name(60, 45), "Effect 60");
    }
}
