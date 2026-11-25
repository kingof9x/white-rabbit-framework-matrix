//! The temperatures and fans apps (`led_mon/plugins/temp_fan_plugin.py`) and the three places
//! Windows readings come from (`led_mon/sensor_sources.py`): Framework Control's local service,
//! Libre Hardware Monitor through WMI, and `framework_tool --thermal`. Only the reading of the
//! answers is here; asking for them is in `wr-monitor`.
//!
//! Where this sits: `wr-monitor`'s `thermal.rs` asks the sources on a background thread, tries them
//! in turn, and turns each answer into a `Thermal` with one of the `parse_*` functions below. When a
//! panel is drawn (the monitor's main loop), `sensors.rs` uses `pick` to choose the three readings the
//! temp app shows and `to_unit` to convert them, and `render.rs` uses `fan_ratios` for the fan bars.
//!
//! Every parser is forgiving: a missing or malformed part is skipped rather than failing, because a
//! half-read answer is still worth showing and these services vary between versions and machines.

use std::cmp::Ordering;

/// Fan speed that fills the bar, in RPM.
/// Faster fans are drawn as a full bar.
pub const MAX_FAN_SPEED: f64 = 6000.0;

/// Temperatures (C) and fan speeds (RPM) from one source.
/// `Default` (all empty) means the source gave nothing usable.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thermal {
    /// (sensor name, C), the most important first.
    /// Names are unique within one `Thermal` (repeats get " 2", " 3"), because the user picks sensors by
    /// name in the app's settings.
    pub named: Vec<(String, f64)>,
    /// The temperatures in Celsius, the most important first. `wr-monitor` uses it to tell whether a
    /// source has any temperatures; the app itself shows readings picked from `named`.
    pub temps: Vec<f64>,
    /// Fan speeds in RPM, in the source's order (0 for a stalled fan).
    pub fans: Vec<f64>,
}

/// A temperature in the unit picked for the app: C (as read) or F.
/// Anything starting with "F" (any case, spaces ignored) means Fahrenheit; everything else is Celsius.
pub fn to_unit(celsius: f64, unit: &str) -> f64 {
    if unit.trim().to_ascii_uppercase().starts_with('F') {
        celsius * 9.0 / 5.0 + 32.0
    } else {
        celsius
    }
}

/// The values to show: for each place, the chosen sensor's reading, or (when none is chosen) the next
/// sensor in the source's own order that isn't shown elsewhere. A chosen sensor without a reading
/// right now is None.
/// `named` is the source's sensors in order, `chosen` the sensor names set in the app (a blank name
/// means "automatic"), and `count` how many places the app has (3). Sensors chosen for another place
/// are not used again for an automatic place, so the same reading does not show twice.
pub fn pick(named: &[(String, f64)], chosen: &[String], count: usize) -> Vec<Option<f64>> {
    let mut chosen: Vec<&str> = chosen.iter().take(count).map(String::as_str).collect();
    // Places without a setting count as automatic.
    chosen.resize(count, "");
    // The sensors nobody chose by name, reversed so `pop` hands them out in the source's order.
    let mut spare: Vec<&(String, f64)> =
        named.iter().filter(|(name, _)| !chosen.iter().any(|c| !c.is_empty() && c == name)).collect();
    spare.reverse();
    chosen
        .iter()
        .map(|name| {
            if name.is_empty() {
                spare.pop().map(|(_, v)| *v)
            } else {
                // The last reading of that name, as Python's dict() keeps it
                named.iter().rev().find(|(n, _)| n == name).map(|(_, v)| *v)
            }
        })
        .collect()
}

/// The fans app's two bars, 0.0-1.0. Missing fans draw empty bars.
/// The fan speed divided by `MAX_FAN_SPEED`, capped at 1.0; the first two fans only.
pub fn fan_ratios(rpms: &[f64]) -> [f64; 2] {
    let at = |i: usize| (rpms.get(i).copied().unwrap_or(0.0) / MAX_FAN_SPEED).min(1.0);
    [at(0), at(1)]
}

// Framework Control ---------------------------------------------------------------------------------

