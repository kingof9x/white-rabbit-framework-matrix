//! The Picture app ("snap"): a built-in picture from `led_mon/snapshot_files`, or a drawing made on the
//! window's Draw tab ("drawing:<name>", a port of the player in `led_mon/drawings.py`).
//!
//! A drawing is `<settings folder>\drawings\<name>.json`: `{"version": 1, "delay_ms": 100, "frames": [...]}`,
//! each frame 34 rows of 9 brightness values (0-255), top row first. The frames loop, each shown for
//! `delay_ms`. The file is read again when it changes.
//!
//! Reading files is kept apart from drawing: `Drawing::from_json` and `draw_frame` are pure, and
//! `Player` does the reading.
//!
//! Where this sits: `render.rs` calls `draw_app` for an app named "snap". In the monitor that happens on
//! the main loop, ten times a second for each panel. The monitor calls `set_drawings_folder` once at
//! start-up; until then (and in the settings window, which never sets it) drawings draw nothing. The
//! window's Draw tab reuses `Drawing`, `safe_name` and the limits below to save and preview drawings.
//!
//! Shared state: one `Player` (with its cache of read drawings), the drawings folder and the list of
//! warnings already logged live in `static` mutexes, so the many calls to `draw_app` share them
//! without the caller having to pass anything in. Each lock is held only for the call that needs it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

use serde_json::Value;

use crate::drawing::draw_snapshot;
use crate::grid::{Grid, HEIGHT, WIDTH};
use crate::layout::App;

/// A `file` setting starting with this names a Draw tab drawing instead of a built-in picture.
pub const PREFIX: &str = "drawing:";
/// How long each frame shows when the file gives no usable delay, in milliseconds.
pub const DEFAULT_DELAY_MS: i64 = 100;
/// The shortest frame time allowed (ms): 50 frames a second, faster than the panels can usefully show.
pub const MIN_DELAY_MS: i64 = 20;
/// The longest frame time allowed (ms): 10 seconds.
pub const MAX_DELAY_MS: i64 = 10000;
/// Frames past this many are ignored when a drawing is read, which caps memory and file size.
pub const MAX_FRAMES: usize = 200;

/// One frame: 34 rows of 9 values, top row first.
pub type Frame = [[u8; WIDTH]; HEIGHT];

/// The pictures that come with the app, by `<panel>/<file>` (or just `<file>`).
/// The JSON files are compiled into the program with `include_str!`, so they work without the
/// `led_mon` folder being installed. Each is a list of 34 rows of 9 values.
const BUILT_IN: [(&str, &str); 8] = [
    ("left/double-check.json", include_str!("../../../../led_mon/snapshot_files/left/double-check.json")),
    ("left/zigzag.json", include_str!("../../../../led_mon/snapshot_files/left/zigzag.json")),
    ("left/the-way-left.json", include_str!("../../../../led_mon/snapshot_files/left/the-way-left.json")),
    ("left/the-way-left_raw.json", include_str!("../../../../led_mon/snapshot_files/left/the-way-left_raw.json")),
    ("right/check.json", include_str!("../../../../led_mon/snapshot_files/right/check.json")),
    ("right/the-way-right.json", include_str!("../../../../led_mon/snapshot_files/right/the-way-right.json")),
    ("right/the-way-right_raw.json", include_str!("../../../../led_mon/snapshot_files/right/the-way-right_raw.json")),
    ("right/every-third-row.json", include_str!("../../../../led_mon/snapshot_files/right/every-third-row.json")),
];
/// The `path` arg of the built-in pictures.
const BUILT_IN_PATH: &str = "snapshot_files";

/// Python's `int(value)` for the delay, then kept between 20 ms and 10 s.
/// Whole numbers, decimals (rounded towards zero), true/false (1/0) and strings holding a whole
/// number are accepted; anything else (null, "1.5", a list) uses the default of 100 ms.
pub fn clamp_delay(v: Option<&Value>) -> i64 {
    let ms = match v {
        None => Some(DEFAULT_DELAY_MS),
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)),
        Some(Value::Bool(b)) => Some(*b as i64),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    };
    ms.unwrap_or(DEFAULT_DELAY_MS).clamp(MIN_DELAY_MS, MAX_DELAY_MS)
}

