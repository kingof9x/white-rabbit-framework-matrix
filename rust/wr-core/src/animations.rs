//! Animations for the panels, and the old-TV switch-off / switch-on effect, a port of
//! `led_mon/animations.py`.
//!
//! The animations (fire, plasma, DNA helix, snowfall, chevrons, bouncing ball) are a whole-panel app
//! called "animation" in the layout (args: look). Each look's `frame(dt)` returns the next picture as
//! a 9 x 34 [`Frame`] of brightness 0.0-1.0 (column, row), moving on by `dt` seconds, so it runs at
//! the same speed however often the monitor draws. [`to_leds`] turns a frame into LED values.
//!
//! [`tv_off_frames`] / [`tv_on_frames`] give the frames of an old TV turning off (the picture
//! squeezes into a bright line, then a dot that fades) and back on (the reverse), played when the
//! panels turn off or on.
//!
//! The random looks use numpy's `default_rng`, number for number ([`crate::nprand`]), so a look with
//! the same seed draws exactly what the Python one draws.

use std::f64::consts::PI;

use crate::grid::{Grid, HEIGHT, WIDTH};
use crate::nprand::Generator;
use crate::pyround::{int, int_round, round_half_even};

/// A picture of brightness 0.0-1.0, indexed `[x][y]` like the Python array `frame[x, y]`.
pub type Frame = [[f64; HEIGHT]; WIDTH];

/// A fire's heat: WIDTH x (HEIGHT + 2), the bottom two rows are the fuel.
pub type Heat = [[f64; HEIGHT + 2]; WIDTH];

fn blank() -> Frame {
    [[0.0; HEIGHT]; WIDTH]
}

/// Python's `a % b` for floats: the result takes the sign of `b`.
fn py_mod(a: f64, b: f64) -> f64 {
    let m = a % b;
    if m != 0.0 && (m < 0.0) != (b < 0.0) {
        m + b
    } else {
        m
    }
}

/// One step of a classic fire: each cell takes the average of the cells below it, minus a little.
///
/// The bottom two rows of `heat` are the fuel, already filled in by the caller. `k` scales the
/// cooling for longer steps. Shared by the Fire animation and the Bonfire music look (the Python
/// default for `cooling` is 0.14).
pub fn spread_heat(heat: &mut Heat, rng: &mut Generator, k: f64, cooling: f64) {
    let old = *heat;
    // rng.random((WIDTH, HEIGHT)) fills in C order: x by x, y by y within each x
    let r = rng.random_n(WIDTH * HEIGHT);
    for x in 0..WIDTH {
        let left = &old[x.saturating_sub(1)];
        let right = &old[(x + 1).min(WIDTH - 1)];
        for y in 0..HEIGHT {
            let cool = (r[x * HEIGHT + y].powf(3.0) * cooling + 0.008) * k;
            heat[x][y] = (old[x][y + 1] * 2.0 + left[y + 1] + right[y + 1] + old[x][y + 2]) / 5.0 - cool;
        }
    }
    for v in heat.iter_mut().flatten() {
        *v = v.clamp(0.0, 1.0);
    }
}

/// How often a look moves on: a steady 25 steps a second, whatever dt the caller gives.
pub const LOOK_STEP: f64 = 0.04;

/// The clock every look keeps: its random numbers, time and the time still owed to it.
#[derive(Clone, Debug)]
pub struct LookState {
    pub rng: Generator,
    pub t: f64,
    owed: f64,
}

impl LookState {
    /// `seed` is numpy's `default_rng(seed)` seed; None seeds it from the clock.
    pub fn new(seed: Option<u64>) -> LookState {
        LookState { rng: seed.map_or_else(Generator::from_entropy, Generator::new), t: 0.0, owed: 0.0 }
    }
}

/// One animation. `frame(dt)` steps it at [`LOOK_STEP`] for as long as `dt` covers and draws it.
pub trait Look: Send {
    fn state(&mut self) -> &mut LookState;

    /// Moves the look on by one [`LOOK_STEP`] (after `t` has moved on).
    fn step(&mut self) {}

    /// The picture now.
    fn draw(&self) -> Frame;

    /// Moves on by `dt` seconds and returns the picture.
    fn frame(&mut self, dt: f64) -> Frame {
        // A long pause (the window was hidden, the PC slept) doesn't fast-forward more than a second
        let s = self.state();
        s.owed += dt.clamp(0.0, 1.0);
        while self.state().owed >= LOOK_STEP - 1e-9 {
            let s = self.state();
            s.owed -= LOOK_STEP;
            s.t += LOOK_STEP;
            self.step();
        }
        self.draw()
    }
}

