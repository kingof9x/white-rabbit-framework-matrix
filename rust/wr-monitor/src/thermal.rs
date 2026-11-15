//! Temperatures and fan speeds on Windows, from the source picked on the Panels tab
//! (`sensor_source` in settings.json), as `led_mon/sensor_sources.py`:
//!   auto                    Framework Control, then Libre Hardware Monitor, then framework_tool
//!   framework_control       Framework Control's local service (no admin rights needed)
//!   libre_hardware_monitor  Libre Hardware Monitor's sensors through WMI (it must run as admin)
//!   framework_tool          framework_tool --thermal (White Rabbit must run as admin)

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use wr_core::apps::temp_fan::{
    framework_control_port, parse_framework_control, parse_framework_tool, parse_libre_hardware_monitor, LhmSensor,
    Thermal,
};

use crate::background::OnceLog;

const DEFAULT_REFRESH: f64 = 2.0;
/// When Libre Hardware Monitor isn't running, ask again this often at most.
const RETRY: Duration = Duration::from_secs(10);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Windows' "the exe asks for admin rights and this program isn't running as admin".
const ERROR_ELEVATION_REQUIRED: i32 = 740;

/// The source picked and how often to read, from settings.json.
pub fn settings() -> (String, Duration) {
    let text = std::fs::read_to_string(crate::paths::settings_file()).unwrap_or_default();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let choice = json.get("sensor_source").and_then(|v| v.as_str()).unwrap_or("auto");
    let choice = match choice {
        "framework_control" | "libre_hardware_monitor" | "framework_tool" => choice,
        _ => "auto",
    };
    let refresh = json.get("sensor_refresh_seconds").and_then(|v| v.as_f64()).unwrap_or(DEFAULT_REFRESH);
    (choice.to_string(), Duration::from_secs_f64(refresh.clamp(0.5, 60.0)))
}

// Framework Control ---------------------------------------------------------------------------------

struct FrameworkControl {
    agent: ureq::Agent,
    log: OnceLog,
}

impl FrameworkControl {
    fn new() -> Self {
        let config = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(1))).build();
        FrameworkControl { agent: config.into(), log: OnceLog::default() }
    }

    fn port() -> Option<u16> {
        for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            let Some(dir) = std::env::var_os(var) else { continue };
            let xml = PathBuf::from(dir).join("FrameworkControl").join("FrameworkControlService.xml");
            if let Some(port) = std::fs::read_to_string(xml).ok().as_deref().and_then(framework_control_port) {
                return Some(port);
            }
        }
        None
    }

    fn read(&mut self) -> Thermal {
        let Some(port) = Self::port() else { return Thermal::default() };
        let url = format!("http://127.0.0.1:{port}/api/thermal");
        let answer = self
            .agent
            .get(&url)
            .call()
            .map_err(|e| e.to_string())
            .and_then(|mut r| r.body_mut().read_json::<serde_json::Value>().map_err(|e| e.to_string()));
        match answer {
            Ok(data) => {
                self.log.ok("Framework Control");
                parse_framework_control(&data)
            }
            Err(e) => {
                self.log.fail(format!("Framework Control didn't answer: {e}"));
                Thermal::default()
            }
        }
    }
}

// Libre Hardware Monitor ----------------------------------------------------------------------------

struct LibreHardwareMonitor {
    com: Option<wmi::COMLibrary>,
    connection: Option<wmi::WMIConnection>,
    next_try: Instant,
    log: OnceLog,
}

fn variant_f64(v: Option<&wmi::Variant>) -> Option<f64> {
    use wmi::Variant::*;
    Some(match v? {
        R4(x) => *x as f64,
        R8(x) => *x,
        I1(x) => *x as f64,
        I2(x) => *x as f64,
        I4(x) => *x as f64,
        I8(x) => *x as f64,
        UI1(x) => *x as f64,
        UI2(x) => *x as f64,
        UI4(x) => *x as f64,
        UI8(x) => *x as f64,
        String(s) => s.trim().parse().ok()?,
        _ => return None,
    })
}

fn variant_str(v: Option<&wmi::Variant>) -> String {
    match v {
        Some(wmi::Variant::String(s)) => s.clone(),
        _ => String::new(),
    }
}

impl LibreHardwareMonitor {
    fn new() -> Self {
        LibreHardwareMonitor { com: None, connection: None, next_try: Instant::now(), log: OnceLog::default() }
    }