/// Sort order for Framework Control's sensor names: CPU first, then APU, then GPU, then the rest.
fn fc_rank(name: &str) -> u8 {
    let name = name.to_lowercase();
    for (part, rank) in [("cpu", 0), ("apu", 1), ("gpu", 2)] {
        if name.starts_with(part) {
            return rank;
        }
    }
    3
}

/// A JSON value as a number, accepting numbers, numeric strings and true/false (1/0), as Python's
/// `float()` would; None for anything else.
fn json_number(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        serde_json::Value::Bool(b) => Some(*b as u8 as f64),
        _ => None,
    }
}

/// Framework Control's `/api/thermal` answer: `{"temps": {name: C}, "fans": [{"name", "rpm"}]}`.
/// Sensors reading 0 (not powered) are left out; the CPU comes first.
/// Values that are not numbers are skipped. Sensors are ordered by `fc_rank`, then name, then value,
/// so the order is stable whatever order the JSON object had. Fans without a usable rpm are skipped.
pub fn parse_framework_control(data: &serde_json::Value) -> Thermal {
    let mut temps: Vec<(u8, String, f64)> = Vec::new();
    if let Some(map) = data.get("temps").and_then(|t| t.as_object()) {
        for (name, value) in map {
            if let Some(v) = json_number(value) {
                if v > 0.0 {
                    temps.push((fc_rank(name), name.clone(), v));
                }
            }
        }
    }
    temps.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2)));
    let mut fans = Vec::new();
    if let Some(list) = data.get("fans").and_then(|f| f.as_array()) {
        for fan in list {
            // Python's int(): a float rpm is cut towards zero
            if let Some(rpm) = fan.get("rpm").and_then(json_number) {
                fans.push(rpm.trunc().max(0.0));
            }
        }
    }
    Thermal {
        temps: temps.iter().map(|t| t.2).collect(),
        named: temps.into_iter().map(|(_, n, v)| (n, v)).collect(),
        fans,
    }
}

/// The port in Framework Control's `FrameworkControlService.xml` (`<env name="FRAMEWORK_CONTROL_PORT" value="..."/>`).
/// A tiny hand-written XML scan (no XML library, to keep the build small): it looks at each tag that
/// starts with "env", reads its name and value attributes (either quote style), and returns the
/// value of the FRAMEWORK_CONTROL_PORT one. None when it is missing, not a number, or 0.
pub fn framework_control_port(xml: &str) -> Option<u16> {
    for tag in xml.split('<').filter(|t| t.starts_with("env")) {
        let attr = |name: &str| {
            // The character right after `name=` is the quote; the value runs to the next same quote.
            let at = tag.find(&format!("{name}="))? + name.len() + 1;
            let quote = tag[at..].chars().next()?;
            let rest = &tag[at + 1..];
            Some(rest[..rest.find(quote)?].to_string())
        };
        if attr("name").as_deref() == Some("FRAMEWORK_CONTROL_PORT") {
            let value = attr("value")?;
            return value.trim().parse::<u16>().ok().filter(|p| *p > 0);
        }
    }
    None
}

// Libre Hardware Monitor ----------------------------------------------------------------------------

/// One row of Libre Hardware Monitor's WMI `Sensor` class.
/// Filled from WMI by `wr-monitor`; only the fields needed here.
#[derive(Clone, Debug, Default)]
pub struct LhmSensor {
    /// The sensor's name, such as "Core (Tctl/Tdie)" or "Fan 1".
    pub name: String,
    /// The hardware it belongs to, as an identifier path such as "/amdcpu/0" or "/nvme/1".
    pub parent: String,
    /// "Temperature", "Fan", "Load", ...; only temperatures and fans are used.
    pub sensor_type: String,
    /// The reading (Celsius for temperatures, RPM for fans); None when WMI gave none.
    pub value: Option<f64>,
    /// The sensor's position within its hardware, used to keep a stable order.
    pub index: i64,
}

