//! The LED panels as the window sees them (`white_rabbit/panels.py`, `bridge.py` and the window's Lender):
//! which is left and which is right, sending commands to them (through the monitor while it runs,
//! straight to the serial port otherwise) and borrowing them from the monitor.
//!
//! # Two ways to reach a panel
//!
//! Windows lets only one program at a time open a serial port. While the monitor (WhiteRabbit.exe) runs,
//! it owns both panels' ports, so `Sender` wraps each command as hex text in a JSON "serial" request
//! and the monitor writes it to the port for us (`link.rs`). When no monitor is running, `Sender`
//! opens the port itself (`wr_panel::winports`) and keeps it open between commands. Before going
//! through the monitor it closes any port it opened itself, so a monitor that has just started can
//! open them.
//!
//! # Borrowing
//!
//! Sending a command through the monitor isn't enough to show something: the monitor redraws its own
//! gauges a few times a second and would paint over it. So before the window draws, `Lender::borrow`
//! sends "hold" (the monitor stops drawing on those panels) and remembers each panel's brightness; when
//! the window is done, `Lender::give_back` stops the window's jobs on those panels, undoes anything the
//! firmware would keep doing (a built-in game, the animate flag, a changed brightness) and sends
//! "release". A generation number per panel stops a delayed automatic give-back from undoing a newer
//! borrow.
//!
//! Everything here blocks on serial or pipe I/O, so it is called from worker threads (`Ctx::act`,
//! `Ctx::in_background`, jobs), never from the UI thread. All state is behind mutexes for that reason.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use wr_panel::hub::{Port, PortSystem};
use wr_panel::ports::{discover, Pins, PortInfo};

use crate::commands as cmd;
use crate::link::{hex, unhex, Link, MAX_READ_SECONDS};
use crate::{paths, settings};

/// One connected LED panel: which side it is on, its serial port and the name the user gave it.
/// Cheap to clone; pages copy the list instead of holding the lock.
#[derive(Clone, Debug, PartialEq)]
pub struct Panel {
    /// "left" or "right" ("panel 3" and so on, just in case)
    pub side: String,
    /// The serial port, e.g. COM4
    pub device: String,
    /// The USB location Windows reports for the port (which socket it is in), used to tell left from
    /// right; None when unknown.
    pub location: Option<String>,
    /// The name the user gave this side, if any
    pub nickname: String,
}

impl Panel {
    /// The longer name for lists and cards: "Left panel (COM4)", or "Desk (left, COM4)" once named.
    pub fn label(&self) -> String {
        if self.nickname.is_empty() {
            let mut side = self.side.clone();
            // Capitalise the first letter ("left" -> "Left"). get_mut(0..1) is None for an empty string, or if the
            // first character were more than one byte, so this can't panic.
            if let Some(first) = side.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            format!("{side} panel ({})", self.device)
        } else {
            format!("{} ({}, {})", self.nickname, self.side, self.device)
        }
    }

    /// How messages name the panel: "left panel", or "left panel (Desk)" once it has a name.
    pub fn called(&self) -> String {
        if self.nickname.is_empty() {
            format!("{} panel", self.side)
        } else {
            format!("{} panel ({})", self.side, self.nickname)
        }
    }
}

/// Which ports the layout file pins to each side.
///
/// The layout file (the user's, else the one bundled with the app) can pin a side to a port name or
/// a USB location. A layout that can't be read is logged and treated as "no pins": the sides are
/// then worked out automatically.
pub fn layout_pins() -> Pins {
    let text = paths::layout_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_else(|| paths::BUNDLED_LAYOUT.to_string());
    match wr_core::layout::Layout::parse(&text) {
        Ok(layout) => Pins {
            left_port: layout.left_port,
            right_port: layout.right_port,
            left_location: layout.left_location,
            right_location: layout.right_location,
        },
        Err(e) => {
            wr_core::warn!("Could not read the layout for the panel order: {}", e.0);
            Pins::default()
        }
    }
}

