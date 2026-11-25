//! The "Now playing" app (`led_mon/now_playing.py`): the cover of whatever is playing across both
//! panels (each shows its half of the 18 x 18 cover), the song name scrolling along the top and a
//! small visualizer at the bottom. Reading what is playing is in `wr-monitor`.
//!
//! Where this sits: `wr-monitor`'s `media.rs` asks Windows' media controls what is playing on a
//! background thread about once a second, and hands a `NowPlaying` to the readings. Each frame,
//! `render::draw_data_app` (on the monitor's main loop, once per panel) builds the `Options` from the
//! app's settings and calls `draw_now_playing`; when it returns false the caller draws a clock instead.
//!
//! How the picture is made: `scene` paints one 18-wide by 34-tall canvas covering both panels side by
//! side (each panel is 9 wide), with brightness as fractions 0.0-1.0. The left panel then copies
//! columns 0-8 and the right panel columns 9-17 (`draw_half`), multiplied by the LED brightness. Both
//! panels compute the same scene from the same clock, so the halves line up without talking to each
//! other. Layers, top to bottom of the canvas: song name (rows 0-4), cover (rows 7-26, drifting up
//! and down by one row), visualizer bars (up to 7 rows from the bottom).

use crate::pyround::{int, int_round, round_half_even};
use crate::{Grid, HEIGHT, WIDTH};

/// The cover's size in LEDs (18 x 18): two panels wide.
pub const COVER: usize = 18;
/// The tallest a visualizer bar can be, in rows.
const BARS_ROWS: f64 = 7.0;
/// Rows taken by the song name at the top (the 3 x 5 font is 5 rows tall).
const TITLE_ROWS: usize = 5;
/// Pixels per second.
/// How fast the song name scrolls.
const TITLE_SPEED: f64 = 12.0;
/// The cover's top row when it is not drifting: row 8, which leaves room for the title above
/// (rows 0-4) and, with the 18 cover rows ending at row 25, for the visualizer bars below.
const COVER_TOP: i64 = 8;

/// What is playing.
/// Filled in by `wr-monitor` from Windows' media session; a `Default` value means nothing is known.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    /// The song's title; empty when unknown.
    pub title: String,
    /// The artist; empty when unknown.
    pub artist: String,
    /// True only while it is actually playing (false when paused or stopped).
    pub playing: bool,
    /// 18 x 18 values 0.0-1.0, row by row, or None.
    pub cover: Option<Vec<f64>>,
}

impl Media {
    /// "Title - Artist", or whichever of the two there is.
    pub fn song(&self) -> String {
        match (self.title.is_empty(), self.artist.is_empty()) {
            (false, false) => format!("{} - {}", self.title, self.artist),
            (false, true) => self.title.clone(),
            _ => self.artist.clone(),
        }
    }
}

/// What the app gets each frame: what is playing, the last cover seen, and the visualizer's 9 bar
/// heights (in rows, 0-7) when there is sound to show.
/// Built by the now-playing background thread in `wr-monitor` and read on the drawing thread.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    /// What the media controls say now.
    pub media: Media,
    /// The most recent cover seen (same layout as `Media::cover`). Kept when a player stops giving a
    /// cover, so "last" can keep showing it and a playing song without art still shows the previous one.
    pub last_cover: Option<Vec<f64>>,
    /// The visualizer's bar heights, bass first, in rows (0-7, fractions allowed); None when there is no
    /// sound to show.
    pub levels: Option<Vec<f64>>,
}

/// The app's settings.
/// Read from the app's layout settings by `render::draw_data_app`; missing settings use the defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// Scroll the song name along the top.
    pub title: bool,
    /// Draw the sound bars at the bottom (only while playing).
    pub visualizer: bool,
    /// Move the cover slowly up and down by one row, so a still cover does not look frozen (and the same
    /// LEDs are not lit all the time).
    pub drift: bool,
    /// What to show when nothing is playing: "clock" always shows the clock; "last" keeps showing the
    /// last cover; "skip" (the default) still shows a paused song's name or cover and otherwise the clock.
    /// In Python "skip" also skipped the app in the quarter's rotation; this port does not do that part.
    pub when_idle: String,
}

/// Everything on, and "skip" when idle: the same defaults as Python and as the settings window.
impl Default for Options {
    fn default() -> Self {
        Options { title: true, visualizer: true, drift: true, when_idle: "skip".into() }
    }
}

