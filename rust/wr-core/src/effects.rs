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
//! Where it sits: `wr-panel/src/overlay.rs` owns the one `Effects` value (behind a mutex) and calls
//! `apply` from each panel's drawing thread. The monitor feeds it events: `plug` from the plug-event
//! watcher, `say` for messages (charging, now playing, the control channel's "say"), `start_slide` from
//! the main loop when a panel changes app, and `configure` when settings.json changes.
//!
//! The main pieces, in file order: the `"effects"` settings (`EffectsSettings`); `Picture`, a float
//! picture that can be wider than one panel; the water `Ripple` played when something is plugged in;
//! the `Drain` played when something is unplugged; the `TextLine` along the top; and `Effects`, which
//! keeps one `SidePlayer` per panel and decides, frame by frame, what each panel shows.
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
/// Width of a text-line letter, in LEDs.
pub const FONT_W: usize = 3;
/// Height of a capital letter on the text line, in LEDs.
pub const FONT_H: usize = 5;
/// 5 rows of letters and a dark row under them.
pub const TEXT_ROWS: usize = 6;
/// Columns of keyboard between the two panels, for a ripple running from one panel to the other.
pub const GAP: usize = 4;

/// Which side of the laptop: a panel, or a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// The left panel, or a port on the left side of the laptop.
    Left,
    /// The right panel, or a port on the right side of the laptop.
    Right,
}

impl Side {
    /// In Python's `SIDES` order, which is also the order the panels sit in.
    pub const ALL: [Side; 2] = [Side::Left, Side::Right];

    /// 0 for left, 1 for right: the index into per-side arrays.
    pub fn index(self) -> usize {
        self as usize
    }

    /// The lowercase name used in settings and position texts.
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
    /// Near the hinge (the top of the panels).
    Back,
    /// Halfway along.
    Middle,
    /// Near the front edge (the bottom of the panels).
    Front,
}

impl Spot {
    /// Every spot, back to front.
    pub const ALL: [Spot; 3] = [Spot::Back, Spot::Middle, Spot::Front];

    /// The lowercase name used in settings and position texts.
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
///
/// Used to place a ripple or drain level with the real port. Each is 0-33; the Effects tab lets the user
/// move them, and `from_json` clamps whatever is saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PortRows {
    pub back: usize,
    pub middle: usize,
    pub front: usize,
}

impl Default for PortRows {
    /// Back row 5, middle row 16, front row 27 (Framework 16 port positions as measured for the Python app).
    fn default() -> Self {
        PortRows { back: 5, middle: 16, front: 27 }
    }
}

impl PortRows {
    /// The row for this spot.
    pub fn row(&self, spot: Spot) -> usize {
        match spot {
            Spot::Back => self.back,
            Spot::Middle => self.middle,
            Spot::Front => self.front,
        }
    }

    /// The row for this spot, to change it.
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
    /// Play the plug effects for the charger.
    pub plug_charger: bool,
    /// Play the plug effects for headphones.
    pub plug_headphones: bool,
    /// Play the plug effects for USB devices.
    pub plug_usb: bool,
    /// Screens plugged into an HDMI or DisplayPort card. Not in the Python app: only saved when off.
    #[serde(skip_serializing_if = "is_true")]
    pub plug_displays: bool,
    /// The ports on each side of the laptop (a port we can't place always plays). Only saved when off.
    #[serde(skip_serializing_if = "is_true")]
    pub plug_left: bool,
    /// As `plug_left`, for the right side.
    #[serde(skip_serializing_if = "is_true")]
    pub plug_right: bool,
    /// Which panel row each port position is level with.
    pub port_rows: PortRows,
    /// Ports the user corrected on the Effects tab: {USB location key or "charger": "left-back", ...}.
    pub learned_ports: BTreeMap<String, String>,
    /// The text line along the top of both panels, and what it tells.
    pub text_line: bool,
    /// Announce the song when it changes.
    pub text_song: bool,
    /// Announce charger plugged in / unplugged.
    pub text_charger: bool,
    /// Announce battery warnings.
    pub text_battery: bool,
    /// 1-10.
    pub text_speed: i64,
    /// Your own messages (one line each), and whether they are going round on the text line, taking
    /// turns, until turned off. Not in the Python app, so only saved when set.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<String>,
    /// Whether `messages` are going round now.
    #[serde(skip_serializing_if = "is_false")]
    pub show_messages: bool,
    /// How the lines of your message line up with each other: "left", "center" or "right". Only saved
    /// when not left.
    #[serde(skip_serializing_if = "is_left")]
    pub messages_align: String,
    /// A message of its own on each panel, running down the panel instead of what it shows, while
    /// `show_vertical` is on. `vertical_style`: "ticker", "sideways" or "stacked" (the Text app's
    /// styles). Not in the Python app, so only saved when set.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vertical_left: String,
    /// As `vertical_left`, for the right panel.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vertical_right: String,
    /// "ticker", "sideways" or "stacked"; only saved when not "ticker".
    #[serde(skip_serializing_if = "is_ticker")]
    pub vertical_style: String,
    /// Whether the vertical messages are showing now.
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
    /// The plug-in animation: "ripple" or a `Look` key (only saved when not "ripple").
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
    /// The defaults: every switch on except messages, vertical text, fade, panels together and full
    /// shades; text speed 5; the original drain and ripple; slide between apps.
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
            messages_align: "left".into(),
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
        // A key set to null counts as missing, as in Python's normalize.
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
        // Message text: runs of spaces become one space, at most 120 characters. `tidy` also drops it
        // when it ends up empty.
        let squeeze = |v: &Value| -> Option<String> {
            Some(v.as_str()?.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect())
        };
        let tidy = |v: &Value| squeeze(v).filter(|t| !t.is_empty());
        if let Some(Value::Array(list)) = get("messages") {
            // Empty lines stay (they show as a gap between lines), unless every line is empty
            s.messages = list.iter().filter_map(squeeze).take(Self::MAX_MESSAGES).collect();
            if s.messages.iter().all(String::is_empty) {
                s.messages.clear();
            }
        }
        if let Some(Value::String(align)) = get("messages_align") {
            if ["left", "center", "right"].contains(&align.as_str()) {
                s.messages_align = align.clone();
            }
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
        // A look key is only kept if it is one this version knows; otherwise the default stays, so an
        // old or mistyped value never breaks the effects.
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
        // Numbers are read like Python's `int()` (so "7", 7.9 and true all work) and then clamped.
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
                    // Only positions with a readable side are kept.
                    if parse_position(text).0.is_some() {
                        s.learned_ports.insert(key.clone(), text.clone());
                    }
                }
            }
        }
        s
    }
}

