//! Opening the panels' serial ports, shared: a port of `led_mon/serial_ports.py` and the monitor's
//! side of `white_rabbit/bridge.py`.
//!
//! Windows lets only one program open a COM port, and only once. So every user of a panel (its drawing
//! thread, and the White Rabbit window's commands that come in over the control pipe) gets a handle onto
//! one shared open port, and writes take turns.
//!
//! Holding a panel: while the window plays a game or shows a pattern on a panel, the drawing threads'
//! writes to it are dropped and only the window's (control) writes get through. Releasing the panel
//! lets the monitor draw on it again.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use wr_core::{info, warn, HEIGHT, WIDTH};

use crate::dimming::Dimming;
use crate::ports::PortInfo;
use crate::protocol;

/// Longest wait for a panel's reply to the window. Old firmware doesn't answer some commands at all.
pub const MAX_READ: Duration = Duration::from_secs(3);

/// An open serial port.
pub trait Port: Send {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
    /// Reads up to `size` bytes, waiting at most `timeout` for them all; fewer if they don't come.
    fn read(&mut self, size: usize, timeout: Duration) -> io::Result<Vec<u8>>;
    /// Throws away anything the panel sent that nobody read.
    fn clear_input(&mut self) -> io::Result<()>;
}

/// Listing and opening serial ports. Windows in the app, made-up ports in the tests.
pub trait PortSystem: Send + Sync {
    fn list(&self) -> Vec<PortInfo>;
    fn open(&self, name: &str) -> io::Result<Box<dyn Port>>;
}

/// What is known about one open port. Locked for a whole frame, so commands from two users never mix.
pub struct PortState {
    io: Box<dyn Port>,
    /// The last full frame drawn and when, or None when unknown. Any other write forgets it.
    pub shown: Option<([[u8; HEIGHT]; WIDTH], Instant)>,
    pub dimming: Dimming,
    broken: bool,
    name: String,
    held: Arc<Mutex<HashMap<String, Option<u32>>>>,
}

impl PortState {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// True while the window holds this panel.
    pub fn is_held(&self) -> bool {
        self.held.lock().unwrap().contains_key(&self.name.to_lowercase())
    }

    /// Writes for the monitor: dropped (as if sent) while the window holds the panel.
    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        if self.is_held() {
            return Ok(());
        }
        self.write_control(data)
    }

    /// Writes for the window: always sent.
    pub fn write_control(&mut self, data: &[u8]) -> io::Result<()> {
        self.shown = None;
        if self.broken {
            return Err(io::Error::other(format!("{} needs to be opened again", self.name)));
        }
        self.io.write_all(data).inspect_err(|_| {
            // e.g. the handle was invalidated by sleep: every user must open it again
            self.broken = true;
        })
    }

    pub fn read(&mut self, size: usize, timeout: Duration) -> io::Result<Vec<u8>> {
        self.io.read(size, timeout)
    }

    pub fn clear_input(&mut self) -> io::Result<()> {
        self.io.clear_input()
    }
}

/// One open port, shared. Closed when the last user lets go of it.
pub struct SharedPort {
    name: String,
    state: Mutex<PortState>,
}

impl SharedPort {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn lock(&self) -> MutexGuard<'_, PortState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// All open panel ports, and which ones the window holds.
pub struct Hub {
    system: Arc<dyn PortSystem>,
    open: Mutex<HashMap<String, Weak<SharedPort>>>,
    /// {port name in lower case: the window's process id, if it said}
    held: Arc<Mutex<HashMap<String, Option<u32>>>>,
}

impl Hub {
    pub fn new(system: Arc<dyn PortSystem>) -> Hub {
        Hub { system, open: Mutex::default(), held: Arc::default() }
    }

    pub fn list(&self) -> Vec<PortInfo> {
        self.system.list()
    }

    /// A handle onto the port, opening it if no one has it open (or its handle broke).
    pub fn open(&self, name: &str) -> io::Result<Arc<SharedPort>> {
        let key = name.to_lowercase();
        let mut open = self.open.lock().unwrap();
        if let Some(port) = open.get(&key).and_then(Weak::upgrade) {
            if !port.lock().broken {
                return Ok(port);
            }
        }
        let io = self.system.open(name)?;
        let port = Arc::new(SharedPort {
            name: name.to_string(),
            state: Mutex::new(PortState {
                io,
                shown: None,
                dimming: Dimming::default(),
                broken: false,
                name: name.to_string(),
                held: self.held.clone(),
            }),
        });
        open.insert(key, Arc::downgrade(&port));
        open.retain(|_, p| p.strong_count() > 0);
        Ok(port)
    }

