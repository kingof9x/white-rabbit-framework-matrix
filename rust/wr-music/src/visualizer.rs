//! One panel's audio visualizer: give it the latest sound, get back the next picture
//! (Python's `looks.Visualizer`). Every look draws exactly what the Python one draws; see
//! `tests/golden.rs`.
//!
//! # How a frame is made
//!
//! `Visualizer::frame` is called about 33 times a second on the monitor's `music-N` thread (through
//! `MusicPanel`), with the last 4096 stereo samples and the seconds since the last frame (`dt`):
//! 1. The samples are reduced to one channel (`mono_or_channel`): the panel's own side, or both mixed.
//! 2. The `Analyzer` measures each frequency band as a level 0..34 (`levels`).
//! 3. The levels become `shown` bar heights: they jump up at once and fall at the chosen speed
//!    (`fall`, levels per second). `peaks` follow the bars up, wait `PEAK_HOLD_SEC`, then fall at
//!    half that speed.
//! 4. The look (`draw_*`) turns `shown`/`peaks` (or the raw samples, for the wave looks) into a
//!    picture of brightness values 0..1, indexed `[x][y]` (x = column 0..8, y = row 0..33, row 0 at
//!    the top).
//! 5. `finish` flips it upside down if asked and clips every value to 0..1.
//!
//! Everything that has to last from frame to frame (bar heights, the fire's heat, raindrops, ...)
//! lives in the `Visualizer`, so each panel has its own. Times are counted with `clock` (the sum of
//! `dt`), not the wall clock, so a test can replay exactly the same frames.
//!
//! # Left and right
//!
//! Many newer looks are drawn for the right panel, whose keyboard-side edge is column 0, and mirrored
//! for the left panel (`keyboard_side`), so the two panels together look like one picture split by
//! the keyboard. Random numbers come from a numpy-compatible generator (`wr_core::nprand`), seeded in
//! tests so the pictures match Python's.
//!
//! # Units used below
//!
//! Levels and `shown`/`peaks` are in bars, 0..34 (`MAX_LEVEL`); most looks divide by 34 to get 0..1.
//! Many animations scale their per-frame steps by `k = dt / 0.04` (capped at 3), the number of 25 fps
//! "ticks" that passed, so they move at the same speed whatever the frame rate.

use std::f64::consts::PI;

use wr_core::nprand::Generator;
use wr_core::pyround::{int, int_round};

use crate::analyzer::{
    default_noise_gate, Analyzer, AUTO_DOWN_DB_PER_SEC, AUTO_LIMIT_DB, AUTO_MIN_SIGNAL_DB, AUTO_UP_DB_PER_SEC,
    FFT_SIZE, MAX_LEVEL,
};
use crate::npmath::{self, clip, mean, py_mod};
use crate::settings::{to_float, to_int, InputMode, Settings, NO_BAR_OPTIONS};

/// Columns of the panel.
pub const WIDTH: usize = 9;
/// Rows of the panel.
pub const HEIGHT: usize = 34;
/// `WIDTH` and `HEIGHT` as floats, for the drawing maths.
const W: f64 = WIDTH as f64;
const H: f64 = HEIGHT as f64;

/// A picture: brightness 0.0-1.0, indexed `[x][y]` like Python's `grid[x, y]`.
pub type Frame = [[f64; HEIGHT]; WIDTH];

/// One stereo sample (left, right).
pub type Stereo = [f64; 2];

/// The calm idle version of each look: how tall its bars are and how long one slow breath takes.
/// Idle bars are 14% of full height (before `idle_boost`).
pub const IDLE_LEVEL: f64 = 0.14;
/// One slow "breath" (the idle picture swelling and shrinking) takes this many seconds.
pub const IDLE_BREATH_SEC: f64 = 5.0;
/// The looks made of beats get one slow beat this often while idle.
pub const IDLE_BEAT_SEC: f64 = 2.5;
/// Seconds a peak mark stays at the top before it starts to fall.
pub const PEAK_HOLD_SEC: f64 = 0.5;

/// Idle bar height multiplier per look: some looks show next to nothing at 14% (a fire needs more fuel
/// to show a flame), and sparkle would be too busy.
fn idle_boost(style: &str) -> f64 {
    match style {
        "bonfire" => 4.5,
        "lava-lamp" => 3.5,
        "pulse" => 2.2,
        "sparkle" => 0.5,
        _ => 1.0,
    }
}

/// A frame is silent when the sum of all bar heights is at most this (microphones always hear a bit).
fn default_silence_sum(mode: InputMode) -> i64 {
    match mode {
        InputMode::Playback => 2,
        InputMode::Microphone => 8,
    }
}

/// The smallest amplitude the waveform looks will stretch to fill the panel, so hiss isn't blown up.
fn wave_min_scale(mode: InputMode) -> f64 {
    match mode {
        InputMode::Playback => 0.002,
        InputMode::Microphone => 0.00002,
    }
}

/// The waveform looks' starting amplitude scale, before they adapt to the sound.
fn wave_start_scale(mode: InputMode) -> f64 {
    match mode {
        InputMode::Playback => 0.05,
        InputMode::Microphone => 0.001,
    }
}

/// (number of bands to measure, the band shown in each of the panel's 9 columns).
///
/// "centre" alone uses 5 bands mirrored around column 4 (bass in the middle); "centre" across both
/// panels uses 9 bands with the bass where the panels meet. Every other look uses 9 bands, or 18 when
/// spanning (the left panel shows bands 0..8, the right one 9..17).
pub fn column_bands(style: &str, span: bool, right: bool) -> (usize, [usize; WIDTH]) {
    let mut columns = [0; WIDTH];
    if style == "centre" {
        if span {
            // Bass where the two panels meet, treble at the outer edges
            for (c, b) in columns.iter_mut().enumerate() {
                *b = if right { c } else { 8 - c };
            }
            return (WIDTH, columns);
        }
        for (c, b) in columns.iter_mut().enumerate() {
            *b = (c as i64 - 4).unsigned_abs() as usize;
        }
        return (5, columns);
    }
    let start = if span && right { WIDTH } else { 0 };
    for (c, b) in columns.iter_mut().enumerate() {
        *b = start + c;
    }
    (if span { 2 * WIDTH } else { WIDTH }, columns)
}

/// One channel of the samples, or both mixed for None.
/// Channel 0 is left, 1 is right (anything higher counts as right). The mix is the average of both.
pub fn mono_or_channel(samples: &[Stereo], channel: Option<usize>) -> Vec<f64> {
    match channel {
        None => samples.iter().map(|s| (s[0] + s[1]) / 2.0).collect(),
        Some(c) => samples.iter().map(|s| s[c.min(1)]).collect(),
    }
}

/// An all-dark picture.
fn zeros() -> Frame {
    [[0.0; HEIGHT]; WIDTH]
}

/// A picture whose every LED is `f(x, y)`.
fn map(f: impl Fn(usize, usize) -> f64) -> Frame {
    let mut g = zeros();
    for (x, column) in g.iter_mut().enumerate() {
        for (y, v) in column.iter_mut().enumerate() {
            *v = f(x, y);
        }
    }
    g
}

/// The picture flipped left to right (column 0 swaps with column 8).
fn mirror(grid: Frame) -> Frame {
    let mut out = grid;
    out.reverse();
    out
}

/// The last n values (all of them when there are fewer).
fn tail(values: &[f64], n: usize) -> &[f64] {
    &values[values.len().saturating_sub(n)..]
}

/// Positions on the panel measured so that a circle fills it: x and y both from -1 to 1.
///
/// Returns (distance from the middle, angle in radians from -pi to pi). The middle is between rows 16
/// and 17 (y = 16.5) and on column 4. Columns are divided by 4.5 and rows by 17, so the panel's edges
/// are at about 1 in both directions even though it is much taller than wide: a "circle" is really an
/// ellipse that fills the panel.
fn polar(x: usize, y: usize) -> (f64, f64) {
    let px = (x as f64 - 4.0) / 4.5;
    let py = (y as f64 - 16.5) / 17.0;
    ((px * px + py * py).sqrt(), py.atan2(px))
}

/// A slope of the Python `grid[low:high + 1, row] = value` kind, for the line looks.
/// Sets columns `low..=high` of one row to `value` (or keeps the brighter of the two when `keep_max`).
/// Columns outside 0..8 are skipped. Used to join neighbouring points of a wave line so it has no gaps.
fn fill_cols(grid: &mut Frame, low: i64, high: i64, row: usize, value: f64, keep_max: bool) {
    let start = low.max(0) as usize;
    let stop = (high + 1).clamp(0, WIDTH as i64) as usize;
    for column in grid.iter_mut().take(stop).skip(start) {
        column[row] = if keep_max { column[row].max(value) } else { value };
    }
}

