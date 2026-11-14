//! One panel's drawing thread: a port of `DrawingThread` in `led_mon/drawing.py`.
//!
//! It takes frames from its queue and sends them to its panel. It finds the panel again when the
//! modules are moved to other slots (or plugged in later), skips a frame that is already on the panel
//! unless it was sent more than 5 seconds ago (the panel firmware sleeps after a minute without
//! commands), and two threads never use the same panel.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::ports::{find_panel_port, Pins, PortInfo};
use crate::protocol;
use wr_core::queue::{FrameQueue, Item};
use wr_core::{Grid, HEIGHT, WIDTH};

use wr_core::{info, warn};

/// An unchanged frame is sent again after this long, so the panel never falls asleep.
pub const RESEND_UNCHANGED: Duration = Duration::from_secs(5);

/// Listing and opening serial ports. Windows in the app, made-up ports in the tests.
pub trait PortSystem: Send + Sync {
    fn list(&self) -> Vec<PortInfo>;
    fn open(&self, name: &str) -> std::io::Result<Box<dyn Write + Send>>;
}

/// Which panel each drawing thread has: {port name in lower case: side}.
#[derive(Default)]
pub struct Claims(Mutex<HashMap<String, usize>>);

impl Claims {
    fn taken_by_other(&self, port: &str, side: usize) -> bool {
        self.0.lock().unwrap().get(&port.to_lowercase()).is_some_and(|s| *s != side)
    }

    fn claim(&self, port: &str, side: usize) -> bool {
        let mut map = self.0.lock().unwrap();
        match map.get(&port.to_lowercase()) {
            Some(s) if *s != side => false,
            _ => {
                map.insert(port.to_lowercase(), side);
                true
            }
        }
    }

    fn unclaim(&self, side: usize) {
        self.0.lock().unwrap().retain(|_, s| *s != side);
    }
}

pub struct Panel {
    side: usize,
    ports: Arc<dyn PortSystem>,
    claims: Arc<Claims>,
    pins: Arc<Mutex<Pins>>,
    /// The panel's USB location (or port name) last seen, looked at first when reconnecting.
    location: Option<String>,
    port_name: Option<String>,
    port: Option<Box<dyn Write + Send>>,
    animate: bool,
    shown: Option<([[u8; HEIGHT]; WIDTH], Instant)>,
    backoff: Duration,
    next_try: Instant,
    warned_missing: bool,
}

impl Panel {
    pub fn new(side: usize, ports: Arc<dyn PortSystem>, claims: Arc<Claims>, pins: Arc<Mutex<Pins>>) -> Panel {
        Panel {
            side,
            ports,
            claims,
            pins,
            location: None,
            port_name: None,
            port: None,
            animate: false,
            shown: None,
            backoff: Duration::from_millis(500),
            next_try: Instant::now(),
            warned_missing: false,
        }
    }

    #[cfg(test)]
    pub fn port_name(&self) -> Option<&str> {
        self.port_name.as_deref()
    }

    fn close(&mut self) {
        self.port = None;
        self.port_name = None;
        self.shown = None;
        self.claims.unclaim(self.side);
    }

    /// Finds and opens this side's panel; false (and try again later) when there is none.
    pub fn connect(&mut self, force: bool) -> bool {
        let now = Instant::now();
        if !force && now < self.next_try {
            return false;
        }
        let list = self.ports.list();
        let pins = self.pins.lock().unwrap().clone();
        let claims = self.claims.clone();
        let side = self.side;
        let taken = move |p: &str| claims.taken_by_other(p, side);
        let found = find_panel_port(&list, &pins, self.location.as_deref(), self.side, &taken);
        let result = match found {
            Some(name) if self.claims.claim(&name, self.side) => match self.ports.open(&name) {
                Ok(mut port) => match port.write_all(&protocol::animate(self.animate)) {
                    Ok(()) => Ok((name, port)),
                    Err(e) => Err(e.to_string()),
                },
                Err(e) => Err(e.to_string()),
            },
            _ => Err("no free LED panel found".to_string()),
        };
        match result {
            Ok((name, port)) => {
                self.location = list
                    .iter()
                    .find(|p| p.device.eq_ignore_ascii_case(&name))
                    .and_then(|p| p.location.clone())
                    .or_else(|| Some(name.clone()));
                info!(
                    "Connected the {} panel on {name} (USB location {})",
                    self.side_name(),
                    self.location.as_deref().unwrap_or("?")
                );
                self.port_name = Some(name);
                self.port = Some(port);
                self.backoff = Duration::from_millis(500);
                self.warned_missing = false;
                true
            }
            Err(e) => {
                self.close();
                self.next_try = now + self.backoff;
                self.backoff = (self.backoff * 2).min(Duration::from_secs(8));
                if !self.warned_missing {
                    warn!("No {} panel yet: {e}", self.side_name());
                    self.warned_missing = true;
                }
                false
            }
        }
    }