/// At most this many temperatures from one piece of hardware.
const PER_HARDWARE: usize = 2;
/// Sort order of hardware by the start of its identifier: CPUs (0), GPUs (1), anything not listed (2),
/// then drives (3). Matched with `starts_with`, so "/gpu-amd" and "/gpu-nvidia" both count as GPUs.
const HARDWARE_ORDER: [(&str, u8); 8] = [
    ("/amdcpu", 0),
    ("/intelcpu", 0),
    ("/cpu", 0),
    ("/gpu", 1),
    ("/nvme", 3),
    ("/hdd", 3),
    ("/ssd", 3),
    ("/storage", 3),
];
/// Parts of sensor names (lower case) that mark the one reading that best stands for the hardware:
/// the CPU's control temperature or package, the GPU core, a drive's composite temperature.
const MAIN_SENSOR_NAMES: [&str; 6] = ["tctl", "package", "core (tctl", "gpu core", "composite", "core average"];
/// Short, friendly names for hardware identifiers, used to build sensor names such as "CPU: Tctl".
const HARDWARE_NAMES: [(&str, &str); 13] = [
    ("/amdcpu", "CPU"),
    ("/intelcpu", "CPU"),
    ("/cpu", "CPU"),
    ("/gpu", "GPU"),
    ("/nvme", "SSD"),
    ("/hdd", "Drive"),
    ("/ssd", "SSD"),
    ("/storage", "Drive"),
    ("/lpc", "Board"),
    ("/motherboard", "Board"),
    ("/ram", "Memory"),
    ("/memory", "Memory"),
    ("/battery", "Battery"),
];

/// The hardware's place in `HARDWARE_ORDER`; 2 when it is not listed.
fn hardware_rank(parent: &str) -> u8 {
    let parent = parent.to_lowercase();
    HARDWARE_ORDER.iter().find(|(p, _)| parent.starts_with(p)).map(|(_, r)| *r).unwrap_or(2)
}

/// 0 for a main sensor (see `MAIN_SENSOR_NAMES`), 1 for any other.
fn sensor_rank(name: &str) -> u8 {
    let name = name.to_lowercase();
    if MAIN_SENSOR_NAMES.iter().any(|part| name.contains(part)) {
        0
    } else {
        1
    }
}

/// The friendly name of a hardware identifier: "CPU", "SSD 2", ... or, for unknown hardware, the
/// identifier without its outer slashes ("Other" when that leaves nothing).
fn hardware_name(parent: &str) -> String {
    let lower = parent.to_lowercase();
    for (prefix, name) in HARDWARE_NAMES {
        if lower.starts_with(prefix) {
            // A second drive or GPU gets its number: "SSD 2"
            // The last part of the path is the hardware's number, counted from 0; the first
            // one is left unnumbered and "/nvme/1" becomes "SSD 2".
            let number = lower.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            if let Ok(n) = number.parse::<u64>() {
                if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) && n > 0 {
                    return format!("{name} {}", n + 1);
                }
            }
            return name.to_string();
        }
    }
    let trimmed = parent.trim_matches('/');
    if trimmed.is_empty() {
        "Other".into()
    } else {
        trimmed.into()
    }
}

/// A temperature with its sensor's rank and index, for sorting.
/// (sensor rank, index, Celsius).
type RankedTemp = (u8, i64, f64);