/// The serial ports on this computer. Away from Windows, WHITE_RABBIT_FAKE_PANELS=COM4,COM5 makes up
/// panels (for screenshots and tests).
pub fn list_ports() -> Vec<PortInfo> {
    if let Ok(fake) = std::env::var("WHITE_RABBIT_FAKE_PANELS") {
        // Each made-up panel gets the real panel's USB ids and a made-up location, so `discover` accepts it
        // and orders them as it would real ones.
        return fake
            .split(',')
            .filter(|s| !s.is_empty())
            .enumerate()
            .map(|(i, name)| PortInfo {
                device: name.to_string(),
                vid: Some(wr_panel::protocol::LED_MATRIX_VID),
                pid: Some(wr_panel::protocol::LED_MATRIX_PID),
                location: Some(format!("1-{}.2:x.0", 3 + i)),
                description: "LED Matrix Input Module".into(),
            })
            .collect();
    }
    #[cfg(windows)]
    {
        wr_panel::winports::list_ports()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// The connected panels in left, right order.
///
/// Reads the port list once (called when the window opens). The first panel found is "left", the
/// second "right", any further ones "panel 3" and so on; each gets its saved nickname for that side.
pub fn find_panels() -> Vec<Panel> {
    let ports = list_ports();
    let found = discover(&ports, &layout_pins());
    let names = settings::panel_names();
    found
        .into_iter()
        .enumerate()
        .map(|(i, (location, device))| {
            let side = match i {
                0 => "left".to_string(),
                1 => "right".to_string(),
                n => format!("panel {}", n + 1),
            };
            let nickname = names.get(&side).and_then(Value::as_str).unwrap_or("").to_string();
            Panel { side, device, location, nickname }
        })
        .collect()
}

/// Opening ports straight away, when the monitor isn't running.
///
/// None away from Windows: there is no serial support there, so commands fail with a clear error.
fn direct_ports() -> Option<Arc<dyn PortSystem>> {
    #[cfg(windows)]
    {
        Some(Arc::new(wr_panel::winports::WindowsPorts))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Sends commands to panels, through the monitor while it runs.
///
/// One `Sender` is shared (in an `Arc`) by the whole window and all its worker threads.
pub struct Sender {
    /// The monitor connection, asked on each send whether the monitor is running (cached 2 s).
    pub link: Arc<Link>,
    /// How ports are opened when there's no monitor; None where that isn't supported.
    direct: Option<Arc<dyn PortSystem>>,
    /// Direct mode: one port open at a time per panel, as Windows allows
    ///
    /// Port name -> the open port. Each port has its own lock, so two threads writing to the same panel
    /// take turns while writes to different panels go on side by side; the outer lock is only held while
    /// looking a port up.
    open: Mutex<HashMap<String, Arc<Mutex<Box<dyn Port>>>>>,
}

impl Sender {
    /// A sender that talks through this monitor link, or straight to the ports when it's not running.
    pub fn new(link: Arc<Link>) -> Sender {
        Sender { link, direct: direct_ports(), open: Mutex::default() }
    }

    /// The open port for `name`, opening it the first time. Err when ports can't be opened here or
    /// Windows refuses (another program has it open, or the panel was unplugged).
    fn direct_port(&self, name: &str) -> Result<Arc<Mutex<Box<dyn Port>>>, String> {
        let system = self.direct.as_ref().ok_or("The panels can only be reached on Windows")?;
        let mut open = self.open.lock().unwrap();
        if let Some(p) = open.get(name) {
            return Ok(p.clone());
        }
        let port = system.open(name).map_err(|e| format!("Couldn't open {name}: {e}"))?;
        let port = Arc::new(Mutex::new(port));
        open.insert(name.to_string(), port.clone());
        Ok(port)
    }

    /// Closes the ports opened directly, so the monitor can open them when it starts.
    ///
    /// Dropping the boxed ports closes them (once no worker still holds a clone of the `Arc`).
    pub fn close_direct(&self) {
        self.open.lock().unwrap().clear();
    }

    /// Writes each command on its own, then reads `read` bytes of reply (0: none).
    ///
    /// `writes`: the raw command bytes, written in order (empty ones skipped in direct mode). `read`:
    /// how many reply bytes to wait for. `timeout`: seconds to wait for them (None: `MAX_READ_SECONDS`).
    /// `reset_input`: throw away bytes already waiting first, so an old late reply isn't taken for this
    /// one. Returns the bytes read (empty when `read` is 0); Err(text) for the user on failure.
    pub fn send(
        &self,
        port: &str,
        writes: &[Vec<u8>],
        read: usize,
        timeout: Option<f64>,
        reset_input: bool,
    ) -> Result<Vec<u8>, String> {
        if self.link.running(false) {
            self.close_direct();
            let message = json!({
                "cmd": "serial", "port": port,
                "writes": writes.iter().map(|w| hex(w)).collect::<Vec<_>>(),
                "read": read, "timeout": timeout, "reset_input": reset_input,
            });
            // The monitor sends back what it read as hex in "data"; a reply without it is taken as no bytes.
            let reply = self.link.call(message)?;
            return Ok(unhex(reply.get("data").and_then(Value::as_str).unwrap_or("")));
        }
        let shared = self.direct_port(port)?;
        let mut p = shared.lock().unwrap();
        // A closure called at once, so `?` can be used on each step and every I/O error lands in `result`.
        let result = (|| -> std::io::Result<Vec<u8>> {
            if reset_input {
                p.clear_input()?;
            }
            for w in writes.iter().filter(|w| !w.is_empty()) {
                p.write_all(w)?;
            }
            if read == 0 {
                return Ok(Vec::new());
            }
            // Never negative or NaN: Duration::from_secs_f64 panics on those
            let wait = timeout.filter(|t| *t >= 0.0).unwrap_or(MAX_READ_SECONDS).min(MAX_READ_SECONDS);
            p.read(read, Duration::from_secs_f64(wait))
        })();
        // Let go of the port's lock before taking the `open` map's lock below, so the two are never held in
        // the opposite order to `direct_port` (which takes the map lock first).
        drop(p);
        result.map_err(|e| {
            // A port that failed is opened again next time
            self.open.lock().unwrap().remove(port);
            format!("{port}: {e}")
        })
    }

    /// Sends commands that have no reply.
    pub fn write(&self, port: &str, writes: &[Vec<u8>]) -> Result<(), String> {
        self.send(port, writes, 0, None, false).map(|_| ())
    }

    ///
    /// Clears old input first. Used for version, brightness and startup questions.
    /// One command, then its 32-byte answer.
    pub fn ask(&self, port: &str, command: Vec<u8>, timeout: Option<f64>) -> Result<Vec<u8>, String> {
        self.send(port, &[command], cmd::RESPONSE_SIZE, timeout, true)
    }

    /// Commands to several panels at once, taking turns between them, so two panels take about as long
    /// as one.
    ///
    /// `by_port`: for each port, the chunks to write. Through the monitor all of it goes in one
    /// "serial_many" request and the monitor interleaves them; directly, chunk 0 to every panel, then
    /// chunk 1 to every panel, and so on. Stops at the first failure.
    pub fn write_many(&self, by_port: &[(String, Vec<Vec<u8>>)]) -> Result<(), String> {
        let by_port: Vec<_> = by_port.iter().filter(|(_, c)| !c.is_empty()).cloned().collect();
        if by_port.is_empty() {
            return Ok(());
        }
        if self.link.running(false) {
            self.close_direct();
            let ports: Vec<Value> = by_port
                .iter()
                .map(
                    |(port, chunks)| json!({"port": port, "writes": chunks.iter().map(|c| hex(c)).collect::<Vec<_>>()}),
                )
                .collect();
            self.link.call(json!({"cmd": "serial_many", "ports": ports}))?;
            return Ok(());
        }
        let steps = by_port.iter().map(|(_, c)| c.len()).max().unwrap_or(0);
        for step in 0..steps {
            for (port, chunks) in &by_port {
                if let Some(chunk) = chunks.get(step) {
                    self.write(port, std::slice::from_ref(chunk))?;
                }
            }
        }
        Ok(())
    }
}

/// Keeps track of the panels borrowed from the monitor, and what to put back when returning them.
///
/// Shared by the window and its workers in an `Arc`; every method takes the one inner lock briefly and
/// never holds it during serial or pipe I/O.
#[derive(Default)]
pub struct Lender {
    inner: Mutex<LenderState>,
}

/// What `Lender` keeps behind its lock.
#[derive(Default)]
struct LenderState {
    /// port -> the firmware brightness before borrowing, if it could be read
    borrowed: HashMap<String, Option<u8>>,
    /// Bumped on each borrow, so a delayed automatic return doesn't undo something newer
    generation: HashMap<String, u64>,
    /// The monitor's last frame on each panel just borrowed, until the app transition takes it
    last_frames: HashMap<String, crate::lead_in::Frame>,
}

/// Port -> generation number at the time of a borrow. `give_back(.., Some(&gens))` only returns a panel
/// whose number is unchanged, i.e. nobody borrowed it again in between.
pub type Generations = HashMap<String, u64>;

impl Lender {
    /// True while this window has this port borrowed from the monitor.
    pub fn is_borrowed(&self, device: &str) -> bool {
        self.inner.lock().unwrap().borrowed.contains_key(device)
    }

    /// True while this window has any panel borrowed.
    pub fn any_borrowed(&self) -> bool {
        !self.inner.lock().unwrap().borrowed.is_empty()
    }

    /// Stops the monitor drawing on these panels (call from a worker thread).
    ///
    /// Every panel's generation goes up by one, already-borrowed ones too (so their pending timed
    /// give-back no longer applies). For newly borrowed panels it reads the brightness first, to put it
    /// back later, then sends one "hold" for all of them. A refused hold is only logged: the window
    /// still draws, the monitor just may draw over it. Returns the new generation of every panel given.
    pub fn borrow(&self, sender: &Sender, panels: &[Panel]) -> Generations {
        let mut new = Vec::new();
        {
            let mut s = self.inner.lock().unwrap();
            for p in panels {
                *s.generation.entry(p.device.clone()).or_default() += 1;
                if !s.borrowed.contains_key(&p.device) {
                    new.push(p.device.clone());
                }
            }
        }
        for device in &new {
            // Asked through the monitor, since it still owns the port; the first reply byte is the brightness
            // (0..255). Failure leaves None and the brightness is simply not restored later.
            let level =
                sender.ask(device, cmd::command(cmd::BRIGHTNESS, &[]), None).ok().and_then(|r| r.first().copied());
            self.inner.lock().unwrap().borrowed.insert(device.clone(), level);
        }
        if !new.is_empty() {
            match sender.link.hold(&new) {
                Ok(frames) => self.inner.lock().unwrap().last_frames.extend(frames),
                Err(e) => wr_core::warn!("Couldn't borrow the panels from the monitor: {e}"),
            }
        }
        let s = self.inner.lock().unwrap();
        panels.iter().map(|p| (p.device.clone(), s.generation[&p.device])).collect()
    }

    /// The monitor's last frames on these panels (in their order), if they were just borrowed from it.
    ///
    /// Used once by the lead-in transition (`lead_in.rs`), which fades from that frame to the new picture.
    /// Frames for other panels are thrown away too, so a stale frame is never used for a later borrow.
    pub fn take_last_frames(&self, panels: &[Panel]) -> Vec<Option<crate::lead_in::Frame>> {
        let mut s = self.inner.lock().unwrap();
        let frames = panels.iter().map(|p| s.last_frames.remove(&p.device)).collect();
        s.last_frames.clear();
        frames
    }

    /// Stops whatever this window started on these panels and lets the monitor draw again (worker thread).
    /// With `only`, a panel borrowed again since is left alone.
    ///
    /// Panels that aren't borrowed are skipped. For the rest: their jobs are stopped (`jobs.rs`), any
    /// firmware game is quit, the animate flag is turned off and the saved brightness is restored, each
    /// write on its own so one failure doesn't stop the others; then one "release" goes to the monitor.
    /// Returns the panels actually given back.
    pub fn give_back(&self, sender: &Sender, panels: &[Panel], only: Option<&Generations>) -> Vec<Panel> {
        let mut returning = Vec::new();
        {
            let mut s = self.inner.lock().unwrap();
            for p in panels {
                if !s.borrowed.contains_key(&p.device) {
                    continue;
                }
                if let Some(only) = only {
                    if s.generation.get(&p.device) != only.get(&p.device) {
                        continue;
                    }
                }
                // Taken out of `borrowed` while still under the lock, so a second give_back racing this one finds
                // nothing to return and does nothing.
                let level = s.borrowed.remove(&p.device).flatten();
                returning.push((p.clone(), level));
            }
        }
        if returning.is_empty() {
            return Vec::new();
        }
        crate::jobs::stop_panel_jobs(&returning.iter().map(|(p, _)| p.device.clone()).collect::<Vec<_>>());
        for (p, level) in &returning {
            // A game in the firmware keeps drawing over the monitor, and the clock/countdown change brightness
            let mut writes = vec![cmd::game_control(cmd::GAME_QUIT), cmd::animate(false)];
            if let Some(level) = level {
                writes.push(cmd::brightness(*level));
            }
            for w in writes {
                if let Err(e) = sender.write(&p.device, &[w]) {
                    wr_core::info!("Resetting {}: {e}", p.device);
                }
            }
        }
        let ports: Vec<String> = returning.iter().map(|(p, _)| p.device.clone()).collect();
        if let Err(e) = sender.link.release(Some(&ports)) {
            wr_core::warn!("Could not give the panels back to the monitor: {e}");
        }
        returning.into_iter().map(|(p, _)| p).collect()
    }
}

/// Tests for how panels are named in the window.
#[cfg(test)]
mod tests {
    use super::*;

    /// `label` and `called` read right with and without a nickname.
    #[test]
    fn labels() {
        let mut p = Panel { side: "left".into(), device: "COM4".into(), location: None, nickname: String::new() };
        assert_eq!(p.label(), "Left panel (COM4)");
        assert_eq!(p.called(), "left panel");
        p.nickname = "Desk".into();
        assert_eq!(p.label(), "Desk (left, COM4)");
        assert_eq!(p.called(), "left panel (Desk)");
    }
}