    fn side_name(&self) -> &'static str {
        if self.side == 0 {
            "left"
        } else {
            "right"
        }
    }

    fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.shown = None;
        match self.port.as_mut() {
            Some(p) => p.write_all(bytes),
            None => Ok(()),
        }
    }

    fn draw(&mut self, grid: &Grid) -> std::io::Result<()> {
        let key = grid.to_bytes();
        if let Some((shown, at)) = &self.shown {
            if *shown == key && at.elapsed() < RESEND_UNCHANGED {
                return Ok(());
            }
        }
        let mut bytes = Vec::with_capacity(WIDTH * (HEIGHT + 4) + 3);
        for command in protocol::frame_commands(grid) {
            bytes.extend_from_slice(&command);
        }
        self.send(&bytes)?;
        self.shown = Some((key, Instant::now()));
        Ok(())
    }

    /// Handles one item from the queue. False when the thread should stop.
    pub fn handle(&mut self, item: Item) -> bool {
        let result = match item {
            Item::Stop => return false,
            Item::Sleep => self.send(&protocol::sleep()),
            Item::Frame(grid, animate) => {
                if let Some(a) = animate {
                    self.animate = a;
                }
                if self.port.is_none() && !self.connect(false) {
                    return true;
                }
                let mut r = Ok(());
                if !self.animate {
                    r = self.draw(&grid);
                }
                if let (Ok(()), Some(a)) = (&r, animate) {
                    r = self.send(&protocol::animate(a));
                }
                r
            }
        };
        if let Err(e) = result {
            warn!("Lost the {} panel ({e}); looking for it again", self.side_name());
            self.close();
            self.connect(true);
        }
        true
    }

    pub fn run(mut self, queue: Arc<FrameQueue>) {
        self.connect(true);
        loop {
            let Some(item) = queue.get(Duration::from_secs(1)) else { continue };
            if !self.handle(item) {
                break;
            }
        }
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{LED_MATRIX_PID, LED_MATRIX_VID};

    /// (port name, bytes) of every write.
    type Log = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    #[derive(Default)]
    struct Fake {
        ports: Mutex<Vec<PortInfo>>,
        written: Log,
    }

    struct Writer(String, Log);
    impl Write for Writer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.1.lock().unwrap().push((self.0.clone(), buf.to_vec()));
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl PortSystem for Fake {
        fn list(&self) -> Vec<PortInfo> {
            self.ports.lock().unwrap().clone()
        }
        fn open(&self, name: &str) -> std::io::Result<Box<dyn Write + Send>> {
            Ok(Box::new(Writer(name.to_string(), self.written.clone())))
        }
    }

    fn panel_port(name: &str, location: &str) -> PortInfo {
        PortInfo {
            device: name.into(),
            vid: Some(LED_MATRIX_VID),
            pid: Some(LED_MATRIX_PID),
            location: Some(location.into()),
            description: String::new(),
        }
    }

    fn setup() -> (Arc<Fake>, Panel, Panel) {
        let fake = Arc::new(Fake::default());
        *fake.ports.lock().unwrap() = vec![panel_port("COM5", "1-3.3:x.0"), panel_port("COM4", "1-3.2:x.0")];
        let claims = Arc::new(Claims::default());
        let pins = Arc::new(Mutex::new(Pins::default()));
        let left = Panel::new(0, fake.clone(), claims.clone(), pins.clone());
        let right = Panel::new(1, fake.clone(), claims, pins);
        (fake, left, right)
    }

    #[test]
    fn left_and_right_by_usb_position() {
        let (_, mut left, mut right) = setup();
        assert!(left.connect(true) && right.connect(true));
        assert_eq!((left.port_name(), right.port_name()), (Some("COM4"), Some("COM5")));
    }

    #[test]
    fn unchanged_frames_are_skipped_until_the_keepalive() {
        let (fake, mut left, _) = setup();
        let mut g = Grid::new();
        g.set(2, 2, 100);
        left.handle(Item::Frame(g.clone(), Some(false)));
        let writes: Vec<_> = fake.written.lock().unwrap().iter().map(|(_, b)| b.clone()).collect();
        // animate off when connecting, the frame (one column + flush), animate off
        assert_eq!(writes.len(), 3);
        assert_eq!(
            writes[1],
            [
                protocol::command(protocol::STAGE_COL, &{
                    let mut p = vec![2u8];
                    p.extend((0..HEIGHT).map(|y| if y == 2 { 100 } else { 0 }));
                    p
                }),
                protocol::command(protocol::FLUSH_COLS, &[])
            ]
            .concat()
        );
        // Any other command forgets what the panel shows (as in Python), so the next frame goes out again
        left.handle(Item::Frame(g.clone(), None));
        assert_eq!(fake.written.lock().unwrap().len(), 4);
        left.handle(Item::Frame(g.clone(), None));
        assert_eq!(fake.written.lock().unwrap().len(), 4);
        left.shown.as_mut().unwrap().1 -= RESEND_UNCHANGED;
        left.handle(Item::Frame(g, None));
        assert_eq!(fake.written.lock().unwrap().len(), 5);
    }

    #[test]
    fn a_moved_panel_is_found_again() {
        let (fake, mut left, mut right) = setup();
        left.connect(true);
        right.connect(true);
        // Both modules moved: new COM ports at new positions
        *fake.ports.lock().unwrap() = vec![panel_port("COM8", "1-4.2:x.0"), panel_port("COM9", "1-4.3:x.0")];
        left.close();
        right.close();
        assert!(right.connect(true) && left.connect(true));
        assert_eq!((left.port_name(), right.port_name()), (Some("COM8"), Some("COM9")));
    }
}
