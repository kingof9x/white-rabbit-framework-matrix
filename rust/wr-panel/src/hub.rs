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
//!
//! # Who uses this file
//!
//! The monitor creates one `Hub` at start-up (with the Windows `PortSystem` from `winports.rs`, or a fake
//! one in the tests). The two drawing threads in `panel.rs` call `Hub::open` to get their panel's
//! `SharedPort` and lock it for each frame. The monitor's control channel thread calls `serial`,
//! `serial_many`, `hold` and `release` on behalf of the White Rabbit window, and the main loop calls
//! `release_dead_holders` every few seconds.
//!
//! # How sharing works
//!
//! `Hub::open` keeps a map of port name -> `Weak<SharedPort>`. A `Weak` does not keep the port open: when
//! the last `Arc<SharedPort>` (held by the drawing thread or a window command in progress) is dropped, the
//! `Box<dyn Port>` inside is dropped and the OS handle is closed. The next `open` then really opens the
//! port again. A port whose write failed is marked `broken` and is never handed out again; `open` makes a
//! fresh one instead.
//!
//! # Locks
//!
//! Three kinds of lock live here: `Hub::open` (the map), `SharedPort::state` (one port) and `Hub::held`
//! (shared with every `PortState` through an `Arc`). The order is the one in the table in `lib.rs`:
//! `open` before `state` before `held`. `Hub::open` checks `broken` under `open` (taking `state` inside
//! it), and `PortState::is_held` takes `held` while the caller has `state`; nothing takes them the other
//! way round. `hold` deliberately lets go of `held` and `open` before it locks the port.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use wr_core::{info, warn, HEIGHT, WIDTH};

use crate::dimming::Dimming;
use crate::ports::PortInfo;
use crate::protocol;

/// Longest wait for a panel's reply to the window. Old firmware doesn't answer some commands at all.
/// Also caps the timeout the window asks for, so a bad value can't block the control thread for long.
pub const MAX_READ: Duration = Duration::from_secs(3);

/// An open serial port.
/// `Send` because the port is created on one thread (whoever opened it) and used from others (the
/// drawing thread, the control channel thread), always behind the `SharedPort` mutex.
pub trait Port: Send {
    /// Writes all of `data` as one USB write. An error means the handle is no longer usable (the panel was
    /// unplugged, or the computer slept), and the caller marks the port broken.
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
    /// Reads up to `size` bytes, waiting at most `timeout` for them all; fewer if they don't come.
    fn read(&mut self, size: usize, timeout: Duration) -> io::Result<Vec<u8>>;
    /// Throws away anything the panel sent that nobody read.
    fn clear_input(&mut self) -> io::Result<()>;
}

/// Listing and opening serial ports. Windows in the app, made-up ports in the tests.
/// `Send + Sync` because one `Hub` (and so one system) is shared by every thread through an `Arc`.
pub trait PortSystem: Send + Sync {
    /// Every serial port the system has now, with its USB IDs and location if known (see `ports.rs`).
    fn list(&self) -> Vec<PortInfo>;
    /// Opens the named port (e.g. "COM4"). Fails if it is gone or another program already has it open.
    fn open(&self, name: &str) -> io::Result<Box<dyn Port>>;
}

/// What is known about one open port. Locked for a whole frame, so commands from two users never mix.
pub struct PortState {
    /// The real handle. Private so every write goes through `write`/`write_control`, which keep `shown`
    /// and `broken` right.
    io: Box<dyn Port>,
    /// The last full frame drawn and when, or None when unknown. Any other write forgets it.
    pub shown: Option<([[u8; HEIGHT]; WIDTH], Instant)>,
    /// The monitor's last frame on this panel, kept while the window writes (for its app transition).
    pub last_frame: Option<[[u8; HEIGHT]; WIDTH]>,
    /// Whether this panel's firmware can dim by LED current and the current it was last sent
    /// (see `dimming.rs`). Reset with the port: a newly opened port is asked again.
    pub dimming: Dimming,
    /// Set by the first failed write. From then on every write fails at once, and `Hub::open` won't hand
    /// this port out any more, so each user ends up opening the port again.
    broken: bool,
    /// The port name as it was first opened (e.g. "COM4"), kept for log messages and the held lookup.
    name: String,
    /// The same map as `Hub::held` (a shared `Arc`), so a port can tell on its own whether it is held.
    held: Arc<Mutex<HashMap<String, Option<u32>>>>,
}