/// How much each cell differs from its four neighbours (edges reflect); rows x columns.
/// Positive where a cell is lower than its neighbours, negative where higher. It is the "spreading"
/// term of the wave and diffusion equations used by the water look. At the edges a missing neighbour
/// is replaced by the cell itself.
fn laplacian(f: &[[f64; WIDTH]; HEIGHT]) -> [[f64; WIDTH]; HEIGHT] {
    let mut out = [[0.0; WIDTH]; HEIGHT];
    for r in 0..HEIGHT {
        for c in 0..WIDTH {
            let up = f[r.saturating_sub(1)][c];
            let down = f[(r + 1).min(HEIGHT - 1)][c];
            let left = f[r][c.saturating_sub(1)];
            let right = f[r][(c + 1).min(WIDTH - 1)];
            out[r][c] = up + down + left + right - 4.0 * f[r][c];
        }
    }
    out
}

/// The shape of one heartbeat on the heart monitor line, in columns left (-) or right (+) of the
/// middle, one value per step of the trace (about 25 steps a second).
const HEARTBEAT: [f64; 15] = [0.0, 1.0, 1.0, 0.0, -1.0, 4.0, 4.0, -3.0, -2.0, 0.0, 0.0, 1.0, 2.0, 1.0, 0.0];
/// The White Rabbit sprite: 10 rows of 7 columns, '1' = lit. Drawn in columns 1..7.
const RABBIT: [&str; 10] =
    ["0110110", "0110110", "0110110", "0110110", "0111110", "1111111", "1101011", "1111111", "0111110", "0011100"];

/// The state of the looks that remember things between frames (Python's `self.fx`).
/// Fields are created on first use (the `Option`s), so a look that is never shown costs nothing.
#[derive(Default)]
struct Fx {
    /// `clock` at the last idle beat, None before the first.
    idle_tick: Option<f64>,
    /// Bass level (0..1) on the previous frame, for beat detection.
    beat_last: f64,
    /// Slow running average of the bass level (each frame moves it 10% towards the new level).
    beat_average: f64,
    // bonfire
    /// Heat 0..1 per column, with two extra rows below the panel that hold the "fuel".
    heat: Option<Vec<[f64; HEIGHT + 2]>>,
    /// Sparks flying up: (column, row as a float, rows per tick).
    fire_sparks: Vec<(i64, f64, f64)>,
    // beat tunnel
    /// The distance (in rows) each tunnel frame has travelled from the keyboard edge.
    rings: Vec<f64>,
    // fountain
    /// Drops in flight: x (column), y (row), y speed (rows per tick; negative = upwards).
    drops: Vec<[f64; 3]>,
    /// The fountain's last picture, faded each frame to leave trails.
    fountain: Option<Frame>,
    // lava lamp
    /// Four blobs: x, y, x speed, y speed, size.
    blobs: Option<Vec<[f64; 5]>>,
    // heartbeat
    /// The heart monitor line: one offset per row, newest at row 0.
    trace: Option<Vec<f64>>,
    /// The rest of the heartbeat shape still to be drawn.
    queue: std::collections::VecDeque<f64>,
    /// Right panel's treble level on the previous frame (its beats are treble jumps).
    level: f64,
    /// Seconds since the trace last moved on.
    wait: f64,
    // dancing rabbit
    /// How high the rabbit is off the floor (0 = standing).
    jump: f64,
    /// The rabbit's upward speed (falls by gravity each tick).
    speed: f64,
    // water, glow and sparks
    water: Option<WaterState>,
}

/// The water look's state, all as rows x columns (`[y][x]`), the way the Python arrays are laid out.
struct WaterState {
    /// Height of the water surface at each LED.
    water: [[f64; WIDTH]; HEIGHT],
    /// How fast the surface moves up or down at each LED.
    speed: [[f64; WIDTH]; HEIGHT],
    /// The warm light from the middle notes, rising and spreading.
    glow: [[f64; WIDTH]; HEIGHT],
    /// x, y, x speed, y speed, light
    sparks: Vec<[f64; 5]>,
    /// The automatic brightness ("camera exposure") multiplier, 0.2..4.
    exposure: f64,
    /// Last frame's band levels, to detect sudden rises (onsets).
    previous: [f64; WIDTH],
}

/// One panel's look and everything it remembers between frames. Not shared between threads.
pub struct Visualizer {
    /// The settings it was made with.
    pub settings: Settings,
    /// True for the right panel.
    right: bool,
    /// The look's key (see `settings::STYLES`).
    style: String,
    /// Both panels show one wide picture.
    span: bool,
    /// How many frequency bands are measured (see `column_bands`).
    n_bands: usize,
    /// Which band each of the 9 columns shows.
    columns: [usize; WIDTH],
    /// Measures the bands (keeps the automatic gain and the microphone's noise history).
    pub analyzer: Analyzer,
    /// `loudness` at or below this means silence.
    silence_sum: i64,
    /// How fast bars fall, in levels per second.
    fall: f64,
    /// Which channel this panel listens to: Some(0) left, Some(1) right, None both mixed.
    channel: Option<usize>,
    /// The bar heights drawn, per band, 0..34.
    shown: Vec<f64>,
    /// The peak marks, per band, 0..34.
    peaks: Vec<f64>,
    /// `clock` when each band's peak was last pushed up.
    peak_time: Vec<f64>,
    /// The waterfall's picture, moved down one row about 20 times a second.
    trail: Frame,
    /// Seconds since the waterfall last moved.
    trail_wait: f64,
    /// The sparkle look's picture, faded each frame.
    sparkle: Frame,
    /// See `wave_min_scale()`.
    wave_min_scale: f64,
    /// The amplitude that fills the panel in the wave looks; follows the loudest recent sample and
    /// shrinks slowly (by 80% of itself per second) when it gets quieter.
    wave_scale: f64,
    /// The pulse look's glow size, 0..1.
    pulse: f64,
    /// Seconds of frames drawn so far (the sum of all `dt`).
    clock: f64,
    /// numpy-compatible random numbers (sparkle, fire, fountain, lava lamp, heartbeat noise, water).
    rng: Generator,
    /// The sum of the bar heights in the last frame (how much was heard).
    pub loudness: i64,
    /// The latest stereo samples (made-up ones while idle), kept for the wave and meter looks.
    samples: Vec<Stereo>,
    /// The level meter's two bars, in rows 0..34.
    meter: [f64; 2],
    /// The meter's own automatic gain, in dB.
    meter_auto: f64,
    /// The meter's peak marks, in rows.
    meter_peak: [f64; 2],
    /// `clock` when each meter peak was last pushed up.
    meter_peak_time: [f64; 2],
    /// State of the looks that remember more.
    fx: Fx,
    /// True while drawing the calm idle version of the look.
    pub idle: bool,
}

impl Visualizer {
    /// side_right: the right panel (else the left). seed: fixed random numbers for tests, None for real use.
    /// The old equalizer settings (`level-gain`, `noise-gate-level`, `silence-level-sum-threshold`) are
    /// read here; one that isn't a number gives its default.
    pub fn new(settings: &Settings, side_right: bool, seed: Option<u64>) -> Self {
        let s = settings.clone();
        let style = s.style.clone();
        let span = s.span_panels;
        let (n_bands, columns) = column_bands(&style, span, side_right);
        let level_gain = s.level_gain.as_ref().map_or(Some(1.0), to_float).unwrap_or(1.0);
        let noise_gate = s.noise_gate_level.as_ref().and_then(to_int);
        let analyzer = Analyzer::new(n_bands, s.input_mode, s.sensitivity, s.auto_sensitivity, level_gain, noise_gate);
        let silence_sum = s
            .silence_level_sum_threshold
            .as_ref()
            .map_or(Some(default_silence_sum(s.input_mode)), to_int)
            .unwrap_or(default_silence_sum(s.input_mode));
        // Across both panels the spectrum is one sound (both channels mixed); otherwise the left panel
        // shows the left channel and the right panel the right one
        let mut channel = if span { None } else { Some(side_right as usize) };
        if NO_BAR_OPTIONS.contains(&style.as_str()) {
            // Both panels hear the same sound, so the two halves of the picture mirror each other
            channel = None;
        }
        // Only there to use the import (the analyzer picks the default noise gate itself).
        let _ = default_noise_gate;
        Visualizer {
            right: side_right,
            span,
            n_bands,
            columns,
            analyzer,
            silence_sum,
            fall: s.fall_speed.levels_per_sec(),
            channel,
            shown: vec![0.0; n_bands],
            peaks: vec![0.0; n_bands],
            peak_time: vec![0.0; n_bands],
            trail: zeros(),
            trail_wait: 0.0,
            sparkle: zeros(),
            wave_min_scale: wave_min_scale(s.input_mode),
            wave_scale: wave_start_scale(s.input_mode),
            pulse: 0.0,
            clock: 0.0,
            rng: match seed {
                Some(seed) => Generator::new(seed),
                None => Generator::from_entropy(),
            },
            loudness: 0,
            samples: vec![[0.0; 2]; FFT_SIZE],
            meter: [0.0; 2],
            meter_auto: 0.0,
            meter_peak: [0.0; 2],
            meter_peak_time: [0.0; 2],
            fx: Fx::default(),
            idle: false,
            style,
            settings: s,
        }
    }