/// Python's `a // b` for floats.
/// Python computes it in this exact way (from the remainder, then corrected towards minus
/// infinity), so the frame number matches Python's even at the edges of a frame.
fn py_floordiv(a: f64, b: f64) -> f64 {
    // Rust's % keeps the sign of `a`; Python's keeps the sign of `b`. Shift the quotient down by
    // one when the signs differ and there is a remainder.
    let m = a % b;
    let mut div = (a - m) / b;
    if m != 0.0 && ((b < 0.0) != (m < 0.0)) {
        div -= 1.0;
    }
    if div != 0.0 {
        // `div` should already be a whole number; this snaps away tiny floating-point errors to
        // the nearest whole number, as CPython does.
        let mut floor = div.floor();
        if div - floor > 0.5 {
            floor += 1.0;
        }
        floor
    } else {
        // A zero result keeps the sign of the true quotient (-0.0 for a negative one), like Python.
        0.0_f64.copysign(a / b)
    }
}

/// A drawing's frames and how long each one shows. A drawing made for both panels also has the right
/// panel's frames (`"right"` in the file, the same number as `frames`); `frames` is then the left
/// panel's. A drawing without them shows the same on either panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawing {
    /// The frames in order (the left panel's for a two-panel drawing); never empty.
    pub frames: Vec<Frame>,
    /// How long each frame shows, in milliseconds, already clamped to `MIN_DELAY_MS`..=`MAX_DELAY_MS`.
    pub delay_ms: i64,
    /// The right panel's frames for a two-panel drawing, the same count as `frames`; empty otherwise.
    pub right: Vec<Frame>,
}

/// A list of frames from a drawing file.
/// `None` (the key is missing) gives an empty list. At most `MAX_FRAMES` frames are read. Any frame
/// of the wrong size, or a value that is not a number or true/false, makes the whole list an error.
fn frames_from_json(value: Option<&Value>) -> Result<Vec<Frame>, String> {
    let frames_value = match value {
        None => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        Some(_) => return Err("the frames are not a list".into()),
    };
    let mut frames = Vec::new();
    for f in frames_value.iter().take(MAX_FRAMES) {
        let rows = f.as_array().ok_or("a frame is not a list")?;
        if rows.len() != HEIGHT {
            return Err(format!("a frame has {} rows, not 34", rows.len()));
        }
        let mut frame = [[0u8; WIDTH]; HEIGHT];
        for (y, row) in rows.iter().enumerate() {
            let values = row.as_array().filter(|r| r.len() == WIDTH).ok_or("a frame is not 9 x 34")?;
            for (x, v) in values.iter().enumerate() {
                let v = match v {
                    Value::Number(n) => n.as_f64().unwrap_or(0.0),
                    Value::Bool(b) => *b as u8 as f64,
                    _ => return Err("a frame has a value that is not a number".into()),
                };
                // Out-of-range values are clamped and decimals rounded down by the cast.
                frame[y][x] = v.clamp(0.0, 255.0) as u8;
            }
        }
        frames.push(frame);
    }
    Ok(frames)
}

impl Drawing {
    /// A drawing from its file's JSON. Values are kept between 0 and 255 and rounded down; a frame that
    /// isn't 9 x 34 is an error, as in Python. A drawing without frames shows one dark frame.
    pub fn from_json(data: &Value) -> Result<Drawing, String> {
        let map = data.as_object().ok_or("not a drawing")?;
        let mut frames = frames_from_json(map.get("frames"))?;
        if frames.is_empty() {
            frames.push([[0; WIDTH]; HEIGHT]);
        }
        // The right panel's frames, as many as the left's
        // A broken "right" list is ignored (the drawing then shows the same on both panels)
        // instead of failing the whole drawing. Extra right frames are cut, missing ones are dark.
        let mut right = frames_from_json(map.get("right")).unwrap_or_default();
        if !right.is_empty() {
            right.resize(frames.len(), [[0; WIDTH]; HEIGHT]);
        }
        Ok(Drawing { frames, delay_ms: clamp_delay(map.get("delay_ms")), right })
    }