/// The 3 x 5 letters of the scrolling text (`FONT` in `led_mon/effects.py`), row by row.
/// Each entry is a character and 15 digits: 5 rows of 3 pixels, left to right, top to bottom, "1" lit.
#[rustfmt::skip]
const FONT: [(char, &str); 59] = [
    ('A', "010101111101101"), ('B', "110101110101110"), ('C', "011100100100011"), ('D', "110101101101110"),
    ('E', "111100110100111"), ('F', "111100110100100"), ('G', "011100101101011"), ('H', "101101111101101"),
    ('I', "111010010010111"), ('J', "001001001101010"), ('K', "101101110101101"), ('L', "100100100100111"),
    ('M', "101111111101101"), ('N', "110101101101101"), ('O', "010101101101010"), ('P', "110101110100100"),
    ('Q', "010101101110011"), ('R', "110101110101101"), ('S', "011100010001110"), ('T', "111010010010010"),
    ('U', "101101101101111"), ('V', "101101101101010"), ('W', "101101111111101"), ('X', "101101010101101"),
    ('Y', "101101010010010"), ('Z', "111001010100111"),
    ('0', "111101101101111"), ('1', "010110010010111"), ('2', "111001111100111"), ('3', "111001011001111"),
    ('4', "101101111001001"), ('5', "111100111001111"), ('6', "111100111101111"), ('7', "111001010010010"),
    ('8', "111101111101111"), ('9', "111101111001111"),
    (' ', "000000000000000"), ('.', "000000000000010"), (',', "000000000010100"), (':', "000010000010000"),
    ('!', "010010010000010"), ('?', "111001011000010"), ('-', "000000111000000"), ('+', "000010111010000"),
    ('%', "101001010100101"), ('\'', "010010000000000"), ('"', "101101000000000"), ('/', "001001010100100"),
    ('(', "001010010010001"), (')', "100010010010100"), ('&', "010101010101011"), ('#', "101111101111101"),
    ('*', "000101010101000"), ('=', "000111000111000"), ('_', "000000000000111"), ('<', "001010100010001"),
    ('>', "100010001010100"), ('@', "010101111100011"), ('$', "011110010011110"),
];

/// The 15-digit pattern for a character; characters not in the font (lower case is upper-cased
/// before this, so mostly accents and symbols) are drawn with the "?" pattern.
fn glyph(ch: char) -> &'static str {
    FONT.iter().find(|(c, _)| *c == ch).map(|(_, g)| *g).unwrap_or("111001011000010")
}

/// The text as 5 rows of 0/1 columns, one dark column between letters (`text_columns`).
/// Each letter is 3 columns wide; the result has 4 columns per letter minus the last gap. An empty
/// text gives 5 empty rows.
pub fn text_columns(text: &str) -> Vec<Vec<u8>> {
    let mut rows = vec![Vec::new(); 5];
    for (i, ch) in text.to_uppercase().chars().enumerate() {
        if i > 0 {
            for row in rows.iter_mut() {
                row.push(0);
            }
        }
        let g = glyph(ch).as_bytes();
        for (r, row) in rows.iter_mut().enumerate() {
            for c in 0..3 {
                // The ASCII digit '0' or '1' minus b'0' is the number 0 or 1.
                row.push(g[r * 3 + c] - b'0');
            }
        }
    }
    rows
}

/// The song name scrolling right to left: 5 rows of `width` 0/1 values.
/// `now` is seconds on a steady clock. The text plus three spaces is repeated end to end, so it
/// loops with a gap; at `TITLE_SPEED` pixels a second the window slides left over it.
pub fn title_strip(text: &str, now: f64, width: usize) -> Vec<Vec<u8>> {
    let strip = text_columns(&format!("{text}   "));
    let w = strip[0].len();
    if w == 0 {
        return vec![vec![0; width]; 5];
    }
    // How many pixels have scrolled by, wrapped to the strip's length; rem_euclid keeps it
    // positive even for a negative `now`.
    let offset = int(now * TITLE_SPEED).rem_euclid(w as i64) as usize;
    strip.iter().map(|row| (0..width).map(|i| row[(offset + i) % w]).collect()).collect()
}