    /// True when the last frame heard (next to) nothing.
    /// Sum of all bar heights (rounded down) at or below the silence threshold.
    pub fn is_silent(&self) -> bool {
        self.loudness <= self.silence_sum
    }

    /// The next picture from the latest sound (the last FFT_SIZE stereo samples).
    /// `dt` in seconds (at least 1 ms is used). The result is clipped to 0..1 and flipped if asked.
    pub fn frame(&mut self, samples: &[Stereo], dt: f64) -> Frame {
        let dt = dt.max(0.001);
        self.idle = false;
        self.clock += dt;
        self.samples = samples.to_vec();
        let chunk = mono_or_channel(samples, self.channel);
        let levels = self.analyzer.levels(&chunk, dt);
        // Truncated, like Python's int(); measured before the bars' fall smoothing.
        self.loudness = npmath::sum(&levels) as i64;
        // Bars jump up at once and fall at the chosen speed
        for b in 0..self.n_bands {
            self.shown[b] = levels[b].max(self.shown[b] - self.fall * dt);
            if self.shown[b] >= self.peaks[b] {
                self.peaks[b] = self.shown[b];
                self.peak_time[b] = self.clock;
            }
            // After the hold the peak falls at half the bars' speed, but never below the bar.
            if (self.clock - self.peak_time[b]) > PEAK_HOLD_SEC {
                self.peaks[b] = self.shown[b].max(self.peaks[b] - 0.5 * self.fall * dt);
            }
        }
        let grid = self.draw(&chunk, dt);
        self.finish(grid)
    }

    /// The next picture of the look's calm idle version, shown while nothing is heard: the look drawn
    /// from made-up, low, slowly drifting sound, with a slow resting rhythm for the beat looks.
    ///
    /// Drawn by the second `Visualizer` in `MusicPanel`, so this one's real bars aren't disturbed.
    pub fn idle_frame(&mut self, dt: f64) -> Frame {
        let dt = dt.max(0.001);
        self.idle = true;
        self.clock += dt;
        let t = self.clock;
        // Low bars, a slow wave travelling across the bands, gently breathing in and out
        let breath = 0.75 + 0.25 * (2.0 * PI * t / IDLE_BREATH_SEC).sin();
        let base = MAX_LEVEL * IDLE_LEVEL * idle_boost(&self.style) * breath;
        let levels: Vec<f64> = (0..self.n_bands)
            .map(|b| {
                let drift = 0.5 + 0.5 * (t * 1.1 - b as f64 * 0.7).sin();
                base * (0.45 + 0.55 * drift)
            })
            .collect();
        // Never counted as silent while idle, so looks that go dark in silence still draw something.
        self.loudness = (self.silence_sum + 1).max(npmath::sum(&levels) as i64);
        for b in 0..self.n_bands {
            self.peaks[b] = levels[b].max(self.peaks[b] - 0.5 * self.fall * dt);
        }
        self.shown = levels;
        // A slow, small sound wave for the waveform looks
        let n = 1024;
        let amp = 0.35 * breath;
        self.samples = (0..n)
            .map(|i| {
                let phase = i as f64 / n as f64 * 2.0 * PI * 1.5 + t * 1.6;
                [amp * phase.sin(), amp * (phase + 0.8).sin()]
            })
            .collect();
        // The made-up wave has amplitude 0.35 or less, so with scale 1 it stays small on the panel.
        self.wave_scale = 1.0;
        let chunk = mono_or_channel(&self.samples, self.channel);
        let grid = self.draw(&chunk, dt);
        self.finish(grid)
    }

    /// Last step of every frame: upside down if `flip` is set, then every value clipped to 0..1.
    fn finish(&self, mut grid: Frame) -> Frame {
        if self.settings.flip {
            for column in grid.iter_mut() {
                column.reverse();
            }
        }
        for column in grid.iter_mut() {
            for v in column.iter_mut() {
                *v = clip(*v, 0.0, 1.0);
            }
        }
        grid
    }

    /// Draws the current look. `chunk` is the mono samples (for the wave looks). An unknown style draws
    /// classic (settings are normalized, so that shouldn't happen).
    fn draw(&mut self, chunk: &[f64], dt: f64) -> Frame {
        match self.style.as_str() {
            "classic" => self.draw_classic(),
            "bars" | "centre" => self.draw_bars(),
            "dots" => self.draw_dots(),
            "waterfall" => self.draw_waterfall(dt),
            "meter" => self.draw_meter(dt),
            "wave" => self.draw_wave(chunk, dt),
            "mirror-wave" => self.draw_mirror_wave(chunk, dt),
            "double-wave" => self.draw_double_wave(chunk, dt),
            "radial" => self.draw_radial(),
            "radial-fan" => self.draw_radial_fan(),
            "radial-facing" => self.draw_radial_facing(),
            "rings" => self.draw_rings(),
            "pulse" => self.draw_pulse(dt),
            "sparkle" => self.draw_sparkle(dt),
            "bonfire" => self.draw_bonfire(dt),
            "beat-tunnel" => self.draw_beat_tunnel(dt),
            "fountain" => self.draw_fountain(dt),
            "lava-lamp" => self.draw_lava_lamp(dt),
            "heartbeat" => self.draw_heartbeat(dt),
            "dancing-rabbit" => self.draw_dancing_rabbit(dt),
            "water" => self.draw_water(dt),
            _ => self.draw_classic(),
        }
    }

    /// While idle: true once every period seconds (a slow resting rhythm instead of beats).
    fn idle_tick(&mut self, period: f64) -> bool {
        if self.clock - self.fx.idle_tick.unwrap_or(-period) >= period {
            self.fx.idle_tick = Some(self.clock);
            return true;
        }
        false
    }

    /// Bar heights for the panel's 9 columns.
    /// `values` is per band (`shown` or `peaks`); picks the band each column shows.
    fn column_levels(&self, values: &[f64]) -> [f64; WIDTH] {
        self.columns.map(|b| values[b])
    }

    /// With "shaded" on: brightness of the LED `distance` steps from the base of a bar `length` long,
    /// from 0.3 at the base to 1.0 at the tip. Without it (or for an empty bar), 1.0.
    fn shade(&self, length: i64, distance: i64) -> f64 {
        if !self.settings.shaded || length <= 0 {
            return 1.0;
        }
        0.3 + 0.7 * (1.0f64).min((distance + 1) as f64 / length as f64)
    }

    /// "bars" and "centre": bars rising from the bottom row, with an optional peak dot above.
    fn draw_bars(&self) -> Frame {
        let mut grid = zeros();
        let peaks = self.column_levels(&self.peaks);
        for (col, value) in self.column_levels(&self.shown).into_iter().enumerate() {
            let length = int_round(value);
            for i in 0..length {
                grid[col][HEIGHT - 1 - i as usize] = self.shade(length, i);
            }
            if self.settings.peak_dots {
                let peak = int_round(peaks[col]);
                if peak > 0 {
                    // A peak above the bar is drawn one row higher, so a gap separates it from the bar; at the top
                    // row it is drawn on row 0.
                    let up = peak + if peak > length { 1 } else { 0 };
                    grid[col][HEIGHT - up.min(HEIGHT as i64) as usize] = 1.0;
                }
            }
        }
        grid
    }

    /// "classic": bars growing up and down from the middle (rows 16 and 17), half the height each way.
    fn draw_classic(&self) -> Frame {
        let mut grid = zeros();
        let middle = (HEIGHT / 2) as i64;
        let peaks = self.column_levels(&self.peaks);
        for (col, value) in self.column_levels(&self.shown).into_iter().enumerate() {
            let length = int_round(value);
            // An odd length puts the extra LED on the upper half (`below` counts LEDs above the middle line,
            // drawn upwards from row 16; `above` those drawn downwards from row 17).
            let above = length.div_euclid(2);
            let below = length - above;
            let half = below.max(1);
            for i in 0..below {
                grid[col][(middle - 1 - i) as usize] = self.shade(half, i);
            }
            for i in 0..above {
                grid[col][(middle + i) as usize] = self.shade(half, i);
            }
            if self.settings.peak_dots {
                let peak = int_round(peaks[col]);
                if peak > 1 {
                    let reach = (peak + 1).div_euclid(2) + if peak > length { 1 } else { 0 };
                    grid[col][(middle - reach).max(0) as usize] = 1.0;
                    grid[col][(middle + reach - 1).min(HEIGHT as i64 - 1) as usize] = 1.0;
                }
            }
        }
        grid
    }

