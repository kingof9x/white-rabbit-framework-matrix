//! A pretend keyboard + macropad that answer like the f16custom firmware.
//! Used by `framework-16-keyboard-control.exe --demo` (try the app without the hardware) and
//! by the engine tests.

use crate::device::{Port, Result};
use crate::keycodes::Board;
use crate::keys::{KEYBOARD_LEDS, MACROPAD_LEDS};
use crate::protocol::{self as proto, Report, ValueId};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Effect count / host effect id of the real firmware.
const EFFECT_COUNT: u8 = 47;
const HOST_EFFECT: u8 = 45;
const PAINT_ONLY_EFFECT: u8 = 46;

struct SimState {
    macropad: bool,
    leds: Vec<(u8, u8)>,
    enabled: bool,
    mode: u8,
    hsv: [u8; 3],
    speed: u8,
    lock: bool,
    layer: u8,
    host: bool,
    last_frame: Instant,
    frame: Vec<[u8; 3]>,
    keys: VecDeque<u8>,
    next_key: Instant,
    seed: u32,
    out: VecDeque<Report>,
    span: (bool, u8, u8),
    paint: Vec<[u8; 3]>,
    saved_paint: Vec<[u8; 3]>,
    paint_mode: u8,
    led_cal: [u8; 4],
    /// VIA keymap buffer: layers of rows x columns, 2 bytes each (big endian).
    keymap: Vec<u8>,
    combos: [u8; 8],
    shortcuts: bool,
    /// Keyboard firmware that can't read or set Fn lock (from before it could).
    no_fn_lock: bool,
}

pub struct SimPort(RefCell<SimState>);

impl SimPort {
    pub fn keyboard() -> Self {
        Self::new(false, KEYBOARD_LEDS.to_vec(), 27, [170, 255, 255])
    }
    /// A keyboard on firmware from before the app could read or set Fn lock.
    pub fn keyboard_without_fn_lock() -> Self {
        let s = Self::keyboard();
        s.0.borrow_mut().no_fn_lock = true;
        s
    }
    pub fn macropad() -> Self {
        Self::new(true, MACROPAD_LEDS.to_vec(), 12, [128, 255, 255])
    }
    fn new(macropad: bool, leds: Vec<(u8, u8)>, mode: u8, hsv: [u8; 3]) -> Self {
        let n = leds.len();
        Self(RefCell::new(SimState {
            macropad,
            leds,
            enabled: true,
            mode,
            hsv,
            speed: 127,
            lock: false,
            layer: 0,
            host: false,
            last_frame: Instant::now(),
            frame: vec![[0; 3]; n],
            keys: VecDeque::new(),
            next_key: Instant::now(),
            seed: if macropad { 7 } else { 3 },
            out: VecDeque::new(),
            span: (false, 0, 224),
            paint: vec![[0; 3]; n],
            saved_paint: vec![[0; 3]; n],
            paint_mode: 1,
            led_cal: [104, 255, 255, 255],
            keymap: default_buffer(if macropad { Board::Macropad } else { Board::Keyboard }),
            combos: [1, 1, 0, 0, 0, 0, 0, 0],
            shortcuts: true,
            no_fn_lock: false,
        }))
    }
}

/// VIA's keymap buffer as the firmware starts out: the f16custom default keymap.
fn default_buffer(board: Board) -> Vec<u8> {
    let (_, cols) = board.matrix_size();
    let per = board.layer_bytes();
    let mut buf = vec![0u8; per * board.layers() as usize];
    for l in 0..board.layers() {
        for (k, &(r, c)) in board.matrix().iter().enumerate() {
            let at = l as usize * per + (r as usize * cols as usize + c as usize) * 2;
            buf[at..at + 2].copy_from_slice(&board.default_code(l, k).to_be_bytes());
        }
    }
    buf
}

