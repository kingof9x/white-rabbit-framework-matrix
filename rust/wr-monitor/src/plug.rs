//! Noticing things being plugged into the laptop's ports and unplugged, and working out which port: a
//! port of `led_mon/plug_events.py`. The Windows side (the hidden window that hears about devices and
//! the charger, and the device properties) is in `plugwin.rs`; the decisions are here, so they can be
//! tested anywhere.
//!
//! Where the port is:
//!   - The charger: framework_tool --pd-info says which USB-C port is charging the laptop. When it
//!     can't run, the port picked on the Effects tab the last time is used.
//!   - A USB device: a port picked for it on the Effects tab (remembered by the USB root port the
//!     device is on), else the side of the laptop Windows reports for the port (ACPI _PLD), else unknown.
//!
//! Threads: `PlugWatcher`'s methods are called from the `plug-events` thread (plugwin.rs). The `fire`
//! callback (built in `app.rs`) then starts the ripple/drain effect on the panels and stores the event
//! for the window. A position is written like "left", "right-back" or "left-middle".

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::Instant;

use serde_json::{json, Value};

/// Framework's USB vendor id; with a product id it identifies Framework's own modules.
pub const FRAMEWORK_VID: u16 = 0x32AC;
/// Product id of the Framework audio expansion card (the headphone jack).
const AUDIO_CARD_PID: u16 = 0x0010;
/// Devices arriving right after waking up are the ones that were already there.
const QUIET_AFTER_RESUME_SEC: f64 = 10.0;
/// A hub or dock brings several devices at once: one ripple per side for those.
const SAME_SIDE_GAP_SEC: f64 = 1.5;

/// Turns a device interface path (what WM_DEVICECHANGE reports) into the device's instance id (what
/// the device properties are looked up by), e.g.
/// `\\?\USB#VID_32AC&PID_0010#123#{a5dcbf10-...}` -> `USB\VID_32AC&PID_0010\123`.
/// The `{...}` at the end is the interface class GUID and is dropped; the result is upper case so ids
/// compare equal however Windows cased them. None for an empty name.
pub fn instance_id_from_interface(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let mut text = name;
    for prefix in [r"\\?\", r"\??\", r"\\.\"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest;
            break;
        }
    }
    if let Some(i) = text.rfind("#{") {
        text = &text[..i];
    }
    Some(text.replace('#', "\\").to_uppercase())
}

/// The USB vendor and product id in an instance id such as `USB\VID_32AC&PID_0010\...` (hex digits).
/// None when the id has no `VID_xxxx&PID_xxxx` part (not a USB device).
pub fn vid_pid(instance_id: &str) -> Option<(u16, u16)> {
    let upper = instance_id.to_uppercase();
    let v = upper.find("VID_")?;
    let vid = u16::from_str_radix(upper.get(v + 4..v + 8)?, 16).ok()?;
    let rest = upper.get(v + 8..)?;
    let p = rest.strip_prefix("&PID_")?;
    let pid = u16::from_str_radix(p.get(..4)?, 16).ok()?;
    Some((vid, pid))
}

/// Framework input modules (keyboards, numpad, macropad, LED matrix): moving them isn't plugging anything in.
pub fn is_ignored(instance_id: &str) -> bool {
    // Framework's input-module product ids are 0x0012-0x002F; the audio card (0x0010) is not ignored.
    matches!(vid_pid(instance_id), Some((FRAMEWORK_VID, pid)) if (0x0012..0x0030).contains(&pid))
}

/// A screen (Windows' monitor devices, `DISPLAY\...`): one plugged into an HDMI or DisplayPort card.
pub fn is_display(instance_id: &str) -> bool {
    instance_id.to_uppercase().starts_with("DISPLAY\\")
}

/// Where a screen is plugged in, to remember its port by: the graphics card's output it is on (its
/// location path, e.g. `...#ACPI(GFX0)#ACPI(DD1F)`), else the screen itself.
pub fn display_key(instance_id: &str, location_paths: &[String]) -> String {
    location_paths.first().map(|p| p.to_uppercase()).unwrap_or_else(|| instance_id.to_uppercase())
}