    /// "dots": only the top of each bar (bright) and the LED under it (dim), with an optional peak dot.
    fn draw_dots(&self) -> Frame {
        let mut grid = zeros();
        let peaks = self.column_levels(&self.peaks);
        for (col, value) in self.column_levels(&self.shown).into_iter().enumerate() {
            let length = int_round(value);
            if length > 0 {
                grid[col][HEIGHT - length as usize] = 1.0;
                if length > 1 {
                    grid[col][HEIGHT - length as usize + 1] = 0.35;
                }
            }
            if self.settings.peak_dots {
                let peak = int_round(peaks[col]);
                if peak > length + 1 {
                    grid[col][HEIGHT - peak as usize] = 0.6;
                }
            }
        }
        grid
    }

    /// "waterfall" (spectrogram): each new row at the top is the current spectrum; older rows move down
    /// and fade by 3% a step. Quiet values under 0.04 are left dark, others at least 0.08 so they show.
    fn draw_waterfall(&mut self, dt: f64) -> Frame {
        // A new row at the top about 20 times a second; older rows move down and fade
        self.trail_wait += dt;
        if self.trail_wait >= 0.05 {
            self.trail_wait = 0.0;
            let mut row = self.column_levels(&self.shown).map(|v| v / MAX_LEVEL);
            if self.settings.shaded {
                row = row.map(|v| v.powf(1.6));
            }
            for (column, r) in self.trail.iter_mut().zip(row) {
                for y in (1..HEIGHT).rev() {
                    column[y] = column[y - 1] * 0.97;
                }
                column[0] = if r > 0.04 { r.max(0.08) } else { 0.0 };
            }
        }
        let mut grid = self.trail;
        if self.settings.peak_dots {
            // The loudest band in the newest row stands out
            let mut best = 0;
            for x in 1..WIDTH {
                if grid[x][0] > grid[best][0] {
                    best = x;
                }
            }
            if grid[best][0] > 0.0 {
                grid[best][0] = 1.0;
            }
        }
        grid
    }

    /// Follows the loudest of the last 1024 samples (after removing their average): jumps up at once,
    /// shrinks by 80% of itself per second, never below `wave_min_scale`.
    fn update_wave_scale(&mut self, chunk: &[f64], dt: f64) {
        let recent = tail(chunk, 1024);
        let peak = if recent.is_empty() {
            0.0
        } else {
            let m = mean(recent);
            recent.iter().map(|v| (v - m).abs()).fold(f64::NEG_INFINITY, f64::max)
        };
        self.wave_scale = peak.max(self.wave_scale * (1.0 - 0.8 * dt)).max(self.wave_min_scale);
    }

    /// "wave": an oscilloscope line running down the panel. 34 points evenly spread over the last 1024
    /// samples (21 ms) become one point per row; the column is 4 + amplitude / scale * 4 (column 4 is
    /// zero). Neighbouring points are joined at half brightness. In silence the line is straight.
    fn draw_wave(&mut self, chunk: &[f64], dt: f64) -> Frame {
        let mut grid = zeros();
        let recent = tail(chunk, 1024);
        if recent.len() < HEIGHT {
            return grid;
        }
        let m = mean(recent);
        let recent: Vec<f64> = recent.iter().map(|v| v - m).collect();
        let mut peak = recent.iter().map(|v| v.abs()).fold(f64::NEG_INFINITY, f64::max);
        // Follow the volume so quiet and loud songs both fill the panel
        self.update_wave_scale(chunk, dt);
        if self.loudness <= self.silence_sum {
            peak = 0.0;
        }
        let cols: Vec<i64> = npmath::spread_indexes(recent.len(), HEIGHT)
            .into_iter()
            .map(|i| {
                if peak == 0.0 {
                    4
                } else {
                    clip((4.0 + recent[i] / self.wave_scale * 4.0).round_ties_even(), 0.0, W - 1.0) as i64
                }
            })
            .collect();
        let mut previous = cols[0];
        for (row, &col) in cols.iter().enumerate() {
            let (low, high) = (previous.min(col), previous.max(col));
            fill_cols(&mut grid, low, high, row, 0.5, false);
            grid[col as usize][row] = 1.0;
            previous = col;
        }
        if self.settings.shaded {
            for v in grid[4].iter_mut() {
                *v = v.max(0.15);
            }
        }
        grid
    }

    /// Loudness of one channel (or both mixed) over the last 2048 samples, as RMS in dB (0 dB = RMS 1);
    /// -120 dB for pure silence.
    fn channel_db(&self, channel: Option<usize>) -> f64 {
        let all = mono_or_channel(&self.samples, channel);
        let x = tail(&all, 2048);
        let rms = if x.is_empty() {
            0.0
        } else {
            let squares: Vec<f64> = x.iter().map(|v| v * v).collect();
            mean(&squares).sqrt()
        };
        if rms > 0.0 {
            20.0 * rms.log10()
        } else {
            -120.0
        }
    }

    /// "meter": two loudness meters (left and right channel, columns 1..3 and 5..7), or one wide meter
    /// (columns 1..7) for this panel's own channel when spanning. Segmented every third row, with peak
    /// marks and faint scale marks every 6 rows at the edges.
    fn draw_meter(&mut self, dt: f64) -> Frame {
        // Loudness of each channel: -60 dB empty, 0 dB full (moved by the sensitivity like the bars)
        let (bottom, top) = (self.analyzer.bottom_db, self.analyzer.top_db);
        let channels: Vec<usize> = if self.span { vec![self.right as usize] } else { vec![0, 1] };
        let mut loud: Vec<f64> = channels.iter().map(|&c| self.channel_db(Some(c)) + self.analyzer.offset_db).collect();
        if self.analyzer.auto {
            let loudest = npmath::max(&loud);
            // Like the bars' automatic sensitivity: the loudest moments reach about 85% of the meter
            let mut target = (-AUTO_LIMIT_DB).max(AUTO_LIMIT_DB.min((top - 0.15 * (top - bottom)) - loudest));
            if loudest < bottom + AUTO_MIN_SIGNAL_DB {
                target = target.min(self.meter_auto);
            }
            let rate = if target < self.meter_auto { AUTO_DOWN_DB_PER_SEC } else { AUTO_UP_DB_PER_SEC };
            let step = rate * dt;
            // Moves at most `step` towards the target in one frame (down fast, up slowly).
            self.meter_auto = (self.meter_auto - step).max((self.meter_auto + step).min(target));
            for db in loud.iter_mut() {
                *db += self.meter_auto;
            }
        }
        for (i, db) in loud.iter().enumerate() {
            // dB mapped linearly onto the 34 rows: `bottom` dB empty, `top` dB full.
            let mut value = 0f64.max(1f64.min((db - bottom) / (top - bottom))) * H;
            if self.analyzer.microphone || self.idle {
                // A microphone is measured against the room's noise, like the bars
                let squares: Vec<f64> = self.shown.iter().map(|v| (v / MAX_LEVEL) * (v / MAX_LEVEL)).collect();
                // The RMS of all bar heights (0..1), boosted by 1.4: this follows the analyzer's noise-floor
                // measurement, which a plain dB figure from a microphone would not.
                value = 1f64.min(1.4 * mean(&squares).sqrt()) * H;
            }
            if self.loudness <= self.silence_sum {
                value = 0.0;
            }
            self.meter[i] = value.max(self.meter[i] - self.fall * dt);
            if self.meter[i] >= self.meter_peak[i] {
                self.meter_peak[i] = self.meter[i];
                self.meter_peak_time[i] = self.clock;
            } else if self.clock - self.meter_peak_time[i] > PEAK_HOLD_SEC * 2.0 {
                self.meter_peak[i] = self.meter[i].max(self.meter_peak[i] - 0.4 * self.fall * dt);
            }
        }
        let mut grid = zeros();
        let spans: Vec<std::ops::Range<usize>> = if self.span { vec![1..8] } else { vec![1..4, 5..8] };
        for (i, cols) in spans.into_iter().enumerate() {
            let length = int_round(self.meter[i]);
            for row in 0..length {
                // Segments, like a hardware meter
                if row % 3 == 2 {
                    continue;
                }
                // Unshaded: the top fifth of the meter is brighter, like the red zone of a hardware meter.
                let shade = if self.settings.shaded {
                    self.shade(length, row)
                } else if row as f64 >= H * 0.8 {
                    1.0
                } else {
                    0.7
                };
                for col in cols.clone() {
                    grid[col][HEIGHT - 1 - row as usize] = shade;
                }
            }
            let peak = int_round(self.meter_peak[i]);
            if peak > 0 {
                for col in cols {
                    grid[col][HEIGHT - peak as usize] = 1.0;
                }
            }
        }
        // Scale marks at the edges
        for y in (0..HEIGHT).rev().step_by(6) {
            grid[0][y] = grid[0][y].max(0.12);
            grid[WIDTH - 1][y] = grid[WIDTH - 1][y].max(0.12);
        }
        grid
    }

