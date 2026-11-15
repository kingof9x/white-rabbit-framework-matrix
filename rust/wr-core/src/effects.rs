//! Effects drawn over whatever the panels show: a ripple when something is plugged in, the picture
//! draining out through the port when it is unplugged, sliding between apps, and a line of scrolling
//! text along the top of both panels. A port of `led_mon/effects.py` that draws exactly the same frames
//! (`tests/golden_effects.rs` checks every pixel against the frames Python saved).
//!
//! The drawing threads pass every frame through [`Effects::apply`] just before it goes to the panel.
//! While an effect is playing they also redraw the last frame on their own about 30 times a second
//! ([`Effects::interval`]), so the effect moves smoothly even though the monitor draws 10 frames a second.
//!
//! Python reads the clock itself; here every call that needs the time takes `now` (seconds, from a
//! monotonic clock), so the effects can be played back exactly in tests. There are no locks either: the
//! panel crate keeps one [`Effects`] behind a mutex, and also keeps which drawing thread owns a side.
//!
//! The arithmetic follows numpy step by step (float64, the same order of operations, round half to
//! even, truncating casts), because a pixel landing on the other side of a .5 would change the picture.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::nprand::Generator;
use crate::pyround::{int_round, round_half_even};
use crate::{Grid, HEIGHT, WIDTH};

/// How often a drawing thread redraws while an effect plays.
pub const FRAME_SECONDS: f64 = 1.0 / 30.0;
pub const FONT_W: usize = 3;
pub const FONT_H: usize = 5;
/// 5 rows of letters and a dark row under them.
pub const TEXT_ROWS: usize = 6;
/// Columns of keyboard between the two panels, for a ripple running from one panel to the other.
pub const GAP: usize = 4;

/// Which side of the laptop: a panel, or a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    /// In Python's `SIDES` order, which is also the order the panels sit in.
    pub const ALL: [Side; 2] = [Side::Left, Side::Right];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }

    /// "left" or "right", exactly (use [`parse_position`] for what users type).
    pub fn from_name(name: &str) -> Option<Side> {
        Side::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// Where along a side a port is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Spot {
    Back,
    Middle,
    Front,
}

impl Spot {
    pub const ALL: [Spot; 3] = [Spot::Back, Spot::Middle, Spot::Front];

    pub fn name(self) -> &'static str {
        match self {
            Spot::Back => "back",
            Spot::Middle => "middle",
            Spot::Front => "front",
        }
    }
}

// Settings -----------------------------------------------------------------------------------------------

/// The panel row each port is level with, counted from the top (the hinge end).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PortRows {
    pub back: usize,
    pub middle: usize,
    pub front: usize,
}

impl Default for PortRows {
    fn default() -> Self {
        PortRows { back: 5, middle: 16, front: 27 }
    }
}

impl PortRows {
    pub fn row(&self, spot: Spot) -> usize {
        match spot {
            Spot::Back => self.back,
            Spot::Middle => self.middle,
            Spot::Front => self.front,
        }
    }

    fn row_mut(&mut self, spot: Spot) -> &mut usize {
        match spot {
            Spot::Back => &mut self.back,
            Spot::Middle => &mut self.middle,
            Spot::Front => &mut self.front,
        }
    }
}

/// The `"effects"` key of settings.json, every key filled in and in range (Python's `normalize`).
/// Serializes to the same keys; deserializing goes through [`EffectsSettings::from_json`], so odd
/// values are fixed the way Python fixes them rather than failing.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectsSettings {
    /// Ripple when something is plugged in, drain when it is unplugged.
    pub plug_animation: bool,
    pub plug_charger: bool,
    pub plug_headphones: bool,
    pub plug_usb: bool,
    pub port_rows: PortRows,
    /// Ports the user corrected on the Effects tab: {USB location key or "charger": "left-back", ...}.
    pub learned_ports: BTreeMap<String, String>,
    /// The text line along the top of both panels, and what it tells.
    pub text_line: bool,
    pub text_song: bool,
    pub text_charger: bool,
    pub text_battery: bool,
    /// 1-10.
    pub text_speed: i64,
    /// Slide the next app in from the bottom when the panel changes app, instead of cutting to it.
    pub slide: bool,
    /// Dim the panels with the LED chip's current instead of darker values (needs newer firmware).
    pub full_shades: bool,
}

impl Default for EffectsSettings {
    fn default() -> Self {
        EffectsSettings {
            plug_animation: true,
            plug_charger: true,
            plug_headphones: true,
            plug_usb: true,
            port_rows: PortRows::default(),
            learned_ports: BTreeMap::new(),
            text_line: true,
            text_song: true,
            text_charger: true,
            text_battery: true,
            text_speed: 5,
            slide: true,
            full_shades: false,
        }
    }
}