    /// Lends panels to the window (`owner` = its process id). Their LED current goes back to full first,
    /// so the window's pictures look as it expects.
    pub fn hold(&self, ports: &[String], owner: Option<u32>) -> Vec<String> {
        for name in ports {
            if let Some(port) = self.open.lock().unwrap().get(&name.to_lowercase()).and_then(Weak::upgrade) {
                let mut state = port.lock();
                if state.dimming.current.is_some_and(|c| c != protocol::FULL_CURRENT) {
                    if let Err(e) = state.write_control(&protocol::global_current(protocol::FULL_CURRENT)) {
                        warn!("Resetting the dimming on {name}: {e}");
                    }
                }
                state.dimming.current = None;
            }
            self.held.lock().unwrap().insert(name.to_lowercase(), owner);
            info!("The window holds {name}");
        }
        self.held()
    }

    /// Gives panels back to the monitor; all of them when `ports` is None. Returns the ones released.
    pub fn release(&self, ports: Option<&[String]>) -> Vec<String> {
        let mut held = self.held.lock().unwrap();
        let names: Vec<String> = match ports {
            Some(p) => p.iter().map(|n| n.to_lowercase()).filter(|n| held.contains_key(n)).collect(),
            None => held.keys().cloned().collect(),
        };
        for n in &names {
            held.remove(n);
            info!("The monitor has {n} again");
        }
        names
    }

    pub fn is_held(&self, name: &str) -> bool {
        self.held.lock().unwrap().contains_key(&name.to_lowercase())
    }

    /// The held ports, sorted.
    pub fn held(&self) -> Vec<String> {
        let mut v: Vec<String> = self.held.lock().unwrap().keys().cloned().collect();
        v.sort();
        v
    }