    /// 34 points of the recent sound wave, scaled to about -1 ... 1.
    /// All zeros when silent. Uses `wave_scale`, which the caller updates first.
    fn wave_points(&self, channel: Option<usize>) -> Vec<f64> {
        let all = mono_or_channel(&self.samples, channel);
        let recent = tail(&all, 1024);
        let m = if recent.is_empty() { 0.0 } else { mean(recent) };
        let recent: Vec<f64> = recent.iter().map(|v| v - m).collect();
        let peak = recent.iter().map(|v| v.abs()).fold(0.0, f64::max);
        if self.loudness <= self.silence_sum || peak == 0.0 {
            return vec![0.0; HEIGHT];
        }
        let scale = self.wave_scale.max(1e-6);
        npmath::spread_indexes(recent.len(), HEIGHT).into_iter().map(|i| recent[i] / scale).collect()
    }

    /// "mirror-wave": per row, a bar out from column 4 to both sides as far as the wave's size at that
    /// point (up to 4 columns), with a bright tip.
    fn draw_mirror_wave(&mut self, chunk: &[f64], dt: f64) -> Frame {
        self.update_wave_scale(chunk, dt);
        let mut grid = zeros();
        let other = if self.settings.shaded { 0.35 } else { 0.6 };
        for (row, amplitude) in self.wave_points(self.channel).into_iter().enumerate() {
            let reach = int_round(1f64.min(amplitude.abs()) * 4.0) as usize;
            for d in 0..=reach {
                let value = if d == reach { 1.0 } else { other };
                grid[4 - d][row] = grid[4 - d][row].max(value);
                grid[4 + d][row] = grid[4 + d][row].max(value);
            }
        }
        grid
    }

    /// "double-wave": two oscilloscope lines, the left channel around column 2 and the right around
    /// column 6, each within two columns of its centre.
    fn draw_double_wave(&mut self, chunk: &[f64], dt: f64) -> Frame {
        self.update_wave_scale(chunk, dt);
        let mut grid = zeros();
        for (channel, centre) in [(0usize, 2.0f64), (1, 6.0)] {
            let cols: Vec<i64> = self
                .wave_points(Some(channel))
                .into_iter()
                .map(|p| clip((centre + p * 2.0).round_ties_even(), centre - 2.0, centre + 2.0) as i64)
                .collect();
            let mut previous = cols[0];
            for (row, &col) in cols.iter().enumerate() {
                let (low, high) = (previous.min(col), previous.max(col));
                fill_cols(&mut grid, low, high, row, 0.5, true);
                grid[col as usize][row] = 1.0;
                previous = col;
            }
        }
        if self.settings.shaded {
            for v in grid[4].iter_mut() {
                *v = v.max(0.12);
            }
        }
        grid
    }

    /// Bars out from inner to their level along each direction; band(x, y) says which bar each LED is on.
    ///
    /// `radius` is each LED's distance from the centre of the circle (0..about 1), `inner` the radius
    /// where the bars start, and `stretch` how far a full bar reaches past the rest of the radius. The
    /// `glow` test marks the LEDs that light up with the bass in the middle.
    fn radial_from(
        &self,
        radius: &Frame,
        band: &[[usize; HEIGHT]; WIDTH],
        inner: f64,
        stretch: f64,
        glow: impl Fn(usize, usize) -> bool,
    ) -> Frame {
        let bass = if self.n_bands > 0 { self.shown[0] / MAX_LEVEL } else { 0.0 };
        map(|x, y| {
            let r = radius[x][y];
            let b = band[x][y];
            // How far this LED's bar reaches; the LED is lit if it lies between `inner` and that.
            let reach = inner + self.shown[b] / MAX_LEVEL * (1.0 - inner) * stretch;
            let mut v = if r >= inner && r <= reach { 1.0 } else { 0.0 };
            if self.settings.shaded {
                v *= clip(0.3 + 0.7 * (r - inner) / (reach - inner).max(1e-6), 0.0, 1.0);
            }
            if self.settings.peak_dots {
                let peak_reach = inner + self.peaks[b] / MAX_LEVEL * (1.0 - inner) * stretch;
                v = v.max(if (r - peak_reach).abs() < 0.05 { 0.8 } else { 0.0 });
            }
            // The middle glows with the bass
            v.max(if glow(x, y) { 0.25 + 0.75 * bass } else { 0.0 })
        })
    }

    /// Drawn for the right panel, whose keyboard side is column 0; the left panel is its mirror image.
    fn keyboard_side(&self, grid: Frame) -> Frame {
        if self.right {
            grid
        } else {
            mirror(grid)
        }
    }

    /// "radial": bars pointing out from a small circle in the middle. The bands run from 0 at one side of
    /// the circle round to the top band at the opposite side and back again (mirrored), so the picture is
    /// symmetric.
    fn draw_radial(&self) -> Frame {
        let n = self.n_bands;
        let mut radius = zeros();
        let mut band = [[0usize; HEIGHT]; WIDTH];
        for x in 0..WIDTH {
            for y in 0..HEIGHT {
                let (r, angle) = polar(x, y);
                radius[x][y] = r;
                // Bands go round the circle and back (mirrored), so it looks the same on both sides
                let position = py_mod(angle / (2.0 * PI), 1.0);
                // `position` is the angle as a fraction of a turn (0..1); folding gives 0 -> 1 -> 0 around the
                // circle, which picks the band.
                let folded = 1.0 - (2.0 * position - 1.0).abs();
                band[x][y] = ((folded * n as f64) as i64).clamp(0, n as i64 - 1) as usize;
            }
        }
        let inner = 0.18;
        let grid = self.radial_from(&radius, &band, inner, 1.15, |x, y| (radius[x][y] - inner * 0.6).abs() < 0.08);
        // The two panels mirror each other: bass points outwards on both
        self.keyboard_side(grid)
    }

    /// "radial-fan": half a circle per panel, centred just outside the keyboard-side edge: bass straight
    /// up, treble straight down. Columns are divided by 9 and rows by 17, so the fan reaches the outer edge.
    fn draw_radial_fan(&self) -> Frame {
        // Half a circle centred just past the keyboard-side edge, reaching the outer edge
        let n = self.n_bands as i64;
        let mut radius = zeros();
        let mut band = [[0usize; HEIGHT]; WIDTH];
        for x in 0..WIDTH {
            for y in 0..HEIGHT {
                let px = (x as f64 + 0.5) / 9.0;
                let py = (y as f64 - 16.5) / 17.0;
                radius[x][y] = (px * px + py * py).sqrt();
                // 0 = straight up, 1 = straight down
                // The angle from straight up (0) to straight down (1); px is never negative, so it stays in 0..1.
                let direction = px.atan2(-py) / PI;
                band[x][y] = ((direction * n as f64) as i64).clamp(0, n - 1) as usize;
            }
        }
        let inner = 0.15;
        let grid = self.radial_from(&radius, &band, inner, 1.1, |x, y| radius[x][y] < inner * 0.9);
        self.keyboard_side(grid)
    }

    /// "radial-facing": a full circle per panel with the bass bars pointing at the keyboard and the
    /// treble outwards, the same above and below the middle.
    fn draw_radial_facing(&self) -> Frame {
        let n = self.n_bands as i64;
        let mut radius = zeros();
        let mut band = [[0usize; HEIGHT]; WIDTH];
        for x in 0..WIDTH {
            for y in 0..HEIGHT {
                let (r, angle) = polar(x, y);
                radius[x][y] = r;
                // 0 = towards the keyboard (bass), 1 = outwards (treble), the same above and below
                let direction = 1.0 - angle.abs() / PI;
                band[x][y] = ((direction * n as f64) as i64).clamp(0, n - 1) as usize;
            }
        }
        let inner = 0.18;
        let grid = self.radial_from(&radius, &band, inner, 1.15, |x, y| (radius[x][y] - inner * 0.6).abs() < 0.08);
        self.keyboard_side(grid)
    }