/// Reading settings never fails on odd values: it goes through `from_json`, which fixes them. It can
/// only fail if the input is not JSON at all.
impl<'de> Deserialize<'de> for EffectsSettings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(EffectsSettings::from_json(&Value::deserialize(deserializer)?))
    }
}

/// Serde helper: leave a flag out of settings.json while it is off (its default).
fn is_false(b: &bool) -> bool {
    !*b
}

/// Serde helper: leave a flag out of settings.json while it is on (its default).
fn is_true(b: &bool) -> bool {
    *b
}

/// Serde helper: leave `messages_align` out while it is "left".
fn is_left(s: &str) -> bool {
    s == "left"
}

/// Serde helper: leave `vertical_style` out while it is "ticker".
fn is_ticker(style: &String) -> bool {
    style == "ticker"
}

/// The original unplug animation's key.
pub const DRAIN: &str = "drain";
/// The original plug-in animation's key.
pub const RIPPLE: &str = "ripple";

/// Serde helper: leave `unplug_look` out while it is the drain.
fn is_drain(key: &String) -> bool {
    key == DRAIN
}

/// Serde helper: leave `plug_look` out while it is the ripple.
fn is_ripple(key: &String) -> bool {
    key == RIPPLE
}

/// How the panel goes from one app to the next (and comes back after the unplug drain).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    /// The new app slides up from the bottom, pushing the old one off the top.
    Slide,
    /// The old app cross-fades into the new one.
    Fade,
    /// The new app appears at once.
    Cut,
    /// The old app falls away as pixel rain while the new one floats up into place.
    Rain,
    /// The old app unzips and falls apart, then the new one zips up.
    Zipper,
}

impl Transition {
    /// The plug animation it borrows its frames from.
    ///
    /// `None` for slide, fade and cut, which are drawn here rather than by `plug_looks`.
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
    ///
    /// A position whose side cannot be read (unknown port) always plays.
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

    /// The app transition in force. A picked look (rain, zipper) wins; then slide; then fade; else cut.
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
///
/// So `0`, `""`, `[]`, `{}` and `null` are false.
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
/// Used for `text_speed` and `port_rows`; anything unreadable leaves the default.
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
///
/// Accepts what users type on the Effects tab and what the plug watcher saves ("Right Middle",
/// "left_back", "right"). Unknown words are ignored.
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
///
/// The opposite of `parse_position`; used when saving a port the user placed.
pub fn position_text(side: Option<Side>, spot: Option<Spot>) -> Option<String> {
    let side = side?;
    Some(match spot {
        Some(spot) => format!("{}-{}", side.name(), spot.name()),
        None => side.name().to_string(),
    })
}

/// "left-back" -> "Left back", for the Effects tab.
///
/// "Unknown port" when the side cannot be read; "Left side" when only the side is known.
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
///
/// Values are brightness 0-255 (but may go out of range while an effect works on them), stored as
/// `data[r * cols + c]`. Wide pictures are `2 * WIDTH + GAP` columns: left panel, keyboard gap, right panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    /// Height (always `HEIGHT` for the effects).
    pub rows: usize,
    /// Width: 9 for one panel, wider for a picture across both.
    pub cols: usize,
    /// `rows * cols` values, row by row.
    pub data: Vec<f64>,
}

impl Picture {
    /// An all-dark picture.
    pub fn zeros(rows: usize, cols: usize) -> Picture {
        Picture { rows, cols, data: vec![0.0; rows * cols] }
    }

    /// A panel's grid as (rows, columns): Python's `np.asarray(grid, dtype=float).T`.
    ///
    /// The grid is stored by column, a `Picture` by row, so this swaps the axes.
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
    ///
    /// Values are rounded half to even (numpy's `rint`) and clamped to 0-255; anything outside the
    /// first 9 columns and 34 rows is dropped.
    pub fn to_grid(&self) -> Grid {
        let mut g = Grid::new();
        for y in 0..self.rows.min(HEIGHT) {
            for x in 0..self.cols.min(WIDTH) {
                g.v[x][y] = round_half_even(self.at(y, x)).clamp(0.0, 255.0) as i32;
            }
        }
        g
    }

    /// The value at row `r`, column `c`. Panics outside the picture.
    pub fn at(&self, r: usize, c: usize) -> f64 {
        self.data[r * self.cols + c]
    }

    /// Sets row `r`, column `c`. Panics outside the picture.
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
    /// Used to cut one panel's part out of a wide picture. Panics if `b < a` or `b > cols`.
    pub fn columns(&self, a: usize, b: usize) -> Picture {
        let mut out = Picture::zeros(self.rows, b - a);
        for r in 0..self.rows {
            out.data[r * (b - a)..(r + 1) * (b - a)].copy_from_slice(&self.data[r * self.cols + a..r * self.cols + b]);
        }
        out
    }

    /// `p[:, a:a + other.cols] = other`.
    /// Pastes a narrower picture (same height) in at column `a`, e.g. a panel into a wide picture.
    fn put_columns(&mut self, a: usize, other: &Picture) {
        for r in 0..self.rows {
            self.data[r * self.cols + a..r * self.cols + a + other.cols]
                .copy_from_slice(&other.data[r * other.cols..(r + 1) * other.cols]);
        }
    }

    /// Rows src..src+n of `from` into rows dst..dst+n of this picture (same width).
    /// Panics if either range runs past its picture. Used by the slide transitions.
    fn copy_rows(&mut self, dst: usize, from: &Picture, src: usize, n: usize) {
        let c = self.cols;
        self.data[dst * c..(dst + n) * c].copy_from_slice(&from.data[src * c..(src + n) * c]);
    }
}

/// The 5-point Laplacian of a (rows, columns) field, the edges repeated outwards (so waves reflect):
/// `p[:-2, 1:-1] + p[2:, 1:-1] + p[1:-1, :-2] + p[1:-1, 2:] - 4 * f` on the edge-padded field.
///
/// In plain words: for each cell, how much higher its four neighbours are on average than the cell
/// (times 4). It is the "curvature" that drives the wave in `Ripple::advance`.
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
///
/// Used to add up the drain's glow so the result matches Python to the last bit. Blocks of 8 are summed
/// in 8 running totals, longer arrays are split in half recursively, exactly as numpy's C code does.
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
///
/// The physics: each fine cell has a height `h` and speed `v`. Every 1/240 s the acceleration is
/// `SPEED^2 x laplacian(h) - DAMP x v - SPRING x h`, plus a push at the jack while it is still driving.
/// That is the damped wave equation with a spring pulling the water back flat. Then the heights are
/// averaged per pixel and used to shift where each pixel is read from (`apply`).
#[derive(Clone, Debug)]
pub struct Ripple {
    /// Size of the picture the ripple runs over, in pixels (columns): one panel or several plus gaps.
    pub width: usize,
    /// Rows (always `HEIGHT`).
    pub height: usize,
    /// Where the jack's drive goes in, on the fine grid.
    splash: Vec<f64>,
    /// Which way, and how far, a pixel moves for a unit of wave (pixel grid).
    ox: Vec<f64>,
    /// See `ox`: the row part of the same direction.
    oy: Vec<f64>,
    /// The water's height and speed (fine grid).
    h: Vec<f64>,
    /// The water's vertical speed per fine cell.
    v: Vec<f64>,
    /// Time into the drive; None once the jack stopped pushing.
    t: Option<f64>,
    /// Seconds since the ripple started; it stops at `MAX_SECONDS` regardless.
    age: f64,
    /// Time given to `advance` but not yet stepped (less than one step).
    carry: f64,
    /// False once the water has calmed down (then `apply` returns the picture unchanged).
    active: bool,
    /// The `now` of the last `advance_to`, so two panels asking at the same moment do not step twice.
    last: Option<f64>,
}