    /// The frame showing on this panel ("left" or "right") after this many seconds of playing.
    /// Time 0 is when the player started. A side other than "right", or a drawing without right-panel
    /// frames, gives the left (main) frames.
    pub fn frame_on(&self, seconds: f64, side: &str) -> &Frame {
        // How many whole frame times have passed; rem_euclid below loops it over the frames.
        let step = py_floordiv(seconds * 1000.0, self.delay_ms as f64) as i64;
        let frames = if side == "right" && !self.right.is_empty() { &self.right } else { &self.frames };
        &frames[step.rem_euclid(frames.len() as i64) as usize]
    }

    /// The frame showing after this many seconds of playing (loops).
    /// The same as `frame_on` for the left (main) frames.
    pub fn frame_at(&self, seconds: f64) -> &Frame {
        let step = py_floordiv(seconds * 1000.0, self.delay_ms as f64) as i64;
        &self.frames[step.rem_euclid(self.frames.len() as i64) as usize]
    }
}

/// A drawing's frame on the panel, scaled to the brightness: `value * fill_value // 255`.
/// `fill_value` is the app's LED brightness (0-255), so a 255 in the drawing becomes `fill_value` and
/// lower values scale down with it (rounded down, as Python's `//`). Every pixel is overwritten.
pub fn draw_frame(grid: &mut Grid, frame: &Frame, fill_value: i32) {
    for (y, row) in frame.iter().enumerate() {
        for (x, v) in row.iter().enumerate() {
            grid.set(x, y, (*v as i32 * fill_value).div_euclid(255));
        }
    }
}

/// A drawing's name made usable as a Windows file name, as the Draw tab saves it.
/// It drops the characters Windows does not allow in file names (`< > : " / \ | ? *` and control
/// characters), trims spaces, removes trailing dots, and keeps at most 60 characters. It must match
/// the Draw tab's rule exactly, or the player would look for a different file than was saved.
pub fn safe_name(name: &str) -> String {
    let kept: String = name.chars().filter(|c| !"<>:\"/\\|?*".contains(*c) && (*c as u32) > 0x1f).collect();
    kept.trim().trim_end_matches('.').chars().take(60).collect()
}

/// Plays saved drawings, reading a file again when it changes.
/// Used through the shared `PLAYER` static by `draw_app`; all its methods run on the caller's thread
/// and read files from disk there.
pub struct Player {
    /// The folder the drawings are read from (`<settings folder>\drawings`).
    pub folder: PathBuf,
    /// When the player was made; the animation's time 0, so all drawings play from their first frame
    /// at that moment and both panels stay in step.
    started: Instant,
    /// name: (when the file was changed, the drawing or None when it can't be read)
    cache: HashMap<String, (SystemTime, Option<Drawing>)>,
}

impl Player {
    /// A player reading from `folder`, with nothing read yet.
    pub fn new(folder: PathBuf) -> Player {
        Player { folder, started: Instant::now(), cache: HashMap::new() }
    }

    /// The file a drawing called `name` is saved in: its safe name plus ".json" in the folder.
    pub fn path_for(&self, name: &str) -> PathBuf {
        self.folder.join(format!("{}.json", safe_name(name)))
    }

    /// The drawing, read again if its file changed. None when it is missing or can't be read.
    /// Checking the file's modified time costs one small disk call per frame; the JSON is only parsed
    /// again when that time changes. A file that cannot be parsed is logged as a warning once per change
    /// and cached as None, so it is not parsed again every frame.
    pub fn get(&mut self, name: &str) -> Option<&Drawing> {
        let path = self.path_for(name);
        // No file (or no modified time) means no drawing; the old cache entry is kept but unused.
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
        let fresh = self.cache.get(name).is_some_and(|(m, _)| *m == modified);
        if !fresh {
            let drawing = read_drawing(&path);
            if let Err(e) = &drawing {
                crate::warn!("Could not read the drawing {name}: {e}");
            }
            self.cache.insert(name.to_string(), (modified, drawing.ok()));
        }
        self.cache.get(name).and_then(|(_, d)| d.as_ref())
    }