    /// "rings": rings around the middle, one per band, bass in the middle; each ring is as bright as its
    /// band is loud. Beyond the last ring is dark. Very dim LEDs (0.05 or less) are turned off.
    fn draw_rings(&self) -> Frame {
        let n = self.n_bands;
        map(|x, y| {
            let (radius, _) = polar(x, y);
            // Which ring (and how far through it) this LED is: the rings fill radius 0..1.05 evenly.
            let position = radius / 1.05 * n as f64;
            let ring = (position as i64).clamp(0, n as i64) as usize;
            let level = |values: &[f64]| if ring < n { values[ring] / MAX_LEVEL } else { 0.0 };
            let mut v = level(&self.shown);
            if !self.settings.shaded {
                // Thin bright rings with gaps between them
                if py_mod(position, 1.0) >= 0.6 {
                    v *= 0.15;
                }
            }
            if self.settings.peak_dots {
                let lit = if v > 0.0 { 1.0 } else { 0.0 };
                v = v.max(if level(&self.peaks) > 0.85 { 1.0 } else { 0.0 } * lit);
            }
            if v > 0.05 {
                v
            } else {
                0.0
            }
        })
    }

    /// Average height of the lowest third of the bands, 0..1.
    fn bass(&self) -> f64 {
        mean(&self.shown[..(self.n_bands / 3).max(1)]) / MAX_LEVEL
    }

    /// Average height of the highest third of the bands, 0..1.
    fn treble(&self) -> f64 {
        mean(&self.shown[self.n_bands - (self.n_bands / 3).max(1)..]) / MAX_LEVEL
    }

    /// Average height of all bands, 0..1.
    fn loud(&self) -> f64 {
        mean(&self.shown) / MAX_LEVEL
    }

    /// "pulse": a round glow in the middle whose size follows mostly the bass. Brightest in the middle,
    /// fading to the edge; an optional thin ring marks the edge.
    fn draw_pulse(&mut self, dt: f64) -> Frame {
        let bass = self.bass();
        let overall = self.loud();
        let target = 1f64.min(0.65 * bass + 0.5 * overall);
        // Swell at once, shrink smoothly
        self.pulse = target.max(self.pulse - (self.fall / MAX_LEVEL) * 0.5 * dt);
        if self.pulse <= 0.02 {
            return zeros();
        }
        let size = 0.08 + 1.1 * self.pulse;
        let shaded = self.settings.shaded;
        let ring = self.settings.peak_dots && self.pulse > 0.05;
        let pulse = self.pulse;
        map(|x, y| {
            let (radius, _) = polar(x, y);
            let mut glow = clip(1.0 - radius / size.max(1e-6), 0.0, 1.0);
            if shaded {
                glow = glow.sqrt();
            }
            let mut v = if glow > 0.0 { 0.15 + 0.85 * glow } else { 0.0 } * (0.4 + 0.6 * pulse);
            if ring {
                // A thin ring at the edge of the glow
                v = v.max(if (radius - size).abs() < 0.08 { 1.0 } else { 0.0 });
            }
            v
        })
    }

    /// "sparkle": LEDs light up at random places and fade quickly (each frame keeps 1 - 4 x dt of it); the
    /// louder, the more new ones per frame (about 40 x loudness per 30 ms).
    fn draw_sparkle(&mut self, dt: f64) -> Frame {
        let fade = 0f64.max(1.0 - 4.0 * dt);
        for column in self.sparkle.iter_mut() {
            for v in column.iter_mut() {
                *v *= fade;
            }
        }
        let overall = self.loud();
        let count = int_round(overall * 40.0 * dt / 0.03).max(0) as usize;
        if count > 0 {
            let cols = self.rng.integers_n(0, WIDTH as i64, count);
            let rows: Vec<i64> = if self.settings.shaded {
                // Louder sparkles reach higher up the panel
                let reach = 1i64.max(int(H * 1f64.min(overall * 1.5)));
                self.rng.integers_n(0, reach, count).into_iter().map(|r| HEIGHT as i64 - 1 - r).collect()
            } else {
                self.rng.integers_n(0, HEIGHT as i64, count)
            };
            for (c, r) in cols.into_iter().zip(rows) {
                self.sparkle[c as usize][r as usize] = 1.0;
            }
        }
        let mut grid = self.sparkle;
        if self.settings.peak_dots {
            for (col, value) in self.column_levels(&self.peaks).into_iter().enumerate() {
                let peak = int_round(value);
                if peak > 0 {
                    grid[col][HEIGHT - peak as usize] = 1.0;
                }
            }
        }
        grid
    }

    // The newer looks. They are drawn for the right panel (keyboard side is column 0); the left panel
    // gets the mirror image, so the two panels look like one picture split by the keyboard.

    /// Loudness of each of the panel's 9 columns, 0 ... 1.
    ///
    /// Column 0 is the lowest band shown (the keyboard side on the right panel).
    fn band_levels(&self) -> [f64; WIDTH] {
        self.column_levels(&self.shown).map(|v| v / MAX_LEVEL)
    }

    /// True on the frame a beat starts: the bass jumps up above its recent average.
    ///
    /// A beat: the bass rose by more than 0.12 since the last frame, is above its running average and
    /// above 0.15. While idle, a slow regular beat every `IDLE_BEAT_SEC` instead. Updates the running
    /// values, so call it once per frame.
    fn beat(&mut self) -> bool {
        if self.idle {
            return self.idle_tick(IDLE_BEAT_SEC);
        }
        let level = self.bass();
        let rise = level - self.fx.beat_last;
        let beat = rise > 0.12 && level > self.fx.beat_average && level > 0.15;
        self.fx.beat_last = level;
        self.fx.beat_average = self.fx.beat_average * 0.9 + level * 0.1;
        beat
    }

    /// "bonfire": a classic fire effect. Each column's band level feeds heat into the two rows below the
    /// panel (with random flicker); `spread_heat` moves heat upwards and cools it; brightness is heat to
    /// the power 1.6 (so the cooler top fades out). On a beat three sparks fly up from rows 20..26 and
    /// drift sideways.
    fn draw_bonfire(&mut self, dt: f64) -> Frame {
        let k = 3f64.min(dt / 0.04);
        let levels = self.band_levels();
        let mut heat = self.fx.heat.take().unwrap_or_else(|| vec![[0.0; HEIGHT + 2]; WIDTH]);
        let fuel = self.rng.random_n(WIDTH * 2);
        for x in 0..WIDTH {
            for j in 0..2 {
                heat[x][HEIGHT + j] = clip(levels[x] * 1.4, 0.0, 1.0) * (0.6 + 0.4 * fuel[x * 2 + j]);
            }
        }
        spread_heat(&mut heat, &mut self.rng, k, 0.14);
        if self.beat() {
            for _ in 0..3 {
                let x = self.rng.integers(0, WIDTH as i64);
                let y = 20.0 + self.rng.random() * 6.0;
                let speed = 0.8 + self.rng.random() * 0.6;
                self.fx.fire_sparks.push((x, y, speed));
            }
        }
        let mut grid = map(|x, y| clip(heat[x][y] * 1.25, 0.0, 1.0).powf(1.6));
        let mut alive = Vec::new();
        for (mut sx, mut sy, speed) in std::mem::take(&mut self.fx.fire_sparks) {
            sy -= speed * k;
            if sy > 0.0 {
                grid[sx as usize][int(sy) as usize] = 1.0;
                if self.rng.random() < 0.3 {
                    sx = (sx + self.rng.integers(-1, 2)).clamp(0, WIDTH as i64 - 1);
                }
                alive.push((sx, sy, speed));
            }
        }
        self.fx.fire_sparks = alive;
        self.fx.heat = Some(heat);
        self.keyboard_side(grid)
    }

