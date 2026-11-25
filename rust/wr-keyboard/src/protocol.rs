//! Raw HID message encoding for the `f16custom` firmware.
//!
//! This file only builds and reads byte arrays; it never touches USB itself. [`crate::device`] sends
//! the reports this file builds and hands the answers back here to be checked and decoded, and the
//! lighting engine ([`crate::engine`], thread `hid-engine`) decides what to send and when. Because
//! everything here is a pure function of its inputs, it is covered by plain unit tests below, and the
//! pretend device in [`crate::sim`] answers the very same messages.
//!
//! Must stay in sync with `firmware/users/f16custom/hostfx_protocol.h` in the keyboard firmware
//! repository (kingof9x/framework-16-keyboard-customization). Every message is a 32 byte report on
//! the VIA raw HID interface (usage page 0xFF60, usage 0x61):
//!
//! ```text
//! byte 0   0x07 = set, 0x08 = get   (VIA id_custom_set_value / get_value)
//! byte 1   0x00                     (VIA id_custom_channel)
//! byte 2   value id                 (ValueId)
//! byte 3.. payload
//! ```
//!
//! The device echoes bytes 0..=2 in its answer, or sets byte 0 to 0xFF when it
//! does not understand the command (for example on stock firmware). So the first three bytes are a
//! header (what was asked) and the remaining 29 bytes are the payload (the values). VIA's own
//! commands (keymap reads and writes, and Framework's bootloader command) use the same 32 byte
//! report but a different header; they are built by hand further down.
//!
//! Multi-byte numbers: key codes and keymap buffer offsets are big endian (VIA's convention); the
//! effect clock and the firmware build number are little endian (the RP2040's own byte order).
//! Colors are 8-bit RGB; hue, saturation, brightness and speed use QMK's 0..=255 scales.

use std::fmt;

/// Framework Computer USB vendor id.
/// All three Framework input modules (keyboard, macropad, numpad) report this id.
pub const VID: u16 = 0x32AC;
/// Framework Laptop 16 ANSI keyboard (the RGB and white versions share this id).
pub const PID_KEYBOARD: u16 = 0x0012;
/// Framework Laptop 16 RGB macropad.
pub const PID_MACROPAD: u16 = 0x0013;
/// VIA raw HID interface.
/// The usage page 0xFF60 is vendor defined: it is how QMK/VIA firmware marks its raw HID interface,
/// so the app can tell it apart from the normal keyboard interface that types letters.
pub const RAW_USAGE_PAGE: u16 = 0xFF60;
/// VIA raw HID usage. Usage page and usage together pick the one HID interface to open.
pub const RAW_USAGE: u16 = 0x61;

/// Every raw HID report, in both directions, is exactly 32 bytes (the size of the USB endpoint).
pub const REPORT_LEN: usize = 32;
/// One raw HID report: the unit everything in this file builds or reads.
pub type Report = [u8; REPORT_LEN];

/// Byte 0 of a "write this value" request (VIA's `id_custom_set_value`).
pub const CMD_SET: u8 = 0x07;
/// Byte 0 of a "read this value" request (VIA's `id_custom_get_value`).
pub const CMD_GET: u8 = 0x08;
/// Byte 1: VIA's custom channel. 0 is the channel the f16custom firmware listens on.
pub const CHANNEL: u8 = 0x00;
/// What the device puts in byte 0 of its answer when it does not know the command.
pub const UNHANDLED: u8 = 0xFF;

/// The first two payload bytes of an INFO answer on f16custom firmware ("FX" for "host effects").
/// Any other firmware answers something else (or 0xFF), which is how stock firmware is detected.
pub const MAGIC: [u8; 2] = *b"FX";
/// The protocol version this app speaks. INFO answers with the firmware's version; a different
/// number is refused ([`ProtocolError::VersionMismatch`]) so a mismatched pair never mis-reads bytes.
pub const PROTOCOL_VERSION: u8 = 2;