    /// The frame to show now on this panel ("left" or "right"), or None if the drawing is missing.
    /// Returns a copy of the frame (it is small: 306 bytes), so the caller does not keep a borrow of
    /// the player.
    pub fn frame(&mut self, name: &str, side: &str) -> Option<Frame> {
        let seconds = self.started.elapsed().as_secs_f64();
        self.get(name).map(|d| *d.frame_on(seconds, side))
    }
}

/// Reads and parses one drawing file. The error is a short message for the log.
fn read_drawing(path: &Path) -> Result<Drawing, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let data: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Drawing::from_json(&data)
}

/// The monitor's player, made the first time a drawing is shown, and the folder it reads.
/// Behind a `Mutex` because statics must be safe to reach from any thread; in practice only the
/// main loop draws pictures.
static PLAYER: Mutex<Option<Player>> = Mutex::new(None);
/// None until the monitor calls `set_drawings_folder`.
static FOLDER: Mutex<Option<PathBuf>> = Mutex::new(None);
/// Pictures already reported missing, so the log says it once.
static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Where the Draw tab saves drawings (`<settings folder>\drawings`). Set once by the monitor.
/// Calling it again just replaces the folder; the player picks up the new one on its next draw.
pub fn set_drawings_folder(folder: PathBuf) {
    *FOLDER.lock().unwrap() = Some(folder);
}

/// Logs `message` as a warning the first time `key` (the app's file setting) is seen missing. The
/// set is never cleared, so a picture that goes missing again later is not reported again.
fn warn_once(key: &str, message: &str) {
    let mut warned = WARNED.lock().unwrap();
    if warned.get_or_insert_with(HashSet::new).insert(key.to_string()) {
        crate::warn!("{message}");
    }
}

/// A built-in or saved picture's rows (34 rows of 9 values, usually 0 or 1). `path` is the app's
/// `path` arg: `snapshot_files` for the built-in ones, or a folder. `panel` picks the `left` or `right`
/// folder inside it when that folder exists.
/// Returns None when the built-in name is unknown or the file is missing or not valid JSON. Note
/// that a built-in picture is only found with its panel: there is no top-level built-in file, so a
/// missing or other `panel` finds nothing in `snapshot_files`.
pub fn load_snapshot(path: &str, panel: Option<&str>, file: &str) -> Option<Vec<Vec<f64>>> {
    let side = panel.filter(|p| *p == "left" || *p == "right");
    let text = if path.trim_end_matches(['/', '\\']) == BUILT_IN_PATH {
        let key = match side {
            Some(side) => format!("{side}/{file}"),
            None => file.to_string(),
        };
        BUILT_IN.iter().find(|(k, _)| *k == key).map(|(_, t)| t.to_string())
    } else {
        let folder = Path::new(path);
        // For your own folder the left/right subfolder is optional: use it only if it exists.
        let sub = side.filter(|s| folder.join(s).is_dir());
        let full = match sub {
            Some(s) => folder.join(s).join(file),
            None => folder.join(file),
        };
        std::fs::read_to_string(full).ok()
    };
    text.and_then(|t| serde_json::from_str(&t).ok())
}