impl PortState {
    /// The port name as given to `Hub::open` when it was opened.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// True while the window holds this panel.
    /// Takes the `held` lock briefly; called with this port's `state` lock already held, which matches
    /// the lock order (state before held).
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
        // Any write may change what the panel shows (a window command, a current change), so the
        // "already shown" frame is forgotten and the next frame from the drawing thread is sent for sure.
        // This is done before the `broken` check: a failed write leaves the picture unknown too.
        self.shown = None;
        if self.broken {
            return Err(io::Error::other(format!("{} needs to be opened again", self.name)));
        }
        self.io.write_all(data).inspect_err(|_| {
            // e.g. the handle was invalidated by sleep: every user must open it again
            self.broken = true;
        })
    }

    /// Reads a reply from the panel (see `Port::read`). Not blocked while held: only the window and the
    /// dimming query (which checks `is_held` itself) read.
    pub fn read(&mut self, size: usize, timeout: Duration) -> io::Result<Vec<u8>> {
        self.io.read(size, timeout)
    }

    /// Throws away unread bytes, so the next `read` gets the answer to the next query and not an old one.
    pub fn clear_input(&mut self) -> io::Result<()> {
        self.io.clear_input()
    }
}

/// One open port, shared. Closed when the last user lets go of it.
/// Handed out as `Arc<SharedPort>`; the `Hub` only keeps a `Weak` to it.
pub struct SharedPort {
    name: String,
    /// Everything about the open port. Each user locks it for a whole frame or command list.
    state: Mutex<PortState>,
}

impl SharedPort {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Locks the port. A poisoned lock (a thread panicked while holding it) is used anyway: the state
    /// is still sound (at worst `shown` is stale and gets resent), and refusing would stop the panel for
    /// good. The release build uses `panic = "abort"`, so poisoning can only happen in debug builds and tests.
    pub fn lock(&self) -> MutexGuard<'_, PortState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// All open panel ports, and which ones the window holds.
pub struct Hub {
    /// The real (Windows) or fake port list and opener.
    system: Arc<dyn PortSystem>,
    /// Port name in lower case (Windows port names ignore case) -> the shared open port, if anyone still
    /// has it. Dead entries are swept out on each new open.
    open: Mutex<HashMap<String, Weak<SharedPort>>>,
    /// {port name in lower case: the window's process id, if it said}
    held: Arc<Mutex<HashMap<String, Option<u32>>>>,
}

impl Hub {
    /// A hub with nothing open and nothing held.
    pub fn new(system: Arc<dyn PortSystem>) -> Hub {
        Hub { system, open: Mutex::default(), held: Arc::default() }
    }

    /// Every serial port the system has now (passes straight through to the `PortSystem`).
    pub fn list(&self) -> Vec<PortInfo> {
        self.system.list()
    }