/// Max LEDs per FRAME message. Host->device messages stay within 31 bytes
/// (3 + 2 + 8 * 3 = 29) because the RP2040 sometimes only receives 31 of 32.
pub const FRAME_LEDS_PER_MSG: usize = 8;
/// Max LEDs per LED_INFO answer (device->host, 3 + 2 + 9 * 3 = 32).
pub const INFO_LEDS_PER_MSG: usize = 9;
/// Max key presses returned per KEY_EVENTS poll.
/// (1 count byte + 28 LED indices fill the 29 byte payload.)
pub const KEYS_PER_MSG: usize = 28;

/// Byte 2 of a request: which value is being read or written. The numbers are the firmware's
/// `hostfx_protocol.h` ids, so they must never be renumbered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ValueId {
    /// GET: firmware identity, LED count, effect ids, Fn lock and build number ([`parse_info`]).
    Info = 0x01,
    /// GET/SET: the built-in effect (on, effect id, hue, saturation, brightness, speed).
    State = 0x02,
    /// GET/SET: 1 turns off the Fn lighting keys (and the macropad's lighting controls).
    HotkeyLock = 0x03,
    /// GET: positions of up to 9 LEDs, starting at the LED index in the request.
    LedInfo = 0x04,
    /// SET: colors for up to 8 LEDs of a PC-drawn frame (only shown while host mode is on).
    Frame = 0x05,
    /// SET: 1 = the PC draws the lights (FRAME messages), 0 = back to the device's own effect.
    HostMode = 0x06,
    /// GET: LED indices of keys pressed since the last poll (for ripples drawn on the PC).
    KeyEvents = 0x07,
    /// GET/SET: macropad layer (0 MIDI, 1 numpad) or keyboard Fn lock (0 off, 1 on).
    Layer = 0x08,
    /// GET/SET: joined surface, which slice of the shared 0..=224 x space this device covers.
    Span = 0x09,
    /// SET: the effect clock in milliseconds, so two joined devices animate in step.
    Clock = 0x0A,
    /// GET/SET: painted keys (a hue and saturation per LED, or not painted).
    Paint = 0x0B,
    /// SET: one [`PaintOp`] (clear, save, mode...); GET: the paint mode and painted LED count.
    PaintCtrl = 0x0C,
    /// GET/SET: the macropad's corner-key combos ([`Combos`]).
    Combos = 0x0D,
    /// GET/SET: LED driver current and per-color gains ([`LedCal`]).
    LedCal = 0x0E,
    /// GET/SET: the keyboard's F12 + arrow shortcuts on or off.
    Shortcuts = 0x0F,
}

/// Painted keys: what PAINT_CTRL can do.
/// The number is the single payload byte of a PAINT_CTRL set message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PaintOp {
    /// Unpaint every LED (in the device's working copy; the saved copy stays until Save).
    Clear = 0,
    /// Store the current paint in the device's flash so it comes back after unplugging.
    Save = 1,
    /// Painted keys show their own color steadily ([`PaintMode::Steady`]).
    FollowOff = 2,
    /// Painted keys brighten and dim with the effect ([`PaintMode::Follow`]).
    FollowOn = 3,
    /// Throw away unsaved paint and load the saved copy again.
    Reload = 4,
    /// Painted keys turn the effect's color round the color wheel ([`PaintMode::Tint`]).
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

/// Turning the app's paint mode into the command that selects it on the device.
impl PaintMode {
    /// The PAINT_CTRL operation that puts the device into this mode.
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
/// Painted LEDs per PAINT get answer (3 + 2 + 9 * 3 = 32 bytes: answers may use all 32).
pub const PAINT_PER_GET_MSG: usize = 9;

/// Which of the two RGB devices a message, status or setting belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    /// The Framework 16 RGB keyboard (product id [`PID_KEYBOARD`]).
    Keyboard,
    /// The Framework 16 RGB macropad (product id [`PID_MACROPAD`]).
    Macropad,
}

