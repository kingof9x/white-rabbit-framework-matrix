//! The "Now playing" app (`led_mon/now_playing.py`): the cover of whatever is playing across both
//! panels (each shows its half of the 18 x 18 cover), the song name scrolling along the top and a
//! small visualizer at the bottom. Reading what is playing is in `wr-monitor`.

use crate::pyround::{int, int_round, round_half_even};
use crate::{Grid, HEIGHT, WIDTH};

pub const COVER: usize = 18;
const BARS_ROWS: f64 = 7.0;
const TITLE_ROWS: usize = 5;
/// Pixels per second.
const TITLE_SPEED: f64 = 12.0;
const COVER_TOP: i64 = 8;

/// What is playing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    pub title: String,
    pub artist: String,
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    pub media: Media,
    pub last_cover: Option<Vec<f64>>,
    pub levels: Option<Vec<f64>>,
}

/// The app's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub title: bool,
    pub visualizer: bool,
    pub drift: bool,
    /// "skip", "last" or "clock"
    pub when_idle: String,
}

impl Default for Options {
    fn default() -> Self {
        Options { title: true, visualizer: true, drift: true, when_idle: "skip".into() }
    }
}

/// The 3 x 5 letters of the scrolling text (`FONT` in `led_mon/effects.py`), row by row.
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

fn glyph(ch: char) -> &'static str {
    FONT.iter().find(|(c, _)| *c == ch).map(|(_, g)| *g).unwrap_or("111001011000010")
}

/// The text as 5 rows of 0/1 columns, one dark column between letters (`text_columns`).
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
                row.push(g[r * 3 + c] - b'0');
            }
        }
    }
    rows
}

/// The song name scrolling right to left: 5 rows of `width` 0/1 values.
pub fn title_strip(text: &str, now: f64, width: usize) -> Vec<Vec<u8>> {
    let strip = text_columns(&format!("{text}   "));
    let w = strip[0].len();
    if w == 0 {
        return vec![vec![0; width]; 5];
    }
    let offset = int(now * TITLE_SPEED).rem_euclid(w as i64) as usize;
    strip.iter().map(|row| (0..width).map(|i| row[(offset + i) % w]).collect()).collect()
}

/// The whole picture across both panels: 34 rows of 18 values 0.0-1.0.
pub fn scene(media: &Media, cover: Option<&[f64]>, now: f64, opts: &Options, levels: Option<&[f64]>) -> Vec<Vec<f64>> {
    let mut canvas = vec![vec![0.0f64; COVER]; HEIGHT];
    if let Some(cover) = cover {
        let dy = if opts.drift { int_round((now * 0.5).sin()) } else { 0 };
        let top = (COVER_TOP + dy) as usize;
        for (r, row) in canvas[top..top + COVER].iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                let v = cover.get(r * COVER + c).copied().unwrap_or(0.0) * 0.85;
                *cell = cell.max(v);
            }
        }
    }
    let song = media.song();
    if opts.title && !song.is_empty() {
        for (r, row) in title_strip(&song, now, COVER).into_iter().enumerate().take(TITLE_ROWS) {
            for (c, v) in row.into_iter().enumerate() {
                canvas[r][c] = v as f64;
            }
        }
    }
    if let (true, Some(levels)) = (opts.visualizer, levels) {
        for b in 0..WIDTH {
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
pub fn draw_now_playing(
    grid: &mut Grid,
    np: &NowPlaying,
    opts: &Options,
    right: bool,
    now: f64,
    fill_value: i32,
) -> bool {
    let mut media = np.media.clone();
    if !media.playing {
        if opts.when_idle == "clock" {
            return false;
        }
        if opts.when_idle == "last" && np.last_cover.is_some() {
            media = Media {
                title: media.title,
                artist: media.artist,
                playing: false,
                cover: media.cover.or_else(|| np.last_cover.clone()),
            };
        } else if media.cover.is_none() && media.song().is_empty() {
            return false;
        }
    }
    let cover = media.cover.clone().or_else(|| if media.playing { np.last_cover.clone() } else { None });
    let levels = if opts.visualizer && media.playing { np.levels.as_deref() } else { None };
    let canvas = scene(&media, cover.as_deref(), now, opts, levels);
    draw_half(grid, &canvas, right, fill_value);
    true
}

/// A made-up cover for previews and tests: a crescent moon over a hill, not a real album.
pub fn demo_cover() -> Vec<f64> {
    let mut cover = vec![0.0f64; COVER * COVER];
    for y in 0..COVER {
        for x in 0..COVER {
            let (xf, yf) = (x as f64, y as f64);
            let moon = (xf - 6.0).hypot(yf - 7.0) < 5.5 && (xf - 8.5).hypot(yf - 5.5) >= 4.5;
            let hill = yf > 13.0 + 2.5 * ((xf - 12.0) / 4.0).cos();
            let v = if moon { 0.95 } else { 0.0 } + if hill { 0.45 } else { 0.0 };
            cover[y * COVER + x] = v;
        }
    }
    for (y, x) in [(2, 14), (5, 16), (1, 9), (9, 15)] {
        cover[y * COVER + x] = 1.0;
    }
    cover.iter().map(|v| v.clamp(0.0, 1.0)).collect()
}