/// True for the Framework audio expansion card, whose plug events count as "headphones".
pub fn is_audio_card(instance_id: &str) -> bool {
    vid_pid(instance_id) == Some((FRAMEWORK_VID, AUDIO_CARD_PID))
}

/// The USB root port a device is on, from its location paths:
/// `PCIROOT(0)#PCI(0801)#PCI(0004)#USBROOT(0)#USB(2)#USB(1)` -> `...#USBROOT(0)#USB(2)`.
/// Anything plugged into the same physical port (directly or through a hub) gets the same key, which
/// is what the Effects tab's learned ports are stored by. None when no path goes through USB.
pub fn location_key(location_paths: &[String]) -> Option<String> {
    for path in location_paths {
        let upper = path.to_uppercase();
        let Some(root) = upper.find("#USBROOT(") else { continue };
        let after_root = &upper[root..];
        let Some(close) = after_root.find(')') else { continue };
        let rest = &after_root[close + 1..];
        let Some(port) = rest.strip_prefix("#USB(") else { continue };
        let Some(end) = port.find(')') else { continue };
        let digits = &port[..end];
        let root_digits = &after_root["#USBROOT(".len()..close];
        // Both numbers must be plain digits; otherwise this path is malformed, try the next one.
        if digits.is_empty()
            || !digits.chars().all(|c| c.is_ascii_digit())
            || root_digits.is_empty()
            || !root_digits.chars().all(|c| c.is_ascii_digit())
        {
            continue;
        }
        // Everything up to and including the first `#USB(n)` after `#USBROOT(m)`.
        return Some(upper[..root + close + 1 + "#USB(".len() + end + 1].to_string());
    }
    None
}

/// Which side of the laptop an ACPI _PLD buffer puts a port on, and its raw numbers for the log.
///
/// _PLD ("physical location of device") is a packed bit field from the laptop's firmware. Bits 67-69
/// are the panel (2 = left side, 3 = right side; others are top/bottom/front/back/unknown), bits 70-71
/// the vertical and 72-73 the horizontal position. Buffers shorter than 10 bytes don't reach those bits.
pub fn pld_panel(buffer: Option<&[u8]>) -> (Option<&'static str>, Option<Value>) {
    let Some(buffer) = buffer.filter(|b| b.len() >= 10) else { return (None, None) };
    let mut bytes = [0u8; 16];
    let n = buffer.len().min(16);
    bytes[..n].copy_from_slice(&buffer[..n]);
    // The first 16 bytes as one little-endian 128-bit number, so a bit field is just a shift and mask.
    let bits = u128::from_le_bytes(bytes);
    let panel = ((bits >> 67) & 0x7) as u8;
    let vertical = ((bits >> 70) & 0x3) as u8;
    let horizontal = ((bits >> 72) & 0x3) as u8;
    let side = match panel {
        2 => Some("left"),
        3 => Some("right"),
        _ => None,
    };
    (side, Some(json!({"panel": panel, "vertical": vertical, "horizontal": horizontal})))
}

/// The port charging the laptop from `framework_tool --pd-info` output, as "right-back" etc.
///
/// The tool prints one block per port, a `USB-C Port N (Right Back):` header then a `Role:` line;
/// the laptop is charging from the port whose role is "Sink". None if no port is a sink or the
/// output isn't understood.
pub fn parse_pd_info(text: &str) -> Option<String> {
    let mut port_name: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();
        if let Some(rest) = lower.strip_prefix("usb-c port") {
            let rest = rest.trim_start();
            let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
            let after = rest[digits..].trim_start();
            if digits > 0 {
                if let Some(inner) = after.strip_prefix('(').and_then(|r| r.split(')').next()) {
                    // Only a header with a closing bracket counts; remember its name for the Role line.
                    if after[1..].contains(')') {
                        port_name = Some(inner.to_string());
                        continue;
                    }
                }
            }
        }
        if let Some(role) = line.trim_start().to_lowercase().strip_prefix("role:") {
            let role: String = role.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if let (Some(name), "sink") = (&port_name, role.as_str()) {
                let words: Vec<&str> = name.split_whitespace().collect();
                let side = words.iter().find(|w| ["left", "right"].contains(w));
                let spot = words.iter().find(|w| ["back", "middle", "front"].contains(w));
                if let Some(side) = side {
                    return Some(match spot {
                        Some(spot) => format!("{side}-{spot}"),
                        None => side.to_string(),
                    });
                }
            }
        }
    }
    None
}