/// Flames rising from the bottom of the panel, about a third of the way up.
pub struct Fire {
    s: LookState,
    heat: Heat,
}

impl Fire {
    pub fn new(seed: Option<u64>) -> Fire {
        Fire { s: LookState::new(seed), heat: [[0.0; HEIGHT + 2]; WIDTH] }
    }
}

impl Look for Fire {
    fn state(&mut self) -> &mut LookState {
        &mut self.s
    }

    fn step(&mut self) {
        // Fuel along the bottom: most columns burning, a few dying down for a moment, so tongues of flame rise
        let r = self.s.rng.random_n(WIDTH);
        for (column, v) in self.heat.iter_mut().zip(r) {
            let fuel = if v < 0.7 { 1.0 } else { 0.2 };
            column[HEIGHT] = fuel;
            column[HEIGHT + 1] = fuel;
        }
        spread_heat(&mut self.heat, &mut self.s.rng, 1.0, 0.2);
    }

    fn draw(&self) -> Frame {
        let mut out = blank();
        for (o, h) in out.iter_mut().zip(&self.heat) {
            for (o, h) in o.iter_mut().zip(h) {
                *o = (h * 1.5).clamp(0.0, 1.0).powf(1.8);
            }
        }
        out
    }
}

/// Soft blobs of light that drift and merge, from a few sine waves added together.
pub struct Plasma {
    s: LookState,
}

impl Plasma {
    pub fn new(seed: Option<u64>) -> Plasma {
        Plasma { s: LookState::new(seed) }
    }
}

impl Look for Plasma {
    fn state(&mut self) -> &mut LookState {
        &mut self.s
    }

    fn draw(&self) -> Frame {
        let t = self.s.t;
        let cx = 3.0 * (t * 0.5).sin();
        let cy = 12.0 * (t * 0.37).cos();
        let mut out = blank();
        for (xi, column) in out.iter_mut().enumerate() {
            let x = xi as f64;
            for (yi, o) in column.iter_mut().enumerate() {
                let y = yi as f64;
                let v = (x / 2.6 + t * 1.1).sin()
                    + (y / 4.5 - t * 0.9).sin()
                    + ((x + y) / 6.0 + t * 0.7).sin()
                    + ((x - 4.0 - cx).hypot(y - 17.0 - cy) / 3.2 - t * 1.3).sin();
                *o = ((v / 4.0 + 1.0) / 2.0).powf(1.3);
            }
        }
        out
    }
}

/// Two strands twisting around each other, joined by rungs; the strand in front is brighter.
pub struct DnaHelix {
    s: LookState,
}

impl DnaHelix {
    /// Rows for one full twist.
    pub const TURN: f64 = 17.0;
    /// Twist per second, in radians.
    pub const SPEED: f64 = 1.6;
    pub const CENTRE: f64 = (WIDTH as f64 - 1.0) / 2.0;
    pub const RADIUS: f64 = 3.6;

    pub fn new(seed: Option<u64>) -> DnaHelix {
        DnaHelix { s: LookState::new(seed) }
    }
}

impl Look for DnaHelix {
    fn state(&mut self) -> &mut LookState {
        &mut self.s
    }

    // Row by row, each row lighting a few columns, as in Python
    #[allow(clippy::needless_range_loop)]
    fn draw(&self) -> Frame {
        let mut grid = blank();
        for y in 0..HEIGHT {
            let angle = (y * 2) as f64 * PI / Self::TURN + self.s.t * Self::SPEED;
            for strand in 0..2 {
                let a = angle + strand as f64 * PI;
                let x = Self::CENTRE + Self::RADIUS * a.sin();
                let depth = 0.5 + 0.5 * a.cos(); // 1 in front, 0 behind
                let xi = int_round(x) as usize;
                grid[xi][y] = grid[xi][y].max(0.3 + 0.7 * depth);
            }
            if y % 3 == 0 {
                // A rung between the strands, dimmer than either
                let xa = int_round(Self::CENTRE + Self::RADIUS * angle.sin());
                let xb = int_round(Self::CENTRE - Self::RADIUS * angle.sin());
                let (lo, hi) = (xa.min(xb), xa.max(xb));
                for xi in lo + 1..hi {
                    let xi = xi as usize;
                    grid[xi][y] = grid[xi][y].max(0.28);
                }
            }
        }
        grid
    }
}

