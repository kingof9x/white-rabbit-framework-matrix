//! Finding the LED panels among the serial ports, and which one is left and which is right: a port
//! of `led_mon/shared_state.py` (discover_led_devices) and the USB location string pyserial builds on
//! Windows (`serial/tools/list_ports_windows.py`).

use crate::protocol::{LED_MATRIX_PID, LED_MATRIX_VID};

/// One serial port, with what Windows says about the USB device behind it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PortInfo {
    /// The port name, e.g. COM4.
    pub device: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    /// The USB position, e.g. 1-3.2:x.0 (pyserial's format), when Windows reports it.
    pub location: Option<String>,
    pub description: String,
}

/// Which ports or USB positions the layout file pins to a side.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pins {
    pub left_port: Option<String>,
    pub right_port: Option<String>,
    pub left_location: Option<String>,
    pub right_location: Option<String>,
}

pub fn is_led_matrix_port(port: &PortInfo) -> bool {
    (port.vid == Some(LED_MATRIX_VID) && port.pid == Some(LED_MATRIX_PID))
        || port.description.contains("LED Matrix Input Module")
}

/// The location string pyserial makes from SPDRP_LOCATION_PATHS, e.g.
/// `PCIROOT(0)#PCI(0801)#PCI(0004)#USBROOT(0)#USB(3)#USB(2)` with interface 0 gives `1-3.2:x.0`.
pub fn location_from_paths(paths: &str, interface: Option<u32>) -> Option<String> {
    let mut location = String::new();
    let mut parts = 0;
    let mut rest = paths;
    loop {
        let root = rest.find("USBROOT(");
        let usb = rest.find("#USB(");
        let (at, is_root) = match (root, usb) {
            (Some(r), Some(u)) => {
                if r <= u {
                    (r, true)
                } else {
                    (u, false)
                }
            }
            (Some(r), None) => (r, true),
            (None, Some(u)) => (u, false),
            (None, None) => break,
        };
        let start = at + if is_root { "USBROOT(".len() } else { "#USB(".len() };
        let Some(len) = rest[start..].find(')') else { break };
        let word = &rest[start..start + len];
        rest = &rest[start + len + 1..];
        if word.is_empty() || !word.chars().all(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        if is_root {
            let Ok(n) = word.parse::<i64>() else { continue };
            location.push_str(&(n + 1).to_string());
        } else {
            location.push(if parts > 1 { '.' } else { '-' });
            location.push_str(word);
        }
        parts += if is_root { 1 } else { 2 };
    }
    if let Some(i) = interface {
        location.push_str(&format!(":x.{i}"));
    }
    if location.is_empty() {
        None
    } else {
        Some(location)
    }
}

/// `VID_32AC&PID_0020&MI_00` in a hardware ID: (vid, pid, interface number).
pub fn usb_ids(id: &str) -> (Option<u16>, Option<u16>, Option<u32>) {
    let upper = id.to_uppercase();
    let hex_after = |tag: &str| {
        let at = upper.find(tag)? + tag.len();
        u16::from_str_radix(upper.get(at..at + 4)?, 16).ok()
    };
    let vid = hex_after("VID_");
    let pid = vid.and(hex_after("&PID_"));
    let interface = upper.find("&MI_").and_then(|at| upper.get(at + 4..at + 6)?.parse::<u32>().ok());
    (vid, pid, interface)
}

/// Sort key: ports with a location first, by the location without its bus and hub
/// (`1-3.2:1.0` sorts as `2:1.0`), then by port name.
fn sort_key(location: &Option<String>, device: &str) -> (u8, String, String) {
    match location {
        Some(l) if !l.is_empty() => (0, strip_bus(l), device.to_string()),
        _ => (1, String::new(), device.to_string()),
    }
}

/// `re.sub(r'^\d+\-\d+\.', '', location)`.
fn strip_bus(location: &str) -> String {
    let b = location.as_bytes();
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let a = digits(0);
    if a == 0 || b.get(a) != Some(&b'-') {
        return location.to_string();
    }
    let c = digits(a + 1);
    if c == 0 || b.get(a + 1 + c) != Some(&b'.') {
        return location.to_string();
    }
    location[a + 1 + c + 1..].to_string()
}

fn find_port_at_location(ports: &[PortInfo], location: &str) -> Option<String> {
    ports
        .iter()
        .find(|p| p.location.as_deref().is_some_and(|l| l.starts_with(location)) && is_led_matrix_port(p))
        .map(|p| p.device.clone())
}

/// `[(location, port name)]` of the LED panels, left first. A port pin wins over a location pin for
/// the same side; a side whose location isn't found is found automatically.
pub fn discover(ports: &[PortInfo], pins: &Pins) -> Vec<(Option<String>, String)> {
    let mut found: Vec<(Option<String>, String)> =
        ports.iter().filter(|p| is_led_matrix_port(p)).map(|p| (p.location.clone(), p.device.clone())).collect();
    found.sort_by_key(|(l, d)| sort_key(l, d));

    let nonempty = |s: &Option<String>| s.clone().filter(|s| !s.is_empty());
    let mut left = nonempty(&pins.left_port);
    let mut right = nonempty(&pins.right_port);
    for (side, location) in [(&mut left, &pins.left_location), (&mut right, &pins.right_location)] {
        if let Some(location) = nonempty(location) {
            if side.is_none() {
                *side = find_port_at_location(ports, &location);
            }
        }
    }
    if left.is_none() && right.is_none() {
        return found;
    }
    let location_of = |port: &str| {
        ports.iter().find(|p| p.device.to_lowercase() == port.to_lowercase()).and_then(|p| p.location.clone())
    };
    let pinned: Vec<String> = [&left, &right].iter().filter_map(|p| p.as_ref().map(|p| p.to_lowercase())).collect();
    let mut remaining: Vec<_> = found.into_iter().filter(|(_, d)| !pinned.contains(&d.to_lowercase())).collect();
    let mut result = Vec::new();
    for side in [left, right] {
        match side {
            Some(port) => result.push((location_of(&port), port)),
            None if !remaining.is_empty() => result.push(remaining.remove(0)),
            None => {}
        }
    }
    result
}

/// The port a panel's drawing should use now: the one at its own location (or port name) first, else
/// the panel now on its side (0 left, 1 right), else any free one. `taken` says whether the other
/// side's drawing already has a port.
pub fn find_panel_port(
    ports: &[PortInfo],
    pins: &Pins,
    location: Option<&str>,
    side: usize,
    taken: &dyn Fn(&str) -> bool,
) -> Option<String> {
    if let Some(location) = location.filter(|l| !l.is_empty()) {
        if let Some(p) = ports.iter().find(|p| p.device.eq_ignore_ascii_case(location) && !taken(&p.device)) {
            return Some(p.device.clone());
        }
        if let Some(p) = ports.iter().find(|p| {
            p.location.as_deref().is_some_and(|l| l.starts_with(location)) && is_led_matrix_port(p) && !taken(&p.device)
        }) {
            return Some(p.device.clone());
        }
    }
    let found = discover(ports, pins);
    if let Some((_, port)) = found.get(side) {
        if !taken(port) {
            return Some(port.clone());
        }
    }
    found.into_iter().map(|(_, p)| p).find(|p| !taken(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(device: &str, location: Option<&str>) -> PortInfo {
        PortInfo {
            device: device.into(),
            vid: Some(LED_MATRIX_VID),
            pid: Some(LED_MATRIX_PID),
            location: location.map(String::from),
            description: "n/a".into(),
        }
    }

    fn other(device: &str, location: Option<&str>) -> PortInfo {
        PortInfo { vid: Some(0x8087), pid: Some(1), ..port(device, location) }
    }

    fn devices(found: Vec<(Option<String>, String)>) -> Vec<String> {
        found.into_iter().map(|(_, d)| d).collect()
    }

    fn none() -> Pins {
        Pins::default()
    }

    #[test]
    fn location_like_pyserial() {
        let paths = "PCIROOT(0)#PCI(0801)#PCI(0004)#USBROOT(0)#USB(3)#USB(2)";
        assert_eq!(location_from_paths(paths, Some(0)).as_deref(), Some("1-3.2:x.0"));
        assert_eq!(location_from_paths("PCIROOT(0)#PCI(1400)#USBROOT(0)#USB(4)", None).as_deref(), Some("1-4"));
        assert_eq!(location_from_paths("ACPI(_SB_)", None), None);
    }

    #[test]
    fn usb_ids_from_the_instance_id() {
        assert_eq!(usb_ids(r"USB\VID_32AC&PID_0020&MI_00\7&1A2B3C&0&0000"), (Some(0x32AC), Some(0x20), Some(0)));
        assert_eq!(usb_ids(r"USB\VID_8087&PID_0026\5&abc"), (Some(0x8087), Some(0x26), None));
        assert_eq!(usb_ids("ACPI\\PNP0501"), (None, None, None));
    }

    #[test]
    fn strips_bus_and_hub() {
        assert_eq!(strip_bus("1-3.2:1.0"), "2:1.0");
        assert_eq!(strip_bus("1-3"), "1-3");
    }

    #[test]
    fn matches_by_vid_pid_and_description() {
        let found = discover(&[other("COM3", None), port("COM4", Some("1-3.2:x.0"))], &none());
        assert_eq!(found, vec![(Some("1-3.2:x.0".into()), "COM4".into())]);
        let by_name =
            PortInfo { vid: None, pid: None, description: "LED Matrix Input Module".into(), ..port("COM9", None) };
        assert_eq!(devices(discover(&[by_name], &none())), ["COM9"]);
    }

    #[test]
    fn sorted_left_to_right() {
        let ports = [port("COM6", Some("1-3.3:1.0")), port("COM5", Some("1-3.2:1.0"))];
        assert_eq!(devices(discover(&ports, &none())), ["COM5", "COM6"]);
        assert_eq!(devices(discover(&[port("COM7", None), port("COM4", None)], &none())), ["COM4", "COM7"]);
        assert_eq!(
            devices(discover(&[port("COM2", None), port("COM9", Some("1-3.2:x.0"))], &none())),
            ["COM9", "COM2"]
        );
    }

    #[test]
    fn pins() {
        let ports = [port("COM4", Some("1-3.2:x.0")), port("COM5", Some("1-3.3:x.0"))];
        let both = Pins { left_port: Some("COM5".into()), right_port: Some("COM4".into()), ..none() };
        assert_eq!(
            discover(&ports, &both),
            vec![(Some("1-3.3:x.0".into()), "COM5".into()), (Some("1-3.2:x.0".into()), "COM4".into())]
        );
        let right = Pins { right_port: Some("com4".into()), ..none() };
        assert_eq!(devices(discover(&ports, &right)), ["COM5", "com4"]);
        let loc = Pins { left_location: Some("1-3.3".into()), ..none() };
        assert_eq!(devices(discover(&ports, &loc)), ["COM5", "COM4"]);
        let missing = Pins { left_location: Some("9-9.9".into()), ..none() };
        assert_eq!(devices(discover(&ports, &missing)), ["COM4", "COM5"]);
        let port_wins = Pins { left_port: Some("COM4".into()), left_location: Some("1-3.3".into()), ..none() };
        assert_eq!(devices(discover(&ports, &port_wins)), ["COM4", "COM5"]);
        let not_panel = [other("COM3", Some("1-4.2:x.0")), port("COM4", Some("1-3.2:x.0"))];
        let pin = Pins { left_location: Some("1-4.2".into()), ..none() };
        assert_eq!(devices(discover(&not_panel, &pin)), ["COM4"]);
    }

    #[test]
    fn a_moved_panel_is_found_on_its_side() {
        let ports = [port("COM8", Some("1-4.2:x.0")), port("COM9", Some("1-4.3:x.0"))];
        let free = |_: &str| false;
        assert_eq!(find_panel_port(&ports, &none(), Some("1-3.2"), 1, &free).as_deref(), Some("COM9"));
        assert_eq!(find_panel_port(&ports, &none(), Some("1-4.3"), 0, &free).as_deref(), Some("COM9"));
        let com9_taken = |p: &str| p == "COM9";
        assert_eq!(find_panel_port(&ports, &none(), None, 1, &com9_taken).as_deref(), Some("COM8"));
    }
}
