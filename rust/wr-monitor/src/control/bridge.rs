//! Sharing the LED panels with the White Rabbit window: a port of the monitor's side of
//! `white_rabbit/bridge.py`.
//!
//! Windows lets only one program open a panel's serial port, and the monitor keeps both open. So the
//! window sends its commands through the control channel instead:
//!
//!   serial       write bytes to a panel and optionally read the reply
//!   serial_many  write to several panels at once, taking turns between them
//!   hold         lend panels to the window: the monitor stops drawing on them
//!   release      give them back to the monitor
//!
//! `Bridge` sits between the panels' drawing threads and the real ports (it is their `PortSystem`).
//! Each port is opened once and shared: the drawing thread writes frames through it, and the control
//! channel writes the window's bytes and reads the replies through the same handle. While a panel is
//! held, the drawing thread's writes to it are dropped.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::{json, Map, Value};

use wr_core::info;
use wr_panel::panel::PortSystem;
use wr_panel::ports::PortInfo;

/// Longest wait for a panel's reply. Old firmware doesn't answer some commands at all.
pub const MAX_READ: Duration = Duration::from_secs(3);

/// A serial port the monitor can both write to and read from.
pub trait RawPort: Read + Write + Send {
    /// How long a read waits for the panel's reply.
    fn set_read_timeout(&mut self, timeout: Duration) -> io::Result<()>;
    /// Forget whatever the panel sent that nobody read.
    fn reset_input(&mut self) -> io::Result<()>;
}

/// Listing and opening ports: Windows in the app, made-up ports in the tests.
pub trait RawPorts: Send + Sync {
    fn list(&self) -> Vec<PortInfo>;
    fn open(&self, name: &str) -> io::Result<Box<dyn RawPort>>;
}

type PortCell = Mutex<Box<dyn RawPort>>;
type SharedPort = Arc<PortCell>;
/// The held panels: {port name in lower case: the window's process id}.
type Held = Arc<Mutex<HashMap<String, Option<u64>>>>;

pub struct Bridge {
    raw: Arc<dyn RawPorts>,
    /// The ports the drawing threads have open: {lower-case name: (name, port)}. A port closes when
    /// its drawing thread lets go of it.
    open: Mutex<HashMap<String, (String, Weak<PortCell>)>>,
    held: Held,
}

/// What a drawing thread writes to: the shared port, or nowhere while the panel is held.
struct DrawingPort {
    key: String,
    port: SharedPort,
    held: Held,
}

impl Write for DrawingPort {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.held.lock().unwrap().contains_key(&self.key) {
            return Ok(buf.len());
        }
        self.port.lock().unwrap().write(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        // One lock for the whole frame, so the window's bytes never land in the middle of it
        if self.held.lock().unwrap().contains_key(&self.key) {
            return Ok(());
        }
        self.port.lock().unwrap().write_all(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.port.lock().unwrap().flush()
    }
}

impl PortSystem for Bridge {
    fn list(&self) -> Vec<PortInfo> {
        self.raw.list()
    }

