//! Temperatures and fan speeds on Windows, from the source picked on the Panels tab
//! (`sensor_source` in settings.json), as `led_mon/sensor_sources.py`:
//!   auto                    Framework Control, then Libre Hardware Monitor, then framework_tool
//!   framework_control       Framework Control's local service (no admin rights needed)
//!   libre_hardware_monitor  Libre Hardware Monitor's sensors through WMI (it must run as admin)
//!   framework_tool          framework_tool --thermal (White Rabbit must run as admin)
//!
//! `reader` runs on the `sensors` reading thread (`background.rs`, started from `sensors.rs`) and is
//! called every `sensor_refresh_seconds` (2 s by default) while a temperature or fan app is on screen.
//! Each source is slow in its own way (an HTTP call, a WMI query, starting a process), which is why
//! none of it runs on the main loop. The JSON parsing of each source's answer lives in
//! `wr_core::apps::temp_fan` so it can be tested without the hardware.
//!
//! After each reading a summary (`SourceStatus`) is stored in a global, so the control thread can
//! answer the window's "sensor_source" question (`source_status`) without touching the sources.

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

/// Seconds between readings when settings.json doesn't say.
const DEFAULT_REFRESH: f64 = 2.0;
/// When Libre Hardware Monitor isn't running, ask again this often at most.
const RETRY: Duration = Duration::from_secs(10);
/// Process creation flag: run framework_tool without flashing a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Windows' "the exe asks for admin rights and this program isn't running as admin".
const ERROR_ELEVATION_REQUIRED: i32 = 740;

/// The source picked and how often to read, from settings.json.
///
/// Read from disk on every call so a change made in the window applies at the next reading. A
/// missing or broken file, or an unknown source name, gives "auto"; the interval is kept within
/// 0.5-60 s.
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

/// Framework Control: a Windows service with a small web API on 127.0.0.1 (this computer only).
struct FrameworkControl {
    /// HTTP client with a 1 s time limit (the service is local, so a slow answer means it's stuck).
    agent: ureq::Agent,
    /// Logs each distinct failure once; its last error is reported to the window.
    log: OnceLog,
}

impl FrameworkControl {
    fn new() -> Self {
        let config = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(1))).build();
        FrameworkControl { agent: config.into(), log: OnceLog::default() }
    }

    /// The service's port, from its config file under Program Files. Read on every call so a changed
    /// port or a fresh install is picked up. None when Framework Control isn't installed.
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

    /// GET /api/thermal and parse it; empty readings when not installed or not answering.
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

/// Libre Hardware Monitor: when running (as admin) it publishes its sensors as WMI objects in the
/// root\LibreHardwareMonitor namespace, which any program can query.
struct LibreHardwareMonitor {
    /// COM set up on this thread (WMI goes through COM); kept so it is initialised once.
    com: Option<wmi::COMLibrary>,
    /// The open WMI connection; None until connected, and again after a failed query.
    connection: Option<wmi::WMIConnection>,
    /// No connection attempt before this time, so a closed LHM isn't asked for every reading.
    next_try: Instant,
    log: OnceLog,
}

/// A WMI value as a number, whatever numeric type WMI used (LHM's Value is a 32-bit float, Index
/// an integer). Numbers written as text are parsed too; anything else is None.
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

/// A WMI value as text; empty when missing or not a string.
fn variant_str(v: Option<&wmi::Variant>) -> String {
    match v {
        Some(wmi::Variant::String(s)) => s.clone(),
        _ => String::new(),
    }
}

impl LibreHardwareMonitor {
    /// Not connected yet; the first `read` connects.
    fn new() -> Self {
        LibreHardwareMonitor { com: None, connection: None, next_try: Instant::now(), log: OnceLog::default() }
    }

    /// All temperature and fan sensors LHM knows. Connects first if needed (at most every 10 s, or
    /// every `interval` if that is longer); empty readings while LHM isn't running.
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
            // `COMLibrary` is a small copyable marker that COM is ready on this thread.
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
        // WQL, WMI's SQL-like query language. Only temperatures and fans are fetched.
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

/// framework_tool: Framework's command-line tool, which talks to the laptop's embedded controller.
struct FrameworkTool {
    /// The tool's path: outer None = not looked for yet; Some(None) = looked for and not found
    /// (it isn't searched for again until a restart).
    tool: Option<Option<PathBuf>>,
    /// Windows refused to start it without admin rights; it won't work until a restart.
    needs_admin: bool,
    log: OnceLog,
}

/// framework_tool's path: the first folder on PATH that has it, then the folder of WhiteRabbit.exe.
pub fn find_framework_tool() -> Option<PathBuf> {
    let names = ["framework_tool.exe", "framework_tool"];
    let mut dirs: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        dirs.push(dir);
    }
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

impl FrameworkTool {
    /// Not looked for yet.
    fn new() -> Self {
        FrameworkTool { tool: None, needs_admin: false, log: OnceLog::default() }
    }