impl DeviceKind {
    /// The kind for a USB product id; None for any other Framework product (numpad, panels...).
    pub fn from_pid(pid: u16) -> Option<Self> {
        match pid {
            PID_KEYBOARD => Some(Self::Keyboard),
            PID_MACROPAD => Some(Self::Macropad),
            _ => None,
        }
    }
    /// Capitalised name for messages in the window ("Keyboard disconnected: ...").
    pub fn name(self) -> &'static str {
        match self {
            Self::Keyboard => "Keyboard",
            Self::Macropad => "Macropad",
        }
    }
}

/// Why an answer from the device could not be used. Shown to the user through
/// [`crate::device::DeviceError`], so the texts in `Display` are written for people.
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

/// Builds a custom-channel request: header bytes 0..=2, then the payload, zero-filled to 32 bytes.
/// Panics if the payload is longer than 29 bytes; every caller passes a fixed, short payload, so
/// that would be a programming error, not something a device could cause.
fn request(cmd: u8, id: ValueId, payload: &[u8]) -> Report {
    assert!(payload.len() <= REPORT_LEN - 3, "payload too long");
    let mut r = [0u8; REPORT_LEN];
    r[0] = cmd;
    // Bytes past the payload stay 0. That matters for FRAME and PAINT: byte 31 is never used,
    // because the RP2040 sometimes drops the last byte of a report.
    r[1] = CHANNEL;
    r[2] = id as u8;
    r[3..3 + payload.len()].copy_from_slice(payload);
    r
}

/// A GET request for `id`. The payload is usually empty, or a start index for chunked reads.
pub fn get(id: ValueId, payload: &[u8]) -> Report {
    request(CMD_GET, id, payload)
}

/// A SET request for `id` carrying `payload`.
pub fn set(id: ValueId, payload: &[u8]) -> Report {
    request(CMD_SET, id, payload)
}

/// Checks that `resp` answers `req` and returns its 29 byte payload.
/// A 0xFF in byte 0 means "unknown command" ([`ProtocolError::Unsupported`]); different header
/// bytes mean the answer belongs to an earlier request ([`ProtocolError::Mismatch`]), which
/// [`crate::device`] skips while waiting for the right one.
pub fn check_response<'a>(req: &Report, resp: &'a Report) -> Result<&'a [u8], ProtocolError> {
    if resp[0] == UNHANDLED {
        return Err(ProtocolError::Unsupported);
    }
    // Compare the whole header: same command, channel and value id. For VIA keymap reads the
    // "header" is command + 2 offset bytes, so a chunk from another offset is rejected too.
    if resp[..3] != req[..3] {
        return Err(ProtocolError::Mismatch { expected: [req[0], req[1], req[2]], got: [resp[0], resp[1], resp[2]] });
    }
    Ok(&resp[3..])
}

// ---------------------------------------------------------------------------
// INFO
// ---------------------------------------------------------------------------

/// What the INFO answer says about a device. Read once when the device is found and again on
/// every status refresh (about once a second), because Fn lock and the hotkey lock can change on
/// the device itself (Fn + Esc, or the macropad's lighting controls).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Always [`PROTOCOL_VERSION`] once parsing succeeded.
    pub protocol_version: u8,
    /// Number of RGB LEDs: 97 on the keyboard, 24 on the macropad.
    pub led_count: u8,
    /// RGB_MATRIX_EFFECT_MAX: valid effect ids are 1..effect_count
    pub effect_count: u8,
    /// Effect id of the host-driven ("PC controlled") effect.
    pub host_effect: u8,
    /// Keyboard or macropad, as the firmware itself reports it.
    pub kind: DeviceKind,
    /// True = the Fn lighting keys are turned off ([`ValueId::HotkeyLock`]).
    pub hotkey_lock: bool,
    /// Macropad layer (0 = MIDI, 1 = F-keys), or on the keyboard Fn lock (0 = off: the top row
    /// is media keys, 1 = on: the top row is F1-F12). None on keyboard firmware too old to say.
    pub layer: Option<u8>,
    /// The firmware's brightness ceiling on QMK's 0..=255 scale.
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
///
/// Used by the Firmware page to compare a downloaded or chosen file with what the device runs.
/// The search is a plain byte scan; the two bytes after the tag are the number, little endian.
/// A number of 0 marks a test build, the same convention INFO uses.
pub fn image_build(image: &[u8]) -> Option<FirmwareBuild> {
    const TAG: &[u8] = b"F16FW-BUILD";
    // `?` returns None when the tag is missing or the image ends right after it.
    let at = image.windows(TAG.len()).position(|w| w == TAG)? + TAG.len();
    let n = u16::from_le_bytes([*image.get(at)?, *image.get(at + 1)?]);
    Some(if n == 0 { FirmwareBuild::Test } else { FirmwareBuild::Build(n) })
}