impl SimState {
    /// VIA's own keymap commands (not on the custom channel).
    fn handle_via(&mut self, req: &Report) -> Option<Report> {
        let mut r = *req;
        let cols = if self.macropad { 8 } else { 16 };
        match req[0] {
            proto::VIA_GET_BUFFER => {
                let off = u16::from_be_bytes([req[1], req[2]]) as usize;
                let n = (req[3] as usize).min(proto::KEYMAP_BYTES_PER_MSG);
                for i in 0..n {
                    r[4 + i] = self.keymap.get(off + i).copied().unwrap_or(0);
                }
            }
            proto::VIA_SET_KEYCODE => {
                let at = (req[1] as usize * self.keymap.len() / if self.macropad { 2 } else { 4 })
                    + (req[2] as usize * cols + req[3] as usize) * 2;
                if at + 1 < self.keymap.len() {
                    self.keymap[at] = req[4];
                    self.keymap[at + 1] = req[5];
                }
            }
            _ => return None,
        }
        Some(r)
    }

    fn rand(&mut self) -> u32 {
        // xorshift: good enough for pretend key presses
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed
    }

    /// While the PC is drawing, pretend someone is typing now and then.
    fn fake_typing(&mut self) {
        if self.host && Instant::now() >= self.next_key {
            let led = (self.rand() % self.leds.len() as u32) as u8;
            self.keys.push_back(led);
            let wait = 250 + self.rand() % 900;
            self.next_key = Instant::now() + Duration::from_millis(wait as u64);
        }
    }