impl EffectsSettings {
    /// Python's `normalize(raw)`: anything that isn't an object gives the defaults, unknown keys and
    /// nulls are ignored, flags are Python truthiness, numbers are `int()` and clamped.
    pub fn from_json(raw: &Value) -> EffectsSettings {
        let mut s = EffectsSettings::default();
        let Value::Object(map) = raw else { return s };
        let get = |key: &str| map.get(key).filter(|v| !v.is_null());
        for (key, flag) in [
            ("plug_animation", &mut s.plug_animation),
            ("plug_charger", &mut s.plug_charger),
            ("plug_headphones", &mut s.plug_headphones),
            ("plug_usb", &mut s.plug_usb),
            ("text_line", &mut s.text_line),
            ("text_song", &mut s.text_song),
            ("text_charger", &mut s.text_charger),
            ("text_battery", &mut s.text_battery),
            ("slide", &mut s.slide),
            ("full_shades", &mut s.full_shades),
        ] {
            if let Some(v) = get(key) {
                *flag = truthy(v);
            }
        }
        if let Some(n) = get("text_speed").and_then(py_int) {
            s.text_speed = n.clamp(1.0, 10.0) as i64;
        }
        if let Some(Value::Object(rows)) = get("port_rows") {
            for spot in Spot::ALL {
                if let Some(n) = rows.get(spot.name()).and_then(py_int) {
                    *s.port_rows.row_mut(spot) = n.clamp(0.0, (HEIGHT - 1) as f64) as usize;
                }
            }
        }
        if let Some(Value::Object(learned)) = get("learned_ports") {
            for (key, v) in learned {
                if let Value::String(text) = v {
                    if parse_position(text).0.is_some() {
                        s.learned_ports.insert(key.clone(), text.clone());
                    }
                }
            }
        }
        s
    }
}

impl<'de> Deserialize<'de> for EffectsSettings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(EffectsSettings::from_json(&Value::deserialize(deserializer)?))
    }
}

/// Python's `bool(value)` for JSON values.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python's `int(value)` for JSON values, as a float (only used to clamp it); None where Python raises
/// TypeError or ValueError. Strings take Python's integer syntax: spaces around, a sign, digits with
/// single underscores between them.
fn py_int(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(*b as u8 as f64),
        Value::Number(n) => n.as_i64().map(|i| i as f64).or_else(|| n.as_f64().map(f64::trunc)),
        Value::String(s) => {
            let t = s.trim();
            let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
            let ok = !digits.is_empty()
                && digits.split('_').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
            if !ok {
                return None;
            }
            let n: f64 = digits.replace('_', "").parse().ok()?;
            Some(if t.starts_with('-') { -n } else { n })
        }
        _ => None,
    }
}

/// "left-back" -> (Left, Back). Case, spaces and underscores don't matter; unknown parts are None, and
/// when a part is given twice the last one counts.
pub fn parse_position(text: &str) -> (Option<Side>, Option<Spot>) {
    let (mut side, mut spot) = (None, None);
    for part in text.to_lowercase().replace(['_', ' '], "-").split('-') {
        if let Some(s) = Side::from_name(part) {
            side = Some(s);
        } else if let Some(s) = Spot::ALL.into_iter().find(|s| s.name() == part) {
            spot = Some(s);
        }
    }
    (side, spot)
}

/// (Left, Back) -> "left-back"; None when the side is unknown.
pub fn position_text(side: Option<Side>, spot: Option<Spot>) -> Option<String> {
    let side = side?;
    Some(match spot {
        Some(spot) => format!("{}-{}", side.name(), spot.name()),
        None => side.name().to_string(),
    })
}

/// "left-back" -> "Left back", for the Effects tab.
pub fn position_label(position: Option<&str>) -> String {
    let (side, spot) = position.map(parse_position).unwrap_or((None, None));
    let Some(side) = side else { return "Unknown port".to_string() };
    let name = side.name();
    let capital = name[..1].to_uppercase() + &name[1..];
    match spot {
        Some(spot) => format!("{capital} {}", spot.name()),
        None => format!("{capital} side"),
    }
}

// Pictures -----------------------------------------------------------------------------------------------

/// A picture as numpy holds it here: (rows, columns) of floats, row by row. The effects work on these
/// rather than on [`Grid`] because the ripple's picture spans both panels.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f64>,
}

impl Picture {
    pub fn zeros(rows: usize, cols: usize) -> Picture {
        Picture { rows, cols, data: vec![0.0; rows * cols] }
    }

    /// A panel's grid as (rows, columns): Python's `np.asarray(grid, dtype=float).T`.
    pub fn from_grid(grid: &Grid) -> Picture {
        let mut p = Picture::zeros(HEIGHT, WIDTH);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                p.data[y * WIDTH + x] = grid.v[x][y] as f64;
            }
        }
        p
    }

    /// Back to a grid: `np.clip(np.rint(p), 0, 255).astype(int).T`. Only for panel-sized pictures.
    pub fn to_grid(&self) -> Grid {
        let mut g = Grid::new();
        for y in 0..self.rows.min(HEIGHT) {
            for x in 0..self.cols.min(WIDTH) {
                g.v[x][y] = round_half_even(self.at(y, x)).clamp(0.0, 255.0) as i32;
            }
        }
        g
    }

    pub fn at(&self, r: usize, c: usize) -> f64 {
        self.data[r * self.cols + c]
    }

    pub fn set(&mut self, r: usize, c: usize, value: f64) {
        self.data[r * self.cols + c] = value;
    }

    /// Left and right swapped: `p[:, ::-1]`.
    pub fn flipped(&self) -> Picture {
        let mut out = self.clone();
        for r in 0..self.rows {
            out.data[r * self.cols..(r + 1) * self.cols].reverse();
        }
        out
    }

    /// Columns a..b: `p[:, a:b]`.
    pub fn columns(&self, a: usize, b: usize) -> Picture {
        let mut out = Picture::zeros(self.rows, b - a);
        for r in 0..self.rows {
            out.data[r * (b - a)..(r + 1) * (b - a)].copy_from_slice(&self.data[r * self.cols + a..r * self.cols + b]);
        }
        out
    }

    /// `p[:, a:a + other.cols] = other`.
    fn put_columns(&mut self, a: usize, other: &Picture) {
        for r in 0..self.rows {
            self.data[r * self.cols + a..r * self.cols + a + other.cols]
                .copy_from_slice(&other.data[r * other.cols..(r + 1) * other.cols]);
        }
    }

    /// Rows src..src+n of `from` into rows dst..dst+n of this picture (same width).
    fn copy_rows(&mut self, dst: usize, from: &Picture, src: usize, n: usize) {
        let c = self.cols;
        self.data[dst * c..(dst + n) * c].copy_from_slice(&from.data[src * c..(src + n) * c]);
    }
}