/// The INFO request: the first thing sent to any device, to learn whether it runs f16custom.
pub fn info_request() -> Report {
    get(ValueId::Info, &[])
}

/// Decodes an INFO answer payload (29 bytes, after the 3 header bytes). Byte layout:
///
/// ```text
/// 0..2  "FX" magic         3  LED count       4  effect count     5  host effect id
/// 2     protocol version   6  0 keyboard, 1 macropad              7  hotkey lock
/// 8     layer / Fn lock (0xFF = not reported)  9  max brightness  10 paint-only effect id
/// 11    'B' when a build number follows       12..14  build number, little endian (0 = test)
/// ```
///
/// Errors: no magic ([`ProtocolError::NotCustomFirmware`]), another protocol version, or a device
/// kind byte other than 0 or 1. Panics if `payload` is shorter than 14 bytes; callers always pass
/// the 29 byte payload of a full report.
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
        // Keyboard firmware from before Fn lock could be read from the app sends 0xFF here.
        layer: if payload[8] == 0xFF { None } else { Some(payload[8]) },
        max_brightness: payload[9],
        paint_only_effect: payload[10],
        build: match payload[11] {
            // 'B' marks that bytes 12..14 hold a build number; older firmware leaves byte 11 as 0.
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
/// This is what a device shows when the PC is not drawing: one of its own effects. `save`ing it
/// (see [`set_state`]) makes the device start with it after a power cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightState {
    /// Lights on at all.
    pub enabled: bool,
    /// Effect id: 1..=44 built-in effects (names in [`EFFECT_NAMES`]), or the firmware's own extras
    /// (host effect, paint-only) reported in [`DeviceInfo`].
    pub mode: u8,
    /// Effect color, 0..=255 around the color wheel (0 red, 85 green, 170 blue).
    pub hue: u8,
    /// Saturation, 0 (white) to 255 (full color).
    pub sat: u8,
    /// Brightness, 0 to 255 (the firmware may cap it at `max_brightness`).
    pub val: u8,
    /// Animation speed, 0 (slowest) to 255.
    pub speed: u8,
}

/// Solid white at full brightness: a safe "something visible" default before the device is read.
impl Default for LightState {
    fn default() -> Self {
        Self { enabled: true, mode: 1, hue: 0, sat: 0, val: 255, speed: 127 }
    }
}

/// The STATE GET request. Also used as a cheap "are you still there" round trip (see
/// `Engine::sync_clocks`).
pub fn state_request() -> Report {
    get(ValueId::State, &[])
}

/// Returns the state plus whether the PC is currently streaming frames.
/// Payload layout: on, effect, hue, sat, val, speed, streaming flag. While the PC is streaming the
/// device reports the host effect id as `mode`.
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
/// Refuses effect id 0 and ids at or above `effect_count` (from INFO) before anything is sent,
/// so a bad id from settings can never reach the device. The payload is the same layout as the
/// GET answer, with the last byte meaning "save" instead of "streaming".
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

/// `on = true` turns the Fn lighting keys (and the macropad's lighting controls) off, so the lights
/// cannot be changed by accident. The device answers with the new value.
pub fn set_hotkey_lock(on: bool) -> Report {
    set(ValueId::HotkeyLock, &[on as u8])
}