    fn handle(&mut self, req: &Report) -> Report {
        if let Some(r) = self.handle_via(req) {
            return r;
        }
        let mut r = *req;
        let d = &mut r[3..];
        let get = req[0] == proto::CMD_GET;
        if req[1] != proto::CHANNEL || !(get || req[0] == proto::CMD_SET) {
            r[0] = proto::UNHANDLED;
            return r;
        }
        const INFO: u8 = ValueId::Info as u8;
        const STATE: u8 = ValueId::State as u8;
        const LOCK: u8 = ValueId::HotkeyLock as u8;
        const LED_INFO: u8 = ValueId::LedInfo as u8;
        const FRAME: u8 = ValueId::Frame as u8;
        const HOST: u8 = ValueId::HostMode as u8;
        const KEYS: u8 = ValueId::KeyEvents as u8;
        const LAYER: u8 = ValueId::Layer as u8;
        const SPAN: u8 = ValueId::Span as u8;
        const CLOCK: u8 = ValueId::Clock as u8;
        const PAINT: u8 = ValueId::Paint as u8;
        const PAINT_CTRL: u8 = ValueId::PaintCtrl as u8;
        const COMBOS: u8 = ValueId::Combos as u8;
        const LED_CAL: u8 = ValueId::LedCal as u8;
        const SHORTCUTS: u8 = ValueId::Shortcuts as u8;
        if self.host && self.last_frame.elapsed() > Duration::from_secs(3) {
            self.host = false; // same fallback as the firmware
        }
        let ok = match (get, req[2]) {
            (true, INFO) => {
                d[..11].copy_from_slice(&[
                    b'F',
                    b'X',
                    proto::PROTOCOL_VERSION,
                    self.leds.len() as u8,
                    EFFECT_COUNT,
                    HOST_EFFECT,
                    self.macropad as u8,
                    self.lock as u8,
                    if self.no_fn_lock { 0xFF } else { self.layer },
                    255,
                    PAINT_ONLY_EFFECT,
                ]);
                // The pretend keyboard has build 12, the pretend macropad firmware from before
                // build numbers (it leaves these 0).
                if !self.macropad {
                    d[11..14].copy_from_slice(&[b'B', 12, 0]);
                }
                true
            }
            (true, STATE) => {
                let mode = if self.host { HOST_EFFECT } else { self.mode };
                d[..7].copy_from_slice(&[
                    self.enabled as u8,
                    mode,
                    self.hsv[0],
                    self.hsv[1],
                    self.hsv[2],
                    self.speed,
                    self.host as u8,
                ]);
                true
            }
            (false, STATE) => {
                self.enabled = d[0] != 0;
                self.mode = d[1];
                self.hsv = [d[2], d[3], d[4]];
                self.speed = d[5];
                self.host = false;
                true
            }
            (true, LOCK) => {
                d[0] = self.lock as u8;
                true
            }
            (false, LOCK) => {
                self.lock = d[0] != 0;
                true
            }
            (true, LED_INFO) => {
                let start = d[0] as usize;
                let n = proto::INFO_LEDS_PER_MSG.min(self.leds.len().saturating_sub(start));
                d[1] = n as u8;
                for i in 0..n {
                    let (x, y) = self.leds[start + i];
                    d[2 + i * 3..5 + i * 3].copy_from_slice(&[x, y, 4]);
                }
                true
            }
            (false, FRAME) => {
                let (start, n) = (d[0] as usize, d[1] as usize);
                for i in 0..n.min(proto::FRAME_LEDS_PER_MSG) {
                    if let Some(px) = self.frame.get_mut(start + i) {
                        px.copy_from_slice(&d[2 + i * 3..5 + i * 3]);
                    }
                }
                self.last_frame = Instant::now();
                true
            }
            (true, HOST) => {
                d[0] = self.host as u8;
                true
            }
            (false, HOST) => {
                self.host = d[0] != 0;
                self.last_frame = Instant::now();
                true
            }
            (true, KEYS) => {
                self.fake_typing();
                let n = self.keys.len().min(proto::KEYS_PER_MSG);
                d[0] = n as u8;
                for i in 0..n {
                    d[1 + i] = self.keys.pop_front().unwrap();
                }
                true
            }
            (true, LAYER) => {
                d[0] = if self.no_fn_lock { 0xFF } else { self.layer };
                true
            }
            (false, LAYER) if d[0] <= 1 && !self.no_fn_lock => {
                self.layer = d[0];
                true
            }
            (true, SPAN) => {
                d[..3].copy_from_slice(&[self.span.0 as u8, self.span.1, self.span.2]);
                true
            }
            (false, SPAN) => {
                self.span = (d[0] != 0, d[1], d[2]);
                true
            }
            (false, CLOCK) => true,
            (true, PAINT) => {
                let start = d[0] as usize;
                let n = proto::PAINT_PER_GET_MSG.min(self.paint.len().saturating_sub(start));
                d[1] = n as u8;
                for i in 0..n {
                    d[2 + i * 3..5 + i * 3].copy_from_slice(&self.paint[start + i]);
                }
                true
            }
            (false, PAINT) => {
                let (start, n) = (d[0] as usize, (d[1] as usize).min(proto::PAINT_PER_SET_MSG));
                for i in 0..n {
                    if let Some(px) = self.paint.get_mut(start + i) {
                        *px = [d[2 + i * 3], d[3 + i * 3], (d[4 + i * 3] != 0) as u8];
                    }
                }
                true
            }
            (true, PAINT_CTRL) => {
                d[0] = self.paint_mode;
                d[1] = self.paint.iter().filter(|p| p[2] != 0).count() as u8;
                true
            }
            (false, PAINT_CTRL) => match d[0] {
                0 => {
                    self.paint.iter_mut().for_each(|p| p[2] = 0);
                    true
                }
                1 => {
                    self.saved_paint = self.paint.clone();
                    true
                }
                2 | 3 | 5 => {
                    self.paint_mode = match d[0] {
                        2 => 0,
                        3 => 1,
                        _ => 2,
                    };
                    true
                }
                4 => {
                    self.paint = self.saved_paint.clone();
                    true
                }
                _ => false,
            },
            (true, SHORTCUTS) if !self.macropad => {
                d[0] = self.shortcuts as u8;
                true
            }
            (false, SHORTCUTS) if !self.macropad => {
                self.shortcuts = d[0] != 0;
                true
            }
            (true, LED_CAL) => {
                d[..4].copy_from_slice(&self.led_cal);
                d[4] = 104;
                true
            }
            (false, LED_CAL) => {
                self.led_cal.copy_from_slice(&d[..4]);
                true
            }
            (true, COMBOS) if self.macropad => {
                d[..8].copy_from_slice(&self.combos);
                true
            }
            (false, COMBOS) if self.macropad => {
                self.combos.copy_from_slice(&d[..8]);
                true
            }
            _ => false,
        };
        if !ok {
            r[0] = proto::UNHANDLED;
        }
        r
    }
}

impl Port for SimPort {
    fn write(&self, report: &Report) -> Result<()> {
        let mut s = self.0.borrow_mut();
        if report[0] == 0x0B {
            return Ok(()); // bootloader request: a real device would reset; ignore
        }
        let resp = s.handle(report);
        s.out.push_back(resp);
        Ok(())
    }

    fn read(&self, _timeout_ms: i32) -> Result<Option<Report>> {
        Ok(self.0.borrow_mut().out.pop_front())
    }
}