    /// Gives back the panels held by windows that have closed or crashed (call every few seconds).
    pub fn release_dead_holders(&self, alive: &dyn Fn(u32) -> bool) {
        let dead: Vec<String> = self
            .held
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, owner)| owner.is_some_and(|pid| !alive(pid)))
            .map(|(name, _)| name.clone())
            .collect();
        if !dead.is_empty() {
            info!("The window holding {} has closed", dead.join(", "));
            self.release(Some(&dead));
        }
    }

    /// The window's `serial` command: write each command on its own (the firmware reads one command
    /// per USB read), then read `read` bytes of reply if asked.
    pub fn serial(
        &self,
        port: &str,
        reset_input: bool,
        writes: &[Vec<u8>],
        read: usize,
        timeout: Option<Duration>,
    ) -> io::Result<Vec<u8>> {
        let shared = self.open(port)?;
        let mut state = shared.lock();
        if reset_input {
            state.clear_input()?;
        }
        for w in writes.iter().filter(|w| !w.is_empty()) {
            state.write_control(w)?;
        }
        if read == 0 {
            return Ok(Vec::new());
        }
        state.read(read, timeout.map_or(MAX_READ, |t| t.min(MAX_READ)))
    }

    /// The window's `serial_many` command: commands to several panels, taking turns between them, so
    /// while one panel works on its command the next already gets its own.
    pub fn serial_many(&self, items: &[(String, Vec<Vec<u8>>)]) -> io::Result<()> {
        let ports = items.iter().map(|(name, _)| self.open(name)).collect::<io::Result<Vec<_>>>()?;
        // Lock in a fixed order, so two of these at once can't each wait for the other's port
        let mut order: Vec<usize> = (0..ports.len()).collect();
        order.sort_by_key(|&i| ports[i].name().to_lowercase());
        order.dedup_by_key(|i| ports[*i].name().to_lowercase());
        let mut guards: HashMap<usize, MutexGuard<'_, PortState>> =
            order.iter().map(|&i| (i, ports[i].lock())).collect();
        let index_of =
            |i: usize| order.iter().copied().find(|&j| ports[j].name().eq_ignore_ascii_case(ports[i].name()));
        let steps = items.iter().map(|(_, c)| c.len()).max().unwrap_or(0);
        for step in 0..steps {
            for (i, (_, chunks)) in items.iter().enumerate() {
                if let Some(chunk) = chunks.get(step).filter(|c| !c.is_empty()) {
                    let g = guards.get_mut(&index_of(i).unwrap()).unwrap();
                    g.write_control(chunk)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! Made-up ports for the tests.
    use super::*;

    /// (port name, bytes) of every write.
    pub type Log = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    #[derive(Default)]
    pub struct Fake {
        pub ports: Mutex<Vec<PortInfo>>,
        pub written: Log,
        /// Ports whose firmware answers queries (with 32 bytes).
        pub answers: Mutex<Vec<String>>,
        pub fail_writes: Mutex<Vec<String>>,
    }

    struct FakePort {
        name: String,
        log: Log,
        answers: bool,
        pending: usize,
        fake: Weak<Fake>,
    }

    impl Port for FakePort {
        fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
            if let Some(f) = self.fake.upgrade() {
                if f.fail_writes.lock().unwrap().contains(&self.name) {
                    return Err(io::Error::other("gone"));
                }
            }
            if self.answers && data == protocol::global_current_query() {
                self.pending = protocol::ANSWER_LEN;
            }
            self.log.lock().unwrap().push((self.name.clone(), data.to_vec()));
            Ok(())
        }
        fn read(&mut self, size: usize, _timeout: Duration) -> io::Result<Vec<u8>> {
            let n = size.min(self.pending);
            self.pending -= n;
            Ok(vec![7; n])
        }
        fn clear_input(&mut self) -> io::Result<()> {
            self.pending = 0;
            Ok(())
        }
    }

    pub struct FakeSystem(pub Arc<Fake>);

    impl PortSystem for FakeSystem {
        fn list(&self) -> Vec<PortInfo> {
            self.0.ports.lock().unwrap().clone()
        }
        fn open(&self, name: &str) -> io::Result<Box<dyn Port>> {
            Ok(Box::new(FakePort {
                name: name.to_string(),
                log: self.0.written.clone(),
                answers: self.0.answers.lock().unwrap().iter().any(|a| a.eq_ignore_ascii_case(name)),
                pending: 0,
                fake: Arc::downgrade(&self.0),
            }))
        }
    }

    pub fn panel_port(name: &str, location: &str) -> PortInfo {
        PortInfo {
            device: name.into(),
            vid: Some(protocol::LED_MATRIX_VID),
            pid: Some(protocol::LED_MATRIX_PID),
            location: Some(location.into()),
            description: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    fn hub() -> (Arc<Fake>, Hub) {
        let fake = Arc::new(Fake::default());
        let hub = Hub::new(Arc::new(FakeSystem(fake.clone())));
        (fake, hub)
    }

    #[test]
    fn one_open_port_shared() {
        let (_, hub) = hub();
        let a = hub.open("COM4").unwrap();
        let b = hub.open("com4").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        drop((a, b));
        // closed when the last user let go: the next open is a new one
        assert_eq!(hub.open.lock().unwrap().get("com4").unwrap().strong_count(), 0);
    }

    #[test]
    fn held_panels_take_only_the_windows_writes() {
        let (fake, hub) = hub();
        let port = hub.open("COM4").unwrap();
        hub.hold(&["COM4".into()], Some(42));
        port.lock().write(b"monitor").unwrap();
        hub.serial("COM4", false, &[b"window".to_vec()], 0, None).unwrap();
        assert_eq!(fake.written.lock().unwrap().iter().map(|(_, b)| b.clone()).collect::<Vec<_>>(), [b"window"]);
        hub.release_dead_holders(&|pid| pid == 42);
        assert!(hub.is_held("com4"));
        hub.release_dead_holders(&|_| false);
        assert!(!hub.is_held("com4"));
        port.lock().write(b"monitor").unwrap();
        assert_eq!(fake.written.lock().unwrap().len(), 2);
    }

    #[test]
    fn serial_reads_the_reply() {
        let (fake, hub) = hub();
        fake.answers.lock().unwrap().push("COM4".into());
        let reply = hub.serial("COM4", true, &[protocol::global_current_query()], 32, None).unwrap();
        assert_eq!(reply.len(), 32);
    }

    #[test]
    fn serial_many_takes_turns() {
        let (fake, hub) = hub();
        let items = vec![
            ("COM4".to_string(), vec![b"a1".to_vec(), b"a2".to_vec()]),
            ("COM5".to_string(), vec![b"b1".to_vec()]),
        ];
        hub.serial_many(&items).unwrap();
        let w: Vec<_> = fake.written.lock().unwrap().iter().map(|(_, b)| b.clone()).collect();
        assert_eq!(w, [b"a1".to_vec(), b"b1".to_vec(), b"a2".to_vec()]);
    }

    #[test]
    fn a_broken_port_is_opened_again() {
        let (fake, hub) = hub();
        let a = hub.open("COM4").unwrap();
        fake.fail_writes.lock().unwrap().push("COM4".into());
        assert!(a.lock().write(b"x").is_err());
        fake.fail_writes.lock().unwrap().clear();
        assert!(a.lock().write(b"x").is_err(), "a broken handle stays broken");
        let b = hub.open("COM4").unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
        b.lock().write(b"x").unwrap();
    }
}