/// Reads the hotkey lock. (The app normally reads it from INFO instead.)
pub fn hotkey_lock_request() -> Report {
    get(ValueId::HotkeyLock, &[])
}

/// `on = true`: the device stops running its own effect and shows FRAME colors from the PC.
/// The firmware falls back to its own effect by itself after about 3 seconds without a frame, so a
/// crashed app never leaves the keys frozen (the simulator copies this).
pub fn set_host_mode(on: bool) -> Report {
    set(ValueId::HostMode, &[on as u8])
}

/// Macropad: switches layer (0 = MIDI, 1 = numpad). Keyboard: sets Fn lock (0 off, 1 on), the same
/// as pressing Fn + Esc; the keyboard remembers it. Firmware that cannot set Fn lock answers 0xFF.
pub fn set_layer(layer: u8) -> Report {
    set(ValueId::Layer, &[layer])
}

/// Reads the layer / Fn lock on its own (INFO also carries it).
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

/// VIA "dynamic_keymap_get_keycode" (read one key). Not used by the app, kept for completeness.
pub const VIA_GET_KEYCODE: u8 = 0x04;
/// VIA "dynamic_keymap_set_keycode": `[0x05, layer, row, col, code_hi, code_lo]`. The firmware writes
/// it to its EEPROM straight away, so key changes survive unplugging and work on any computer.
pub const VIA_SET_KEYCODE: u8 = 0x05;
/// VIA "dynamic_keymap_get_buffer": `[0x12, offset_hi, offset_lo, size]`, answered with the same
/// four bytes followed by `size` bytes of the keymap. The keymap is one flat buffer: every layer
/// back to back, each layer rows x columns, 2 bytes per key (big endian).
pub const VIA_GET_BUFFER: u8 = 0x12;
/// Keymap bytes per VIA buffer read (4 header bytes + 28).
pub const KEYMAP_BYTES_PER_MSG: usize = 28;

/// Reads `size` bytes of the keymap buffer starting at `offset`.
/// `size` is capped at [`KEYMAP_BYTES_PER_MSG`] because 4 header bytes + 28 data bytes fill a report.
pub fn keymap_buffer_request(offset: u16, size: u8) -> Report {
    let mut r = [0u8; REPORT_LEN];
    // This is a VIA command, not a custom-channel one: [0x12, offset high, offset low, size].
    let o = offset.to_be_bytes();
    r[..4].copy_from_slice(&[VIA_GET_BUFFER, o[0], o[1], size.min(KEYMAP_BYTES_PER_MSG as u8)]);
    r
}

/// Requests that read the first `len` bytes of the keymap buffer.
/// One request per 28 byte chunk; the last one asks only for what is left.
pub fn keymap_buffer_requests(len: usize) -> Vec<Report> {
    (0..len)
        .step_by(KEYMAP_BYTES_PER_MSG)
        .map(|o| keymap_buffer_request(o as u16, (len - o).min(KEYMAP_BYTES_PER_MSG) as u8))
        .collect()
}

/// The data part of a keymap buffer answer (its payload starts at byte 3 = size).
/// The answer's header check compares bytes 0..=2 (command and offset), so payload[0] is the size
/// byte and the data follows it. A size larger than 28 (a garbled answer) is capped, not trusted.
pub fn parse_keymap_buffer(payload: &[u8]) -> &[u8] {
    let n = (payload[0] as usize).min(KEYMAP_BYTES_PER_MSG);
    &payload[1..1 + n]
}

/// Writes one key of the keymap. `row`/`col` are the switch matrix position (see
/// [`crate::keycodes::Board::matrix`]), `code` a QMK key code. Saved on the device immediately.
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
    /// The top corner combo is active.
    pub top_on: bool,
    /// The bottom corner combo is active.
    pub bottom_on: bool,
    /// Key code the top combo sends instead of its built-in job; 0 = built-in job.
    pub top_action: u16,
    /// Key code the bottom combo sends instead of its built-in job; 0 = built-in job.
    pub bottom_action: u16,
    /// The top corner keys keep the active layer's color (cyan A, orange B).
    pub corner_lights: bool,
    /// The top combo is the whole top row (four keys) instead of the two corners.
    pub top_row: bool,
}

