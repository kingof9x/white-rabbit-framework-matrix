//! The LED panels as the window sees them (`white_rabbit/panels.py`, `bridge.py` and the window's Lender):
//! which is left and which is right, sending commands to them (through the monitor while it runs,
//! straight to the serial port otherwise) and borrowing them from the monitor.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use wr_panel::hub::{Port, PortSystem};
use wr_panel::ports::{discover, Pins, PortInfo};

use crate::commands as cmd;
use crate::link::{hex, unhex, Link, MAX_READ_SECONDS};
use crate::{paths, settings};

#[derive(Clone, Debug, PartialEq)]
pub struct Panel {
    /// "left" or "right" ("panel 3" and so on, just in case)
    pub side: String,
    /// The serial port, e.g. COM4
    pub device: String,
    pub location: Option<String>,
    /// The name the user gave this side, if any
    pub nickname: String,
}

impl Panel {
    pub fn label(&self) -> String {
        if self.nickname.is_empty() {
            let mut side = self.side.clone();
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
pub struct Sender {
    pub link: Arc<Link>,
    direct: Option<Arc<dyn PortSystem>>,
    /// Direct mode: one port open at a time per panel, as Windows allows
    open: Mutex<HashMap<String, Arc<Mutex<Box<dyn Port>>>>>,
}

impl Sender {
    pub fn new(link: Arc<Link>) -> Sender {
        Sender { link, direct: direct_ports(), open: Mutex::default() }
    }

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
    pub fn close_direct(&self) {
        self.open.lock().unwrap().clear();
    }

    /// Writes each command on its own, then reads `read` bytes of reply (0: none).
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
            let reply = self.link.call(message)?;
            return Ok(unhex(reply.get("data").and_then(Value::as_str).unwrap_or("")));
        }
        let shared = self.direct_port(port)?;
        let mut p = shared.lock().unwrap();
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
            let wait = timeout.unwrap_or(MAX_READ_SECONDS).min(MAX_READ_SECONDS);
            p.read(read, Duration::from_secs_f64(wait))
        })();
        drop(p);
        result.map_err(|e| {
            // A port that failed is opened again next time
            self.open.lock().unwrap().remove(port);
            format!("{port}: {e}")
        })
    }

    pub fn write(&self, port: &str, writes: &[Vec<u8>]) -> Result<(), String> {
        self.send(port, writes, 0, None, false).map(|_| ())
    }

    /// One command, then its 32-byte answer.
    pub fn ask(&self, port: &str, command: Vec<u8>, timeout: Option<f64>) -> Result<Vec<u8>, String> {
        self.send(port, &[command], cmd::RESPONSE_SIZE, timeout, true)
    }

    /// Commands to several panels at once, taking turns between them, so two panels take about as long
    /// as one.
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
#[derive(Default)]
pub struct Lender {
    inner: Mutex<LenderState>,
}

#[derive(Default)]
struct LenderState {
    /// port -> the firmware brightness before borrowing, if it could be read
    borrowed: HashMap<String, Option<u8>>,
    /// Bumped on each borrow, so a delayed automatic return doesn't undo something newer
    generation: HashMap<String, u64>,
}

pub type Generations = HashMap<String, u64>;

impl Lender {
    pub fn is_borrowed(&self, device: &str) -> bool {
        self.inner.lock().unwrap().borrowed.contains_key(device)
    }

    pub fn any_borrowed(&self) -> bool {
        !self.inner.lock().unwrap().borrowed.is_empty()
    }

    /// Stops the monitor drawing on these panels (call from a worker thread).
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
            let level =
                sender.ask(device, cmd::command(cmd::BRIGHTNESS, &[]), None).ok().and_then(|r| r.first().copied());
            self.inner.lock().unwrap().borrowed.insert(device.clone(), level);
        }
        if !new.is_empty() {
            if let Err(e) = sender.link.hold(&new) {
                wr_core::warn!("Couldn't borrow the panels from the monitor: {e}");
            }
        }
        let s = self.inner.lock().unwrap();
        panels.iter().map(|p| (p.device.clone(), s.generation[&p.device])).collect()
    }

    /// Stops whatever this window started on these panels and lets the monitor draw again (worker thread).
    /// With `only`, a panel borrowed again since is left alone.
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

#[cfg(test)]
mod tests {
    use super::*;

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
