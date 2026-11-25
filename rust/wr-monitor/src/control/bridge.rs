//! Sharing the LED panels with the White Rabbit window: the monitor's side of
//! `white_rabbit/bridge.py`, as JSON over wr-panel's `Hub`.
//!
//! Windows lets only one program open a panel's serial port, and the monitor keeps both open. So the
//! window sends its commands through the control channel instead:
//!
//!   serial       write bytes to a panel and optionally read the reply
//!   serial_many  write to several panels at once, taking turns between them
//!   hold         lend panels to the window: the monitor stops drawing on them
//!   release      give them back to the monitor
//!
//! The ports themselves are shared by the hub: the drawing threads and these commands write through
//! the same open handle, and a held panel only takes the window's writes.
//!
//! Everything here runs on the control thread (one request at a time), except `release_orphans`,
//! which the `holders` thread in `app.rs` calls every 2 s. Bytes travel in JSON as hex text
//! ("32ac" = 0x32 0xAC). Port names are compared without regard to case (the hub stores them lower case).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};

use wr_panel::hub::MAX_READ;
use wr_panel::panel::Panels;

/// The window's access to the panels' serial ports, through the panels' shared hub.
pub struct Bridge {
    /// Both panels and their hub (the same object the drawing threads use).
    pub panels: Arc<Panels>,
}

impl Bridge {
    /// A bridge over these panels.
    pub fn new(panels: Arc<Panels>) -> Bridge {
        Bridge { panels }
    }

    /// The ports the panels draw on, for the window's status request.
    pub fn open_ports(&self) -> Vec<String> {
        let mut names: Vec<String> = self.panels.port_names().into_iter().flatten().collect();
        names.sort();
        names
    }

    /// "serial": write the window's commands one by one (the panel's firmware reads one command per
    /// USB read) and read the reply.
    pub fn serial(&self, message: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let name = message.get("port").and_then(Value::as_str).ok_or("no port given")?;
        let chunks = chunks(message)?;
        // How many reply bytes to read after the writes; 0 = don't read.
        let size = message.get("read").and_then(Value::as_u64).unwrap_or(0) as usize;
        let timeout = match message.get("timeout").and_then(Value::as_f64) {
            // Clamped before converting: Duration::from_secs_f64 panics on a value too big for it
            // (1e300 from a client would otherwise end the whole monitor in a release build)
            Some(t) if t >= 0.0 => Duration::from_secs_f64(t.min(MAX_READ.as_secs_f64())),
            _ => MAX_READ,
        };
        // Throw away any stale bytes waiting on the port before writing, so the reply read is fresh.
        let reset = message.get("reset_input").and_then(Value::as_bool).unwrap_or(false);
        let data =
            self.panels.hub().serial(name, reset, &chunks, size, Some(timeout)).map_err(|e| format!("{name}: {e}"))?;
        // `unwrap` can't fail: a `json!` object literal is always an object.
        Ok(json!({"data": to_hex(&data)}).as_object().unwrap().clone())
    }

    /// "serial_many": commands for several panels, one command per panel in turn.
    pub fn serial_many(&self, message: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let items = message.get("ports").and_then(Value::as_array).ok_or("no ports given")?;
        let mut work = Vec::new();
        for item in items {
            let item = item.as_object().ok_or("each port must be an object")?;
            let name = item.get("port").and_then(Value::as_str).ok_or("no port given")?.to_string();
            let mut writes = item.clone();
            // A port with no "writes" gets an empty list (rather than falling back to "write").
            writes.entry("writes").or_insert(json!([]));
            work.push((name, chunks(&writes)?));
        }
        self.panels.hub().serial_many(&work).map_err(|e| e.to_string())?;
        Ok(Map::new())
    }

    /// "hold": the monitor stops drawing on these panels until they are released. The reply also has
    /// the monitor's last frame on each one, column by column as hex, so the window can play the app
    /// transition from it.
    pub fn hold(&self, message: &Map<String, Value>) -> Map<String, Value> {
        // The window's process id, so the panels come back by themselves if that process dies.
        let owner = message.get("pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok());
        let names = port_names(message.get("ports"));
        let mut frames = Map::new();
        for name in &names {
            if let Some(frame) = self.panels.hub().last_frame(name) {
                frames.insert(name.clone(), json!(to_hex(&frame.concat())));
            }
        }
        let held = self.panels.hub().hold(&names, owner);
        json!({"held": held, "frames": frames}).as_object().unwrap().clone()
    }

    /// "frames": what each connected panel shows now, column by column as hex by port (for the window's
    /// Home page picture).
    pub fn frames(&self) -> Map<String, Value> {
        let mut frames = Map::new();
        for name in self.panels.port_names().into_iter().flatten() {
            if let Some(frame) = self.panels.hub().last_frame(&name) {
                frames.insert(name, json!(to_hex(&frame.concat())));
            }
        }
        json!({"frames": frames}).as_object().unwrap().clone()
    }

    /// "release": these panels (all when "ports" is null) go back to the monitor. Returns the ones
    /// that were held; the caller has the monitor draw on them again.
    pub fn release(&self, ports: Option<&Value>) -> Vec<String> {
        let mut released = match ports {
            Some(v @ Value::Array(_)) => self.panels.hub().release(Some(&port_names(Some(v)))),
            _ => self.panels.hub().release(None),
        };
        released.sort();
        released
    }

    /// Gives back the panels whose window has closed or crashed. True when any were given back.
    /// `alive` says whether a process id is still running (`pid_alive`).
    pub fn release_orphans(&self, alive: impl Fn(u32) -> bool) -> bool {
        let before = self.panels.hub().held();
        self.panels.hub().release_dead_holders(&alive);
        self.panels.hub().held() != before
    }
}

/// Whether a process is still running, to notice a window that closed without giving panels back.
///
/// OpenProcess fails with ERROR_INVALID_PARAMETER for a pid that doesn't exist (that includes 0);
/// a process that exists but can't be opened (access denied) counts as alive, and so does one whose
/// exit code can't be read. An exited process whose handle someone still holds reports an exit code
/// other than STILL_ACTIVE, so it counts as gone.
#[cfg(windows)]
pub fn pid_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: plain calls; the handle is closed before returning.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // No such process; any other refusal (access denied) means it is running
            return GetLastError() != ERROR_INVALID_PARAMETER;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok == 0 || code == STILL_ACTIVE as u32
    }
}