/// The 5-point Laplacian of a (rows, columns) field, the edges repeated outwards (so waves reflect):
/// `p[:-2, 1:-1] + p[2:, 1:-1] + p[1:-1, :-2] + p[1:-1, 2:] - 4 * f` on the edge-padded field.
pub fn laplacian(f: &[f64], rows: usize, cols: usize) -> Vec<f64> {
    let mut out = vec![0.0; rows * cols];
    for r in 0..rows {
        let up = r.saturating_sub(1);
        let down = (r + 1).min(rows - 1);
        for c in 0..cols {
            let left = c.saturating_sub(1);
            let right = (c + 1).min(cols - 1);
            out[r * cols + c] = f[up * cols + c] + f[down * cols + c] + f[r * cols + left] + f[r * cols + right]
                - 4.0 * f[r * cols + c];
        }
    }
    out
}

/// numpy's pairwise summation (`np.sum` of a float64 array), which adds in a different order from a
/// plain loop and so can differ in the last bit.
fn pairwise_sum(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        a.iter().fold(0.0, |s, &x| s + x)
    } else if n <= 128 {
        let mut r = [0.0; 8];
        r.copy_from_slice(&a[..8]);
        let mut i = 8;
        while i < n - n % 8 {
            for j in 0..8 {
                r[j] += a[i + j];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        for &x in &a[i..] {
            res += x;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise_sum(&a[..n2]) + pairwise_sum(&a[n2..])
    }
}

// Ripple -------------------------------------------------------------------------------------------------

/// The panels as a pool something was dropped into at (jack_x, jack_y), in pixels.
///
/// A damped wave on a grid three times finer than the picture. Crests push the picture away from the
/// jack and troughs pull it back, so the picture wobbles out and in and ends exactly as it was.
#[derive(Clone, Debug)]
pub struct Ripple {
    pub width: usize,
    pub height: usize,
    /// Where the jack's drive goes in, on the fine grid.
    splash: Vec<f64>,
    /// Which way, and how far, a pixel moves for a unit of wave (pixel grid).
    ox: Vec<f64>,
    oy: Vec<f64>,
    /// The water's height and speed (fine grid).
    h: Vec<f64>,
    v: Vec<f64>,
    /// Time into the drive; None once the jack stopped pushing.
    t: Option<f64>,
    age: f64,
    carry: f64,
    active: bool,
    last: Option<f64>,
}

impl Ripple {
    const S: usize = 3;
    const STEP: f64 = 1.0 / 240.0;
    const SPEED: f64 = 120.0;
    const DAMP: f64 = 5.0;
    const SPRING: f64 = 6.25;
    const FREQ: f64 = 3.4;
    const DRIVE: f64 = 5000.0;
    const PUSH: f64 = 10.0;
    const PUSH_MAX: f64 = 12.0;
    const MAX_SECONDS: f64 = 3.0;

    pub fn new(jack_x: f64, jack_y: f64, width: usize, height: usize) -> Ripple {
        let s = Self::S;
        let (fine_rows, fine_cols) = (height * s, width * s);
        let jx = jack_x * s as f64 - 0.5;
        let jy = jack_y * s as f64 - 0.5;
        let mut splash = Vec::with_capacity(fine_rows * fine_cols);
        for y in 0..fine_rows {
            for x in 0..fine_cols {
                let dx = x as f64 - jx;
                let dy = y as f64 - jy;
                splash.push((-(dx * dx + dy * dy) / (2.0 * 25.0)).exp());
            }
        }
        let (mut ox, mut oy) = (Vec::with_capacity(height * width), Vec::with_capacity(height * width));
        for py in 0..height {
            for px in 0..width {
                let dx = px as f64 + 0.5 - jack_x;
                let dy = py as f64 + 0.5 - jack_y;
                let r = dx.hypot(dy);
                let fade = (r * s as f64 / 5.0).min(1.0) / r.max(1e-9);
                ox.push(dx * fade);
                oy.push(dy * fade);
            }
        }
        Ripple {
            width,
            height,
            splash,
            ox,
            oy,
            h: vec![0.0; fine_rows * fine_cols],
            v: vec![0.0; fine_rows * fine_cols],
            t: Some(0.0),
            age: 0.0,
            carry: 0.0,
            active: true,
            last: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Advance to the time now. Panels sharing a ripple each call this; it only moves once.
    pub fn advance_to(&mut self, now: f64) {
        let Some(last) = self.last else {
            self.last = Some(now);
            return;
        };
        let dt = now - last;
        self.last = Some(last.max(now));
        if dt > 0.0 {
            self.advance(dt);
        }
    }

    /// Runs the wave on in steps of 1/240 s; the time left over waits for the next call.
    pub fn advance(&mut self, dt: f64) {
        if !self.active {
            return;
        }
        let dt = dt.clamp(0.0, 0.1);
        self.age += dt;
        self.carry += dt;
        let (rows, cols) = (self.height * Self::S, self.width * Self::S);
        while self.carry >= Self::STEP {
            self.carry -= Self::STEP;
            let lap = laplacian(&self.h, rows, cols);
            let drive = self.t.map(|t| {
                let env = (1.0 - (-t / 0.04).exp()) * (-t / 0.2).exp();
                Self::DRIVE * env * (2.0 * std::f64::consts::PI * Self::FREQ * t).sin()
            });
            for (i, lap) in lap.iter().enumerate() {
                let mut a = Self::SPEED * Self::SPEED * lap - Self::DAMP * self.v[i] - Self::SPRING * self.h[i];
                if let Some(d) = drive {
                    a += d * self.splash[i];
                }
                self.v[i] += Self::STEP * a;
                self.h[i] += Self::STEP * self.v[i];
            }
            if let Some(t) = self.t {
                let t = t + Self::STEP;
                self.t = if t > 1.0 / Self::FREQ { None } else { Some(t) };
            }
        }
        let max_abs = |a: &[f64]| a.iter().fold(f64::NEG_INFINITY, |m, x| m.max(x.abs()));
        let quiet = max_abs(&self.h) < 0.05 && max_abs(&self.v) < 6.0;
        if (self.t.is_none() && quiet) || self.age > Self::MAX_SECONDS {
            self.active = false;
        }
    }

    /// The picture (`height` x `width`) as seen through the water.
    pub fn apply(&self, pixels: &Picture) -> Picture {
        if !self.active {
            return pixels.clone();
        }
        let s = Self::S;
        let fine_cols = self.width * s;
        let mut out = Picture::zeros(self.height, self.width);
        // Python's src(): reflect off the far edge, round to the nearest pixel, clip
        let src = |along: f64, moved: f64, size: usize| -> usize {
            let top = (size - 1) as f64;
            let mut p = (along - moved).abs();
            if p > top {
                p = 2.0 * top - p;
            }
            round_half_even(p).clamp(0.0, top) as usize
        };
        for py in 0..self.height {
            for px in 0..self.width {
                // The mean of the 3 x 3 fine cells: each row of 3 summed, then the rows, as numpy does
                let mut rows = [0.0; 3];
                for (a, row) in rows.iter_mut().enumerate() {
                    let at = (py * s + a) * fine_cols + px * s;
                    *row = self.h[at] + self.h[at + 1] + self.h[at + 2];
                }
                let hp = (rows[0] + rows[1] + rows[2]) / 9.0;
                let push = Self::PUSH_MAX * (Self::PUSH * hp / Self::PUSH_MAX).tanh() / s as f64;
                let i = py * self.width + px;
                let x = src(px as f64, push * self.ox[i], self.width);
                let y = src(py as f64, push * self.oy[i], self.height);
                out.set(py, px, pixels.at(y, x));
            }
        }
        out
    }
}

// Drain --------------------------------------------------------------------------------------------------

/// One piece of the picture on its way out.
#[derive(Clone, Copy, Debug)]
struct Particle {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    light: f64,
    /// How slowly it takes up the flow's speed.
    lag: f64,
    swirl: f64,
}

/// The picture as particles, sucked out through a hole in the right-hand wall at row jack_y.
/// For a port on the left, the caller flips the picture.
#[derive(Clone, Debug)]
pub struct Drain {
    pub width: usize,
    pub height: usize,
    jack: (f64, f64),
    opening: f64,
    particles: Vec<Particle>,
    trail: Vec<f64>,
    time: f64,
    glow: f64,
    pub drained: bool,
}

impl Drain {
    const SUB: usize = 2;
    const GIVE_UP: f64 = 1.6;

    /// Python's `Drain(pixels, jack_y, opening=2.0, seed=11)`: each lit pixel becomes 4 particles.
    pub fn new(pixels: &Picture, jack_y: f64, opening: f64, seed: u64) -> Drain {
        let (width, height) = (pixels.cols, pixels.rows);
        let n = Self::SUB * Self::SUB;
        let mut lit = Vec::new();
        for r in 0..height {
            for c in 0..width {
                if pixels.at(r, c) > 0.0 {
                    lit.push((r, c, pixels.at(r, c)));
                }
            }
        }
        let count = lit.len() * n;
        let mut rng = Generator::new(seed);
        let jitter_x = rng.uniform_n(-0.2, 0.2, count);
        let jitter_y = rng.uniform_n(-0.2, 0.2, count);
        let lag = rng.uniform_n(0.04, 0.12, count);
        let swirl = rng.uniform_n(-1.0, 1.0, count);
        let particles = (0..count)
            .map(|i| {
                let (r, c, value) = lit[i / n];
                Particle {
                    x: (c as f64 + 0.5) + jitter_x[i],
                    y: (r as f64 + 0.5) + jitter_y[i],
                    vx: 0.0,
                    vy: 0.0,
                    light: value / n as f64,
                    lag: lag[i],
                    swirl: swirl[i],
                }
            })
            .collect();
        Drain {
            width,
            height,
            jack: (width as f64, jack_y),
            opening,
            particles,
            trail: vec![0.0; height * width],
            time: 0.0,
            glow: 0.0,
            drained: false,
        }
    }

    /// Moves the particles on: pulled towards the hole, faster as time goes on, with eddies around it.
    /// Those that reach the hole go, and light it up for a moment.
    pub fn advance(&mut self, dt: f64) {
        let dt = dt.clamp(0.0, 0.1);
        self.time += dt;
        if self.particles.is_empty() {
            return;
        }
        let (jx, jy) = self.jack;
        let time = self.time;
        let drive = (1.0 - (-time / 0.08).exp()) * (1.0 + 3.0 * time);
        let (w, h) = (self.width as f64, self.height as f64);
        let give_up = time > Self::GIVE_UP;
        let mut gone = Vec::new();
        let mut kept = Vec::with_capacity(self.particles.len());
        for p in &self.particles {
            let mut p = *p;
            let dx = jx - p.x;
            let dy = jy - p.y;
            let r = dx.hypot(dy) + 1e-6;
            let speed = drive * (25.0 + 160.0 / (r + 2.0));
            let (tx, ty) = (-dy / r, dx / r);
            let sw = p.swirl * 18.0 * (time * 9.0 + p.y * 0.7).sin() * (r / 6.0).min(1.0);
            let fx = dx / r * speed + tx * sw;
            let fy = dy / r * speed + ty * sw;
            let k = 1.0 - (-dt / p.lag).exp();
            p.vx += (fx - p.vx) * k;
            p.vy += (fy - p.vy) * k;
            p.x = (p.x + p.vx * dt).clamp(0.0, w - 1e-6);
            p.y = (p.y + p.vy * dt).clamp(0.0, h - 1e-6);
            let beside = ((p.y - jy).abs() - self.opening / 2.0).max(0.0);
            if give_up || (p.x - jx).hypot(beside) < 0.9 {
                gone.push(p.light);
            } else {
                kept.push(p);
            }
        }
        self.glow += pairwise_sum(&gone) * 0.02;
        self.particles = kept;
    }

    /// The picture now: each particle spread over the 4 pixels around it, fading trails, and the glow
    /// at the hole. Sets `drained` once nothing is left to see.
    pub fn pixels(&mut self, dt: f64) -> Picture {
        let (w, h) = (self.width, self.height);
        let mut f = vec![0.0; h * w];
        if !self.particles.is_empty() {
            // numpy's four bincounts, each summed on its own and then added in turn
            let mut parts = [vec![0.0; h * w], vec![0.0; h * w], vec![0.0; h * w], vec![0.0; h * w]];
            for p in &self.particles {
                let x = (p.x - 0.5).clamp(0.0, w as f64 - 1.001);
                let y = (p.y - 0.5).clamp(0.0, h as f64 - 1.001);
                let (x0, y0) = (x as usize, y as usize);
                let (fx, fy) = (x - x0 as f64, y - y0 as f64);
                let i = y0 * w + x0;
                parts[0][i] += p.light * ((1.0 - fx) * (1.0 - fy));
                parts[1][i + 1] += p.light * (fx * (1.0 - fy));
                parts[2][i + w] += p.light * ((1.0 - fx) * fy);
                parts[3][i + w + 1] += p.light * (fx * fy);
            }
            for part in &parts {
                for (a, b) in f.iter_mut().zip(part) {
                    *a += b;
                }
            }
        }
        let fade = (-dt.max(0.0) / 0.06).exp();
        for (t, &v) in self.trail.iter_mut().zip(&f) {
            *t = v.max(*t * fade);
        }
        self.glow *= (-dt.max(0.0) / 0.12).exp();
        let mut out = Picture { rows: h, cols: w, data: self.trail.clone() };
        let jy = self.jack.1.max(0.0).min((h - 1) as f64) as usize;
        for yy in jy.saturating_sub(1)..(jy + 2).min(h) {
            let at = yy * w + w - 1;
            out.data[at] += self.glow * if yy == jy { 1.0 } else { 0.5 };
        }
        if self.particles.is_empty() && out.data.iter().fold(f64::NEG_INFINITY, |m, &v| m.max(v)) < 2.0 {
            self.drained = true;
        }
        for v in &mut out.data {
            *v = v.clamp(0.0, 255.0);
        }
        out
    }
}

// Text line ----------------------------------------------------------------------------------------------

/// 3 x 5 letters for the text line, row by row.
const FONT: &[(char, &str)] = &[
    ('A', "010101111101101"),
    ('B', "110101110101110"),
    ('C', "011100100100011"),
    ('D', "110101101101110"),
    ('E', "111100110100111"),
    ('F', "111100110100100"),
    ('G', "011100101101011"),
    ('H', "101101111101101"),
    ('I', "111010010010111"),
    ('J', "001001001101010"),
    ('K', "101101110101101"),
    ('L', "100100100100111"),
    ('M', "101111111101101"),
    ('N', "110101101101101"),
    ('O', "010101101101010"),
    ('P', "110101110100100"),
    ('Q', "010101101110011"),
    ('R', "110101110101101"),
    ('S', "011100010001110"),
    ('T', "111010010010010"),
    ('U', "101101101101111"),
    ('V', "101101101101010"),
    ('W', "101101111111101"),
    ('X', "101101010101101"),
    ('Y', "101101010010010"),
    ('Z', "111001010100111"),
    ('0', "111101101101111"),
    ('1', "010110010010111"),
    ('2', "111001111100111"),
    ('3', "111001011001111"),
    ('4', "101101111001001"),
    ('5', "111100111001111"),
    ('6', "111100111101111"),
    ('7', "111001010010010"),
    ('8', "111101111101111"),
    ('9', "111101111001111"),
    (' ', "000000000000000"),
    ('.', "000000000000010"),
    (',', "000000000010100"),
    (':', "000010000010000"),
    ('!', "010010010000010"),
    ('?', "111001011000010"),
    ('-', "000000111000000"),
    ('+', "000010111010000"),
    ('%', "101001010100101"),
    ('\'', "010010000000000"),
    ('"', "101101000000000"),
    ('/', "001001010100100"),
    ('(', "001010010010001"),
    (')', "100010010010100"),
    ('&', "010101010101011"),
    ('#', "101111101111101"),
    ('*', "000101010101000"),
    ('=', "000111000111000"),
    ('_', "000000000000111"),
    ('<', "001010100010001"),
    ('>', "100010001010100"),
    ('@', "010101111100011"),
    ('$', "011110010011110"),
];

/// A letter's 15 bits, row by row; letters we don't have are '?'.
fn glyph(ch: char) -> &'static str {
    let find = |c: char| FONT.iter().find(|(k, _)| *k == c).map(|(_, g)| *g);
    find(ch).or_else(|| find('?')).unwrap()
}

/// One column of the text line, top row first.
pub type TextColumn = [u8; FONT_H];

/// The text in capitals as columns of 0/1, one dark column between letters.
pub fn text_columns(text: &str) -> Vec<TextColumn> {
    let mut cols = Vec::new();
    for ch in text.to_uppercase().chars() {
        if !cols.is_empty() {
            cols.push([0; FONT_H]);
        }
        let bits = glyph(ch).as_bytes();
        for c in 0..FONT_W {
            let mut col = [0; FONT_H];
            for (r, v) in col.iter_mut().enumerate() {
                *v = (bits[r * FONT_W + c] == b'1') as u8;
            }
            cols.push(col);
        }
    }
    cols
}

pub fn text_width(text: &str) -> usize {
    text_columns(text).len()
}

/// Messages scrolling right to left along the top rows of both panels, one after another.
#[derive(Clone, Debug)]
pub struct TextLine {
    queue: VecDeque<String>,
    /// The message showing, and when it started.
    current: Option<(Vec<TextColumn>, f64)>,
    /// Columns a second.
    pub speed: f64,
}

impl Default for TextLine {
    fn default() -> Self {
        TextLine::new()
    }
}

impl TextLine {
    /// Up to 8 messages wait; more push the oldest out.
    const QUEUE: usize = 8;

    pub fn new() -> TextLine {
        TextLine { queue: VecDeque::new(), current: None, speed: 18.0 }
    }

    /// Queues a message, its spaces squeezed to one and cut to 120 letters.
    pub fn say(&mut self, text: &str) {
        let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
        if !text.is_empty() {
            if self.queue.len() == Self::QUEUE {
                self.queue.pop_front();
            }
            self.queue.push_back(text);
        }
    }

    /// The speed setting, 1-10.
    pub fn set_speed(&mut self, setting: i64) {
        self.speed = 6.0 + 3.0 * setting.clamp(1, 10) as f64;
    }

    /// Drops what is showing and waiting.
    pub fn clear(&mut self) {
        self.queue.clear();
        self.current = None;
    }

    /// The text line across all panels as TEXT_ROWS rows of `width` 0/1 values, or None when it is idle.
    pub fn strip(&mut self, width: usize, now: f64) -> Option<Vec<Vec<u8>>> {
        if self.current.is_none() {
            let next = self.queue.pop_front()?;
            self.current = Some((text_columns(&next), now));
        }
        let (cols, start) = self.current.as_ref().unwrap();
        let mut x = width as f64 - (now - start) * self.speed;
        if x + (cols.len() as f64) < 0.0 {
            // Gone off the left edge: the next message, if any
            self.current = None;
            let next = self.queue.pop_front()?;
            self.current = Some((text_columns(&next), now));
            x = width as f64;
        }
        let cols = &self.current.as_ref().unwrap().0;
        let mut out = vec![vec![0u8; width]; TEXT_ROWS];
        let xi = int_round(x);
        for (c, col) in cols.iter().enumerate() {
            let at = xi + c as i64;
            if (0..width as i64).contains(&at) {
                for (r, &v) in col.iter().enumerate() {
                    out[r][at as usize] = v;
                }
            }
        }
        Some(out)
    }

    pub fn active(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty()
    }
}

/// Where a panel's 9 columns are in the picture spanning all panels (with the keyboard between);
/// None when the side isn't in `order`.
pub fn panel_columns(order: &[Side], side: Side) -> Option<(usize, usize)> {
    let start = order.iter().position(|&s| s == side)? * (WIDTH + GAP);
    Some((start, start + WIDTH))
}

// The manager --------------------------------------------------------------------------------------------

/// What a panel is doing after something was unplugged from its side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The picture is draining out through the port.
    Drain,
    /// Dark for a moment.
    Empty,
    /// The picture coming back up from the bottom.
    Rise,
}

/// The effects on one panel.
#[derive(Clone, Debug, Default)]
pub struct SidePlayer {
    /// The drain, and whether its picture is flipped (a port on the left).
    pub drain: Option<(Drain, bool)>,
    pub phase: Option<Phase>,
    pub phase_since: f64,
    /// The frame sliding away, and when it started.
    pub slide: Option<(Picture, f64)>,
    /// The last frame from the monitor.
    pub last_base: Option<Picture>,
    pub last_time: Option<f64>,
}

impl SidePlayer {
    const SLIDE_SECONDS: f64 = 0.45;
    const EMPTY_SECONDS: f64 = 0.2;
    const RISE_SECONDS: f64 = 0.25;

    pub fn new() -> SidePlayer {
        SidePlayer::default()
    }

    /// Python's player also has a list of its own ripples, which is always empty (ripples span the
    /// panels and belong to [`Effects`]), so it is left out here.
    pub fn active(&self) -> bool {
        self.phase.is_some() || self.slide.is_some()
    }

    fn start_rise(&mut self, now: f64) {
        self.drain = None;
        self.phase = Some(Phase::Rise);
        self.phase_since = now;
    }
}

/// All the effects, for both panels. One per process, shared by the drawing threads.
#[derive(Clone, Debug)]
pub struct Effects {
    pub settings: EffectsSettings,
    players: [SidePlayer; 2],
    pub text: TextLine,
    /// Sides with a panel drawing now.
    panels: [bool; 2],
    /// The text line's brightness.
    pub foreground: i32,
    /// Ripples across all panels at once, each with the panel order it was made for.
    ripples: Vec<(Ripple, Vec<Side>)>,
}

impl Default for Effects {
    fn default() -> Self {
        Effects::new()
    }
}

impl Effects {
    pub fn new() -> Effects {
        Effects {
            settings: EffectsSettings::default(),
            players: [SidePlayer::new(), SidePlayer::new()],
            text: TextLine::new(),
            panels: [false; 2],
            foreground: 160,
            ripples: Vec::new(),
        }
    }

    // Settings and panels

    /// New settings. Note the text line keeps its first speed (18 columns a second) until this is called.
    pub fn configure(&mut self, settings: EffectsSettings) {
        self.settings = settings;
        self.text.set_speed(self.settings.text_speed);
        if !self.settings.text_line {
            self.text.clear();
        }
    }

    pub fn panel_present(&mut self, side: Side, present: bool) {
        self.panels[side.index()] = present;
    }

    pub fn player(&self, side: Side) -> &SidePlayer {
        &self.players[side.index()]
    }

    /// The panels drawing now, left first; when there are none, `fallback`.
    fn order(&self, fallback: Side) -> Vec<Side> {
        let order: Vec<Side> = Side::ALL.into_iter().filter(|s| self.panels[s.index()]).collect();
        if order.is_empty() {
            vec![fallback]
        } else {
            order
        }
    }

    /// Which panel shows a port on this side of the laptop, and whether its port is at the panel's left
    /// edge (so the picture is flipped for the drain).
    pub fn panel_for(&self, side: Option<Side>) -> (Option<Side>, bool) {
        let panels = self.order(Side::Left);
        match side {
            Some(Side::Left) => (Some(panels[0]), true),
            Some(Side::Right) => (Some(panels[panels.len() - 1]), false),
            None => (None, false),
        }
    }

    // Events

    /// Something went into (or came out of) the port at position ("left-back", "right", None...).
    pub fn plug(&mut self, position: Option<&str>, plugged_in: bool, now: f64) {
        if !self.settings.plug_animation {
            return;
        }
        let (side, spot) = position.map(parse_position).unwrap_or((None, None));
        let row = spot.map(|s| self.settings.port_rows.row(s)).unwrap_or(HEIGHT / 2);
        let (panel, at_left) = self.panel_for(side);
        if plugged_in {
            // One ripple across all the panels, starting at the port's edge and running on to the other
            // panel. A port we can't place ripples out from between the panels.
            let order = self.order(Side::Left);
            let wide = order.len() * WIDTH + (order.len() - 1) * GAP;
            let (jack_x, jack_y) = match panel {
                None => (wide as f64 / 2.0, HEIGHT as f64 / 2.0),
                Some(panel) => {
                    let player = &mut self.players[panel.index()];
                    if matches!(player.phase, Some(Phase::Drain | Phase::Empty)) {
                        player.start_rise(now);
                    }
                    (if at_left { 0.0 } else { wide as f64 }, row as f64 + 0.5)
                }
            };
            self.ripples.push((Ripple::new(jack_x, jack_y, wide, HEIGHT), order));
            if self.ripples.len() > 3 {
                self.ripples.drain(..self.ripples.len() - 3);
            }
            return;
        }
        let Some(panel) = panel else { return };
        let player = &mut self.players[panel.index()];
        let Some(base) = &player.last_base else { return };
        let frame = if at_left { base.flipped() } else { base.clone() };
        player.drain = Some((Drain::new(&frame, row as f64 + 0.5, 2.0, 11), at_left));
        player.phase = Some(Phase::Drain);
        player.phase_since = now;
    }

    /// A message for the text line (ignored while it is turned off).
    pub fn say(&mut self, text: &str) {
        if self.settings.text_line {
            self.text.say(text);
        }
    }

    /// The panel on this side is changing app: slide the new one in over the last frame.
    pub fn start_slide(&mut self, side: Side, now: f64) {
        if !self.settings.slide {
            return;
        }
        let player = &mut self.players[side.index()];
        if player.phase.is_none() {
            if let Some(base) = &player.last_base {
                player.slide = Some((base.clone(), now));
            }
        }
    }

    // Drawing

    fn ripples_on(&self, side: Side) -> bool {
        self.ripples.iter().any(|(r, order)| order.contains(&side) && r.is_active())
    }

    pub fn active(&self, side: Side) -> bool {
        self.players[side.index()].active() || self.ripples_on(side) || self.text.active()
    }

    /// How long a drawing thread may wait for a new frame before redrawing the effect itself; None
    /// while nothing is playing.
    pub fn interval(&self, side: Side) -> Option<f64> {
        self.active(side).then_some(FRAME_SECONDS)
    }

    /// The panel's picture was replaced by something else (a game, sleep): drop what is playing.
    pub fn forget(&mut self, side: Side) {
        self.players[side.index()] = SidePlayer::new();
    }

    /// The frame to send to the panel on this side. `new_frame` is false when the drawing thread is only
    /// redrawing its last frame to move an effect on.
    pub fn apply(&mut self, side: Side, grid: &Grid, new_frame: bool, now: f64) -> Grid {
        let i = side.index();
        let base = Picture::from_grid(grid);
        let others_active = self.ripples_on(side) || self.text.active();
        let player = &mut self.players[i];
        if new_frame || player.last_base.is_none() {
            player.last_base = Some(base.clone());
        }
        let dt = match player.last_time {
            None => 0.0,
            Some(last) => (now - last).min(0.1),
        };
        player.last_time = Some(now);
        if !player.active() && !others_active {
            return grid.clone();
        }
        let mut out = base;

        if let Some((old, start)) = &player.slide {
            let k = ((now - start) / SidePlayer::SLIDE_SECONDS).min(1.0);
            let k = 1.0 - (1.0 - k).powf(3.0);
            // Clamped: Python can't slide backwards either (the clock only goes forwards)
            let shift = int_round(HEIGHT as f64 * k).max(0) as usize;
            if shift >= HEIGHT {
                player.slide = None;
            } else {
                let mut slid = Picture::zeros(HEIGHT, WIDTH);
                slid.copy_rows(0, old, shift, HEIGHT - shift);
                slid.copy_rows(HEIGHT - shift, &out, 0, shift);
                out = slid;
            }
        }

        match player.phase {
            Some(Phase::Drain) => {
                if let Some((drain, flipped)) = &mut player.drain {
                    drain.advance(dt);
                    let picture = drain.pixels(dt);
                    out = if *flipped { picture.flipped() } else { picture };
                    if drain.drained {
                        player.drain = None;
                        player.phase = Some(Phase::Empty);
                        player.phase_since = now;
                    }
                }
            }
            Some(Phase::Empty) => {
                out = Picture::zeros(HEIGHT, WIDTH);
                if now - player.phase_since >= SidePlayer::EMPTY_SECONDS {
                    player.start_rise(now);
                }
            }
            Some(Phase::Rise) => {
                let k = ((now - player.phase_since) / SidePlayer::RISE_SECONDS).min(1.0);
                let shift = int_round(HEIGHT as f64 * (1.0 - k)).clamp(0, HEIGHT as i64) as usize;
                let mut risen = Picture::zeros(HEIGHT, WIDTH);
                risen.copy_rows(shift, &out, 0, HEIGHT - shift);
                out = risen;
                if k >= 1.0 {
                    player.phase = None;
                }
            }
            None => {}
        }

        if matches!(self.players[i].phase, None | Some(Phase::Rise)) && self.ripples_on(side) {
            for (ripple, order) in self.ripples.iter_mut() {
                if !order.contains(&side) || !ripple.is_active() {
                    continue;
                }
                ripple.advance_to(now);
                // This panel's part of the picture across all panels: the water can carry a bit of the
                // other panel's picture over, and the wave runs on from one to the other
                let mut wide = Picture::zeros(HEIGHT, ripple.width);
                for &other in order.iter() {
                    let (a, _) = panel_columns(order, other).unwrap();
                    if other == side {
                        wide.put_columns(a, &out);
                    } else if let Some(theirs) = &self.players[other.index()].last_base {
                        wide.put_columns(a, theirs);
                    }
                }
                let (a, b) = panel_columns(order, side).unwrap();
                out = ripple.apply(&wide).columns(a, b);
            }
            self.ripples.retain(|(r, _)| r.is_active());
        }

        if self.settings.text_line {
            let order = self.order(side);
            if let Some(at) = order.iter().position(|&s| s == side) {
                if let Some(strip) = self.text.strip(WIDTH * order.len(), now) {
                    for (r, row) in strip.iter().enumerate() {
                        for x in 0..WIDTH {
                            out.set(r, x, (row[at * WIDTH + x] as i32 * self.foreground) as f64);
                        }
                    }
                }
            }
        }

        out.to_grid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairwise_sum_matches_a_plain_sum_on_small_arrays() {
        assert_eq!(pairwise_sum(&[]), 0.0);
        assert_eq!(pairwise_sum(&[1.0, 2.0, 3.5]), 6.5);
        let many: Vec<f64> = (0..300).map(|i| i as f64).collect();
        assert_eq!(pairwise_sum(&many), 44850.0);
    }

    #[test]
    fn letters_have_a_dark_column_between_them() {
        assert_eq!(text_width("AB"), 7);
        assert_eq!(text_width(""), 0);
        assert_eq!(text_columns("~"), text_columns("?"));
        assert!(FONT.iter().all(|(_, g)| g.len() == FONT_W * FONT_H));
    }

    #[test]
    fn positions() {
        assert_eq!(parse_position("Right Middle"), (Some(Side::Right), Some(Spot::Middle)));
        assert_eq!(position_text(Some(Side::Left), None).as_deref(), Some("left"));
        assert_eq!(position_text(None, Some(Spot::Back)), None);
        assert_eq!(panel_columns(&[Side::Left, Side::Right], Side::Right), Some((13, 22)));
    }

    #[test]
    fn settings_deserialize_like_normalize() {
        let s: EffectsSettings = serde_json::from_str(r#"{"text_speed": "1_0", "port_rows": {"back": 40}}"#).unwrap();
        assert_eq!(s.text_speed, 10);
        assert_eq!(s.port_rows.back, 33);
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(EffectsSettings::from_json(&back), s);
    }
}