    /// Runs `framework_tool --thermal` once and parses what it prints. Blocks up to 10 s; a tool
    /// that takes longer is killed. Gives up for the rest of the run if Windows says it needs admin.
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
                // Still running: poll every 50 ms. (`wait_with_output` alone has no time limit.)
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
        // The tool has exited; this only collects its output. The pipes can't fill up and block it,
        // since `thermal` output is a few hundred bytes.
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
///
/// `interval` is the reading thread's period, updated here from `sensor_refresh_seconds`. Each call
/// also stores a `SourceStatus` for `source_status`. The three sources keep their state between calls.
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
        // "auto" tries all three; temperatures and fans may end up coming from different sources.
        let order: Vec<&str> = match choice.as_str() {
            "auto" => vec!["framework_control", "libre_hardware_monitor", "framework_tool"],
            other => vec![other],
        };
        let mut result = Thermal::default();
        let mut status = SourceStatus { choice: choice.clone(), ..Default::default() };
        for source in order.iter().copied() {
            let t = match source {
                "framework_control" => fc.read(),
                "libre_hardware_monitor" => lhm.read(refresh),
                _ => tool.read(),
            };
            if status.temps_from.is_none() && !t.temps.is_empty() {
                result.temps = t.temps;
                result.named = t.named;
                status.temps_from = Some(source.to_string());
            }
            if status.fans_from.is_none() && !t.fans.is_empty() {
                result.fans = t.fans;
                status.fans_from = Some(source.to_string());
            }
            // Both found: the later sources aren't asked (saves starting framework_tool).
            if status.temps_from.is_some() && status.fans_from.is_some() {
                break;
            }
        }
        // Collect each source's current error (if any) for the window, even of sources that were skipped.
        for source in order {
            let error = match source {
                "framework_control" => fc.log.error(),
                "libre_hardware_monitor" => lhm.log.error(),
                _ => tool.log.error(),
            };
            if let Some(error) = error {
                status.errors.insert(source.to_string(), error.into());
            }
        }
        status.thermal = result.clone();
        status.read_at = Some(Instant::now());
        *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = Some(status);
        result
    })
}

// For the layout editor ------------------------------------------------------------------------------

/// Where the last readings came from, for the window's Panels tab and layout editor.
#[derive(Clone, Default)]
struct SourceStatus {
    /// The source setting at the time ("auto" or one source name).
    choice: String,
    /// The source the temperatures came from; None when none gave any.
    temps_from: Option<String>,
    /// The source the fan speeds came from; None when none gave any.
    fans_from: Option<String>,
    /// Source name -> its last error message, for sources that are failing.
    errors: serde_json::Map<String, serde_json::Value>,
    /// The readings themselves.
    thermal: Thermal,
    /// When the reading was taken.
    read_at: Option<Instant>,
}

/// The latest `SourceStatus`, written by whichever thread read last, read by the control thread.
static STATUS: std::sync::Mutex<Option<SourceStatus>> = std::sync::Mutex::new(None);

/// A reading older than this means no panel shows Temperatures or Fans right now.
const STALE: Duration = Duration::from_secs(15);
/// Longest wait for a one-off reading when nothing is being read.
const ONE_OFF_WAIT: Duration = Duration::from_secs(5);

/// The stored status if it is less than 15 s old.
fn current_status() -> Option<SourceStatus> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|s| s.read_at.is_some_and(|t| t.elapsed() < STALE))
}

/// The reply to the window's {"cmd": "sensor_source"}, as Python's SensorSource.status() plus the
/// sensors' names and readings ("named"), which the editor's Temperatures app offers to pick from.
/// When nothing is being read (or `reload` asks for the new settings), it reads once, so the editor
/// still gets the names.
///
/// Called on the control thread. The one-off reading runs on a short-lived `sensors-once` thread
/// (fresh sources, so COM/WMI are set up there) and is waited for at most 5 s; if it takes longer,
/// whatever status was stored before is used. Temperatures are rounded to 0.1 °C.
pub fn source_status(reload: bool) -> serde_json::Value {
    let fresh = current_status().filter(|s| !reload || s.choice == settings().0);
    let (status, reading) = match fresh {
        Some(s) => (Some(s), true),
        None => {
            let (send, receive) = std::sync::mpsc::channel();
            let _ = std::thread::Builder::new().name("sensors-once".into()).spawn(move || {
                let mut read = reader(std::sync::Arc::new(std::sync::Mutex::new(Duration::from_secs(2))));
                read();
                let _ = send.send(());
            });
            // Timing out or the thread failing to start both just mean: answer with what we have.
            let _ = receive.recv_timeout(ONE_OFF_WAIT);
            // Read once for the editor: not counted as "now reading" for the Panels tab
            (STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone(), false)
        }
    };
    let (choice, refresh) = settings();
    let Some(s) = status else {
        return serde_json::json!({"choice": choice, "refresh": refresh.as_secs_f64(), "reading": false});
    };
    let named: Vec<serde_json::Value> =
        s.thermal.named.iter().map(|(name, c)| serde_json::json!([name, (c * 10.0).round() / 10.0])).collect();
    serde_json::json!({
        "choice": choice,
        "refresh": refresh.as_secs_f64(),
        "reading": reading,
        "temps_from": s.temps_from,
        "fans_from": s.fans_from,
        "temps": s.thermal.temps.len(),
        "fans": s.thermal.fans.len(),
        "errors": s.errors,
        "sensors": s.thermal.named.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
        "named": named,
    })
}