    fn read(&mut self, interval: Duration) -> Thermal {
        if self.connection.is_none() {
            if Instant::now() < self.next_try {
                return Thermal::default();
            }
            self.next_try = Instant::now() + RETRY.max(interval);
            if self.com.is_none() {
                match wmi::COMLibrary::new() {
                    Ok(com) => self.com = Some(com),
                    Err(e) => {
                        self.log.fail(format!("Libre Hardware Monitor: can't use WMI ({e})"));
                        return Thermal::default();
                    }
                }
            }
            let Some(com) = self.com else { return Thermal::default() };
            match wmi::WMIConnection::with_namespace_path("root\\LibreHardwareMonitor", com) {
                Ok(c) => self.connection = Some(c),
                Err(e) => {
                    self.log.fail(format!("Libre Hardware Monitor isn't running ({e})"));
                    return Thermal::default();
                }
            }
        }
        let Some(connection) = self.connection.as_ref() else { return Thermal::default() };
        let query = "SELECT Name, Identifier, Parent, SensorType, Value, Index FROM Sensor \
                     WHERE SensorType = 'Temperature' OR SensorType = 'Fan'";
        let rows: Result<Vec<HashMap<String, wmi::Variant>>, _> = connection.raw_query(query);
        match rows {
            Ok(rows) => {
                self.log.ok("Libre Hardware Monitor");
                let sensors: Vec<LhmSensor> = rows
                    .iter()
                    .map(|r| LhmSensor {
                        name: variant_str(r.get("Name")),
                        parent: variant_str(r.get("Parent")),
                        sensor_type: variant_str(r.get("SensorType")),
                        value: variant_f64(r.get("Value")),
                        index: variant_f64(r.get("Index")).unwrap_or(0.0) as i64,
                    })
                    .collect();
                parse_libre_hardware_monitor(&sensors)
            }
            Err(e) => {
                // It was closed: connect again later
                self.log.fail(format!("Libre Hardware Monitor isn't running ({e})"));
                self.connection = None;
                Thermal::default()
            }
        }
    }
}

// framework_tool ------------------------------------------------------------------------------------

struct FrameworkTool {
    tool: Option<Option<PathBuf>>,
    /// Windows refused to start it without admin rights; it won't work until a restart.
    needs_admin: bool,
    log: OnceLog,
}

fn find_framework_tool() -> Option<PathBuf> {
    let names = ["framework_tool.exe", "framework_tool"];
    let mut dirs: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        dirs.push(dir);
    }
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

impl FrameworkTool {
    fn new() -> Self {
        FrameworkTool { tool: None, needs_admin: false, log: OnceLog::default() }
    }

    fn read(&mut self) -> Thermal {
        if self.needs_admin {
            return Thermal::default();
        }
        let Some(tool) = self.tool.get_or_insert_with(find_framework_tool).clone() else {
            self.log.fail(
                "framework_tool was not found on PATH or next to the exe; temperature and fan apps will be empty"
                    .into(),
            );
            return Thermal::default();
        };
        let child = Command::new(&tool)
            .arg("--thermal")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) if e.raw_os_error() == Some(ERROR_ELEVATION_REQUIRED) => {
                self.needs_admin = true;
                wr_core::warn!(
                    "framework_tool needs admin rights, so it won't be run again until White Rabbit restarts; \
                     choose another source for temperatures and fans"
                );
                return Thermal::default();
            }
            Err(e) => {
                self.log.fail(format!("framework_tool --thermal failed; temperature and fan apps will be empty: {e}"));
                return Thermal::default();
            }
        };
        // At most 10 seconds
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() < Duration::from_secs(10) => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                _ => {
                    let _ = child.kill();
                    self.log.fail("framework_tool --thermal took too long".into());
                    let _ = child.wait();
                    return Thermal::default();
                }
            }
        }
        match child.wait_with_output() {
            Ok(out) if out.status.success() => {
                let thermal = parse_framework_tool(&String::from_utf8_lossy(&out.stdout));
                if thermal.temps.is_empty() && thermal.fans.is_empty() {
                    self.log.fail("framework_tool --thermal gave no readings".into());
                } else {
                    self.log.ok("framework_tool");
                }
                thermal
            }
            Ok(out) => {
                let text = String::from_utf8_lossy(if out.stderr.is_empty() { &out.stdout } else { &out.stderr });
                self.log.fail(format!("framework_tool --thermal failed: {} {}", out.status, text.trim()));
                Thermal::default()
            }
            Err(e) => {
                self.log.fail(format!("framework_tool --thermal failed: {e}"));
                Thermal::default()
            }
        }
    }
}

// All together --------------------------------------------------------------------------------------

/// Reads the picked source (in auto, the first that gives temperatures, and the first that gives
/// fans). Made and called on the sensors thread.
pub fn reader(interval: crate::background::Interval) -> Box<dyn FnMut() -> Thermal> {
    let mut fc = FrameworkControl::new();
    let mut lhm = LibreHardwareMonitor::new();
    let mut tool = FrameworkTool::new();
    let mut last_choice = String::new();
    Box::new(move || {
        let (choice, refresh) = settings();
        if let Ok(mut i) = interval.lock() {
            *i = refresh;
        }
        if choice != last_choice {
            wr_core::info!("Temperatures and fans now come from: {choice}");
            last_choice = choice.clone();
        }
        let order: Vec<&str> = match choice.as_str() {
            "auto" => vec!["framework_control", "libre_hardware_monitor", "framework_tool"],
            other => vec![other],
        };
        let mut result = Thermal::default();
        let (mut have_temps, mut have_fans) = (false, false);
        for source in order {
            let t = match source {
                "framework_control" => fc.read(),
                "libre_hardware_monitor" => lhm.read(refresh),
                _ => tool.read(),
            };
            if !have_temps && !t.temps.is_empty() {
                result.temps = t.temps;
                result.named = t.named;
                have_temps = true;
            }
            if !have_fans && !t.fans.is_empty() {
                result.fans = t.fans;
                have_fans = true;
            }
            if have_temps && have_fans {
                break;
            }
        }
        result
    })
}