/// Where a USB device's port is, and how we know: (position, "learned" | "side" | "unknown").
///
/// A port the user taught on the Effects tab wins, then the side from _PLD.
pub fn resolve_position(
    key: Option<&str>,
    learned: &BTreeMap<String, String>,
    pld_side: Option<&str>,
) -> (Option<String>, &'static str) {
    if let Some(p) = key.and_then(|k| learned.get(k)) {
        return (Some(p.clone()), "learned");
    }
    if let Some(side) = pld_side {
        return (Some(side.to_string()), "side");
    }
    (None, "unknown")
}

/// What Windows says about a USB device.
///
/// Read by plugwin.rs from the Windows device properties.
#[derive(Clone, Debug, Default)]
pub struct DeviceProps {
    /// DEVPKEY_Device_LocationPaths: the device's place in the bus tree (see `location_key`).
    pub location_paths: Vec<String>,
    /// The raw ACPI _PLD buffer of its port, if the firmware gives one.
    pub pld: Option<Vec<u8>>,
    /// A friendly name ("USB Mass Storage Device"), for the log and the Effects tab.
    pub name: Option<String>,
}

/// Something plugged in or out, for the effects and the Effects tab.
#[derive(Clone, Debug, PartialEq)]
pub struct PlugEvent {
    /// "charger", "headphones", "display" or "usb".
    pub kind: &'static str,
    /// Which port: "left", "right-back" ...; None when unknown.
    pub position: Option<String>,
    /// True for plugged in, false for unplugged.
    pub plugged_in: bool,
    /// The device's name (or its instance id when it has none).
    pub name: String,
    /// What the port is remembered by when the user corrects it (`location_key`, or "charger").
    pub key: Option<String>,
    /// How the position was found: "learned", "side", "framework_tool", "remembered" or "unknown".
    pub how: &'static str,
}

impl PlugEvent {
    /// The Effects tab's "last_plug": {kind, position, plugged_in, name, key, how, time}.
    pub fn to_json(&self, time: f64) -> Value {
        json!({"kind": self.kind, "position": self.position, "plugged_in": self.plugged_in,
               "name": self.name, "key": self.key, "how": self.how, "time": time})
    }
}

/// A device that is plugged in now, remembered so its unplug event can say where it was.
#[derive(Clone)]
struct Present {
    key: Option<String>,
    position: Option<String>,
    how: &'static str,
    name: String,
    kind: &'static str,
}

/// Everything `PlugWatcher` remembers between messages; behind one mutex.
#[derive(Default)]
struct State {
    /// Instance id -> the device's details, for every device plugged in.
    present: HashMap<String, Present>,
    /// Last known "on mains power" state; None until known.
    ac_online: Option<bool>,
    /// Where the charger was found when it was plugged in, used again for its unplug.
    charger_position: Option<String>,
    /// Seconds (since the watcher was made) until which events are not fired: just after waking up.
    quiet_until: f64,
    /// Side ("left", "right", None) -> when its last plug-in effect fired, for `SAME_SIDE_GAP_SEC`.
    last_side_time: HashMap<Option<String>, f64>,
}

/// Decides what each Windows message means. `learned` gives the Effects tab's corrected ports, `fire`
/// plays the effect.
pub struct PlugWatcher {
    /// What is plugged in and the timing state.
    state: Mutex<State>,
    /// When the watcher was made; times in `State` are seconds since then.
    start: Instant,
    /// Returns the learned ports (key -> position) from the Effects settings.
    pub learned: Box<dyn Fn() -> BTreeMap<String, String> + Send + Sync>,
    /// Plays the effect for an event and records it (never called with the state lock held).
    pub fire: Box<dyn Fn(PlugEvent) + Send + Sync>,
}

impl PlugWatcher {
    /// A watcher with nothing plugged in yet.
    pub fn new(
        learned: Box<dyn Fn() -> BTreeMap<String, String> + Send + Sync>,
        fire: Box<dyn Fn(PlugEvent) + Send + Sync>,
    ) -> PlugWatcher {
        PlugWatcher { state: Mutex::default(), start: Instant::now(), learned, fire }
    }