/// Temperatures, named and in order, and fan speeds from Libre Hardware Monitor's sensors.
/// Temperatures of 0 or less (sensors that are off) and missing or NaN values are skipped. `named`
/// holds every temperature, ordered by hardware then main sensor then index; `temps` holds at most
/// two per piece of hardware, main sensor first, CPUs first. Fan speeds are rounded to whole RPM.
pub fn parse_libre_hardware_monitor(sensors: &[LhmSensor]) -> Thermal {
    // Named: every temperature, CPU first
    type Key = (u8, String, u8, i64);
    let mut named: Vec<(Key, String, f64)> = Vec::new();
    // Per hardware, at most two temperatures, the main sensor first
    let mut by_hardware: Vec<(String, Vec<RankedTemp>)> = Vec::new();
    let mut fans: Vec<(u8, String, i64, f64)> = Vec::new();
    for s in sensors {
        let Some(value) = s.value else { continue };
        if value.is_nan() {
            continue;
        }
        match s.sensor_type.to_lowercase().as_str() {
            "temperature" if value > 0.0 => {
                let name = format!("{}: {}", hardware_name(&s.parent), if s.name.is_empty() { "?" } else { &s.name });
                named.push(((hardware_rank(&s.parent), s.parent.clone(), sensor_rank(&s.name), s.index), name, value));
                // Find this hardware's list, or start one. A Vec (not a map) keeps it simple;
                // there are only a handful of pieces of hardware.
                let entry = match by_hardware.iter_mut().find(|(p, _)| *p == s.parent) {
                    Some(e) => e,
                    None => {
                        by_hardware.push((s.parent.clone(), Vec::new()));
                        by_hardware.last_mut().unwrap()
                    }
                };
                entry.1.push((sensor_rank(&s.name), s.index, value));
            }
            "fan" => {
                let rpm = crate::pyround::round_half_even(value).max(0.0);
                fans.push((hardware_rank(&s.parent), s.parent.clone(), s.index, rpm));
            }
            _ => {}
        }
    }
    // Sort by the key, then by name and value, so equal keys still give a fixed order.
    named.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2)));
    let mut unique_named = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (_, name, value) in named {
        // Two sensors with the same name on the same hardware get " 2", " 3", ...
        let (mut unique, mut n) = (name.clone(), 2);
        while seen.contains(&unique) {
            unique = format!("{name} {n}");
            n += 1;
        }
        seen.insert(unique.clone());
        unique_named.push((unique, value));
    }
    by_hardware.sort_by(|a, b| hardware_rank(&a.0).cmp(&hardware_rank(&b.0)).then(a.0.cmp(&b.0)));
    let mut temps = Vec::new();
    for (_, mut list) in by_hardware {
        list.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2)));
        temps.extend(list.iter().take(PER_HARDWARE).map(|t| t.2));
    }
    fans.sort_by(|a, b| {
        a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)).then(a.3.partial_cmp(&b.3).unwrap_or(Ordering::Equal))
    });
    Thermal { named: unique_named, temps, fans: fans.into_iter().map(|f| f.3).collect() }
}

// framework_tool --thermal --------------------------------------------------------------------------

/// "  Name:  value rest" split into the name and what follows the colon; the name has no colon.
/// None for lines without a "name:" part, an empty name, or no space after the colon (so a time
/// such as "12:30" is not mistaken for a reading).
fn split_line(line: &str) -> Option<(&str, &str)> {
    let (name, rest) = line.split_once(':')?;
    if name.trim_start().is_empty() || !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some((name.trim(), rest.trim_start()))
}

/// "1234 RPM" or "65534 RPM (Stalled)": the speed, 0 when stalled.
/// None for any line that is not a fan reading. framework_tool prints 65534 for a stalled fan,
/// which would otherwise show as a full bar.
fn fan_line(line: &str) -> Option<f64> {
    let (_, rest) = split_line(line)?;
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let value: f64 = rest[..digits].parse().ok()?;
    let after = rest[digits..].trim_start();
    if !after.to_ascii_lowercase().starts_with("rpm") {
        return None;
    }
    // "rpm" is 3 ASCII bytes (checked just above), so cutting at byte 3 is safe.
    let tail = after[3..].trim_start().to_ascii_lowercase();
    Some(if tail.starts_with("(stalled)") { 0.0 } else { value })
}

/// "45 C" or "-3.5 C": the temperature.
/// Accepts only a plain decimal number followed by "C", so lines such as "n/a" are skipped.
fn temp_line(line: &str) -> Option<(String, f64)> {
    let (name, rest) = split_line(line)?;
    let rest = rest.trim_end();
    let number = rest.strip_suffix('C')?.trim_end();
    let unsigned = number.strip_prefix('-').unwrap_or(number);
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (unsigned, None),
    };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if !digits(whole) || fraction.is_some_and(|f| !digits(f)) {
        return None;
    }
    Some((name.to_string(), number.parse().ok()?))
}