/// One snowflake: where it is, how fast it falls, its sway and its brightness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flake {
    pub x: f64,
    pub y: f64,
    /// Rows per second.
    pub speed: f64,
    /// Sway phase.
    pub phase: f64,
    pub brightness: f64,
}

impl Flake {
    fn column(&self) -> usize {
        int_round(self.x + 0.7 * self.phase.sin()).clamp(0, WIDTH as i64 - 1) as usize
    }
}

/// Snowflakes drifting down and settling in a thin layer at the bottom, which slowly melts.
pub struct Snowfall {
    s: LookState,
    pub flakes: Vec<Flake>,
    /// How much snow has settled in each column, in rows.
    pub ground: [f64; WIDTH],
}

impl Snowfall {
    pub const FLAKES: usize = 16;

    pub fn new(seed: Option<u64>) -> Snowfall {
        let mut s = LookState::new(seed);
        let flakes = (0..Self::FLAKES).map(|_| Self::new_flake(&mut s.rng, false)).collect();
        Snowfall { s, flakes, ground: [0.0; WIDTH] }
    }

    fn new_flake(r: &mut Generator, top: bool) -> Flake {
        let x = r.uniform(0.0, WIDTH as f64 - 1.0);
        let y = if top { -r.uniform(0.0, 6.0) } else { r.uniform(0.0, HEIGHT as f64 - 2.0) };
        let speed = r.uniform(3.5, 7.5);
        let phase = r.uniform(0.0, 2.0 * PI);
        let brightness = r.uniform(0.6, 1.0);
        Flake { x, y, speed, phase, brightness }
    }
}

impl Look for Snowfall {
    fn state(&mut self) -> &mut LookState {
        &mut self.s
    }

    fn step(&mut self) {
        for i in 0..self.flakes.len() {
            let f = &mut self.flakes[i];
            f.y += f.speed * LOOK_STEP;
            f.phase += LOOK_STEP * 1.7;
            let x = f.column();
            let floor = (HEIGHT - 1) as i64 - int(self.ground[x]);
            if f.y >= floor as f64 {
                self.ground[x] = 2.0f64.min(self.ground[x] + 0.35);
                self.flakes[i] = Self::new_flake(&mut self.s.rng, true);
            }
        }
        for g in &mut self.ground {
            *g = 0.0f64.max(*g - 0.004);
        }
    }

    fn draw(&self) -> Frame {
        let mut grid = blank();
        for (x, &depth) in self.ground.iter().enumerate() {
            for row in 0..depth.ceil() as usize {
                grid[x][HEIGHT - 1 - row] = 0.35 * 1.0f64.min(depth - row as f64);
            }
        }
        for f in &self.flakes {
            let x = f.column();
            let y = int(f.y);
            if (0..HEIGHT as i64).contains(&y) {
                let y = y as usize;
                grid[x][y] = grid[x][y].max(f.brightness);
            }
        }
        grid
    }
}

/// Arrow shapes (V) marching down the panel.
pub struct Chevrons {
    s: LookState,
}

impl Chevrons {
    /// Rows from one chevron to the next.
    pub const GAP: f64 = 8.0;
    /// Rows per second.
    pub const SPEED: f64 = 6.0;

    pub fn new(seed: Option<u64>) -> Chevrons {
        Chevrons { s: LookState::new(seed) }
    }
}

impl Look for Chevrons {
    fn state(&mut self) -> &mut LookState {
        &mut self.s
    }

    fn draw(&self) -> Frame {
        let mut grid = blank();
        let shift = self.s.t * Self::SPEED;
        let centre = (WIDTH as f64 - 1.0) / 2.0;
        for (x, column) in grid.iter_mut().enumerate() {
            let arm = (x as f64 - centre).abs(); // 0 at the point, 4 at the edges
            for (y, g) in column.iter_mut().enumerate() {
                // Position along the repeating pattern, with the arms raised above the point
                let p = py_mod(y as f64 + arm - shift, Self::GAP);
                if p < 2.0 {
                    *g = 1.0;
                } else if p < 3.0 {
                    *g = 0.25; // a faint edge behind each chevron
                }
            }
        }
        grid
    }
}

/// A ball thrown around the panel, falling under gravity and bouncing off the edges, with a short tail.
pub struct BouncingBall {
    s: LookState,
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    /// The last few places the ball was, oldest first.
    pub trail: Vec<(usize, usize)>,
}

impl BouncingBall {
    /// Rows per second per second.
    pub const GRAVITY: f64 = 60.0;
    pub const TAIL: usize = 5;