/// The whole picture across both panels: 34 rows of 18 values 0.0-1.0.
/// `cover` is the 18 x 18 cover (row by row) or None; `levels` is None to draw no bars. Values
/// outside the cover's length count as 0, so a short cover vector cannot panic.
pub fn scene(media: &Media, cover: Option<&[f64]>, now: f64, opts: &Options, levels: Option<&[f64]>) -> Vec<Vec<f64>> {
    let mut canvas = vec![vec![0.0f64; COVER]; HEIGHT];
    if let Some(cover) = cover {
        // sin(now / 2) goes between -1 and 1 over about 12.6 seconds; rounding gives -1, 0 or 1,
        // so the cover sits one row higher, at its place, or one row lower (rows 7-24 to 9-26).
        let dy = if opts.drift { int_round((now * 0.5).sin()) } else { 0 };
        let top = (COVER_TOP + dy) as usize;
        for (r, row) in canvas[top..top + COVER].iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                // The cover is dimmed to 85% so the title and bars, drawn brighter, stand out on it.
                let v = cover.get(r * COVER + c).copied().unwrap_or(0.0) * 0.85;
                *cell = cell.max(v);
            }
        }
    }
    let song = media.song();
    if opts.title && !song.is_empty() {
        // The title is drawn over the cover, replacing it (lit pixels 1.0, gaps 0.0), so the text
        // stays readable on a bright cover.
        for (r, row) in title_strip(&song, now, COVER).into_iter().enumerate().take(TITLE_ROWS) {
            for (c, v) in row.into_iter().enumerate() {
                canvas[r][c] = v as f64;
            }
        }
    }
    if let (true, Some(levels)) = (opts.visualizer, levels) {
        for b in 0..WIDTH {
            // Bar b is 0-7 rows tall, drawn 0.7 bright from the bottom row up, twice: mirrored
            // on the two panels so both halves look the same.
            let h = int_round(BARS_ROWS.min(levels.get(b).copied().unwrap_or(0.0)));
            for k in 0..h.max(0) as usize {
                // Bass in the middle, treble towards the outside
                canvas[HEIGHT - 1 - k][WIDTH - 1 - b] = 0.7;
                canvas[HEIGHT - 1 - k][WIDTH + b] = 0.7;
            }
        }
    }
    canvas
}

/// Puts one panel's half of the scene on the grid, keeping what is brighter.
/// `right` picks the right panel's half (canvas columns 9-17) instead of the left one (0-8).
/// `fill_value` is the LED brightness (0-255) that a canvas value of 1.0 becomes. Taking the
/// brighter of the old and new value lets the app be drawn over something already on the grid.
pub fn draw_half(grid: &mut Grid, canvas: &[Vec<f64>], right: bool, fill_value: i32) {
    let x0 = if right { WIDTH } else { 0 };
    for (y, row) in canvas.iter().enumerate().take(HEIGHT) {
        for x in 0..WIDTH {
            let v = round_half_even(row[x0 + x] * fill_value as f64) as i32;
            grid.set(x, y, grid.get(x, y).max(v));
        }
    }
}

/// Draws the app. Returns false when it shows the clock instead (nothing playing and nothing to
/// keep showing, or set to show the clock then); the caller draws the clock.
/// `right` says which half this panel shows, `now` is seconds on a steady clock (moves the title and
/// the drift) and `fill_value` is the LED brightness for full white.
pub fn draw_now_playing(
    grid: &mut Grid,
    np: &NowPlaying,
    opts: &Options,
    right: bool,
    now: f64,
    fill_value: i32,
) -> bool {
    // Decide what to show when nothing is playing (paused, stopped, or no player).
    let mut media = np.media.clone();
    if !media.playing {
        if opts.when_idle == "clock" {
            return false;
        }
        // Keep showing the last cover (and any title still known), as if paused.
        if opts.when_idle == "last" && np.last_cover.is_some() {
            media = Media {
                title: media.title,
                artist: media.artist,
                playing: false,
                cover: media.cover.or_else(|| np.last_cover.clone()),
            };
        // Nothing at all to show: let the caller draw the clock.
        } else if media.cover.is_none() && media.song().is_empty() {
            return false;
        }
    }
    // A playing song without its own cover borrows the last one seen; bars only while playing.
    let cover = media.cover.clone().or_else(|| if media.playing { np.last_cover.clone() } else { None });
    let levels = if opts.visualizer && media.playing { np.levels.as_deref() } else { None };
    let canvas = scene(&media, cover.as_deref(), now, opts, levels);
    draw_half(grid, &canvas, right, fill_value);
    true
}

/// A made-up cover for previews and tests: a crescent moon over a hill, not a real album.
/// Used by the settings window's preview and by tests, so they do not need a real song. Pixel
/// values are 0.0-1.0 and the layout is the same as `Media::cover`.
pub fn demo_cover() -> Vec<f64> {
    let mut cover = vec![0.0f64; COVER * COVER];
    for y in 0..COVER {
        for x in 0..COVER {
            let (xf, yf) = (x as f64, y as f64);
            // The moon is a disc of radius 5.5 around (6, 7) minus a disc of radius 4.5 around
            // (8.5, 5.5), which leaves a crescent. The hill is everything below a cosine curve.
            let moon = (xf - 6.0).hypot(yf - 7.0) < 5.5 && (xf - 8.5).hypot(yf - 5.5) >= 4.5;
            let hill = yf > 13.0 + 2.5 * ((xf - 12.0) / 4.0).cos();
            let v = if moon { 0.95 } else { 0.0 } + if hill { 0.45 } else { 0.0 };
            cover[y * COVER + x] = v;
        }
    }
    // A few single bright pixels as stars.
    for (y, x) in [(2, 14), (5, 16), (1, 9), (9, 15)] {
        cover[y * COVER + x] = 1.0;
    }
    cover.iter().map(|v| v.clamp(0.0, 1.0)).collect()
}