/// Whether a process is still running (Linux, for the tests): its /proc entry exists.
#[cfg(not(windows))]
pub fn pid_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Hex text ("32ac", any case, surrounding spaces ignored) as bytes; Err for an odd length,
/// non-ASCII text or a non-hex digit. The ASCII check makes the 2-byte slicing below safe.
fn hex_bytes(text: &str) -> Result<Vec<u8>, String> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err(format!("not hex: {text:?}"));
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| format!("not hex: {text:?}")))
        .collect()
}

/// Bytes as lower-case hex text.
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The "writes" list (or the single "write") of a request, as bytes.
/// Each entry is one firmware command, written separately. A missing "write" is an empty write;
/// a non-string entry in "writes" also counts as empty.
fn chunks(message: &Map<String, Value>) -> Result<Vec<Vec<u8>>, String> {
    match message.get("writes") {
        Some(Value::Array(list)) => list.iter().map(|c| hex_bytes(c.as_str().unwrap_or(""))).collect(),
        Some(Value::Null) | None => Ok(vec![hex_bytes(message.get("write").and_then(Value::as_str).unwrap_or(""))?]),
        Some(_) => Err("writes must be a list".into()),
    }
}

/// The port names in a JSON list; anything that isn't a list gives none, non-strings are skipped.
fn port_names(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => vec![],
    }
}

/// Tests of the bridge with fake serial ports. Public so the commands' tests can reuse `bridge()`.
#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io;
    use std::sync::Mutex;
    use wr_panel::hub::{Port, PortSystem};
    use wr_panel::ports::{Pins, PortInfo};

    /// Every write to any fake port: (port name, bytes).
    type Log = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    /// A port that records writes and answers any read with the bytes 1, 2, 3 ...
    struct FakePort(String, Log);
    impl Port for FakePort {
        fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
            self.1.lock().unwrap().push((self.0.clone(), data.to_vec()));
            Ok(())
        }
        fn read(&mut self, size: usize, _: Duration) -> io::Result<Vec<u8>> {
            Ok((1..=size as u8).collect())
        }
        fn clear_input(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A port system whose ports all open (as fakes) and that lists none.
    #[derive(Default)]
    pub struct FakePorts(Log);
    impl PortSystem for FakePorts {
        fn list(&self) -> Vec<PortInfo> {
            vec![]
        }
        fn open(&self, name: &str) -> io::Result<Box<dyn Port>> {
            Ok(Box::new(FakePort(name.into(), self.0.clone())))
        }
    }

    /// A bridge over fake ports, and the fakes to inspect.
    pub fn bridge() -> (Arc<FakePorts>, Bridge) {
        let fake = Arc::new(FakePorts::default());
        let panels = Panels::new(fake.clone(), Arc::new(Mutex::new(Pins::default())));
        (fake, Bridge::new(panels))
    }

    /// A JSON object literal as a request map.
    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    /// Each command is its own write, and the requested reply bytes come back as hex.
    #[test]
    fn serial_writes_each_command_and_reads_the_reply() {
        let (fake, bridge) = bridge();
        let reply = bridge.serial(&obj(json!({"port": "COM4", "writes": ["3286", "328603"], "read": 2}))).unwrap();
        assert_eq!(reply, obj(json!({"data": "0102"})));
        assert_eq!(
            *fake.0.lock().unwrap(),
            vec![("COM4".into(), vec![0x32, 0x86]), ("COM4".into(), vec![0x32, 0x86, 0x03])]
        );
    }

    /// Holding reports the held ports, a dead holder's panels are released, and release lists the rest.
    #[test]
    fn hold_release_and_closed_windows() {
        let (_, bridge) = bridge();
        // No frames: nothing has been drawn on COM4
        assert_eq!(
            bridge.hold(&obj(json!({"ports": ["COM4"], "pid": 7}))),
            obj(json!({"held": ["com4"], "frames": {}}))
        );
        bridge.hold(&obj(json!({"ports": ["COM5"], "pid": 8})));
        assert!(bridge.release_orphans(|pid| pid == 8));
        assert_eq!(bridge.release(None), vec!["com5"]);
    }

    /// With nothing drawn yet there are no frames to report.
    #[test]
    fn frames_are_empty_before_anything_is_drawn() {
        let (_, bridge) = bridge();
        assert_eq!(bridge.frames(), obj(json!({"frames": {}})));
    }

    /// Huge, negative and non-numeric timeouts are handled without panicking.
    #[test]
    fn odd_timeouts_are_clamped_not_a_panic() {
        let (_, bridge) = bridge();
        for t in [json!(1e300), json!(-1), json!(f64::MAX), json!("x"), json!(null)] {
            let reply = bridge.serial(&obj(json!({"port": "COM4", "write": "32ac", "read": 1, "timeout": t})));
            assert!(reply.is_ok(), "{t}: {reply:?}");
        }
    }

    /// Text that isn't hex is refused.
    #[test]
    fn bad_hex_is_an_error() {
        let (_, bridge) = bridge();
        assert!(bridge.serial(&obj(json!({"port": "COM4", "write": "zz"}))).is_err());
    }
}