impl Ripple {
    /// Fine cells per pixel in each direction.
    const S: usize = 3;
    /// Simulation step, seconds. With SPEED 120 a wave moves half a fine cell per step, inside the 0.707
    /// limit where this kind of 2-D simulation stays stable.
    const STEP: f64 = 1.0 / 240.0;
    /// Wave speed, fine cells per second (40 pixels a second).
    const SPEED: f64 = 120.0;
    /// Friction on the water's speed, per second.
    const DAMP: f64 = 5.0;
    /// Pull back to flat, per second squared.
    const SPRING: f64 = 6.25;
    /// The jack's push goes up and down 3.4 times a second, for one period only.
    const FREQ: f64 = 3.4;
    /// Strength of the push.
    const DRIVE: f64 = 5000.0;
    /// How far a unit of water height moves the picture (before the cap).
    const PUSH: f64 = 10.0;
    /// The most the picture can be moved, in fine cells (4 pixels), applied softly with `tanh`.
    const PUSH_MAX: f64 = 12.0;
    /// A ripple never runs longer than this, even if the water has not calmed.
    const MAX_SECONDS: f64 = 3.0;

    /// A new ripple about to start. `jack_x` / `jack_y` are in pixels from the top-left of the picture
    /// (for a port at the right edge, `jack_x` is `width`); `width` / `height` are the picture size.
    pub fn new(jack_x: f64, jack_y: f64, width: usize, height: usize) -> Ripple {
        let s = Self::S;
        // The jack's position on the fine grid (the -0.5 centres it on the fine cells).
        let (fine_rows, fine_cols) = (height * s, width * s);
        let jx = jack_x * s as f64 - 0.5;
        let jy = jack_y * s as f64 - 0.5;
        let mut splash = Vec::with_capacity(fine_rows * fine_cols);
        for y in 0..fine_rows {
            for x in 0..fine_cols {
                let dx = x as f64 - jx;
                let dy = y as f64 - jy;
                // A soft round bump (Gaussian, about 5 fine cells across) centred on the jack.
                splash.push((-(dx * dx + dy * dy) / (2.0 * 25.0)).exp());
            }
        }
        let (mut ox, mut oy) = (Vec::with_capacity(height * width), Vec::with_capacity(height * width));
        for py in 0..height {
            for px in 0..width {
                let dx = px as f64 + 0.5 - jack_x;
                let dy = py as f64 + 0.5 - jack_y;
                let r = dx.hypot(dy);
                // A unit vector pointing away from the jack, shortened for pixels very close to it so the
                // jack itself does not tear the picture apart.
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

    /// Whether the ripple is still moving the picture.
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
        // A long gap (the thread stalled, the PC slept) only advances 0.1 s, so the water cannot explode.
        let dt = dt.clamp(0.0, 0.1);
        self.age += dt;
        self.carry += dt;
        let (rows, cols) = (self.height * Self::S, self.width * Self::S);
        while self.carry >= Self::STEP {
            self.carry -= Self::STEP;
            // The curvature of the water surface, computed from the heights at the start of the step.
            let lap = laplacian(&self.h, rows, cols);
            let drive = self.t.map(|t| {
                // The push swells in over about 0.04 s and dies away over about 0.2 s.
                let env = (1.0 - (-t / 0.04).exp()) * (-t / 0.2).exp();
                Self::DRIVE * env * (2.0 * std::f64::consts::PI * Self::FREQ * t).sin()
            });
            for (i, lap) in lap.iter().enumerate() {
                let mut a = Self::SPEED * Self::SPEED * lap - Self::DAMP * self.v[i] - Self::SPRING * self.h[i];
                if let Some(d) = drive {
                    a += d * self.splash[i];
                }
                // Speed first, then height from the new speed (semi-implicit Euler, which keeps
                // the wave stable).
                self.v[i] += Self::STEP * a;
                self.h[i] += Self::STEP * self.v[i];
            }
            // The jack pushes for one full period of FREQ (about 0.29 s), then lets the water run free.
            if let Some(t) = self.t {
                let t = t + Self::STEP;
                self.t = if t > 1.0 / Self::FREQ { None } else { Some(t) };
            }
        }
        let max_abs = |a: &[f64]| a.iter().fold(f64::NEG_INFINITY, |m, x| m.max(x.abs()));
        // Done when the jack has stopped and the water is almost flat and still.
        let quiet = max_abs(&self.h) < 0.05 && max_abs(&self.v) < 6.0;
        if (self.t.is_none() && quiet) || self.age > Self::MAX_SECONDS {
            self.active = false;
        }
    }

    /// The picture (`height` x `width`) as seen through the water.
    ///
    /// Each output pixel shows the input pixel a little closer to or further from the jack, depending on
    /// the water height there. `pixels` must be `height` x `width`. Returns a copy once inactive.
    pub fn apply(&self, pixels: &Picture) -> Picture {
        if !self.active {
            return pixels.clone();
        }
        let s = Self::S;
        let fine_cols = self.width * s;
        let mut out = Picture::zeros(self.height, self.width);
        // Python's src(): reflect off the far edge, round to the nearest pixel, clip
        // `along` is this pixel's position, `moved` how far the water shifts it towards the jack.
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
                // The average water height over this pixel's 3 x 3 fine cells.
                let hp = (rows[0] + rows[1] + rows[2]) / 9.0;
                // `tanh` caps the shift smoothly at PUSH_MAX; dividing by S turns fine cells into pixels.
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
///
/// Positions are in pixels (0.0 to width / height), speeds in pixels per second.
#[derive(Clone, Copy, Debug)]
struct Particle {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    /// The brightness this particle carries (a quarter of its pixel's value).
    light: f64,
    /// How slowly it takes up the flow's speed.
    lag: f64,
    /// How strongly (and which way, -1 to 1) it circles around the hole.
    swirl: f64,
}

/// The picture as particles, sucked out through a hole in the right-hand wall at row jack_y.
/// For a port on the left, the caller flips the picture.
///
/// Each lit pixel becomes four particles carrying a quarter of its light. Every frame they are pulled
/// towards the hole, faster as time goes on, and drawn back onto pixels; trails fade behind them and the
/// hole glows as they leave. Seeded (seed 11) so it plays exactly as Python's does.
#[derive(Clone, Debug)]
pub struct Drain {
    pub width: usize,
    pub height: usize,
    /// The hole, in pixels: at the right-hand edge (x = width), row `jack_y`.
    jack: (f64, f64),
    /// Height of the hole, in rows.
    opening: f64,
    /// The particles still inside.
    particles: Vec<Particle>,
    /// The last picture, fading, so moving particles leave short streaks.
    trail: Vec<f64>,
    /// Seconds since the drain started.
    time: f64,
    /// Brightness of the glow at the hole; grows as particles leave and fades away.
    glow: f64,
    /// True once nothing visible is left; the caller then moves on to the dark pause.
    pub drained: bool,
}

impl Drain {
    /// Each pixel is split 2 x 2 into four particles.
    const SUB: usize = 2;
    /// After this many seconds any particles left are taken out at once, so the drain always ends.
    const GIVE_UP: f64 = 1.6;

    /// Python's `Drain(pixels, jack_y, opening=2.0, seed=11)`: each lit pixel becomes 4 particles.
    pub fn new(pixels: &Picture, jack_y: f64, opening: f64, seed: u64) -> Drain {
        // Collect the lit pixels row by row; dark pixels make no particles.
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
        // The random draws are made in the same order as Python's (all x jitters, then all y, ...) so the
        // numbers match.
        let jitter_x = rng.uniform_n(-0.2, 0.2, count);
        let jitter_y = rng.uniform_n(-0.2, 0.2, count);
        let lag = rng.uniform_n(0.04, 0.12, count);
        let swirl = rng.uniform_n(-1.0, 1.0, count);
        // Particle `i` comes from lit pixel `i / 4`, starting at its centre plus a small random offset.
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
        // The pull starts gently (over about 0.08 s) and keeps getting stronger, so the picture holds a
        // moment and then rushes out.
        let drive = (1.0 - (-time / 0.08).exp()) * (1.0 + 3.0 * time);
        let (w, h) = (self.width as f64, self.height as f64);
        let give_up = time > Self::GIVE_UP;
        let mut gone = Vec::new();
        let mut kept = Vec::with_capacity(self.particles.len());
        for p in &self.particles {
            let mut p = *p;
            let dx = jx - p.x;
            let dy = jy - p.y;
            // (The tiny addition avoids dividing by zero for a particle exactly at the hole.)
            let r = dx.hypot(dy) + 1e-6;
            // Faster close to the hole, like water near a plughole.
            let speed = drive * (25.0 + 160.0 / (r + 2.0));
            // At right angles to the pull: the direction of the swirl.
            let (tx, ty) = (-dy / r, dx / r);
            let sw = p.swirl * 18.0 * (time * 9.0 + p.y * 0.7).sin() * (r / 6.0).min(1.0);
            let fx = dx / r * speed + tx * sw;
            let fy = dy / r * speed + ty * sw;
            // Each particle eases towards the flow's speed with its own lag, so they do not all move alike.
            let k = 1.0 - (-dt / p.lag).exp();
            p.vx += (fx - p.vx) * k;
            p.vy += (fy - p.vy) * k;
            // Particles stay inside the picture; they can only leave through the hole.
            p.x = (p.x + p.vx * dt).clamp(0.0, w - 1e-6);
            p.y = (p.y + p.vy * dt).clamp(0.0, h - 1e-6);
            // Distance from the hole's opening (zero anywhere along its height), used to see if it is out.
            let beside = ((p.y - jy).abs() - self.opening / 2.0).max(0.0);
            if give_up || (p.x - jx).hypot(beside) < 0.9 {
                gone.push(p.light);
            } else {
                kept.push(p);
            }
        }
        // 2% of the light that just left lights up the hole.
        self.glow += pairwise_sum(&gone) * 0.02;
        self.particles = kept;
    }

    /// The picture now: each particle spread over the 4 pixels around it, fading trails, and the glow
    /// at the hole. Sets `drained` once nothing is left to see.
    ///
    /// `dt` is the time since the last picture, for the fading. Values are clamped to 0-255.
    pub fn pixels(&mut self, dt: f64) -> Picture {
        let (w, h) = (self.width, self.height);
        let mut f = vec![0.0; h * w];
        if !self.particles.is_empty() {
            // numpy's four bincounts, each summed on its own and then added in turn
            let mut parts = [vec![0.0; h * w], vec![0.0; h * w], vec![0.0; h * w], vec![0.0; h * w]];
            for p in &self.particles {
                // Bilinear splat: the particle's light is shared between the four pixels around it,
                // more to the nearer ones. The clamp keeps `x0 + 1` and `y0 + 1` inside the picture.
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
            // Adding the four parts one after another, as numpy does, keeps the rounding identical.
            for part in &parts {
                for (a, b) in f.iter_mut().zip(part) {
                    *a += b;
                }
            }
        }
        // Trails fade with a time constant of 0.06 s, the glow with 0.12 s.
        let fade = (-dt.max(0.0) / 0.06).exp();
        for (t, &v) in self.trail.iter_mut().zip(&f) {
            // A pixel shows its new light or its faded old light, whichever is brighter.
            *t = v.max(*t * fade);
        }
        self.glow *= (-dt.max(0.0) / 0.12).exp();
        let mut out = Picture { rows: h, cols: w, data: self.trail.clone() };
        let jy = self.jack.1.max(0.0).min((h - 1) as f64) as usize;
        // The glow is drawn in the last column, on the hole's row and (at half strength) the rows
        // either side.
        for yy in jy.saturating_sub(1)..(jy + 2).min(h) {
            let at = yy * w + w - 1;
            out.data[at] += self.glow * if yy == jy { 1.0 } else { 0.5 };
        }
        // Finished once every particle has left and what is left of the trails and glow is too dim
        // to show (below 2 of 255).
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
/// Each entry is 15 characters, '1' lit, 3 per row from the top.
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

/// Small letters, 3 x 6: the bottom row is for the tails of g, j, p, q and y.
/// Each entry is 18 characters, 3 per row from the top.
const LOWER: &[(char, &str)] = &[
    ('a', "000110011101111000"),
    ('b', "100100110101110000"),
    ('c', "000000011100011000"),
    ('d', "001001011101011000"),
    ('e', "000010101110011000"),
    ('f', "011100111100100000"),
    ('g', "000000011101011110"),
    ('h', "100100110101101000"),
    ('i', "010000110010111000"),
    ('j', "001000001001101010"),
    ('k', "100100101110101000"),
    ('l', "110010010010111000"),
    ('m', "000000110111101000"),
    ('n', "000000110101101000"),
    ('o', "000000111101111000"),
    ('p', "000000110101110100"),
    ('q', "000000011101011001"),
    ('r', "000000011100100000"),
    ('s', "000000011010110000"),
    ('t', "010010111010011000"),
    ('u', "000000101101011000"),
    ('v', "000000101101010000"),
    ('w', "000000101111111000"),
    ('x', "000000101010101000"),
    ('y', "000000101101011110"),
    ('z', "000000111010111000"),
];

/// A letter's bits row by row, and its width (3 x 5 capitals, 3 x 6 small letters); letters we
/// don't have are drawn as their capital, else '?'.
fn glyph(ch: char) -> &'static str {
    if let Some((_, g)) = LOWER.iter().find(|(k, _)| *k == ch) {
        return g;
    }
    // e.g. 'é' becomes 'É'; if that is not in the font either, '?' is drawn. The final `unwrap` cannot
    // fail: '?' is in the font.
    let ch = ch.to_uppercase().next().unwrap_or(ch);
    let find = |c: char| FONT.iter().find(|(k, _)| *k == c).map(|(_, g)| *g);
    find(ch).or_else(|| find('?')).unwrap()
}

/// One column of the text line, top row first (the last row only has the tails of small letters).
///
/// Values are 0 (off) or 1 (lit).
pub type TextColumn = [u8; TEXT_ROWS];

/// The text as columns of 0/1, one dark column between letters. Small letters stay small.
pub fn text_columns(text: &str) -> Vec<TextColumn> {
    let mut cols = Vec::new();
    for ch in text.chars() {
        if !cols.is_empty() {
            cols.push([0; TEXT_ROWS]);
        }
        let bits = glyph(ch).as_bytes();
        for c in 0..FONT_W {
            let mut col = [0; TEXT_ROWS];
            // Capitals have 5 rows and leave the sixth dark; small letters fill all 6.
            for (r, v) in col.iter_mut().enumerate().take(bits.len() / FONT_W) {
                *v = (bits[r * FONT_W + c] == b'1') as u8;
            }
            cols.push(col);
        }
    }
    cols
}

/// The width of `text` on the text line, in columns (3 per letter plus 1 between letters).
pub fn text_width(text: &str) -> usize {
    text_columns(text).len()
}

/// Messages scrolling right to left along the top rows of both panels, one after another.
#[derive(Clone, Debug)]
pub struct TextLine {
    /// Messages waiting their turn, oldest first (at most `QUEUE`).
    queue: VecDeque<String>,
    /// The message showing (one or more lines, one under the other), and when it started.
    current: Option<(Vec<Vec<TextColumn>>, f64)>,
    /// Columns a second.
    pub speed: f64,
    /// Your own message, its lines one under the other, shown again and again whenever nothing else is
    /// waiting, until unpinned. Empty lines show as a gap.
    pinned: Vec<String>,
    /// How shorter lines of a message line up with the longest: 0 left, 0.5 center, 1 right.
    align: f64,
}

impl Default for TextLine {
    fn default() -> Self {
        TextLine::new()
    }
}

impl TextLine {
    /// Up to 8 messages wait; more push the oldest out.
    const QUEUE: usize = 8;

    /// An idle line at 18 columns a second (speed setting 4) until `set_speed` is called.
    pub fn new() -> TextLine {
        TextLine { queue: VecDeque::new(), current: None, speed: 18.0, pinned: Vec::new(), align: 0.0 }
    }

    /// Queues a message in capitals, its spaces squeezed to one and cut to 120 letters.
    ///
    /// Empty text is ignored; with 8 waiting the oldest is dropped.
    pub fn say(&mut self, text: &str) {
        let text: String =
            text.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase().chars().take(120).collect();
        if !text.is_empty() {
            if self.queue.len() == Self::QUEUE {
                self.queue.pop_front();
            }
            self.queue.push_back(text);
        }
    }

    /// The speed setting, 1-10.
    ///
    /// 1 is 9 columns a second, 10 is 36; out-of-range settings are clamped.
    pub fn set_speed(&mut self, setting: i64) {
        self.speed = 6.0 + 3.0 * setting.clamp(1, 10) as f64;
    }

    /// "left", "center" or "right": how the lines of your message line up.
    pub fn set_align(&mut self, align: &str) {
        self.align = match align {
            "center" => 0.5,
            "right" => 1.0,
            _ => 0.0,
        };
    }

    /// Keeps this message (its lines one under the other, as typed, empty ones as a gap) going round
    /// until it is unpinned (an empty list, or only empty lines).
    ///
    /// Each line has its runs of spaces squeezed; at most `EffectsSettings::MAX_MESSAGES` lines show.
    pub fn pin(&mut self, texts: &[String]) {
        let mut texts: Vec<String> = texts.iter().map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
        if texts.iter().all(String::is_empty) {
            texts.clear();
        }
        // The same message again (settings are applied every time they are saved): keep it running
        // where it is rather than restarting it.
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
    /// Queued messages come first, so announcements interrupt the pinned message between its rounds.
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
    ///
    /// `width` is the width of all panels side by side (9 per panel; the keyboard gap is not counted, so
    /// text jumps straight from one panel to the next). The message enters at the right edge and moves
    /// left at `speed` columns a second, measured from `now` when it started.
    pub fn strip(&mut self, width: usize, now: f64) -> Option<Vec<Vec<u8>>> {
        if self.current.is_none() {
            let next = self.next()?;
            self.current = Some((next, now));
        }
        // Cannot fail: `current` was set just above if it was empty.
        let (lines, start) = self.current.as_ref().unwrap();
        let mut longest = lines.iter().map(Vec::len).max().unwrap_or(0);
        // The message's left edge, in columns from the left of the strip.
        let mut x = width as f64 - (now - start) * self.speed;
        if x + (longest as f64) < 0.0 {
            // Gone off the left edge: the next message, if any
            self.current = None;
            let next = self.next()?;
            self.current = Some((next, now));
            x = width as f64;
            longest = self.current.as_ref().unwrap().0.iter().map(Vec::len).max().unwrap_or(0);
        }
        // One block of TEXT_ROWS rows per message line, all scrolling at the same `x`.
        let lines = &self.current.as_ref().unwrap().0;
        let mut out = vec![vec![0u8; width]; TEXT_ROWS * lines.len()];
        let xi = int_round(x);
        for (n, cols) in lines.iter().enumerate() {
            // Shorter lines are shifted right by part of the difference: none (left), half (centre) or all
            // (right).
            let indent = int_round((longest - cols.len()) as f64 * self.align);
            for (c, col) in cols.iter().enumerate() {
                let at = xi + indent + c as i64;
                if (0..width as i64).contains(&at) {
                    for (r, &v) in col.iter().enumerate() {
                        out[n * TEXT_ROWS + r][at as usize] = v;
                    }
                }
            }
        }
        Some(out)
    }

    /// True while something is showing or waiting, or a message is pinned.
    pub fn active(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty() || !self.pinned.is_empty()
    }
}

/// Where a panel's 9 columns are in the picture spanning all panels (with the keyboard between);
/// None when the side isn't in `order`.
///
/// Panels are `WIDTH + GAP` apart. For example with both panels the right one is columns 13-21.
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
    /// What the plug effects are doing on this panel; `None` while nothing is.
    pub phase: Option<Phase>,
    /// When the current phase started (the caller's clock, seconds).
    pub phase_since: f64,
    /// The frame sliding away, and when it started.
    pub slide: Option<(Picture, f64)>,
    /// The last frame from the monitor.
    pub last_base: Option<Picture>,
    /// When `apply` last ran for this panel, to work out the time between frames.
    pub last_time: Option<f64>,
}

impl SidePlayer {
    /// How long the slide between apps takes.
    const SLIDE_SECONDS: f64 = 0.45;
    /// How long a cross-fade (or fade-in) takes.
    const FADE_SECONDS: f64 = 0.4;
    /// The dark pause after the picture has gone.
    const EMPTY_SECONDS: f64 = 0.2;
    /// How long the picture takes to slide back up.
    const RISE_SECONDS: f64 = 0.25;

    /// Nothing playing.
    pub fn new() -> SidePlayer {
        SidePlayer::default()
    }

    /// Python's player also has a list of its own ripples, which is always empty (ripples span the
    /// panels and belong to [`Effects`]), so it is left out here.
    pub fn active(&self) -> bool {
        self.phase.is_some() || self.slide.is_some()
    }

    /// Ends any drain or look and starts bringing the picture back.
    fn start_rise(&mut self, now: f64) {
        self.drain = None;
        self.look = None;
        self.phase = Some(Phase::Rise);
        self.phase_since = now;
    }
}

/// All the effects, for both panels. One per process, shared by the drawing threads.
///
/// Not thread-safe by itself: the panel crate wraps it in a mutex, so a drawing thread holds the lock for
/// the whole of an `apply` (a few milliseconds at most).
#[derive(Clone, Debug)]
pub struct Effects {
    /// The settings as last given to `configure`.
    pub settings: EffectsSettings,
    /// Each panel's own effects, indexed by `Side::index`.
    players: [SidePlayer; 2],
    /// The text line, shared by both panels (it runs across them).
    pub text: TextLine,
    /// Sides with a panel drawing now.
    panels: [bool; 2],
    /// The text line's brightness.
    pub foreground: i32,
    /// Ripples across all panels at once, each with the panel order it was made for.
    ripples: Vec<(Ripple, Vec<Side>)>,
    /// One drain across both panels (`panels_together`), shared by the two players.
    wide: Option<WideDrain>,
    /// A panel just came on: bring its first picture in with the app transition (not dropped by
    /// `forget`, so swapping the panels round at the start keeps it).
    intro: [bool; 2],
}

/// The unplug drain over both panels side by side: left panel, the gap, right panel.
#[derive(Clone, Debug)]
struct WideDrain {
    /// The drain over the wide picture.
    drain: Drain,
    /// The port is on the left, so the drain runs on the flipped picture.
    flipped: bool,
    /// When it last moved on, and the picture it made then (both panels ask for it each frame).
    last: Option<f64>,
    picture: Picture,
}

impl WideDrain {
    /// The whole picture at `now`, moving the drain on only once per moment.
    ///
    /// The first panel to ask at a new moment advances the drain (by at most 0.1 s); the other gets the
    /// same picture, so the two halves stay in step.
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
    /// Same as `Effects::new()`.
    fn default() -> Self {
        Effects::new()
    }
}

impl Effects {
    /// No panels known yet, default settings, foreground 160 (full brightness).
    pub fn new() -> Effects {
        Effects {
            settings: EffectsSettings::default(),
            players: [SidePlayer::new(), SidePlayer::new()],
            text: TextLine::new(),
            panels: [false; 2],
            foreground: 160,
            ripples: Vec::new(),
            wide: None,
            intro: [false; 2],
        }
    }

    // Settings and panels

    /// New settings. Note the text line keeps its first speed (18 columns a second) until this is called.
    ///
    /// Called when settings.json changes. Turning the text line off drops what it was showing; the pinned
    /// message follows `text_line && show_messages`.
    pub fn configure(&mut self, settings: EffectsSettings) {
        self.settings = settings;
        self.text.set_speed(self.settings.text_speed);
        self.text.set_align(&self.settings.messages_align);
        if !self.settings.text_line {
            self.text.clear();
        }
        let on = self.settings.text_line && self.settings.show_messages;
        self.text.pin(if on { &self.settings.messages } else { &[] });
    }

    /// Records whether a panel is connected and drawing on this side. The ripple and text line use it to
    /// know how wide "all panels" is. A panel going away forgets its pending intro.
    pub fn panel_present(&mut self, side: Side, present: bool) {
        self.panels[side.index()] = present;
        if !present {
            self.intro[side.index()] = false;
        }
    }

    /// Whether a panel is drawing on this side.
    pub fn present(&self, side: Side) -> bool {
        self.panels[side.index()]
    }

    /// The panel on this side just came on (White Rabbit starting, or the panel connecting again):
    /// its first picture comes in with the chosen app transition instead of cutting on.
    pub fn start_intro(&mut self, side: Side) {
        self.intro[side.index()] = self.settings.transition() != Transition::Cut;
    }

    /// This side's player (read-only; used by tests and the monitor's checks).
    pub fn player(&self, side: Side) -> &SidePlayer {
        &self.players[side.index()]
    }

    /// The panels drawing now, left first; when there are none, `fallback`.
    ///
    /// With only one panel the "picture across all panels" is just that panel.
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
    ///
    /// A port on the left side belongs to the leftmost panel, a port on the right to the rightmost (with
    /// one panel, both to it). `None` for a port whose side is unknown.
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
    ///
    /// Plugging in plays the chosen plug-in look on the port's panel (both panels for a look that spans
    /// them), or by default starts a water ripple across all panels from the port, also cutting short a
    /// drain still playing there. Unplugging plays the chosen unplug look, or by default the drain (on
    /// one panel, or across both with `panels_together`). Needs the panel's last picture to unplug; with
    /// no picture yet nothing plays. `now` is the caller's clock in seconds.
    pub fn plug(&mut self, position: Option<&str>, plugged_in: bool, now: f64) {
        if !self.settings.plug_animation {
            return;
        }
        // Work out which panel the port belongs to, the row it is level with, and whether the port is at
        // that panel's left edge (then the pictures are flipped so the looks can assume a right edge).
        let (side, spot) = position.map(parse_position).unwrap_or((None, None));
        // A port without a known spot is taken to be in the middle.
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
            // The ripple runs over all panels with the keyboard gap between them, so it crosses over.
            let wide = order.len() * WIDTH + (order.len() - 1) * GAP;
            let (jack_x, jack_y) = match panel {
                None => (wide as f64 / 2.0, HEIGHT as f64 / 2.0),
                Some(panel) => {
                    // A drain or a look still playing near this port is cut short: the picture starts
                    // coming back while the ripple runs over it.
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
            // At most three ripples at once (each costs a fair amount of work per frame).
            if self.ripples.len() > 3 {
                self.ripples.drain(..self.ripples.len() - 3);
            }
            return;
        }
        // Unplugging: nothing plays for a port whose side is unknown.
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
            // No picture yet on this panel: nothing to take away.
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
                // Python's settings: a 2-row hole and seed 11.
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
    ///
    /// Messages are shown in capitals and queued behind any already waiting.
    pub fn say(&mut self, text: &str) {
        if self.settings.text_line {
            self.text.say(text);
        }
    }

    /// The panel on this side is changing app: slide the new one in over the last frame.
    ///
    /// Called by the monitor's main loop just before it sends the new app's first frame. Nothing happens
    /// with the "cut" transition, while a plug effect is playing, or before the panel has had a picture.
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

    /// Whether a ripple that covers this side is still moving.
    fn ripples_on(&self, side: Side) -> bool {
        self.ripples.iter().any(|(r, order)| order.contains(&side) && r.is_active())
    }

    /// Whether anything is moving on this side, so the drawing thread should keep redrawing.
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
    ///
    /// A drain across both panels is kept while the other panel is still draining.
    pub fn forget(&mut self, side: Side) {
        self.players[side.index()] = SidePlayer::new();
        if self.players.iter().all(|p| p.phase != Some(Phase::Drain)) {
            self.wide = None;
        }
    }

    /// The frame to send to the panel on this side. `new_frame` is false when the drawing thread is only
    /// redrawing its last frame to move an effect on.
    ///
    /// Order of work: a vertical message replaces the picture; the app transition (slide, fade, rain,
    /// zipper) plays; then the plug phase (drain, look, dark pause, coming back) overrides it; a ripple
    /// distorts the result; and the text line is drawn over the top rows last. When nothing plays the
    /// grid is returned unchanged. `now` is the caller's monotonic clock in seconds.
    pub fn apply(&mut self, side: Side, grid: &Grid, new_frame: bool, now: f64) -> Grid {
        let i = side.index();
        // A vertical message takes the panel's place (and is what the other effects play over)
        // The vertical message is drawn fresh each time (it scrolls), so it always counts as a new frame.
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
        // Whether anything shared by the panels is playing on this side.
        let others_active = self.ripples_on(side) || self.text.active() || vertical.is_some();
        // The other panel's last picture, for looks that span both panels (copied before `player`
        // borrows this panel's player mutably).
        let other_base = self.players[1 - i].last_base.clone();
        let player = &mut self.players[i];
        // Only a fresh frame from the monitor replaces the remembered picture; a redraw keeps it.
        if new_frame || player.last_base.is_none() {
            player.last_base = Some(base.clone());
        }
        if new_frame && std::mem::take(&mut self.intro[i]) && player.phase.is_none() {
            // The first picture after the panel came on: in from dark with the app transition
            player.slide = None;
            player.start_rise(now);
        }
        // Time since this panel's last `apply`, capped at 0.1 s so a stalled thread does not jump.
        let dt = match player.last_time {
            None => 0.0,
            Some(last) => (now - last).min(0.1),
        };
        player.last_time = Some(now);
        // Nothing playing on this panel: pass the frame through untouched (the common case).
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
            // Ease-out: fast at first, slowing as it settles.
            let k = 1.0 - (1.0 - k).powf(3.0);
            // Clamped: Python can't slide backwards either (the clock only goes forwards)
            let shift = int_round(HEIGHT as f64 * k).max(0) as usize;
            if shift >= HEIGHT {
                player.slide = None;
            } else {
                // The old picture moves up by `shift` rows and the new one fills in from the bottom.
                let mut slid = Picture::zeros(HEIGHT, WIDTH);
                slid.copy_rows(0, old, shift, HEIGHT - shift);
                slid.copy_rows(HEIGHT - shift, &out, 0, shift);
                out = slid;
            }
        }

        match player.phase {
            // The drain across both panels: cut this panel's columns out of the shared wide picture.
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
            // Another unplug look: frames from `plug_looks`, flipped for a left port and cut to this
            // panel's columns when it spans both.
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
            // A plug-in look bringing the picture in; when it spans both panels the current pictures of
            // both are put side by side first.
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
            // The dark moment between the picture going and coming back.
            Some(Phase::Empty) => {
                out = Picture::zeros(HEIGHT, WIDTH);
                if now - player.phase_since >= SidePlayer::EMPTY_SECONDS {
                    player.start_rise(now);
                }
            }
            // The picture coming back, the same way apps change.
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

        // Ripples only show on a panel that is showing its picture (not while it drains or is dark).
        if matches!(self.players[i].phase, None | Some(Phase::Rise)) && self.ripples_on(side) {
            for (ripple, order) in self.ripples.iter_mut() {
                if !order.contains(&side) || !ripple.is_active() {
                    continue;
                }
                // Both panels call this; the ripple moves on only once per moment.
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
            // Finished ripples are dropped.
            self.ripples.retain(|(r, _)| r.is_active());
        }

        // The text line goes over everything else, on the top 6 rows (more for a message of several
        // lines), at the foreground brightness.
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

        // Back to LED values, rounded and clamped like Python.
        out.to_grid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: a grid with every LED at `v`.
    fn filled(v: i32) -> Grid {
        let mut g = Grid::new();
        for col in g.v.iter_mut() {
            col.iter_mut().for_each(|p| *p = v);
        }
        g
    }

    /// Test helper: effects with these transition settings and a left panel present.
    fn with(slide: bool, fade: bool) -> Effects {
        let mut e = Effects::new();
        e.configure(EffectsSettings { slide, fade, ..EffectsSettings::default() });
        e.panel_present(Side::Left, true);
        e
    }

    /// Proves the fade transition is halfway at half time and finishes on the new app.
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

    /// Proves a panel that just came on fades its first picture in from dark, once only.
    #[test]
    fn a_panel_coming_on_fades_its_first_picture_in() {
        let mut e = with(false, true);
        e.start_intro(Side::Left);
        let first = e.apply(Side::Left, &filled(200), true, 0.0);
        assert!(first.is_dark(), "starts dark");
        let half = e.apply(Side::Left, &filled(200), false, SidePlayer::FADE_SECONDS / 2.0);
        assert_eq!(half.v[3][5], 100);
        let done = e.apply(Side::Left, &filled(200), false, SidePlayer::FADE_SECONDS);
        assert_eq!(done.v[3][5], 200);
        assert!(!e.player(Side::Left).active());
        // Only once
        assert_eq!(e.apply(Side::Left, &filled(120), true, 5.0).v[3][5], 120);
    }

    /// Proves that with "cut" a panel that comes on shows its picture at once.
    #[test]
    fn a_panel_coming_on_cuts_on_with_cut() {
        let mut e = with(false, false);
        e.start_intro(Side::Left);
        assert_eq!(e.apply(Side::Left, &filled(200), true, 0.0).v[3][5], 200);
        assert!(!e.player(Side::Left).active());
    }

    /// Proves "cut" skips the app slide and brings the picture back whole after the unplug pause.
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

    /// Proves the "panels together" drain runs on both panels at once and ends on both together.
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

    /// Proves a pinned message's lines scroll together and keep going round until unpinned, and that the
    /// message settings (including the old `pinned_message` key) load as expected.
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
        assert_eq!(e.settings.messages, vec!["Back soon", "", "Coffee"]);
        let blank = serde_json::json!({"messages": ["", " "], "show_messages": true});
        assert!(EffectsSettings::from_json(&blank).messages.is_empty());
        assert!(e.text.active());
        e.configure(EffectsSettings::from_json(&serde_json::json!({"messages": ["Back soon"]})));
        assert!(!e.text.active(), "kept but not showing");
        // The one-message test build's setting still loads
        let old = EffectsSettings::from_json(&serde_json::json!({"pinned_message": "Hi"}));
        assert_eq!((old.messages, old.show_messages), (vec!["Hi".to_string()], true));
    }

    /// Proves the per-side plug switches, that unknown ports always play, and that defaults are not saved.
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

    /// Proves a vertical message replaces only its own panel's picture, and how its settings are saved.
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

    /// Proves picked looks load and save, unknown or retired ones fall back to drain / ripple, and the
    /// transition setting is read the same way.
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

    /// Proves a chosen unplug look runs for its length, then the dark pause, then the picture is back.
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

    /// Proves the ripple ring starts on the port's panel, reaches the other panel later, and ends clean.
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

    /// Proves the rain and zipper transitions start on the old app and finish on the new one quickly.
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

    /// Proves a chosen plug-in look plays only on the port's panel and no water ripple starts.
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

    /// Proves `fade` is only saved when on and that slide wins when both are on.
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

    /// Proves the numpy-style sum gives the plain answer where no rounding is involved.
    #[test]
    fn pairwise_sum_matches_a_plain_sum_on_small_arrays() {
        assert_eq!(pairwise_sum(&[]), 0.0);
        assert_eq!(pairwise_sum(&[1.0, 2.0, 3.5]), 6.5);
        let many: Vec<f64> = (0..300).map(|i| i as f64).collect();
        assert_eq!(pairwise_sum(&many), 44850.0);
    }

    /// Proves the text-line font: spacing, fallbacks to '?' or the capital, and which letters have tails.
    #[test]
    fn letters_have_a_dark_column_between_them() {
        assert_eq!(text_width("AB"), 7);
        assert_eq!(text_width(""), 0);
        assert_eq!(text_columns("~"), text_columns("?"));
        assert!(FONT.iter().all(|(_, g)| g.len() == FONT_W * FONT_H));
        assert!(LOWER.iter().all(|(_, g)| g.len() == FONT_W * TEXT_ROWS));
        // Small letters stay small; letters without a small form use the capital
        assert_ne!(text_columns("a"), text_columns("A"));
        assert_eq!(text_columns("é"), text_columns("É"));
        // Only g, j, p, q and y reach the bottom row
        let tails: String = LOWER.iter().filter(|(_, g)| g[15..].contains('1')).map(|(k, _)| *k).collect();
        assert_eq!(tails, "gjpqy");
    }

    /// Proves left / centre / right alignment of message lines and that a blank line is kept as a gap.
    #[test]
    fn message_lines_line_up_and_keep_blank_lines() {
        let width = 40;
        let first_lit = |rows: &[Vec<u8>], line: usize| {
            (0..width).find(|&x| rows[line * TEXT_ROWS..(line + 1) * TEXT_ROWS].iter().any(|r| r[x] == 1))
        };
        for (align, want) in [("left", 0), ("center", 4), ("right", 8)] {
            let mut t = TextLine::new();
            t.set_align(align);
            t.pin(&["AAA".to_string(), "".to_string(), "A".to_string()]);
            t.strip(width, 0.0);
            // Move the block fully on: AAA is 11 columns wide, A is 3
            let rows = t.strip(width, 1.0).unwrap();
            assert_eq!(rows.len(), 3 * TEXT_ROWS);
            assert_eq!(first_lit(&rows, 1), None, "the empty line is a gap");
            let (a, b) = (first_lit(&rows, 0).unwrap(), first_lit(&rows, 2).unwrap());
            assert_eq!(b - a, want, "{align}");
        }
        let mut t = TextLine::new();
        t.pin(&["".to_string(), " ".to_string()]);
        assert!(!t.active(), "only empty lines show nothing");
    }

    /// Proves position text parsing and formatting, and where the right panel sits in the wide picture.
    #[test]
    fn positions() {
        assert_eq!(parse_position("Right Middle"), (Some(Side::Right), Some(Spot::Middle)));
        assert_eq!(position_text(Some(Side::Left), None).as_deref(), Some("left"));
        assert_eq!(position_text(None, Some(Spot::Back)), None);
        assert_eq!(panel_columns(&[Side::Left, Side::Right], Side::Right), Some((13, 22)));
    }

    /// Proves deserializing fixes odd values like Python's normalize (`"1_0"` -> 10, row 40 -> 33) and
    /// that saved settings read back the same.
    #[test]
    fn settings_deserialize_like_normalize() {
        let s: EffectsSettings = serde_json::from_str(r#"{"text_speed": "1_0", "port_rows": {"back": 40}}"#).unwrap();
        assert_eq!(s.text_speed, 10);
        assert_eq!(s.port_rows.back, 33);
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(EffectsSettings::from_json(&back), s);
    }
}