/// What a freshly flashed macropad does: both combos on, both doing their built-in jobs.
impl Default for Combos {
    fn default() -> Self {
        Self { top_on: true, bottom_on: true, top_action: 0, bottom_action: 0, corner_lights: false, top_row: false }
    }
}

/// Reads the combos. Keyboards and older macropad firmware answer 0xFF (unsupported).
pub fn combos_request() -> Report {
    get(ValueId::Combos, &[])
}

/// Payload layout: top on, bottom on, top action (2 bytes, big endian), bottom action (2 bytes),
/// corner lights, top row.
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

/// Writes the combos (saved on the macropad). Same layout as [`parse_combos`].
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

/// Reads whether F12 + arrow keys change the lighting (payload byte 0: 1 = on).
pub fn shortcuts_request() -> Report {
    get(ValueId::Shortcuts, &[])
}

/// Turns the F12 + arrow shortcuts on or off (saved on the keyboard).
pub fn set_shortcuts(on: bool) -> Report {
    set(ValueId::Shortcuts, &[on as u8])
}

// ---------------------------------------------------------------------------
// LED calibration: the LED driver's current and per-color gains
// ---------------------------------------------------------------------------

/// How hard the LED driver chip drives the LEDs. Lets the user balance white (the gains) and trade
/// brightness against heat and power (the current).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedCal {
    /// LED driver current (the chip's global current setting).
    pub current: u8,
    /// Red, green, blue gains, 255 = full.
    pub gains: [u8; 3],
    /// The firmware's own current, for "Reset".
    pub default_current: u8,
}

/// Reads the calibration; older firmware answers 0xFF.
pub fn led_cal_request() -> Report {
    get(ValueId::LedCal, &[])
}

/// Payload layout: current, red gain, green gain, blue gain, default current.
pub fn parse_led_cal(p: &[u8]) -> LedCal {
    LedCal { current: p[0], gains: [p[1], p[2], p[3]], default_current: p[4] }
}

/// `save` keeps it after a restart.
/// The default current is read-only, so it is not sent.
pub fn set_led_cal(c: &LedCal, save: bool) -> Report {
    set(ValueId::LedCal, &[c.current, c.gains[0], c.gains[1], c.gains[2], save as u8])
}

// ---------------------------------------------------------------------------
// Joined surface, effect clock, painted keys
// ---------------------------------------------------------------------------

/// Joined surface: this device takes `x_start..=x_end` of the shared 0..=224 space.
/// The firmware squeezes its LED x positions into that slice, so a built-in effect (a wave, a
/// sweep) runs on across both devices as if they were one keyboard. `on = false` goes back to the
/// device's own full 0..=224 space. The slices come from [`crate::builtin::span_slices`].
pub fn set_span(on: bool, x_start: u8, x_end: u8) -> Report {
    set(ValueId::Span, &[on as u8, x_start, x_end])
}

/// Reads the joined-surface setting.
pub fn span_request() -> Report {
    get(ValueId::Span, &[])
}

/// (joined, x_start, x_end).
pub fn parse_span(payload: &[u8]) -> (bool, u8, u8) {
    (payload[0] != 0, payload[1], payload[2])
}

/// Sets the effect clock to `ms`. Sent to both joined devices back to back.
/// The firmware animates its effects from a millisecond timer that starts at power on, so two
/// devices plugged in at different times would be out of step. The engine sends the same value to
/// both (little endian u32) every 10 seconds while joined. Wraps after about 49 days, harmlessly.
pub fn set_clock(ms: u32) -> Report {
    set(ValueId::Clock, &ms.to_le_bytes())
}

/// One LED's paint: `None` = not painted.
/// `Some((hue, saturation))` on QMK's 0..=255 scales; brightness comes from the effect.
pub type Paint = Option<(u8, u8)>;