/// `framework_tool --thermal` output: temperatures in the order it lists them, and fan speeds.
/// Lines that are neither a fan nor a temperature are ignored. A repeated sensor name gets " 2",
/// " 3", ... so each can be picked.
pub fn parse_framework_tool(text: &str) -> Thermal {
    let mut thermal = Thermal::default();
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        if let Some(rpm) = fan_line(line) {
            thermal.fans.push(rpm);
        } else if let Some((name, value)) = temp_line(line) {
            let (mut unique, mut n) = (name.clone(), 2);
            while seen.contains(&unique) {
                unique = format!("{name} {n}");
                n += 1;
            }
            seen.insert(unique.clone());
            thermal.temps.push(value);
            thermal.named.push((unique, value));
        }
    }
    thermal
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Celsius converts to Fahrenheit, and fan speeds become bar heights capped at full, with missing fans
    /// empty.
    #[test]
    fn units_and_fans() {
        assert_eq!(to_unit(100.0, "F"), 212.0);
        assert_eq!(to_unit(100.0, "C"), 100.0);
        assert_eq!(fan_ratios(&[3000.0]), [0.5, 0.0]);
        assert_eq!(fan_ratios(&[9000.0, 600.0]), [1.0, 0.1]);
    }

    /// Automatic places take the sensors in order, skipping ones chosen elsewhere; an unknown chosen name
    /// gives None; with too few sensors the extra places are None.
    #[test]
    fn picking_like_python() {
        let named = vec![("A".to_string(), 1.0), ("B".to_string(), 2.0), ("C".to_string(), 3.0)];
        assert_eq!(pick(&named, &[], 3), vec![Some(1.0), Some(2.0), Some(3.0)]);
        assert_eq!(pick(&named, &["".into(), "A".into(), "X".into()], 3), vec![Some(2.0), Some(1.0), None]);
        assert_eq!(pick(&named[..1], &[], 3), vec![Some(1.0), None, None]);
    }

    /// Temperatures and fans are read from framework_tool's text, a stalled fan reads 0, a repeated
    /// name gets " 2", and an "n/a" line is skipped.
    #[test]
    fn framework_tool_output() {
        let text = "  F75303_Local:  45 C\n  APU:           60 C\n  APU:  61 C\n  Left Fan:     1234 RPM\n  \
                    Right Fan:  65534 RPM (Stalled)\n  Bad: n/a\n";
        let t = parse_framework_tool(text);
        assert_eq!(t.temps, vec![45.0, 60.0, 61.0]);
        assert_eq!(t.fans, vec![1234.0, 0.0]);
        assert_eq!(t.named[2].0, "APU 2");
    }

    /// Sensors reading 0 are left out, numeric strings are accepted, the CPU comes first, fan speeds are
    /// cut to whole RPM, and the port is found in the service's XML.
    #[test]
    fn framework_control_answer() {
        let data = serde_json::json!({"temps": {"ambient": 30, "cpu": 55.5, "gpu": 0, "apu": "50"},
                                      "fans": [{"name": "L", "rpm": 2000.7}, {"rpm": null}]});
        let t = parse_framework_control(&data);
        assert_eq!(t.temps, vec![55.5, 50.0, 30.0]);
        assert_eq!(t.fans, vec![2000.0]);
        let xml =
            r#"<service><env name="OTHER" value="1"/><env name="FRAMEWORK_CONTROL_PORT" value="8090"/></service>"#;
        assert_eq!(framework_control_port(xml), Some(8090));
    }

    /// At most two temperatures per piece of hardware with the main sensor first, CPU before GPU before
    /// drives, a sensor reading 0 left out, fans rounded, and the friendly names built correctly.
    #[test]
    fn libre_hardware_monitor_rows() {
        let s = |name: &str, parent: &str, kind: &str, value: f64, index: i64| LhmSensor {
            name: name.into(),
            parent: parent.into(),
            sensor_type: kind.into(),
            value: Some(value),
            index,
        };
        let rows = vec![
            s("Composite", "/nvme/0", "Temperature", 40.0, 0),
            s("Core 1", "/amdcpu/0", "Temperature", 60.0, 2),
            s("Core (Tctl/Tdie)", "/amdcpu/0", "Temperature", 70.0, 3),
            s("Core 2", "/amdcpu/0", "Temperature", 65.0, 1),
            s("GPU Core", "/gpu-amd/0", "Temperature", 50.0, 0),
            s("Fan 1", "/lpc/it8", "Fan", 1500.4, 0),
            s("Off", "/lpc/it8", "Temperature", 0.0, 0),
        ];
        let t = parse_libre_hardware_monitor(&rows);
        assert_eq!(t.temps, vec![70.0, 65.0, 50.0, 40.0]);
        assert_eq!(t.fans, vec![1500.0]);
        assert_eq!(t.named[0], ("CPU: Core (Tctl/Tdie)".to_string(), 70.0));
        assert_eq!(t.named.last().unwrap().0, "SSD: Composite");
    }
}