    /// A handle onto the port, opening it if no one has it open (or its handle broke).
    /// Errors are the `PortSystem`'s (port gone, or in use by another program).
    ///
    /// The `open` map stays locked while the port is opened, so two threads asking for the same port at
    /// once can't both open it (Windows would refuse the second one anyway). It also waits on the port's
    /// own lock to read `broken`, which can take as long as one frame being written.
    pub fn open(&self, name: &str) -> io::Result<Arc<SharedPort>> {
        let key = name.to_lowercase();
        let mut open = self.open.lock().unwrap();
        if let Some(port) = open.get(&key).and_then(Weak::upgrade) {
            // A broken port is skipped here and replaced below. Its old handle stays open until its last
            // user drops its `Arc`, so on Windows the new open can fail with "access denied" until then;
            // the drawing threads simply retry later.
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
                last_frame: None,
                dimming: Dimming::default(),
                broken: false,
                name: name.to_string(),
                held: self.held.clone(),
            }),
        });
        // Only a weak reference is kept, so the port closes when its users are done with it. Entries
        // whose port has already closed are removed here rather than on every drop.
        open.insert(key, Arc::downgrade(&port));
        open.retain(|_, p| p.strong_count() > 0);
        Ok(port)
    }

    /// The monitor's last frame on this port, if it is open and has drawn one.
    pub fn last_frame(&self, name: &str) -> Option<[[u8; HEIGHT]; WIDTH]> {
        let port = self.open.lock().unwrap().get(&name.to_lowercase()).and_then(Weak::upgrade)?;
        // Copied out into a local so the port's lock guard is dropped before `port` itself.
        let frame = port.lock().last_frame;
        frame
    }

    /// Lends panels to the window (`owner` = its process id). Their LED current goes back to full first,
    /// so the window's pictures look as it expects.
    pub fn hold(&self, ports: &[String], owner: Option<u32>) -> Vec<String> {
        for name in ports {
            // Held first, so from here on the drawing threads' writes (and their dimming) are dropped;
            // then the current is reset under the port's lock, after any frame being drawn now
            // (`held` is locked only for this statement, so no other lock is taken while it is held.)
            self.held.lock().unwrap().insert(name.to_lowercase(), owner);
            // (The map's lock is let go before the port's: a write can take a while)
            let port = self.open.lock().unwrap().get(&name.to_lowercase()).and_then(Weak::upgrade);
            if let Some(port) = port {
                let mut state = port.lock();
                if state.dimming.current.is_some_and(|c| c != protocol::FULL_CURRENT) {
                    if let Err(e) = state.write_control(&protocol::global_current(protocol::FULL_CURRENT)) {
                        warn!("Resetting the dimming on {name}: {e}");
                    }
                }
                // None = "unknown", so when the panel comes back the drawing thread sends its dimmed current
                // again instead of assuming it is still set.
                state.dimming.current = None;
            }
            info!("The window holds {name}");
        }
        self.held()
    }

    /// Gives panels back to the monitor; all of them when `ports` is None. Returns the ones released.
    /// Port names are matched in lower case; names that weren't held are ignored. Nothing is written to
    /// the panel here: the drawing thread's next frame (and its current, see `dimming.rs`) takes it back.
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

    /// True while the window holds the named port (any case).
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
    /// `alive` says whether a process id is still running (on Windows the monitor asks the OS). Panels held
    /// without a process id are never released here; only `release` gives those back.
    pub fn release_dead_holders(&self, alive: &dyn Fn(u32) -> bool) {
        // The names are collected first and `held` is unlocked at the end of this statement, because
        // `release` locks it again (std's Mutex is not re-entrant: locking twice would deadlock).
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
    /// Used for the window's panel games and patterns. The port is opened if needed (so the window can
    /// use a panel the monitor hasn't found), all writes go out under one lock so a frame from the drawing
    /// thread can't land in the middle, and they are sent even if the panel is held (they are the
    /// window's). `timeout` is capped at `MAX_READ`; None means `MAX_READ`. Fewer bytes than `read` come
    /// back if the panel doesn't answer in time. Any error is returned to the window as is.
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
        // Capped here as a `Duration`; the control channel caps the client's number before converting it.
        state.read(read, timeout.map_or(MAX_READ, |t| t.min(MAX_READ)))
    }

    /// The window's `serial_many` command: commands to several panels, taking turns between them, so
    /// while one panel works on its command the next already gets its own.
    /// `items` is (port name, list of commands) per panel. Step 0 writes each panel's first command,
    /// step 1 each panel's second, and so on; panels with fewer commands just stop early. Every port
    /// stays locked until all steps are done. The first failed write stops everything and returns its
    /// error (the commands already written stay written).
    pub fn serial_many(&self, items: &[(String, Vec<Vec<u8>>)]) -> io::Result<()> {
        let ports = items.iter().map(|(name, _)| self.open(name)).collect::<io::Result<Vec<_>>>()?;
        // Lock in a fixed order, so two of these at once can't each wait for the other's port
        let mut order: Vec<usize> = (0..ports.len()).collect();
        order.sort_by_key(|&i| ports[i].name().to_lowercase());
        // The same port named twice (even in other case) is the same `SharedPort`: locking it twice
        // would deadlock this thread on itself, so only the first of them is kept and locked.
        order.dedup_by_key(|i| ports[*i].name().to_lowercase());
        let mut guards: HashMap<usize, MutexGuard<'_, PortState>> =
            order.iter().map(|&i| (i, ports[i].lock())).collect();
        // For item i, the index whose guard holds its port (the kept duplicate, if it was dropped above).
        let index_of =
            |i: usize| order.iter().copied().find(|&j| ports[j].name().eq_ignore_ascii_case(ports[i].name()));
        let steps = items.iter().map(|(_, c)| c.len()).max().unwrap_or(0);
        for step in 0..steps {
            for (i, (_, chunks)) in items.iter().enumerate() {
                if let Some(chunk) = chunks.get(step).filter(|c| !c.is_empty()) {
                    // Both unwraps hold: every item's port has a kept entry in `order` and so a guard.
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
    /// Shared with the test so it can check exactly what was written to which port, in order.
    pub type Log = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    /// The state of the made-up system; tests change it while threads use the ports.
    #[derive(Default)]
    pub struct Fake {
        /// What `list` returns (tests add, remove and move panels here).
        pub ports: Mutex<Vec<PortInfo>>,
        pub written: Log,
        /// Ports whose firmware answers queries (with 32 bytes).
        pub answers: Mutex<Vec<String>>,
        /// Ports whose writes fail with an error (as if unplugged).
        pub fail_writes: Mutex<Vec<String>>,
    }

    /// One opened made-up port: logs writes and answers the GlobalCurrent query if its firmware "can".
    struct FakePort {
        name: String,
        log: Log,
        answers: bool,
        /// Bytes of answer waiting to be read (32 after a GlobalCurrent query, if `answers`).
        pending: usize,
        /// Weak so a port left open by a test doesn't keep the fake alive; used to check `fail_writes`.
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
        /// Returns up to `size` bytes of the pending answer (all 7s) without waiting.
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

    /// The `PortSystem` handed to `Hub::new` in tests.
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

    /// A port that looks like an LED panel (the right USB IDs) at the given USB location.
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

    /// A hub on an empty fake system, with the fake returned so the test can inspect and change it.
    fn hub() -> (Arc<Fake>, Hub) {
        let fake = Arc::new(Fake::default());
        let hub = Hub::new(Arc::new(FakeSystem(fake.clone())));
        (fake, hub)
    }

    /// Proves that two opens of the same name (any case) share one port, and that it closes once both
    /// users let go.
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

    /// Proves that a held panel drops the monitor's writes but passes the window's, and that a hold is
    /// only given back when the process holding it has gone.
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

    /// Proves that the window's `serial` command returns the panel's 32-byte answer.
    #[test]
    fn serial_reads_the_reply() {
        let (fake, hub) = hub();
        fake.answers.lock().unwrap().push("COM4".into());
        let reply = hub.serial("COM4", true, &[protocol::global_current_query()], 32, None).unwrap();
        assert_eq!(reply.len(), 32);
    }

    /// Proves that `serial_many` interleaves the panels' commands (a1, b1, a2) instead of sending one
    /// panel's list and then the other's.
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

    /// Proves that a failed write breaks the port for good, and that the next `open` hands out a fresh
    /// port that works.
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