    /// "beat-tunnel": every beat starts a rectangular frame at the keyboard edge that moves outwards,
    /// speeding up as it goes (like flying down a tunnel), and disappears 22 rows out; a soft glow by the
    /// keyboard breathes with the music.
    fn draw_beat_tunnel(&mut self, dt: f64) -> Frame {
        let k = 3f64.min(dt / 0.04);
        let loud = self.loud();
        if self.beat() {
            self.fx.rings.push(0.0);
        }
        // Measured from just past the keyboard edge (the gap counts as two columns), as a rectangle
        // "Distance" is the larger of the column distance (times 1.8, as columns are further apart than
        // rows look) and the row distance from the middle, so equal distances form a rectangle.
        let distance = |x: usize, y: usize| ((x as f64 + 1.5).abs() * 1.8).max((y as f64 - 16.5).abs());
        let mut grid = zeros();
        let mut alive = Vec::new();
        for mut r in std::mem::take(&mut self.fx.rings) {
            r += (0.6 + r * 0.06) * k;
            let strength = 1f64.min(0.5 + loud);
            for (x, column) in grid.iter_mut().enumerate() {
                for (y, v) in column.iter_mut().enumerate() {
                    *v = v.max(clip(1.0 - (distance(x, y) - r).abs() / 1.3, 0.0, 1.0) * strength);
                }
            }
            if r < 22.0 {
                alive.push(r);
            }
        }
        self.fx.rings = alive;
        // A glow by the keyboard that breathes with the music
        let glow = |x, y| clip(1.0 - distance(x, y) / (2.0 + loud * 6.0), 0.0, 1.0) * 0.5 * 1f64.min(loud * 3.0);
        let grid = map(|x, y| grid[x][y].max(glow(x, y)));
        self.keyboard_side(grid)
    }

    /// "fountain": each column sometimes throws a drop up from the bottom row (more often and higher the
    /// louder its band); drops slow down and fall back under gravity, and leave fading trails (the old
    /// picture is kept at 40% per tick).
    fn draw_fountain(&mut self, dt: f64) -> Frame {
        let k = 3f64.min(dt / 0.04);
        for (col, level) in self.band_levels().into_iter().enumerate() {
            if level > 0.02 && self.rng.random() < (0.04 + level * 0.3) * k {
                let speed = (0.4 + level * 1.45) * (0.8 + 0.3 * self.rng.random());
                let x = col as f64 + (self.rng.random() - 0.5) * 0.4;
                self.fx.drops.push([x, H - 1.0, -speed]);
            }
        }
        let fade = 0.4f64.powf(k);
        let mut grid = self.fx.fountain.unwrap_or_else(zeros);
        for column in grid.iter_mut() {
            for v in column.iter_mut() {
                *v *= fade;
            }
        }
        let mut alive = Vec::new();
        for mut drop in std::mem::take(&mut self.fx.drops) {
            drop[2] += 0.06 * k;
            drop[1] += drop[2] * k;
            let (col, row) = (int_round(drop[0]), int_round(drop[1]));
            if (0..HEIGHT as i64).contains(&row) && (0..WIDTH as i64).contains(&col) {
                grid[col as usize][row as usize] = 1.0;
                alive.push(drop);
            }
        }
        self.fx.drops = alive;
        self.fx.fountain = Some(grid);
        self.keyboard_side(grid)
    }

    /// "lava-lamp": four blobs drift around and bounce off the edges, faster when it is loud, bigger with
    /// the bass. Each LED adds up every blob's size^2 / distance^2 (a "metaball" field), so blobs that
    /// come close merge smoothly; the LED is lit where the sum is over 0.8.
    fn draw_lava_lamp(&mut self, dt: f64) -> Frame {
        let k = 3f64.min(dt / 0.04);
        if self.fx.blobs.is_none() {
            let r = self.rng.random_n(20);
            self.fx.blobs = Some(
                r.chunks(5)
                    .map(|v| [v[0] * 9.0, v[1] * 34.0, v[2] * 0.3 - 0.15, v[3] * 0.6 - 0.3, 2.0 + v[4] * 1.5])
                    .collect(),
            );
        }
        let bass = self.bass();
        let loud = self.loud();
        let mut field = zeros();
        for blob in self.fx.blobs.as_mut().unwrap() {
            blob[0] += blob[2] * k;
            blob[1] += blob[3] * (0.6 + loud * 2.0) * k;
            if !(0.0 < blob[0] && blob[0] < W - 1.0) {
                blob[2] = -blob[2];
                blob[0] = blob[0].max(0.0).min(W - 1.0);
            }
            if !(0.0 < blob[1] && blob[1] < H - 1.0) {
                blob[3] = -blob[3];
                blob[1] = blob[1].max(0.0).min(H - 1.0);
            }
            let size = blob[4] * (0.25 + bass * 1.4);
            for (x, column) in field.iter_mut().enumerate() {
                for (y, v) in column.iter_mut().enumerate() {
                    let dx = x as f64 - blob[0];
                    let dy = y as f64 - blob[1];
                    *v += size * size / (dx * dx * 1.6 + dy * dy + 0.5);
                }
            }
        }
        self.keyboard_side(map(|x, y| clip((field[x][y] - 0.8) / 1.2, 0.0, 1.0)))
    }

    /// "heartbeat": a heart monitor line running down the panel (25 steps a second), with a heartbeat
    /// shape on each beat: bass beats on the left panel, treble jumps on the right. Fades towards the
    /// bottom. Not mirrored: the line is centred on column 4 on both panels.
    fn draw_heartbeat(&mut self, dt: f64) -> Frame {
        let level = if self.right { self.treble() } else { self.bass() };
        let hit = if !self.right {
            self.beat()
        } else {
            let hit = if self.idle { self.beat() } else { level - self.fx.level > 0.08 && level > 0.1 };
            self.fx.level = level;
            hit
        };
        if hit && self.fx.queue.is_empty() {
            self.fx.queue.extend(HEARTBEAT.iter().map(|v| v * (0.5 + level)));
        }
        let mut trace = self.fx.trace.take().unwrap_or_else(|| vec![0.0; HEIGHT]);
        // The line moves on about 25 times a second, whatever the frame rate
        self.fx.wait += dt;
        while self.fx.wait >= 0.04 {
            self.fx.wait -= 0.04;
            let noise = if self.loudness > self.silence_sum { (self.rng.random() - 0.5) * 0.4 } else { 0.0 };
            // The newest value goes in at the top, the oldest falls off the bottom.
            trace.insert(0, self.fx.queue.pop_front().unwrap_or(noise));
            trace.pop();
        }
        let mut grid = zeros();
        for row in 0..HEIGHT {
            let fade = 1.0 - row as f64 / H * 0.85;
            let here = int_round(4.0 + trace[row]);
            let there = if row + 1 < HEIGHT { int_round(4.0 + trace[row + 1]) } else { here };
            let (low, high) = (here.min(there), here.max(there));
            fill_cols(&mut grid, low.max(0), high.min(WIDTH as i64 - 1), row, fade, false);
        }
        self.fx.trace = Some(trace);
        grid
    }

    /// "dancing-rabbit": the White Rabbit jumps on the beat (higher with more bass) over a small spectrum
    /// at the bottom (up to 4 rows).
    fn draw_dancing_rabbit(&mut self, dt: f64) -> Frame {
        let k = 3f64.min(dt / 0.04);
        let mut grid = zeros();
        // The floor: a small spectrum, up to 4 rows tall
        for (col, level) in self.band_levels().into_iter().enumerate() {
            let h = int_round(level * 4.0);
            if h > 0 {
                for v in grid[col][HEIGHT - h as usize..].iter_mut() {
                    *v = 0.45;
                }
            }
        }
        let (mut jump, mut speed) = (self.fx.jump, self.fx.speed);
        if self.beat() && jump == 0.0 {
            speed = 1.6 + self.bass() * 1.4;
        }
        // Simple physics per tick: height += speed, speed -= gravity 0.5, landing at height 0.
        jump = 0f64.max(jump + speed * k);
        speed = if jump > 0.0 { speed - 0.5 * k } else { 0.0 };
        self.fx.jump = jump;
        self.fx.speed = speed;
        let top = int_round(H - RABBIT.len() as f64 - 4.0 - jump * 1.6);
        for (dy, line) in RABBIT.iter().enumerate() {
            for (dx, c) in line.chars().enumerate() {
                let y = top + dy as i64;
                if c == '1' && (0..HEIGHT as i64).contains(&y) {
                    grid[1 + dx][y as usize] = 1.0;
                }
            }
        }
        self.keyboard_side(grid)
    }

