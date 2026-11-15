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
use crate::plug_looks::{return_frame, return_seconds, transition_frame, Look};
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
    /// Screens plugged into an HDMI or DisplayPort card. Not in the Python app: only saved when off.
    #[serde(skip_serializing_if = "is_true")]
    pub plug_displays: bool,
    /// The ports on each side of the laptop (a port we can't place always plays). Only saved when off.
    #[serde(skip_serializing_if = "is_true")]
    pub plug_left: bool,
    #[serde(skip_serializing_if = "is_true")]
    pub plug_right: bool,
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
    /// Your own messages (one line each), and whether they are going round on the text line, taking
    /// turns, until turned off. Not in the Python app, so only saved when set.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub show_messages: bool,
    /// A message of its own on each panel, running down the panel instead of what it shows, while
    /// `show_vertical` is on. `vertical_style`: "ticker", "sideways" or "stacked" (the Text app's
    /// styles). Not in the Python app, so only saved when set.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vertical_left: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vertical_right: String,
    #[serde(skip_serializing_if = "is_ticker")]
    pub vertical_style: String,
    #[serde(skip_serializing_if = "is_false")]
    pub show_vertical: bool,
    /// Slide the next app in from the bottom when the panel changes app, instead of cutting to it.
    pub slide: bool,
    /// With `slide` off: fade from one app to the next instead of cutting. Also how the picture comes
    /// back after the unplug drain. Not in the Python app, so only saved when on.
    #[serde(skip_serializing_if = "is_false")]
    pub fade: bool,
    /// The two panels sit next to each other (no keyboard between them): the unplug drain runs across
    /// both as one picture. Not in the Python app, so only saved when on.
    #[serde(skip_serializing_if = "is_false")]
    pub panels_together: bool,
    /// The plug animations: "drain" (unplug) and "ripple" (plug in) are the originals, the others are
    /// [`Look`] keys. Not in the Python app, so only saved when changed.
    #[serde(skip_serializing_if = "is_drain")]
    pub unplug_look: String,
    #[serde(skip_serializing_if = "is_ripple")]
    pub plug_look: String,
    /// Changing apps with pixel rain ("rain") or the zipper ("zipper") instead of `slide` / `fade`;
    /// empty for those. Not in the Python app, so only saved when set.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub transition_look: String,
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
            plug_displays: true,
            plug_left: true,
            plug_right: true,
            port_rows: PortRows::default(),
            learned_ports: BTreeMap::new(),
            text_line: true,
            text_song: true,
            text_charger: true,
            text_battery: true,
            text_speed: 5,
            messages: Vec::new(),
            show_messages: false,
            vertical_left: String::new(),
            vertical_right: String::new(),
            vertical_style: "ticker".into(),
            show_vertical: false,
            slide: true,
            fade: false,
            panels_together: false,
            unplug_look: DRAIN.to_string(),
            plug_look: RIPPLE.to_string(),
            transition_look: String::new(),
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
            ("plug_displays", &mut s.plug_displays),
            ("plug_left", &mut s.plug_left),
            ("plug_right", &mut s.plug_right),
            ("text_line", &mut s.text_line),
            ("text_song", &mut s.text_song),
            ("text_charger", &mut s.text_charger),
            ("text_battery", &mut s.text_battery),
            ("slide", &mut s.slide),
            ("fade", &mut s.fade),
            ("panels_together", &mut s.panels_together),
            ("full_shades", &mut s.full_shades),
        ] {
            if let Some(v) = get(key) {
                *flag = truthy(v);
            }
        }
        let tidy = |v: &Value| -> Option<String> {
            let text: String = v.as_str()?.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
            (!text.is_empty()).then_some(text)
        };
        if let Some(Value::Array(list)) = get("messages") {
            s.messages = list.iter().filter_map(tidy).take(Self::MAX_MESSAGES).collect();
        }
        if let Some(v) = get("show_messages") {
            s.show_messages = truthy(v);
        }
        for (key, field) in [("vertical_left", &mut s.vertical_left), ("vertical_right", &mut s.vertical_right)] {
            if let Some(Value::String(text)) = get(key) {
                *field = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect();
            }
        }
        if let Some(Value::String(style)) = get("vertical_style") {
            if ["ticker", "sideways", "stacked"].contains(&style.as_str()) {
                s.vertical_style = style.clone();
            }
        }
        if let Some(v) = get("show_vertical") {
            s.show_vertical = truthy(v);
        }
        // Test builds before 1.5 saved one message as "pinned_message"
        if let Some(text) = get("pinned_message").and_then(tidy) {
            if s.messages.is_empty() {
                s.messages.push(text);
                s.show_messages = true;
            }
        }
        if let Some(Value::String(key)) = get("unplug_look") {
            if key == DRAIN || Look::plug_from_key(key).is_some() {
                s.unplug_look = key.clone();
            }
        }
        if let Some(Value::String(key)) = get("plug_look") {
            if key == RIPPLE || Look::plug_from_key(key).is_some() {
                s.plug_look = key.clone();
            }
        }
        if let Some(Value::String(key)) = get("transition_look") {
            if matches!(key.as_str(), "rain" | "zipper") {
                s.transition_look = key.clone();
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

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_ticker(style: &String) -> bool {
    style == "ticker"
}

/// The original unplug animation's key.
pub const DRAIN: &str = "drain";
/// The original plug-in animation's key.
pub const RIPPLE: &str = "ripple";

fn is_drain(key: &String) -> bool {
    key == DRAIN
}

fn is_ripple(key: &String) -> bool {
    key == RIPPLE
}

/// How the panel goes from one app to the next (and comes back after the unplug drain).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    Slide,
    Fade,
    Cut,
    /// The old app falls away as pixel rain while the new one floats up into place.
    Rain,
    /// The old app unzips and falls apart, then the new one zips up.
    Zipper,
}

impl Transition {
    /// The plug animation it borrows its frames from.
    fn look(self) -> Option<Look> {
        match self {
            Transition::Rain => Some(Look::Rain),
            Transition::Zipper => Some(Look::Zipper),
            _ => None,
        }
    }
}

impl EffectsSettings {
    /// Lines of your own message: 5 lines of 6 rows fill most of the 34-row panel.
    pub const MAX_MESSAGES: usize = 5;

    /// The vertical message on this side's panel, when one is showing.
    pub fn vertical(&self, side: Side) -> Option<&str> {
        let text = match side {
            Side::Left => &self.vertical_left,
            Side::Right => &self.vertical_right,
        };
        (self.show_vertical && !text.trim().is_empty()).then_some(text.as_str())
    }

    /// Whether the plug effect plays for a port at this position ("left-back", "right", None...).
    pub fn plug_side_on(&self, position: Option<&str>) -> bool {
        match position.map(parse_position).and_then(|(side, _)| side) {
            Some(Side::Left) => self.plug_left,
            Some(Side::Right) => self.plug_right,
            None => true,
        }
    }

    /// The unplug animation, None for the drain.
    pub fn unplug(&self) -> Option<Look> {
        Look::plug_from_key(&self.unplug_look)
    }

    /// The plug-in animation, None for the ripple.
    pub fn plug_in(&self) -> Option<Look> {
        Look::plug_from_key(&self.plug_look)
    }

    pub fn transition(&self) -> Transition {
        match self.transition_look.as_str() {
            "rain" => return Transition::Rain,
            "zipper" => return Transition::Zipper,
            _ => {}
        }
        if self.slide {
            Transition::Slide
        } else if self.fade {
            Transition::Fade
        } else {
            Transition::Cut
        }
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
    /// The message showing (one or more lines, one under the other), and when it started.
    current: Option<(Vec<Vec<TextColumn>>, f64)>,
    /// Columns a second.
    pub speed: f64,
    /// Your own message, its lines one under the other, shown again and again whenever nothing else is
    /// waiting, until unpinned.
    pinned: Vec<String>,
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
        TextLine { queue: VecDeque::new(), current: None, speed: 18.0, pinned: Vec::new() }
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

    /// Keeps this message (its lines one under the other) going round until it is unpinned (an empty
    /// list).
    pub fn pin(&mut self, texts: &[String]) {
        let texts: Vec<String> = texts
            .iter()
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|t| !t.is_empty())
            .collect();
        if texts == self.pinned {
            return;
        }
        self.pinned = texts;
        // Turned off: stop it now rather than letting it scroll to the end
        if self.pinned.is_empty() {
            self.current = None;
        }
    }

    /// The next message's lines: one waiting, else the pinned one.
    fn next(&mut self) -> Option<Vec<Vec<TextColumn>>> {
        let lines = match self.queue.pop_front() {
            Some(text) => vec![text],
            None if !self.pinned.is_empty() => self.pinned.clone(),
            None => return None,
        };
        Some(lines.iter().take(EffectsSettings::MAX_MESSAGES).map(|l| text_columns(l)).collect())
    }

    /// Drops what is showing and waiting.
    pub fn clear(&mut self) {
        self.queue.clear();
        self.current = None;
    }

    /// The text line across all panels as TEXT_ROWS rows of `width` 0/1 values for each line of the
    /// message (one under the other, scrolling together), or None when it is idle.
    pub fn strip(&mut self, width: usize, now: f64) -> Option<Vec<Vec<u8>>> {
        if self.current.is_none() {
            let next = self.next()?;
            self.current = Some((next, now));
        }
        let (lines, start) = self.current.as_ref().unwrap();
        let longest = lines.iter().map(Vec::len).max().unwrap_or(0);
        let mut x = width as f64 - (now - start) * self.speed;
        if x + (longest as f64) < 0.0 {
            // Gone off the left edge: the next message, if any
            self.current = None;
            let next = self.next()?;
            self.current = Some((next, now));
            x = width as f64;
        }
        let lines = &self.current.as_ref().unwrap().0;
        let mut out = vec![vec![0u8; width]; TEXT_ROWS * lines.len()];
        let xi = int_round(x);
        for (n, cols) in lines.iter().enumerate() {
            for (c, col) in cols.iter().enumerate() {
                let at = xi + c as i64;
                if (0..width as i64).contains(&at) {
                    for (r, &v) in col.iter().enumerate() {
                        out[n * TEXT_ROWS + r][at as usize] = v;
                    }
                }
            }
        }
        Some(out)
    }

    pub fn active(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty() || !self.pinned.is_empty()
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
    /// The picture coming back: up from the bottom, fading in, or all at once (Effects tab).
    Rise,
    /// One of the other unplug animations ([`Look`]) taking the picture away.
    Out,
    /// One of the other plug-in animations bringing the picture in.
    In,
}

/// A [`Look`] playing on a panel.
#[derive(Clone, Debug)]
pub struct LookPlay {
    pub look: Look,
    /// The port is at the panel's left edge, so the picture is flipped for it.
    pub flipped: bool,
    /// The row the port is level with (the middle of the row).
    pub row: f64,
    /// Going out, the picture as it was when unplugged (both panels and the gap when `wide`).
    pub picture: Option<Picture>,
    /// Plays across both panels as one picture, the port at the outer edge of its panel.
    pub wide: bool,
}

/// The effects on one panel.
#[derive(Clone, Debug, Default)]
pub struct SidePlayer {
    /// The drain, and whether its picture is flipped (a port on the left).
    pub drain: Option<(Drain, bool)>,
    /// The other unplug and plug-in animations.
    pub look: Option<LookPlay>,
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
    const FADE_SECONDS: f64 = 0.4;
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
        self.look = None;
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
    /// One drain across both panels (`panels_together`), shared by the two players.
    wide: Option<WideDrain>,
}

/// The unplug drain over both panels side by side: left panel, the gap, right panel.
#[derive(Clone, Debug)]
struct WideDrain {
    drain: Drain,
    /// The port is on the left, so the drain runs on the flipped picture.
    flipped: bool,
    /// When it last moved on, and the picture it made then (both panels ask for it each frame).
    last: Option<f64>,
    picture: Picture,
}

impl WideDrain {
    /// The whole picture at `now`, moving the drain on only once per moment.
    fn at(&mut self, now: f64) -> &Picture {
        let dt = match self.last {
            Some(last) if now <= last => return &self.picture,
            Some(last) => (now - last).min(0.1),
            None => 0.0,
        };
        self.last = Some(now);
        self.drain.advance(dt);
        let picture = self.drain.pixels(dt);
        self.picture = if self.flipped { picture.flipped() } else { picture };
        &self.picture
    }
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
            wide: None,
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
        let on = self.settings.text_line && self.settings.show_messages;
        self.text.pin(if on { &self.settings.messages } else { &[] });
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
            if let Some(look) = self.settings.plug_in() {
                // The picture comes in on the port's panel, or every panel for a port we can't place
                self.wide = None;
                let wide = look.spans_panels() && panel.is_some() && self.panels == [true, true];
                let targets: Vec<Side> = match panel {
                    Some(_) if wide => Side::ALL.to_vec(),
                    Some(panel) => vec![panel],
                    None => self.order(Side::Left),
                };
                for side in targets {
                    let player = &mut self.players[side.index()];
                    player.drain = None;
                    player.slide = None;
                    player.look = Some(LookPlay { look, flipped: at_left, row: row as f64 + 0.5, picture: None, wide });
                    player.phase = Some(Phase::In);
                    player.phase_since = now;
                }
                return;
            }
            // One ripple across all the panels, starting at the port's edge and running on to the other
            // panel. A port we can't place ripples out from between the panels.
            let order = self.order(Side::Left);
            let wide = order.len() * WIDTH + (order.len() - 1) * GAP;
            let (jack_x, jack_y) = match panel {
                None => (wide as f64 / 2.0, HEIGHT as f64 / 2.0),
                Some(panel) => {
                    let together = self.wide.take().is_some();
                    for (i, player) in self.players.iter_mut().enumerate() {
                        if (together || i == panel.index())
                            && matches!(player.phase, Some(Phase::Drain | Phase::Empty | Phase::Out | Phase::In))
                        {
                            player.start_rise(now);
                        }
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
        if let Some(look) = self.settings.unplug() {
            self.wide = None;
            if look.spans_panels() && self.panels == [true, true] {
                if let (Some(left), Some(right)) = (&self.players[0].last_base, &self.players[1].last_base) {
                    // Both panels as one picture, the port at the outer edge of the panel on its side
                    let mut both = Picture::zeros(HEIGHT, 2 * WIDTH + GAP);
                    both.put_columns(0, left);
                    both.put_columns(WIDTH + GAP, right);
                    let picture = if at_left { both.flipped() } else { both };
                    for player in self.players.iter_mut() {
                        player.drain = None;
                        player.slide = None;
                        player.look = Some(LookPlay {
                            look,
                            flipped: at_left,
                            row: row as f64 + 0.5,
                            picture: Some(picture.clone()),
                            wide: true,
                        });
                        player.phase = Some(Phase::Out);
                        player.phase_since = now;
                    }
                    return;
                }
            }
            let player = &mut self.players[panel.index()];
            let Some(base) = &player.last_base else { return };
            let picture = if at_left { base.flipped() } else { base.clone() };
            player.drain = None;
            player.slide = None;
            player.look =
                Some(LookPlay { look, flipped: at_left, row: row as f64 + 0.5, picture: Some(picture), wide: false });
            player.phase = Some(Phase::Out);
            player.phase_since = now;
            return;
        }
        if self.settings.panels_together && self.panels == [true, true] {
            if let (Some(left), Some(right)) = (&self.players[0].last_base, &self.players[1].last_base) {
                // Both panels as one picture, the port at the outer edge of the panel on its side
                let mut both = Picture::zeros(HEIGHT, 2 * WIDTH + GAP);
                both.put_columns(0, left);
                both.put_columns(WIDTH + GAP, right);
                let frame = if at_left { both.flipped() } else { both };
                let drain = Drain::new(&frame, row as f64 + 0.5, 2.0, 11);
                self.wide = Some(WideDrain { drain, flipped: at_left, last: None, picture: frame });
                for player in self.players.iter_mut() {
                    player.drain = None;
                    player.slide = None;
                    player.phase = Some(Phase::Drain);
                    player.phase_since = now;
                }
                return;
            }
        }
        self.wide = None;
        let player = &mut self.players[panel.index()];
        let Some(base) = &player.last_base else { return };
        let frame = if at_left { base.flipped() } else { base.clone() };
        player.drain = Some((Drain::new(&frame, row as f64 + 0.5, 2.0, 11), at_left));
        player.look = None;
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
        if self.settings.transition() == Transition::Cut {
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
        self.players[side.index()].active()
            || self.ripples_on(side)
            || self.text.active()
            || self.settings.vertical(side).is_some()
    }

    /// How long a drawing thread may wait for a new frame before redrawing the effect itself; None
    /// while nothing is playing.
    pub fn interval(&self, side: Side) -> Option<f64> {
        self.active(side).then_some(FRAME_SECONDS)
    }

    /// The panel's picture was replaced by something else (a game, sleep): drop what is playing.
    pub fn forget(&mut self, side: Side) {
        self.players[side.index()] = SidePlayer::new();
        if self.players.iter().all(|p| p.phase != Some(Phase::Drain)) {
            self.wide = None;
        }
    }

    /// The frame to send to the panel on this side. `new_frame` is false when the drawing thread is only
    /// redrawing its last frame to move an effect on.
    pub fn apply(&mut self, side: Side, grid: &Grid, new_frame: bool, now: f64) -> Grid {
        let i = side.index();
        // A vertical message takes the panel's place (and is what the other effects play over)
        let vertical = self.settings.vertical(side).map(|text| {
            let mut g = Grid::new();
            crate::apps::text::draw_text(
                &mut g,
                text,
                &self.settings.vertical_style,
                self.foreground.clamp(1, 255),
                now,
            );
            g
        });
        let new_frame = new_frame || vertical.is_some();
        let grid = vertical.as_ref().unwrap_or(grid);
        let transition = self.settings.transition();
        let base = Picture::from_grid(grid);
        let others_active = self.ripples_on(side) || self.text.active() || vertical.is_some();
        let other_base = self.players[1 - i].last_base.clone();
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

        if let (Some((old, start)), Some(look)) = (&player.slide, transition.look()) {
            // Pixel rain or the zipper, from the last frame of the old app to the new one
            match transition_frame(look, old, &out, now - start, self.foreground as f64) {
                Some(frame) => out = frame,
                None => player.slide = None,
            }
        } else if let (Some((old, start)), Transition::Fade) = (&player.slide, transition) {
            // Cross-fade from the last frame of the old app to the new one
            let k = ((now - start) / SidePlayer::FADE_SECONDS).clamp(0.0, 1.0);
            if k >= 1.0 {
                player.slide = None;
            } else {
                for (o, n) in out.data.iter_mut().zip(&old.data) {
                    *o = *n * (1.0 - k) + *o * k;
                }
            }
        } else if let Some((old, start)) = &player.slide {
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
            Some(Phase::Drain) if self.wide.is_some() => {
                let wide = self.wide.as_mut().unwrap();
                let start = if i == 0 { 0 } else { WIDTH + GAP };
                out = wide.at(now).columns(start, start + WIDTH);
                if wide.drain.drained {
                    self.wide = None;
                    for p in self.players.iter_mut() {
                        if p.phase == Some(Phase::Drain) {
                            p.phase = Some(Phase::Empty);
                            p.phase_since = now;
                        }
                    }
                }
            }
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
            Some(Phase::Out) => {
                let t = now - player.phase_since;
                let frame = player.look.as_ref().and_then(|p| {
                    let frame = p.look.out_frame(p.picture.as_ref()?, t, p.row, self.foreground as f64)?;
                    let frame = if p.flipped { frame.flipped() } else { frame };
                    Some(if p.wide { frame.columns(i * (WIDTH + GAP), i * (WIDTH + GAP) + WIDTH) } else { frame })
                });
                match frame {
                    Some(frame) => out = frame,
                    None => {
                        out = Picture::zeros(HEIGHT, WIDTH);
                        player.look = None;
                        player.phase = Some(Phase::Empty);
                        player.phase_since = now;
                    }
                }
            }
            Some(Phase::In) => {
                let t = now - player.phase_since;
                let frame = player.look.as_ref().and_then(|p| {
                    let start = i * (WIDTH + GAP);
                    let picture = if p.wide {
                        // This panel's picture and the other one's, side by side with the gap
                        let mut both = Picture::zeros(HEIGHT, 2 * WIDTH + GAP);
                        both.put_columns(start, &out);
                        if let Some(theirs) = &other_base {
                            both.put_columns((1 - i) * (WIDTH + GAP), theirs);
                        }
                        both
                    } else {
                        out.clone()
                    };
                    let picture = if p.flipped { picture.flipped() } else { picture };
                    let frame = p.look.in_frame(&picture, t, p.row, self.foreground as f64)?;
                    let frame = if p.flipped { frame.flipped() } else { frame };
                    Some(if p.wide { frame.columns(start, start + WIDTH) } else { frame })
                });
                match frame {
                    Some(frame) => out = frame,
                    None => {
                        player.look = None;
                        player.phase = None;
                    }
                }
            }
            Some(Phase::Empty) => {
                out = Picture::zeros(HEIGHT, WIDTH);
                if now - player.phase_since >= SidePlayer::EMPTY_SECONDS {
                    player.start_rise(now);
                }
            }
            Some(Phase::Rise) => match transition {
                Transition::Slide => {
                    let k = ((now - player.phase_since) / SidePlayer::RISE_SECONDS).min(1.0);
                    let shift = int_round(HEIGHT as f64 * (1.0 - k)).clamp(0, HEIGHT as i64) as usize;
                    let mut risen = Picture::zeros(HEIGHT, WIDTH);
                    risen.copy_rows(shift, &out, 0, HEIGHT - shift);
                    out = risen;
                    if k >= 1.0 {
                        player.phase = None;
                    }
                }
                Transition::Fade => {
                    let k = ((now - player.phase_since) / SidePlayer::FADE_SECONDS).clamp(0.0, 1.0);
                    out.data.iter_mut().for_each(|v| *v *= k);
                    if k >= 1.0 {
                        player.phase = None;
                    }
                }
                // Straight back to the picture, as when a new app comes on with sliding off
                Transition::Cut => player.phase = None,
                Transition::Rain | Transition::Zipper => {
                    let look = transition.look().unwrap();
                    let t = now - player.phase_since;
                    match return_frame(look, &out, t, self.foreground as f64) {
                        Some(frame) if t < return_seconds(look) => out = frame,
                        _ => player.phase = None,
                    }
                }
            },
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
                    for (r, row) in strip.iter().enumerate().take(HEIGHT) {
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

    fn filled(v: i32) -> Grid {
        let mut g = Grid::new();
        for col in g.v.iter_mut() {
            col.iter_mut().for_each(|p| *p = v);
        }
        g
    }

    fn with(slide: bool, fade: bool) -> Effects {
        let mut e = Effects::new();
        e.configure(EffectsSettings { slide, fade, ..EffectsSettings::default() });
        e.panel_present(Side::Left, true);
        e
    }

    #[test]
    fn fade_cross_fades_between_apps() {
        let mut e = with(false, true);
        e.apply(Side::Left, &filled(200), true, 0.0);
        e.start_slide(Side::Left, 0.0);
        let half = e.apply(Side::Left, &filled(0), true, SidePlayer::FADE_SECONDS / 2.0);
        assert_eq!(half.v[3][5], 100);
        let done = e.apply(Side::Left, &filled(0), true, SidePlayer::FADE_SECONDS);
        assert_eq!(done.v[3][5], 0);
        assert!(!e.player(Side::Left).active());
    }

    #[test]
    fn cut_skips_the_slide_and_comes_straight_back_after_unplug() {
        let mut e = with(false, false);
        e.apply(Side::Left, &filled(200), true, 0.0);
        e.start_slide(Side::Left, 0.0);
        assert!(!e.player(Side::Left).active());
        // After the drain and the dark moment, the picture is back whole at once
        e.players[0].phase = Some(Phase::Rise);
        e.players[0].phase_since = 1.0;
        let back = e.apply(Side::Left, &filled(200), true, 1.0);
        assert_eq!(back.v[0][HEIGHT - 1], 200);
        assert_eq!(back.v[0][0], 200);
        assert!(!e.player(Side::Left).active());
    }

    #[test]
    fn panels_together_drain_both_as_one_picture() {
        let mut e = Effects::new();
        e.configure(EffectsSettings { panels_together: true, ..EffectsSettings::default() });
        e.panel_present(Side::Left, true);
        e.panel_present(Side::Right, true);
        e.apply(Side::Left, &filled(200), true, 0.0);
        e.apply(Side::Right, &filled(200), true, 0.0);
        e.plug(Some("right-middle"), false, 0.0);
        assert_eq!(e.player(Side::Left).phase, Some(Phase::Drain));
        assert_eq!(e.player(Side::Right).phase, Some(Phase::Drain));
        let mut t = 0.0;
        while e.player(Side::Left).phase == Some(Phase::Drain) && t < 5.0 {
            t += 1.0 / 30.0;
            e.apply(Side::Left, &filled(200), true, t);
            e.apply(Side::Right, &filled(200), true, t);
        }
        // Both panels finish draining together, then come back
        assert_eq!(e.player(Side::Left).phase, Some(Phase::Empty));
        assert_eq!(e.player(Side::Right).phase, Some(Phase::Empty));
        assert!(e.wide.is_none());
    }

    #[test]
    fn pinned_message_lines_show_together_until_unpinned() {
        let mut t = TextLine::new();
        t.pin(&["HI".to_string(), "YO".to_string()]);
        assert!(t.active());
        let width = 2 * WIDTH + GAP;
        // Both lines at once, the second under the first
        t.strip(width, 0.0);
        let rows = t.strip(width, 0.6).unwrap();
        assert_eq!(rows.len(), 2 * TEXT_ROWS);
        assert!(rows[..TEXT_ROWS].iter().any(|r| r.contains(&1)));
        assert!(rows[TEXT_ROWS..].iter().any(|r| r.contains(&1)));
        t.say("SONG");
        t.clear();
        t.pin(&["HI".to_string(), "YO".to_string()]);
        let mut shown = 0;
        let mut now = 0.0;
        while now < 5.0 {
            if t.strip(width, now).is_some_and(|rows| rows.iter().any(|r| r.contains(&1))) {
                shown += 1;
            }
            now += 0.05;
        }
        assert!(shown > 50, "the pinned messages should keep showing");
        t.pin(&[]);
        assert!(!t.active());
        assert!(t.strip(width, now).is_none());

        let mut e = Effects::new();
        let on = serde_json::json!({"messages": ["  Back   soon ", "", "Coffee"], "show_messages": true});
        e.configure(EffectsSettings::from_json(&on));
        assert_eq!(e.settings.messages, vec!["Back soon", "Coffee"]);
        assert!(e.text.active());
        e.configure(EffectsSettings::from_json(&serde_json::json!({"messages": ["Back soon"]})));
        assert!(!e.text.active(), "kept but not showing");
        // The one-message test build's setting still loads
        let old = EffectsSettings::from_json(&serde_json::json!({"pinned_message": "Hi"}));
        assert_eq!((old.messages, old.show_messages), (vec!["Hi".to_string()], true));
    }

    #[test]
    fn plug_sides_and_screens_settings() {
        let s = EffectsSettings::from_json(&serde_json::json!({"plug_right": false, "plug_displays": false}));
        assert!(s.plug_side_on(Some("left-back")));
        assert!(!s.plug_side_on(Some("right-middle")));
        assert!(!s.plug_side_on(Some("right")));
        assert!(s.plug_side_on(None), "a port we can't place still plays");
        let saved = serde_json::to_value(&s).unwrap();
        assert_eq!(
            (saved["plug_right"].clone(), saved["plug_displays"].clone()),
            (Value::Bool(false), Value::Bool(false))
        );
        assert!(saved.get("plug_left").is_none(), "on is the default, so it isn't saved");
    }

    #[test]
    fn vertical_messages_take_each_panel() {
        let mut e = Effects::new();
        let on = serde_json::json!({"vertical_left": "HELLO", "vertical_right": "", "show_vertical": true, "vertical_style": "stacked"});
        e.configure(EffectsSettings::from_json(&on));
        e.panel_present(Side::Left, true);
        e.panel_present(Side::Right, true);
        assert!(e.active(Side::Left));
        let left = e.apply(Side::Left, &filled(0), true, 0.0);
        assert!(left.v.iter().flatten().any(|&v| v > 0), "the message is drawn");
        let right = e.apply(Side::Right, &filled(7), true, 0.0);
        assert_eq!(right.v[4][20], 7, "a panel without a message keeps its picture");
        let saved = serde_json::to_value(&e.settings).unwrap();
        assert_eq!(saved["vertical_style"], "stacked");
        assert!(saved.get("vertical_right").is_none());
        let off = EffectsSettings::from_json(&serde_json::json!({"vertical_left": "HI"}));
        assert_eq!(off.vertical(Side::Left), None, "kept but not showing");
        assert!(serde_json::to_value(EffectsSettings::default()).unwrap().get("vertical_style").is_none());
    }

    #[test]
    fn plug_looks_are_saved_only_when_picked_and_unknown_ones_are_ignored() {
        let default = serde_json::to_value(EffectsSettings::default()).unwrap();
        assert!(default.get("unplug_look").is_none() && default.get("plug_look").is_none());
        let s = EffectsSettings::from_json(&serde_json::json!({"unplug_look": "tv", "plug_look": "surge"}));
        assert_eq!((s.unplug(), s.plug_in()), (Some(Look::Tv), Some(Look::Surge)));
        assert_eq!(serde_json::to_value(&s).unwrap()["unplug_look"], "tv");
        // The zipper is an app transition now, not a plug animation
        let odd = EffectsSettings::from_json(&serde_json::json!({"unplug_look": "zipper", "plug_look": 3}));
        assert_eq!((odd.unplug_look.as_str(), odd.plug_look.as_str()), (DRAIN, RIPPLE));
        assert!(default.get("transition_look").is_none());
        let rain = EffectsSettings::from_json(&serde_json::json!({"slide": false, "transition_look": "rain"}));
        assert_eq!(rain.transition(), Transition::Rain);
        assert_eq!(
            EffectsSettings::from_json(&serde_json::json!({"transition_look": "tv"})).transition(),
            Transition::Slide
        );
    }

    #[test]
    fn a_picked_unplug_look_plays_then_the_picture_comes_back() {
        let mut e = Effects::new();
        e.configure(EffectsSettings { unplug_look: "glitch".into(), ..EffectsSettings::default() });
        e.panel_present(Side::Right, true);
        e.apply(Side::Right, &filled(160), true, 0.0);
        e.plug(Some("right-middle"), false, 0.0);
        assert_eq!(e.player(Side::Right).phase, Some(Phase::Out));
        let mut t = 0.0;
        while e.player(Side::Right).phase == Some(Phase::Out) && t < 5.0 {
            t += FRAME_SECONDS;
            e.apply(Side::Right, &filled(160), false, t);
        }
        assert_eq!(e.player(Side::Right).phase, Some(Phase::Empty));
        assert!((t - Look::Glitch.out_seconds()).abs() < 0.1);
        while e.player(Side::Right).active() && t < 10.0 {
            t += FRAME_SECONDS;
            e.apply(Side::Right, &filled(160), false, t);
        }
        assert_eq!(e.apply(Side::Right, &filled(160), false, t + 0.1), filled(160));
    }

    #[test]
    fn the_ripple_ring_runs_across_both_panels_from_the_port() {
        let mut e = Effects::new();
        e.configure(EffectsSettings { plug_look: "ring".into(), ..EffectsSettings::default() });
        e.panel_present(Side::Left, true);
        e.panel_present(Side::Right, true);
        e.apply(Side::Left, &filled(160), true, 0.0);
        e.apply(Side::Right, &filled(160), true, 0.0);
        e.plug(Some("right-back"), true, 0.0);
        assert_eq!(e.player(Side::Left).phase, Some(Phase::In));
        assert_eq!(e.player(Side::Right).phase, Some(Phase::In));
        // Early on only the right panel (the port's) shows anything; later the ring reaches the left
        let lit = |g: &Grid| g.v.iter().flatten().filter(|&&v| v > 0).count();
        assert_eq!(lit(&e.apply(Side::Left, &filled(160), true, 0.2)), 0);
        assert!(lit(&e.apply(Side::Right, &filled(160), true, 0.2)) > 0);
        assert!(lit(&e.apply(Side::Left, &filled(160), true, 0.7)) > 0);
        let end = Look::Ring.in_seconds() + 0.05;
        assert_eq!(e.apply(Side::Left, &filled(160), true, end), filled(160));
        assert_eq!(e.apply(Side::Right, &filled(160), true, end), filled(160));
        assert!(!e.player(Side::Left).active() && !e.player(Side::Right).active());
    }

    #[test]
    fn pixel_rain_and_zipper_change_apps() {
        for key in ["rain", "zipper"] {
            let mut e = Effects::new();
            e.configure(EffectsSettings { slide: false, transition_look: key.into(), ..EffectsSettings::default() });
            e.panel_present(Side::Left, true);
            e.apply(Side::Left, &filled(160), true, 0.0);
            e.start_slide(Side::Left, 0.0);
            assert!(e.player(Side::Left).active());
            let first = e.apply(Side::Left, &filled(0), true, 0.0);
            assert!(first.v.iter().flatten().filter(|&&v| v > 0).count() > 250, "{key}: starts on the old app");
            let mut t = 0.0;
            while e.player(Side::Left).active() && t < 3.0 {
                t += FRAME_SECONDS;
                e.apply(Side::Left, &filled(0), true, t);
            }
            assert!(t < 1.5, "{key} took {t} s");
            assert!(e.apply(Side::Left, &filled(0), true, t).is_dark());
        }
    }

    #[test]
    fn a_picked_plug_in_look_brings_the_picture_in_on_the_ports_side() {
        let mut e = Effects::new();
        e.configure(EffectsSettings { plug_look: "glitch".into(), ..EffectsSettings::default() });
        e.panel_present(Side::Left, true);
        e.panel_present(Side::Right, true);
        e.apply(Side::Left, &filled(160), true, 0.0);
        e.apply(Side::Right, &filled(160), true, 0.0);
        e.plug(Some("left-back"), true, 0.0);
        assert_eq!(e.player(Side::Left).phase, Some(Phase::In));
        assert_eq!(e.player(Side::Right).phase, None);
        // No water ripple with another look picked
        assert!(!e.ripples_on(Side::Right));
        let done = e.apply(Side::Left, &filled(160), true, Look::Glitch.in_seconds() + 0.05);
        assert_eq!(done, filled(160));
        assert!(!e.player(Side::Left).active());
    }

    #[test]
    fn fade_setting_round_trips_and_stays_out_of_python_settings() {
        let off = serde_json::to_value(EffectsSettings::default()).unwrap();
        assert!(off.get("fade").is_none());
        let on = EffectsSettings::from_json(&serde_json::json!({"slide": false, "fade": true}));
        assert_eq!(on.transition(), Transition::Fade);
        assert_eq!(serde_json::to_value(&on).unwrap()["fade"], true);
        // Slide wins when both are on
        assert_eq!(EffectsSettings::from_json(&serde_json::json!({"fade": true})).transition(), Transition::Slide);
    }

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
