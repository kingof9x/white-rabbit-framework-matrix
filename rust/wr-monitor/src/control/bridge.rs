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

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};

use wr_panel::hub::MAX_READ;
use wr_panel::panel::Panels;

pub struct Bridge {
    pub panels: Arc<Panels>,
}

impl Bridge {
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
        let size = message.get("read").and_then(Value::as_u64).unwrap_or(0) as usize;
        let timeout = match message.get("timeout").and_then(Value::as_f64) {
            Some(t) if t >= 0.0 => Duration::from_secs_f64(t).min(MAX_READ),
            _ => MAX_READ,
        };
        let reset = message.get("reset_input").and_then(Value::as_bool).unwrap_or(false);
        let data =
            self.panels.hub().serial(name, reset, &chunks, size, Some(timeout)).map_err(|e| format!("{name}: {e}"))?;
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
            writes.entry("writes").or_insert(json!([]));
            work.push((name, chunks(&writes)?));
        }
        self.panels.hub().serial_many(&work).map_err(|e| e.to_string())?;
        Ok(Map::new())
    }

    /// "hold": the monitor stops drawing on these panels until they are released.
    pub fn hold(&self, message: &Map<String, Value>) -> Map<String, Value> {
        let owner = message.get("pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok());
        let held = self.panels.hub().hold(&port_names(message.get("ports")), owner);
        json!({"held": held}).as_object().unwrap().clone()
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
    pub fn release_orphans(&self, alive: impl Fn(u32) -> bool) -> bool {
        let before = self.panels.hub().held();
        self.panels.hub().release_dead_holders(&alive);
        self.panels.hub().held() != before
    }
}

/// Whether a process is still running, to notice a window that closed without giving panels back.
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

#[cfg(not(windows))]
pub fn pid_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

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

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The "writes" list (or the single "write") of a request, as bytes.
fn chunks(message: &Map<String, Value>) -> Result<Vec<Vec<u8>>, String> {
    match message.get("writes") {
        Some(Value::Array(list)) => list.iter().map(|c| hex_bytes(c.as_str().unwrap_or(""))).collect(),
        Some(Value::Null) | None => Ok(vec![hex_bytes(message.get("write").and_then(Value::as_str).unwrap_or(""))?]),
        Some(_) => Err("writes must be a list".into()),
    }
}

fn port_names(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => vec![],
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io;
    use std::sync::Mutex;
    use wr_panel::hub::{Port, PortSystem};
    use wr_panel::ports::{Pins, PortInfo};

    type Log = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

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

    pub fn bridge() -> (Arc<FakePorts>, Bridge) {
        let fake = Arc::new(FakePorts::default());
        let panels = Panels::new(fake.clone(), Arc::new(Mutex::new(Pins::default())));
        (fake, Bridge::new(panels))
    }

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

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

    #[test]
    fn hold_release_and_closed_windows() {
        let (_, bridge) = bridge();
        assert_eq!(bridge.hold(&obj(json!({"ports": ["COM4"], "pid": 7}))), obj(json!({"held": ["com4"]})));
        bridge.hold(&obj(json!({"ports": ["COM5"], "pid": 8})));
        assert!(bridge.release_orphans(|pid| pid == 8));
        assert_eq!(bridge.release(None), vec!["com5"]);
    }

    #[test]
    fn bad_hex_is_an_error() {
        let (_, bridge) = bridge();
        assert!(bridge.serial(&obj(json!({"port": "COM4", "write": "zz"}))).is_err());
    }
}