/// The snap app: a drawing ("drawing:<name>") or a built-in picture, on the whole panel. `side` is the
/// panel it is on, for drawings made for both panels.
/// A missing drawing or picture leaves the grid as it was and logs a warning once. Locks: the
/// folder lock is taken and released at once; the player lock is held while the drawing file is
/// checked and read.
pub fn draw_app(grid: &mut Grid, app: &App, fill_value: i32, side: &str) {
    let file = app.arg_str("file").unwrap_or_default();
    if let Some(name) = file.strip_prefix(PREFIX) {
        // No folder set (for example in the settings window's preview): draw nothing.
        let Some(folder) = FOLDER.lock().unwrap().clone() else { return };
        let mut player = PLAYER.lock().unwrap();
        let player = player.get_or_insert_with(|| Player::new(folder.clone()));
        // Keep the player's folder in step with the latest `set_drawings_folder`.
        player.folder = folder;
        match player.frame(name, side) {
            Some(frame) => draw_frame(grid, &frame, fill_value),
            None => warn_once(&file, &format!("Drawing {name} not found")),
        }
        return;
    }
    let Some(path) = app.arg_str("path") else { return };
    match load_snapshot(&path, app.arg_str("panel").as_deref(), &file) {
        Some(rows) => draw_snapshot(grid, &rows, fill_value),
        None => warn_once(&file, &format!("File {file} not found")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Delays are read the way Python's `int()` would read them and kept between 20 ms and 10 s;
    /// values it cannot read give the 100 ms default.
    #[test]
    fn delays_like_python() {
        assert_eq!(clamp_delay(None), 100);
        assert_eq!(clamp_delay(Some(&Value::from(5))), 20);
        assert_eq!(clamp_delay(Some(&Value::from(12.9))), 20);
        assert_eq!(clamp_delay(Some(&Value::from(150.9))), 150);
        assert_eq!(clamp_delay(Some(&Value::from(" 250 "))), 250);
        assert_eq!(clamp_delay(Some(&Value::from("1.5"))), 100);
        assert_eq!(clamp_delay(Some(&Value::Null)), 100);
        assert_eq!(clamp_delay(Some(&Value::from(99999))), 10000);
    }

    /// Forbidden and control characters are dropped, outer spaces and trailing dots trimmed (Python's
    /// order, so a space before the final dots stays), and names are cut to 60 characters.
    #[test]
    fn names_lose_what_windows_refuses() {
        assert_eq!(safe_name(" a<b>:c/d\\e|f?g*h\u{1}. ."), "abcdefgh. ");
        assert_eq!(safe_name(" ab.. "), "ab");
        assert_eq!(safe_name(&"x".repeat(80)).len(), 60);
    }

    /// Floor division gives the right frame number at and just before a frame boundary.
    #[test]
    fn floor_division_like_python() {
        assert_eq!(py_floordiv(150.0, 150.0), 1.0);
        assert_eq!(py_floordiv(149.0, 150.0), 0.0);
        assert_eq!(py_floordiv(12340.0, 150.0), 82.0);
    }

    /// Built-in pictures are found by panel, with or without a trailing slash on the path, and not
    /// without a panel.
    #[test]
    fn built_in_pictures_are_found() {
        assert!(load_snapshot("snapshot_files", Some("left"), "zigzag.json").is_some());
        assert!(load_snapshot("snapshot_files/", Some("right"), "check.json").is_some());
        assert!(load_snapshot("snapshot_files", None, "zigzag.json").is_none());
    }

    /// A missing drawing gives None, a new file is read, and a file with a later modified time is read
    /// again (the time is set by hand because file times can be too coarse to see a quick rewrite).
    #[test]
    fn a_changed_file_is_read_again() {
        let dir = std::env::temp_dir().join(format!("wr-drawings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut player = Player::new(dir.clone());
        assert!(player.get("pic").is_none());
        let frame = |v: u8| serde_json::json!({"version": 1, "frames": [vec![vec![v; 9]; 34]]});
        std::fs::write(dir.join("pic.json"), frame(7).to_string()).unwrap();
        assert_eq!(player.get("pic").unwrap().frames[0][0][0], 7);
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::write(dir.join("pic.json"), frame(9).to_string()).unwrap();
        std::fs::File::options().write(true).open(dir.join("pic.json")).unwrap().set_modified(later).unwrap();
        assert_eq!(player.get("pic").unwrap().frames[0][0][0], 9);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