    /// Seconds since the watcher was made.
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Works out kind, key, position and name for a device, plus the raw _PLD numbers for the log.
    fn present_entry(&self, instance_id: &str, props: &DeviceProps) -> (Present, Option<Value>) {
        let display = is_display(instance_id);
        // A screen is remembered by the graphics output it's on; a USB device by its root port.
        let key = if display {
            Some(display_key(instance_id, &props.location_paths))
        } else {
            location_key(&props.location_paths)
        };
        let (side, raw) = pld_panel(props.pld.as_deref());
        let (position, how) = resolve_position(key.as_deref(), &(self.learned)(), side);
        let kind = if display {
            "display"
        } else if is_audio_card(instance_id) {
            "headphones"
        } else {
            "usb"
        };
        let name = props.name.clone().unwrap_or_else(|| instance_id.to_string());
        (Present { key, position, how, name, kind }, raw)
    }

    /// A device that was already there when the monitor started.
    /// No effect is played; this only lets its later unplug be placed.
    pub fn remember(&self, instance_id: &str, props: &DeviceProps) {
        let (entry, _) = self.present_entry(instance_id, props);
        self.state.lock().unwrap().present.insert(instance_id.to_string(), entry);
    }

    /// Records the power state at start-up without firing anything.
    pub fn set_ac_online(&self, online: Option<bool>) {
        self.state.lock().unwrap().ac_online = online;
    }

    /// The PC just woke up: ignore plug events for the next 10 s, since Windows re-announces
    /// devices that were plugged in all along.
    pub fn resumed(&self) {
        self.state.lock().unwrap().quiet_until = self.now() + QUIET_AFTER_RESUME_SEC;
    }

    /// A device was plugged in: remember it, log where, and fire a plug-in event.
    pub fn device_arrived(&self, instance_id: &str, props: &DeviceProps) {
        if is_ignored(instance_id) {
            return;
        }
        let (entry, raw) = self.present_entry(instance_id, props);
        self.state.lock().unwrap().present.insert(instance_id.to_string(), entry.clone());
        wr_core::info!(
            "Plugged in: {} ({instance_id}) at {}, port {} ({}), _PLD {}",
            entry.name,
            entry.key.as_deref().unwrap_or("?"),
            entry.position.as_deref().unwrap_or("unknown"),
            entry.how,
            raw.map(|r| r.to_string()).unwrap_or_else(|| "none".into())
        );
        self.send(PlugEvent {
            kind: entry.kind,
            position: entry.position,
            plugged_in: true,
            name: entry.name,
            key: entry.key,
            how: entry.how,
        });
    }

    /// A device was unplugged: fire an unplug event at the port it was on. Devices we never saw
    /// arrive (or ignored ones) are skipped.
    pub fn device_removed(&self, instance_id: &str) {
        if is_ignored(instance_id) {
            return;
        }
        let Some(entry) = self.state.lock().unwrap().present.remove(instance_id) else { return };
        // A port corrected since it was plugged in
        let position = match &entry.key {
            Some(k) => (self.learned)().get(k).cloned().or(entry.position),
            None => entry.position,
        };
        wr_core::info!("Unplugged: {} from port {}", entry.name, position.as_deref().unwrap_or("unknown"));
        self.send(PlugEvent {
            kind: entry.kind,
            position,
            plugged_in: false,
            name: entry.name,
            key: entry.key,
            how: entry.how,
        });
    }

    /// The power source changed. `charger_port` asks framework_tool where the charger is (slow).
    /// Fires only on a real change (Windows repeats power messages); `charger_port` is only called
    /// when the charger was just plugged in. The state lock is not held while it runs.
    pub fn power_changed(&self, online: Option<bool>, charger_port: impl FnOnce() -> Option<String>) {
        {
            let mut s = self.state.lock().unwrap();
            if online.is_none() || online == s.ac_online {
                return;
            }
            s.ac_online = online;
        }
        if online == Some(true) {
            let (mut position, mut how) = (charger_port(), "framework_tool");
            if position.is_none() {
                position = (self.learned)().get("charger").cloned();
                how = if position.is_some() { "learned" } else { "unknown" };
            }
            self.state.lock().unwrap().charger_position = position.clone();
            wr_core::info!("Charger plugged in at port {} ({how})", position.as_deref().unwrap_or("unknown"));
            self.send(PlugEvent {
                kind: "charger",
                position,
                plugged_in: true,
                name: "Charger".into(),
                key: Some("charger".into()),
                how,
            });
        } else {
            let position = self.state.lock().unwrap().charger_position.clone();
            self.send(PlugEvent {
                kind: "charger",
                position,
                plugged_in: false,
                name: "Charger".into(),
                key: Some("charger".into()),
                how: "remembered",
            });
        }
    }