    fn open(&self, name: &str) -> io::Result<Box<dyn Write + Send>> {
        let port: SharedPort = Arc::new(Mutex::new(self.raw.open(name)?));
        let key = name.to_lowercase();
        self.open.lock().unwrap().insert(key.clone(), (name.to_string(), Arc::downgrade(&port)));
        Ok(Box::new(DrawingPort { key, port, held: self.held.clone() }))
    }
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

/// Up to `size` bytes, waiting no longer than the port's read timeout (pyserial's `read(size)`).
fn read_reply(port: &mut dyn RawPort, size: usize) -> io::Result<Vec<u8>> {
    let mut data = vec![0u8; size];
    let mut got = 0;
    while got < size {
        match port.read(&mut data[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::TimedOut => break,
            Err(e) => return Err(e),
        }
    }
    data.truncate(got);
    Ok(data)
}

impl Bridge {
    pub fn new(raw: Arc<dyn RawPorts>) -> Bridge {
        Bridge { raw, open: Mutex::new(HashMap::new()), held: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// The ports the drawing threads have open, for the window's status request.
    pub fn open_ports(&self) -> Vec<String> {
        let mut open = self.open.lock().unwrap();
        open.retain(|_, (_, port)| port.strong_count() > 0);
        let mut names: Vec<String> = open.values().map(|(name, _)| name.clone()).collect();
        names.sort();
        names
    }

    /// The drawing thread's handle to this port, or the port opened just for this request.
    fn port(&self, name: &str) -> io::Result<SharedPort> {
        let live = self.open.lock().unwrap().get(&name.to_lowercase()).and_then(|(_, p)| p.upgrade());
        match live {
            Some(port) => Ok(port),
            None => Ok(Arc::new(Mutex::new(self.raw.open(name)?))),
        }
    }

    /// "serial": write the window's commands one by one (the panel's firmware reads one command per
    /// USB read, so commands sent together would be lost after the first) and read the reply.
    pub fn serial(&self, message: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let name = message.get("port").and_then(Value::as_str).ok_or("no port given")?;
        let chunks = chunks(message)?;
        let size = message.get("read").and_then(Value::as_u64).unwrap_or(0) as usize;
        let timeout = match message.get("timeout").and_then(Value::as_f64) {
            Some(t) if t >= 0.0 => Duration::from_secs_f64(t).min(MAX_READ),
            _ => MAX_READ,
        };
        let shared = self.port(name).map_err(|e| format!("could not open {name}: {e}"))?;
        let mut port = shared.lock().unwrap();
        let result = (|| -> io::Result<Vec<u8>> {
            if message.get("reset_input").and_then(Value::as_bool).unwrap_or(false) {
                port.reset_input()?;
            }
            for chunk in chunks.iter().filter(|c| !c.is_empty()) {
                port.write_all(chunk)?;
            }
            if size == 0 {
                return Ok(vec![]);
            }
            port.set_read_timeout(timeout)?;
            read_reply(port.as_mut(), size)
        })();
        let data = result.map_err(|e| format!("{name}: {e}"))?;
        Ok(json!({"data": to_hex(&data)}).as_object().unwrap().clone())
    }

    /// "serial_many": commands for several panels, one command per panel in turn, so while one panel
    /// works on its command the next one already gets its own.
    pub fn serial_many(&self, message: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let items = message.get("ports").and_then(Value::as_array).ok_or("no ports given")?;
        let mut work = Vec::new();
        for item in items {
            let item = item.as_object().ok_or("each port must be an object")?;
            let name = item.get("port").and_then(Value::as_str).ok_or("no port given")?.to_string();
            let mut writes = item.clone();
            writes.entry("writes").or_insert(json!([]));
            let shared = self.port(&name).map_err(|e| format!("could not open {name}: {e}"))?;
            work.push((name, chunks(&writes)?, shared));
        }
        // Lock in a fixed order, so two of these at once can't each wait for the other's port
        work.sort_by_key(|(name, _, _)| name.to_lowercase());
        let mut guards: Vec<_> = work.iter().map(|(_, _, p)| p.lock().unwrap()).collect();
        let steps = work.iter().map(|(_, c, _)| c.len()).max().unwrap_or(0);
        for step in 0..steps {
            for (i, (name, chunks, _)) in work.iter().enumerate() {
                if let Some(chunk) = chunks.get(step).filter(|c| !c.is_empty()) {
                    guards[i].write_all(chunk).map_err(|e| format!("{name}: {e}"))?;
                }
            }
        }
        Ok(Map::new())
    }

    /// "hold": the monitor stops drawing on these panels until they are released.
    pub fn hold(&self, message: &Map<String, Value>) -> Map<String, Value> {
        let owner = message.get("pid").and_then(Value::as_u64);
        let mut held = self.held.lock().unwrap();
        for name in port_names(message.get("ports")) {
            held.insert(name.to_lowercase(), owner);
        }
        let mut names: Vec<&String> = held.keys().collect();
        names.sort();
        json!({"held": names}).as_object().unwrap().clone()
    }

    /// "release": these panels (all when "ports" is null) go back to the monitor. Returns the ones
    /// that were held; the caller has the monitor draw on them again.
    pub fn release(&self, ports: Option<&Value>) -> Vec<String> {
        let mut held = self.held.lock().unwrap();
        let names: Vec<String> = match ports {
            Some(v @ Value::Array(_)) => port_names(Some(v)).iter().map(|n| n.to_lowercase()).collect(),
            _ => held.keys().cloned().collect(),
        };
        let mut released: Vec<String> = names.into_iter().filter(|n| held.remove(n).is_some()).collect();
        released.sort();
        released
    }

    /// Gives back the panels whose window has closed or crashed. Returns the ones given back.
    pub fn release_orphans(&self, alive: impl Fn(u64) -> bool) -> Vec<String> {
        let orphans: Vec<String> = {
            let held = self.held.lock().unwrap();
            held.iter().filter(|(_, pid)| pid.is_some_and(|p| !alive(p))).map(|(n, _)| n.clone()).collect()
        };
        for port in &orphans {
            info!("The window holding {port} has closed");
        }
        self.release(Some(&json!(orphans)))
    }

    #[cfg(test)]
    pub fn is_held(&self, name: &str) -> bool {
        self.held.lock().unwrap().contains_key(&name.to_lowercase())
    }
}

/// Whether a process is still running, to notice a window that closed without giving panels back.
#[cfg(windows)]
pub fn pid_alive(pid: u64) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let Ok(pid) = u32::try_from(pid) else { return false };
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
pub fn pid_alive(pid: u64) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::VecDeque;

    type Log = Arc<Mutex<Vec<(String, Vec<u8>)>>>;
    type Replies = Arc<Mutex<HashMap<String, VecDeque<u8>>>>;

    /// (port name, bytes) of every write, and the bytes each port answers with.
    #[derive(Default)]
    pub struct FakePorts {
        pub written: Log,
        pub replies: Replies,
    }

    struct FakePort {
        name: String,
        written: Log,
        replies: Replies,
    }

    impl Read for FakePort {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut replies = self.replies.lock().unwrap();
            let q = replies.entry(self.name.clone()).or_default();
            let n = buf.len().min(q.len());
            for b in buf.iter_mut().take(n) {
                *b = q.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    impl Write for FakePort {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.lock().unwrap().push((self.name.clone(), buf.to_vec()));
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl RawPort for FakePort {
        fn set_read_timeout(&mut self, _: Duration) -> io::Result<()> {
            Ok(())
        }
        fn reset_input(&mut self) -> io::Result<()> {
            self.replies.lock().unwrap().remove(&self.name);
            Ok(())
        }
    }

    impl RawPorts for FakePorts {
        fn list(&self) -> Vec<PortInfo> {
            vec![]
        }
        fn open(&self, name: &str) -> io::Result<Box<dyn RawPort>> {
            Ok(Box::new(FakePort { name: name.into(), written: self.written.clone(), replies: self.replies.clone() }))
        }
    }

    pub fn bridge() -> (Arc<FakePorts>, Bridge) {
        let fake = Arc::new(FakePorts::default());
        (fake.clone(), Bridge::new(fake))
    }

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn writes(fake: &FakePorts) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut *fake.written.lock().unwrap())
    }

    #[test]
    fn a_held_panel_gets_the_windows_bytes_and_not_the_monitors() {
        let (fake, bridge) = bridge();
        let mut drawing = PortSystem::open(&bridge, "COM4").unwrap();
        assert_eq!(bridge.open_ports(), vec!["COM4"]);
        drawing.write_all(b"frame").unwrap();
        assert_eq!(writes(&fake), vec![("COM4".into(), b"frame".to_vec())]);

        assert_eq!(
            bridge.hold(&obj(json!({"cmd": "hold", "ports": ["com4"], "pid": 1}))),
            obj(json!({"held": ["com4"]}))
        );
        drawing.write_all(b"frame").unwrap();
        fake.replies.lock().unwrap().insert("COM4".into(), VecDeque::from(b"\x01\x02\x03".to_vec()));
        let reply = bridge
            .serial(&obj(json!({"cmd": "serial", "port": "COM4", "writes": ["3286", "", "328603"], "read": 2})))
            .unwrap();
        assert_eq!(reply, obj(json!({"data": "0102"})));
        // Each command on its own; the drawing thread's frame went nowhere
        assert_eq!(writes(&fake), vec![("COM4".into(), vec![0x32, 0x86]), ("COM4".into(), vec![0x32, 0x86, 0x03])]);

        assert_eq!(bridge.release(None), vec!["com4"]);
        drawing.write_all(b"again").unwrap();
        assert_eq!(writes(&fake), vec![("COM4".into(), b"again".to_vec())]);
        drop(drawing);
        assert!(bridge.open_ports().is_empty());
    }

    #[test]
    fn serial_many_takes_turns() {
        let (fake, bridge) = bridge();
        bridge
            .serial_many(&obj(json!({"cmd": "serial_many", "ports": [
                {"port": "COM5", "writes": ["01", "02"]},
                {"port": "COM4", "writes": ["0a"]}]})))
            .unwrap();
        let order: Vec<_> = writes(&fake).into_iter().map(|(p, b)| (p, b[0])).collect();
        assert_eq!(order, vec![("COM4".into(), 0x0a), ("COM5".into(), 1), ("COM5".into(), 2)]);
    }

    #[test]
    fn a_closed_window_gives_its_panels_back() {
        let (_, bridge) = bridge();
        bridge.hold(&obj(json!({"ports": ["COM4"], "pid": 7})));
        bridge.hold(&obj(json!({"ports": ["COM5"], "pid": 8})));
        assert_eq!(bridge.release_orphans(|pid| pid == 8), vec!["com4"]);
        assert!(!bridge.is_held("COM4") && bridge.is_held("COM5"));
    }

    #[test]
    fn bad_hex_is_an_error() {
        let (_, bridge) = bridge();
        assert!(bridge.serial(&obj(json!({"port": "COM4", "write": "zz"}))).is_err());
    }
}