    pub fn new(seed: Option<u64>) -> BouncingBall {
        BouncingBall { s: LookState::new(seed), x: 0.0, y: 0.0, vx: 6.0, vy: 0.0, trail: Vec::new() }
    }
}

impl Look for BouncingBall {
    fn state(&mut self) -> &mut LookState {
        &mut self.s
    }

    fn step(&mut self) {
        let dt = LOOK_STEP;
        let right = WIDTH as f64 - 1.0;
        let bottom = HEIGHT as f64 - 1.0;
        self.vy += Self::GRAVITY * dt;
        self.x += self.vx * dt;
        self.y += self.vy * dt;
        if self.x < 0.0 || self.x > right {
            self.x = right.min(0.0f64.max(self.x));
            self.vx = -self.vx;
        }
        if self.y > bottom {
            self.y = bottom;
            self.vy = -self.vy.abs() * 0.82;
            if self.vy.abs() < 22.0 {
                // Running out of bounce: throw it up again
                self.vy = -self.s.rng.uniform(42.0, 50.0);
                self.vx = self.s.rng.uniform(4.0, 9.0).copysign(self.vx);
            }
        }
        if self.y < 0.0 {
            self.y = 0.0;
            self.vy = self.vy.abs();
        }
        self.trail.push((int_round(self.x) as usize, int_round(self.y) as usize));
        if self.trail.len() > Self::TAIL {
            self.trail.drain(..self.trail.len() - Self::TAIL);
        }
    }

    fn draw(&self) -> Frame {
        let mut grid = blank();
        let n = self.trail.len() as f64;
        for (i, &(x, y)) in self.trail.iter().enumerate() {
            grid[x][y] = grid[x][y].max(0.12 + 0.88 * ((i + 1) as f64 / n).powf(2.0));
        }
        grid
    }
}

/// (setting value, name shown in the window, what it shows) for each look.
pub const LOOKS: [(&str, &str, &str); 6] = [
    ("fire", "Fire", "Flames flickering up from the bottom of the panel."),
    ("plasma", "Plasma", "Soft blobs of light drifting and merging."),
    ("dna", "DNA helix", "Two strands twisting around each other."),
    ("snow", "Snowfall", "Snowflakes drifting down and settling at the bottom."),
    ("chevrons", "Chevrons", "Arrow shapes marching down the panel."),
    ("ball", "Bouncing ball", "A ball bouncing around the panel with a short tail."),
];

/// The setting values of the looks, in the order the window lists them.
pub const LOOK_KEYS: [&str; 6] = ["fire", "plasma", "dna", "snow", "chevrons", "ball"];

/// A new animation by its setting value (unknown values give Fire). None seeds it from the clock.
pub fn make_look(key: &str, seed: Option<u64>) -> Box<dyn Look> {
    match key {
        "plasma" => Box::new(Plasma::new(seed)),
        "dna" => Box::new(DnaHelix::new(seed)),
        "snow" => Box::new(Snowfall::new(seed)),
        "chevrons" => Box::new(Chevrons::new(seed)),
        "ball" => Box::new(BouncingBall::new(seed)),
        _ => Box::new(Fire::new(seed)),
    }
}

/// Brightness 0.0-1.0 to LED values, keeping faint LEDs just visible.
pub fn to_leds(frame: &Frame, foreground_value: i32) -> Grid {
    let scale = foreground_value.max(1) as f64;
    let mut grid = Grid::new();
    for (x, column) in frame.iter().enumerate() {
        for (y, &f) in column.iter().enumerate() {
            let value = round_half_even(f * scale) as i64;
            let value = if f > 0.02 { value.max(1) } else { 0 };
            grid.v[x][y] = value.clamp(0, 255) as i32;
        }
    }
    grid
}

// Old-TV switch off and on -------------------------------------------------------------------------

/// How long each frame of the TV effect shows, in seconds.
pub const TV_FRAME_SECONDS: f64 = 0.04;
/// Picture height, as a share of the panel, while squeezing.
const SQUEEZE: [f64; 5] = [0.72, 0.48, 0.3, 0.16, 0.07];
/// Half-width of the bright line, in LEDs.
const SHRINK: [usize; 5] = [4, 3, 2, 1, 0];
/// The last dot fading out.
const FADE: [f64; 5] = [1.0, 0.7, 0.45, 0.25, 0.1];