    /// Fires an event unless it falls in the quiet time after waking, or it is a second plug-in on the
    /// same side within 1.5 s (a dock bringing several devices). Unplugs and the charger always fire.
    fn send(&self, event: PlugEvent) {
        let now = self.now();
        {
            let mut s = self.state.lock().unwrap();
            if now < s.quiet_until {
                return;
            }
            if event.kind != "charger" && event.plugged_in {
                let side = event
                    .position
                    .as_deref()
                    // "left-back" -> "left": the side is the part before the dash.
                    .and_then(|p| p.split('-').next())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                if now - s.last_side_time.get(&side).copied().unwrap_or(-99.0) < SAME_SIDE_GAP_SEC {
                    return;
                }
                s.last_side_time.insert(side, now);
            }
        }
        // Outside the lock: the effect code may take other locks, and must not wait on ours.
        (self.fire)(event);
    }
}

/// Tests for the device id parsing, port finding and event timing.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// The interface path's prefix and class GUID are dropped and `#` becomes `\`.
    #[test]
    fn interface_names_become_instance_ids() {
        assert_eq!(
            instance_id_from_interface(r"\\?\USB#VID_32AC&PID_0010#123#{a5dcbf10-6530-11d2-901f-00c04fb951ed}"),
            Some(r"USB\VID_32AC&PID_0010\123".into())
        );
        assert_eq!(instance_id_from_interface(""), None);
    }

    /// Framework input modules are ignored, the audio card isn't, other vendors aren't.
    #[test]
    fn input_modules_are_ignored_and_the_audio_card_is_headphones() {
        assert!(is_ignored(r"USB\VID_32AC&PID_0020\X"));
        assert!(!is_ignored(r"USB\VID_32AC&PID_0010\X"));
        assert!(is_audio_card(r"USB\VID_32AC&PID_0010\X"));
        assert!(!is_ignored(r"USB\VID_046D&PID_C52B\X"));
        assert_eq!(vid_pid("nothing"), None);
    }

    /// Screens are recognised, keyed by their graphics output, and placed by a learned port.
    #[test]
    fn screens_are_displays_remembered_by_their_output() {
        assert!(is_display(r"DISPLAY\GSM5B7F\5&1A2B&0&UID4352"));
        assert!(!is_display(r"USB\VID_32AC&PID_0010\X"));
        assert_eq!(
            instance_id_from_interface(r"\\?\DISPLAY#GSM5B7F#5&1a2b&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"),
            Some(r"DISPLAY\GSM5B7F\5&1A2B&0&UID4352".into())
        );
        let paths = vec!["ACPI(_SB_)#ACPI(PC00)#ACPI(GFX0)#ACPI(DD1F)".to_string()];
        assert_eq!(display_key("DISPLAY\\X", &paths), "ACPI(_SB_)#ACPI(PC00)#ACPI(GFX0)#ACPI(DD1F)");
        assert_eq!(display_key(r"display\x\1", &[]), r"DISPLAY\X\1");
        let (events, w) = watcher(BTreeMap::from([(paths[0].clone(), "left-back".to_string())]));
        w.device_arrived(
            r"DISPLAY\GSM5B7F\1",
            &DeviceProps { location_paths: paths, pld: None, name: Some("LG".into()) },
        );
        w.device_removed(r"DISPLAY\GSM5B7F\1");
        let got = events.lock().unwrap().clone();
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].kind, got[0].position.as_deref(), got[0].how), ("display", Some("left-back"), "learned"));
        assert!(!got[1].plugged_in);
    }

    /// The key stops at the root port, skipping paths that don't go through USB.
    #[test]
    fn location_key_is_the_root_port() {
        let paths = vec![
            "ACPI(_SB_)#ACPI(PC00)".to_string(),
            "PCIROOT(0)#PCI(0801)#PCI(0004)#USBROOT(0)#USB(2)#USB(1)".to_string(),
        ];
        assert_eq!(location_key(&paths), Some("PCIROOT(0)#PCI(0801)#PCI(0004)#USBROOT(0)#USB(2)".into()));
        assert_eq!(location_key(&["no usb".into()]), None);
    }

    /// The panel bits of _PLD give left/right; a too-short buffer gives nothing.
    #[test]
    fn pld_side() {
        // panel = bits 67-69: 2 is left, 3 is right
        let mut left = [0u8; 16];
        left[8] = 2 << 3;
        assert_eq!(pld_panel(Some(&left)).0, Some("left"));
        let mut right = [0u8; 16];
        right[8] = 3 << 3;
        assert_eq!(pld_panel(Some(&right)).0, Some("right"));
        assert_eq!(pld_panel(Some(&[0u8; 4])).0, None);
    }

    /// The sink port is the charging one; with no sink there is no answer.
    #[test]
    fn pd_info_finds_the_charging_port() {
        let text =
            "USB-C Port 0 (Right Back):\n  Role:          Source\nUSB-C Port 2 (Left Middle):\n  Role:          Sink\n";
        assert_eq!(parse_pd_info(text), Some("left-middle".into()));
        assert_eq!(parse_pd_info("USB-C Port 1 (Right Back):\n  Role: Source\n"), None);
    }

    /// A watcher with fixed learned ports that collects fired events in a list.
    fn watcher(learned: BTreeMap<String, String>) -> (Arc<Mutex<Vec<PlugEvent>>>, PlugWatcher) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let e = events.clone();
        let w = PlugWatcher::new(Box::new(move || learned.clone()), Box::new(move |ev| e.lock().unwrap().push(ev)));
        (events, w)
    }

    /// A USB stick's properties on root port `port`.
    fn props(port: u8) -> DeviceProps {
        DeviceProps {
            location_paths: vec![format!("PCIROOT(0)#PCI(0801)#USBROOT(0)#USB({port})#USB(1)")],
            pld: None,
            name: Some("Stick".into()),
        }
    }

    /// A learned port is used, a second device on the same side right after is not fired, unplug is.
    #[test]
    fn learned_ports_win_and_a_dock_ripples_once_per_side() {
        let key = "PCIROOT(0)#PCI(0801)#USBROOT(0)#USB(2)".to_string();
        let (events, w) = watcher(BTreeMap::from([(key.clone(), "left-back".to_string())]));
        w.device_arrived(r"USB\VID_1234&PID_0001\A", &props(2));
        w.device_arrived(r"USB\VID_1234&PID_0002\B", &props(2));
        w.device_removed(r"USB\VID_1234&PID_0001\A");
        let ev = events.lock().unwrap();
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].position.as_deref(), Some("left-back"));
        assert_eq!(ev[0].how, "learned");
        assert!(!ev[1].plugged_in);
    }

    /// The charger fires on a change only, and its unplug reuses the remembered port.
    #[test]
    fn charger_in_and_out() {
        let (events, w) = watcher(BTreeMap::new());
        w.set_ac_online(Some(false));
        w.power_changed(Some(true), || Some("right-back".into()));
        w.power_changed(Some(true), || panic!("no change, no question"));
        w.power_changed(Some(false), || None);
        let ev = events.lock().unwrap();
        assert_eq!(ev.len(), 2);
        assert_eq!((ev[0].kind, ev[0].position.as_deref(), ev[0].plugged_in), ("charger", Some("right-back"), true));
        assert_eq!((ev[1].position.as_deref(), ev[1].plugged_in), (Some("right-back"), false));
    }

    /// Nothing fires in the first seconds after waking up.
    #[test]
    fn quiet_after_waking_up() {
        let (events, w) = watcher(BTreeMap::new());
        w.resumed();
        w.device_arrived(r"USB\VID_1234&PID_0001\A", &props(2));
        assert!(events.lock().unwrap().is_empty());
    }
}