/// PAINT set messages for every LED (index order).
/// Each message: `[start index, count, then count x (hue, sat, painted 1/0)]`, 8 LEDs at most so the
/// message stays within 29 bytes (see [`FRAME_LEDS_PER_MSG`] for why). 97 keyboard LEDs take 13.
pub fn encode_paint(leds: &[Paint]) -> Vec<Report> {
    leds.chunks(PAINT_PER_SET_MSG)
        .enumerate()
        .map(|(i, chunk)| {
            // The start index fits in a byte: the keyboard has 97 LEDs, the macropad 24.
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

/// GET requests that read every LED's paint, 9 per answer, each carrying its start index.
pub fn paint_requests(led_count: u8) -> Vec<Report> {
    (0..led_count as usize).step_by(PAINT_PER_GET_MSG).map(|s| get(ValueId::Paint, &[s as u8])).collect()
}

/// Decodes one PAINT answer: `[start, count, count x (hue, sat, painted)]`. The start byte is not
/// needed because answers come back in request order. A count above 9 is capped.
pub fn parse_paint(payload: &[u8]) -> Vec<Paint> {
    let n = (payload[1] as usize).min(PAINT_PER_GET_MSG);
    (0..n)
        .map(|i| {
            let b = &payload[2 + i * 3..5 + i * 3];
            // The third byte is the "painted" flag; hue and saturation of an unpainted LED mean nothing.
            (b[2] != 0).then_some((b[0], b[1]))
        })
        .collect()
}

/// One painted-keys operation (see [`PaintOp`]).
pub fn paint_ctrl(op: PaintOp) -> Report {
    set(ValueId::PaintCtrl, &[op as u8])
}

/// Reads the paint mode and how many LEDs are painted.
pub fn paint_ctrl_request() -> Report {
    get(ValueId::PaintCtrl, &[])
}

/// (follow the effect, number of painted LEDs)
/// Note: byte 0 is the mode number; any non-zero mode (follow or tint) reads as "follow" here.
pub fn parse_paint_ctrl(payload: &[u8]) -> (bool, u8) {
    (payload[0] != 0, payload[1])
}

// ---------------------------------------------------------------------------
// LED positions
// ---------------------------------------------------------------------------

/// LED position in QMK's coordinate space (x 0..=224, y 0..=64) + LED flags.
/// The Framework QMK sources define these per LED; the app keeps its own copy in [`crate::keys`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedPoint {
    /// 0 (left edge) to 224 (right edge) of this device, whatever its physical width.
    pub x: u8,
    /// 0 (top) to 64 (bottom).
    pub y: u8,
    /// QMK LED flags; 4 = "key light" (an LED under a key, not underglow).
    pub flags: u8,
}

/// Asks for the positions of up to 9 LEDs starting at LED index `start`.
pub fn led_info_request(start: u8) -> Report {
    get(ValueId::LedInfo, &[start])
}

/// All LED_INFO requests needed to read `led_count` LEDs.
pub fn led_info_requests(led_count: u8) -> Vec<Report> {
    (0..led_count as usize).step_by(INFO_LEDS_PER_MSG).map(|start| led_info_request(start as u8)).collect()
}

/// Decodes `[start, count, count x (x, y, flags)]`; a count above 9 is capped so a garbled answer
/// can't read past the payload.
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
/// Each message is `[first LED index, count, count x (R, G, B)]` with at most 8 LEDs, so the
/// keyboard's 97 LEDs take 13 messages and the macropad's 24 take 3. At 30 frames a second that is
/// 480 reports a second for both devices together.
pub fn encode_frame(colors: &[[u8; 3]]) -> Vec<Report> {
    colors
        .chunks(FRAME_LEDS_PER_MSG)
        .enumerate()
        .map(|(i, chunk)| {
            let mut payload = Vec::with_capacity(2 + chunk.len() * 3);
            // Start index of this chunk, then how many LEDs follow, then 3 color bytes per LED.
            payload.push((i * FRAME_LEDS_PER_MSG) as u8);
            payload.push(chunk.len() as u8);
            for c in chunk {
                payload.extend_from_slice(c);
            }
            set(ValueId::Frame, &payload)
        })
        .collect()
}

/// Polls for key presses. Each poll empties the device's queue, so presses are reported once.
pub fn key_events_request() -> Report {
    get(ValueId::KeyEvents, &[])
}

/// LED indices of keys pressed since the last poll.
/// Payload: `[count, LED index...]`. Indices are LED numbers (not key numbers), so the engine can
/// place a ripple at that LED's position. A count above 28 is capped, never trusted.
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

/// A display name for an effect id. The host effect is the one the device reports while the PC
/// draws; unknown ids (newer firmware) get a plain "Effect N" instead of failing.
pub fn effect_name(mode: u8, host_effect: u8) -> String {
    if mode == host_effect {
        return "PC controlled (Linked)".to_string();
    }
    EFFECT_NAMES.get(mode as usize).map(|s| s.to_string()).unwrap_or_else(|| format!("Effect {mode}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GET and SET requests start with the VIA custom header and are always 32 bytes.
    #[test]
    fn requests_have_via_custom_header() {
        let r = get(ValueId::Info, &[]);
        assert_eq!(&r[..3], &[0x08, 0x00, 0x01]);
        let r = set_hotkey_lock(true);
        assert_eq!(&r[..4], &[0x07, 0x00, 0x03, 0x01]);
        assert_eq!(r.len(), 32);
    }

    /// The bootloader request is exactly Framework's factory command, nothing else set.
    #[test]
    fn bootloader_is_framework_factory_command() {
        let r = bootloader_request();
        assert_eq!(&r[..2], &[0x0B, 0xFE]);
        assert!(r[2..].iter().all(|&b| b == 0));
    }

    /// SPAN, CLOCK (little endian), PAINT chunking and PAINT_CTRL bytes, plus never using byte 31.
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

    /// Answers are accepted, refused as unsupported (0xFF), or skipped as belonging to another request.
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

    /// Every INFO field lands in the right place, including the three build-number cases and errors.
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

    /// The build number is found inside a firmware image, and 0 is read as a test build.
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

    /// Keymap reads cover the whole buffer in 28 byte chunks; key, combo and calibration writes encode.
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

    /// A SET STATE payload decodes back to the same state through the GET parser.
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

    /// Effect 0 and ids at or past the firmware's effect count are refused before sending.
    #[test]
    fn set_state_rejects_bad_effect() {
        let mut s = LightState { mode: 0, ..Default::default() };
        assert!(set_state(&s, false, 46).is_err());
        s.mode = 46;
        assert!(set_state(&s, false, 46).is_err());
        s.mode = 45;
        assert!(set_state(&s, false, 46).is_ok());
    }

    /// Frames split into 8-LED messages with the right start indices, never using the last bytes.
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

    /// LED_INFO requests cover all 97 LEDs, 9 at a time, and answers decode.
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

    /// A garbage key count is capped instead of reading past the payload.
    #[test]
    fn key_events_are_clamped() {
        let mut p = [0u8; 29];
        p[0] = 3;
        p[1..4].copy_from_slice(&[5, 6, 7]);
        assert_eq!(parse_key_events(&p), vec![5, 6, 7]);
        p[0] = 200; // garbage count must not panic
        assert_eq!(parse_key_events(&p).len(), KEYS_PER_MSG);
    }

    /// Effect names line up with the firmware's effect numbers.
    #[test]
    fn effect_names_match_firmware_numbering() {
        // firmware: RGB_MATRIX_TYPING_HEATMAP == 31, SOLID_MULTISPLASH == 44, host effect == 45
        assert_eq!(EFFECT_NAMES[31], "Typing heatmap");
        assert_eq!(EFFECT_NAMES.len(), 45);
        assert_eq!(effect_name(45, 45), "PC controlled (Linked)");
        assert_eq!(effect_name(60, 45), "Effect 60");
    }
}