    /// "water": three layers. The bass pushes a wobbling spot in the lower part of a simulated water
    /// surface (a damped wave equation); the middle notes add a warm glow that rises and spreads; sudden
    /// treble throws sparks that fall back under gravity. Light is the glow, plus brightness where the
    /// surface curves or slopes (like light caught by ripples), plus the sparks; then an automatic
    /// exposure keeps the picture well lit whether the music is quiet or loud.
    fn draw_water(&mut self, dt: f64) -> Frame {
        // Worked out in rows x columns for the right panel, then turned into the panel's columns x rows
        // Long frames are capped so the simulation can't blow up after a pause.
        let dt = dt.min(0.1);
        let mut st = self.fx.water.take().unwrap_or(WaterState {
            water: [[0.0; WIDTH]; HEIGHT],
            speed: [[0.0; WIDTH]; HEIGHT],
            glow: [[0.0; WIDTH]; HEIGHT],
            sparks: Vec::new(),
            exposure: 1.0,
            previous: [0.0; WIDTH],
        });
        let loud = self.band_levels();
        let mut onset = [0.0; WIDTH];
        for i in 0..WIDTH {
            onset[i] = clip((loud[i] - st.previous[i]) * 4.0, 0.0, 1.0);
        }
        st.previous = loud;
        let (bass, mids, treble) = (mean(&loud[..3]), mean(&loud[3..6]), mean(&loud[6..]));
        let bass_onset = npmath::max(&onset[..3]);
        let treble_onset = npmath::max(&onset[6..]);
        let t = self.clock;
        // The bass: a wobbling spot low in the panel pushes the water
        let (sx, sy) = (4.0 + 2.5 * (t * 0.7).sin(), 26.0 + 3.0 * (t * 0.43).sin());
        let spot = |cx: f64, cy: f64, spread: f64| {
            let mut out = [[0.0; WIDTH]; HEIGHT];
            for (r, row) in out.iter_mut().enumerate() {
                for (c, v) in row.iter_mut().enumerate() {
                    let dx = c as f64 - cx;
                    let dy = r as f64 - cy;
                    *v = (-(dx * dx + dy * dy) / spread).exp();
                }
            }
            out
        };
        let blob = spot(sx, sy, 6.0);
        // A kick on each sudden bass rise, plus a steady wobble (12 rad/s, about 2 Hz) as long as there
        // is bass.
        let drive = bass_onset * 400.0 + bass * 30.0 * (t * 12.0).sin();
        // Four small steps per frame keep the wave equation stable. Each cell is pulled towards its
        // neighbours (40 x laplacian = wave speed), slowed by friction (3 x speed) and pulled back to
        // flat (4 x height), and pushed by the bass spot.
        for _ in 0..4 {
            let step = dt / 4.0;
            let lap = laplacian(&st.water);
            for r in 0..HEIGHT {
                for c in 0..WIDTH {
                    let mut push = 40.0 * lap[r][c] - 3.0 * st.speed[r][c] - 4.0 * st.water[r][c];
                    push += blob[r][c] * drive;
                    st.speed[r][c] += step * push;
                }
            }
            for r in 0..HEIGHT {
                for c in 0..WIDTH {
                    st.water[r][c] += step * st.speed[r][c];
                }
            }
        }
        // The middle notes: a warm light that rises and spreads
        let (mx, my) = (4.0 + 3.0 * (t * 0.9 + 1.0).sin(), 22.0 + 6.0 * (t * 0.31).sin());
        let warm = spot(mx, my, 8.0);
        let mut glow = st.glow;
        for r in 0..HEIGHT {
            for c in 0..WIDTH {
                glow[r][c] += warm[r][c] * mids * 0.7;
            }
        }
        for r in 0..HEIGHT {
            for c in 0..WIDTH {
                glow[r][c] += blob[r][c] * bass_onset * 1.5;
            }
        }
        let lap = laplacian(&glow);
        for r in 0..HEIGHT {
            for c in 0..WIDTH {
                glow[r][c] += 0.18 * lap[r][c];
            }
        }
        // The glow fades with a time constant of 0.18 s and moves up a quarter row per frame.
        let fade = (-dt / 0.18).exp();
        let mut risen = [[0.0; WIDTH]; HEIGHT];
        for r in 0..HEIGHT {
            for c in 0..WIDTH {
                let below = if r + 1 < HEIGHT { glow[r + 1][c] } else { 0.0 };
                risen[r][c] = (below * 0.25 + glow[r][c] * 0.75) * fade;
            }
        }
        st.glow = risen;
        // The treble: sparks thrown up from the glow, falling back down
        let lucky = if self.rng.random() < treble * 1.5 { 2.0 } else { 0.0 };
        // Up to 10 sparks for a sudden treble rise, plus 2 now and then while there is treble.
        let n = int(treble_onset * 10.0 + lucky).max(0) as usize;
        if n > 0 {
            let xs = self.rng.uniform_n(-1.5, 1.5, n);
            let vxs = self.rng.uniform_n(-6.0, 6.0, n);
            let vys = self.rng.uniform_n(-28.0, -12.0, n);
            for i in 0..n {
                st.sparks.push([mx + xs[i], my, vxs[i], vys[i], 1.0]);
            }
        }
        let fade = (-dt / 0.5).exp();
        for s in st.sparks.iter_mut() {
            // Gravity: 30 rows per second per second (rows grow downwards, so + is down).
            s[3] += 30.0 * dt;
            s[0] += s[2] * dt;
            s[1] += s[3] * dt;
            s[4] *= fade;
        }
        st.sparks.retain(|s| s[4] > 0.05 && s[1] > -2.0 && s[1] < H && s[0] > -1.0 && s[0] < W);
        // Light: the glow, bright edges where the water bends, and the sparks
        // Where the surface curves (a crest: negative laplacian) or slopes, it catches the light.
        let lap = laplacian(&st.water);
        let mut lit = [[0.0; WIDTH]; HEIGHT];
        for r in 0..HEIGHT {
            for c in 0..WIDTH {
                let f = &st.water;
                let gy = if r == 0 {
                    f[1][c] - f[0][c]
                } else if r == HEIGHT - 1 {
                    f[r][c] - f[r - 1][c]
                } else {
                    (f[r + 1][c] - f[r - 1][c]) / 2.0
                };
                let gx = if c == 0 {
                    f[r][1] - f[r][0]
                } else if c == WIDTH - 1 {
                    f[r][c] - f[r][c - 1]
                } else {
                    (f[r][c + 1] - f[r][c - 1]) / 2.0
                };
                lit[r][c] = st.glow[r][c] + (-lap[r][c]).max(0.0) * 0.6 + gx.hypot(gy) * 0.4;
            }
        }
        for s in &st.sparks {
            let (xi, yi) = (int(s[0]), int(s[1]));
            if (0..WIDTH as i64).contains(&xi) && (0..HEIGHT as i64).contains(&yi) {
                lit[yi as usize][xi as usize] += s[4] * 3.0;
            }
        }
        // Like a camera: quiet music is turned up, loud music down
        let flat: Vec<f64> = lit.iter().flatten().copied().collect();
        // Exposure aims to put the brightest 2% of the light at 0.8, within 0.2x..4x, and moves 15% of
        // the way there each frame so it adapts smoothly.
        let bright = npmath::percentile(&flat, 98.0).max(1.5 * mean(&flat)).max(1e-3);
        st.exposure += ((0.8 / bright).max(0.2).min(4.0) - st.exposure) * 0.15;
        let exposure = st.exposure;
        self.fx.water = Some(st);
        let grid = map(|x, y| {
            // A soft "film" curve: bright light saturates gently instead of clipping. The lowest 6% is cut
            // so that faint haze doesn't light every LED.
            let value = 1.0 - (-exposure * lit[y][x] * 1.6).exp();
            clip((value - 0.06).max(0.0) / 0.94, 0.0, 1.0)
        });
        self.keyboard_side(grid)
    }
}

/// One step of a classic fire (Python's `animations.spread_heat`): each cell takes the average of the
/// cells below it, minus a little. heat is WIDTH x (HEIGHT + 2); the bottom two rows are the fuel.
///
/// `k` scales the cooling to the frame time; `cooling` is how much random cooling a cell can get (the
/// cube of a random number makes most cells cool little and a few a lot, which gives flickering
/// tongues). Values are clipped to 0..1.
pub fn spread_heat(heat: &mut [[f64; HEIGHT + 2]], rng: &mut Generator, k: f64, cooling: f64) {
    let cool = rng.random_n(WIDTH * HEIGHT);
    // Every new value is computed from last step's values, not from ones already updated this step.
    let old = heat.to_vec();
    for x in 0..WIDTH {
        let left = &old[x.saturating_sub(1)];
        let right = &old[(x + 1).min(WIDTH - 1)];
        for y in 0..HEIGHT {
            let c = (cool[x * HEIGHT + y].powf(3.0) * cooling + 0.008) * k;
            // Weighted average of the cell below (twice), the cells below-left and below-right, and the cell
            // two below; so heat rises and spreads a little sideways.
            heat[x][y] = (old[x][y + 1] * 2.0 + left[y + 1] + right[y + 1] + old[x][y + 2]) / 5.0 - c;
        }
    }
    for column in heat.iter_mut() {
        for v in column.iter_mut() {
            *v = clip(*v, 0.0, 1.0);
        }
    }
}