/// The picture squashed towards the middle row, as an old TV does when it turns off.
pub fn squeezed(grid: &Grid, share: f64) -> Frame {
    let mut out = blank();
    let centre = (HEIGHT as f64 - 1.0) / 2.0;
    // Squeezing the same light into fewer rows makes it brighter
    let gain = 2.0f64.min(1.0 / share.sqrt());
    for y in 0..HEIGHT {
        let src = centre + (y as f64 - centre) / share;
        if 0.0 <= src && src <= HEIGHT as f64 - 1.0 {
            let src = int_round(src) as usize;
            for (o, g) in out.iter_mut().zip(&grid.v) {
                o[y] = g[src] as f64 * gain;
            }
        }
    }
    out
}

/// A bright line across row `row`, `half_width` LEDs either side of the middle column.
pub fn line(brightness: i32, half_width: usize, row: usize) -> Grid {
    let mut out = Grid::new();
    let centre = WIDTH / 2;
    out.fill_x(centre as i64 - half_width as i64, (centre + half_width + 1) as i64, row, brightness);
    out
}

fn mid_line(brightness: i32, half_width: usize) -> Grid {
    line(brightness, half_width, HEIGHT / 2)
}

/// `np.clip(np.rint(frame), 0, 255).astype(int)`.
fn rint_leds(frame: &Frame) -> Grid {
    let mut g = Grid::new();
    for (o, f) in g.v.iter_mut().zip(frame) {
        for (o, &f) in o.iter_mut().zip(f) {
            *o = round_half_even(f).clamp(0.0, 255.0) as i32;
        }
    }
    g
}

fn any_lit(grid: Option<&Grid>) -> Option<&Grid> {
    grid.filter(|g| g.v.iter().flatten().any(|&v| v != 0))
}

fn tv_bright(foreground_value: i32) -> i32 {
    foreground_value.clamp(1, 255)
}

fn faded(bright: i32, fade: f64) -> i32 {
    (int(bright as f64 * fade) as i32).max(1)
}

/// The frames of an old TV turning off, from the picture now on the panel (LED values).
///
/// The picture squeezes into a bright line across the middle, the line shrinks to a dot, and the dot
/// fades. Ends with a dark frame. If nothing is known about the picture (None or all dark), it
/// starts at the line.
pub fn tv_off_frames(grid: Option<&Grid>, foreground_value: i32) -> Vec<Grid> {
    let bright = tv_bright(foreground_value);
    let mut frames = Vec::new();
    if let Some(g) = any_lit(grid) {
        frames.extend(SQUEEZE.iter().map(|&share| rint_leds(&squeezed(g, share))));
    }
    frames.extend(SHRINK.iter().map(|&half_width| mid_line(bright, half_width)));
    frames.extend(FADE[1..].iter().map(|&fade| mid_line(faded(bright, fade), 0)));
    frames.push(Grid::new());
    frames
}

/// The frames of an old TV turning on into the given picture: a dot, a line, then the picture
/// opening out from the middle. The last frame is the picture itself (clipped to 0-255).
pub fn tv_on_frames(grid: Option<&Grid>, foreground_value: i32) -> Vec<Grid> {
    let bright = tv_bright(foreground_value);
    let mut frames: Vec<Grid> = FADE[1..3].iter().rev().map(|&fade| mid_line(faded(bright, fade), 0)).collect();
    frames.extend(SHRINK[..SHRINK.len() - 1].iter().rev().map(|&half_width| mid_line(bright, half_width)));
    if let Some(g) = any_lit(grid) {
        frames.extend(SQUEEZE.iter().rev().map(|&share| rint_leds(&squeezed(g, share))));
    }
    let mut last = grid.cloned().unwrap_or_default();
    for v in last.v.iter_mut().flatten() {
        *v = (*v).clamp(0, 255);
    }
    frames.push(last);
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_float_mod() {
        assert_eq!(py_mod(-1.5, 8.0), 6.5);
        assert_eq!(py_mod(9.0, 8.0), 1.0);
    }

    #[test]
    fn steps_at_25_a_second() {
        let mut look = Chevrons::new(Some(1));
        look.frame(0.1);
        assert_eq!(look.s.t, 0.08);
        look.frame(5.0);
        assert!((look.s.t - 1.08).abs() < 1e-9);
    }

    #[test]
    fn tv_off_without_a_picture_starts_at_the_line() {
        let frames = tv_off_frames(None, 160);
        assert_eq!(frames.len(), 10);
        assert_eq!(frames[0].get(0, 17), 160);
        assert!(frames[9].is_dark());
    }
}
