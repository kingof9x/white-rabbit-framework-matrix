//! The graphs and simulations on the Math page (`math_sims.py` and `math_effects.py`).
//!
//! Each animation hands out pictures forever: 34 rows tall and `width` columns wide (9 for one panel,
//! 18 for both side by side, wider with the keyboard gap), brightness 0.0 to 1.0.
//!
//! The ideas (graphs of functions, Game of Life and other Life-like rules, the Biham-Middleton-Levine
//! traffic model, the Hardy-Pomeau-Pazzis lattice gas, one-dimensional automata, and seeding the
//! simulations from a graph) come from mariobx's Framework-LED-Matrix project. Like the Python
//! version, this is written from the textbook definitions; no code was copied.
//!
//! How it is used: `pages/math.rs` picks an animation (a `Step`, made by one of the public functions
//! here, or one of `EFFECTS`), wraps it in an `Anim` and moves it to a worker thread. That thread
//! calls `Anim::next_frame` on a fixed schedule (the page's speed setting, in steps per second),
//! rearranges the picture for the panel layout and sends it to the LED panels, and also hands it to
//! the window's preview. Nothing in this file does any input/output; it only computes pictures, so
//! the tests below can run it anywhere.
//!
//! The shape shared by every animation: a `Step` is a closure that owns its own simulation state
//! (captured variables) and, each time it is called, returns one or more new pictures. Simulations
//! that end (Life dies out, traffic jams, a maze is solved) hold their last picture for a moment and
//! then start over by themselves, so an animation never stops.
//!
//! Coordinates are as on the panels: x (column) from the left, y (row) from the top. Smooth curves
//! (the Lorenz dot, the pendulum, the Fourier circles) use fractional positions and are drawn with
//! `splat`, which shares a dot between the four nearest LEDs so motion looks smooth on a coarse grid.

use std::collections::{HashMap, HashSet, VecDeque};
use std::f64::consts::PI;

use crate::formula::Formula;
use crate::games::{Pic, Rng, HEIGHT};

/// Brightness for background detail such as graph axes (45 of 255).
pub const DIM: f64 = 45.0 / 255.0;
/// Brightness for a second kind of thing next to a bright one (120 of 255), e.g. downward cars.
pub const MID: f64 = 120.0 / 255.0;
/// Full brightness: the main subject (the graph's curve, a live Life cell).
pub const FULL: f64 = 1.0;

/// What a step function gets: the random numbers, the picture width, and a place to leave a message
/// for the window ("Started from the sine graph").
pub struct Gen {
    /// Random numbers for the animation (seeded from the OS when the page starts one).
    pub rng: Rng,
    /// The picture width in LEDs; the height is always `HEIGHT` (34).
    pub width: usize,
    /// A message for the window, taken (and cleared) by `Anim::take_note`.
    pub note: Option<String>,
}

/// Makes the next picture(s) each time it is called.
///
/// `FnMut` because it keeps and changes its own state between calls; `Send` because it is built on
/// the UI thread and run on the animation thread. Returning several pictures at once lets an
/// animation work out a whole sequence in one go (a sort, a maze); `Anim` hands them out one by one.
pub type Step = Box<dyn FnMut(&mut Gen) -> Vec<Pic> + Send>;

/// An animation: pictures one after the other, forever.
pub struct Anim {
    /// The recipe that makes new pictures.
    step: Step,
    /// What the recipe is given each call (random numbers, width, note).
    gen: Gen,
    /// Pictures made but not shown yet, oldest first.
    buf: VecDeque<Pic>,
}

impl Anim {
    /// An animation `width` LEDs wide running `step`.
    pub fn new(width: usize, rng: Rng, step: Step) -> Anim {
        Anim { step, gen: Gen { rng, width, note: None }, buf: VecDeque::new() }
    }

    /// The next picture: from the buffer, or by calling the step again when it is empty.
    ///
    /// A step that returns nothing (a maze too narrow to build, for example) gives one dark picture,
    /// so this always returns and never loops forever.
    pub fn next_frame(&mut self) -> Pic {
        while self.buf.is_empty() {
            let frames = (self.step)(&mut self.gen);
            if frames.is_empty() {
                self.buf.push_back(Pic::new(self.gen.width));
            }
            self.buf.extend(frames);
        }
        self.buf.pop_front().unwrap_or_else(|| Pic::new(self.gen.width))
    }

    /// A new message from the animation for the window, if there is one (each message is given once).
    pub fn take_note(&mut self) -> Option<String> {
        self.gen.note.take()
    }
}

/// A picture from a true/false grid (row by row): true cells at brightness `on`, the rest dark.
fn bools(grid: &[bool], w: usize, on: f64) -> Pic {
    Pic { w, v: grid.iter().map(|b| if *b { on } else { 0.0 }).collect() }
}

/// Adds a soft dot at a point between LEDs, shared by the 4 LEDs around it.
///
/// This is "bilinear" splatting: with (x0, y0) the LED up and to the left of the point and (fx, fy)
/// how far the point is past it (0..1), the four LEDs around it get shares (1-fx)(1-fy), fx(1-fy),
/// (1-fx)fy and fx·fy of `v`, which add up to `v`. So a dot moving smoothly fades from one LED into the
/// next instead of jumping. The shares are boosted by 1.6 (a dot split four ways would look too dim)
/// and added to what is there, capped at full brightness. LEDs off the picture and points that are
/// not finite (NaN, infinity) are ignored.
fn splat(img: &mut Pic, x: f64, y: f64, v: f64) {
    if !x.is_finite() || !y.is_finite() {
        return;
    }
    // (x0, y0) is the LED up and to the left of the point; (fx, fy) the point's position within it.
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    for (dx, dy, share) in [
        (0.0, 0.0, (1.0 - fx) * (1.0 - fy)),
        (1.0, 0.0, fx * (1.0 - fy)),
        (0.0, 1.0, (1.0 - fx) * fy),
        (1.0, 1.0, fx * fy),
    ] {
        let (xx, yy) = (x0 + dx, y0 + dy);
        if xx >= 0.0 && yy >= 0.0 && (xx as usize) < img.w && (yy as usize) < HEIGHT {
            let (xx, yy) = (xx as usize, yy as usize);
            let old = img.get(xx, yy);
            img.set(xx, yy, (old + v * share * 1.6).min(1.0));
        }
    }
}

/// A soft straight line from (x0, y0) to (x1, y1): splats every half LED along it (counted along the
/// longer of the two directions), each at half brightness because neighbouring splats overlap.
fn line(img: &mut Pic, x0: f64, y0: f64, x1: f64, y1: f64, v: f64) {
    // Number of steps: two per LED of the line's longer side, at least one.
    let n = ((x1 - x0).abs().max((y1 - y0).abs()) * 2.0) as usize + 1;
    for i in 0..=n {
        let t = i as f64 / n as f64;
        splat(img, x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, v / 2.0);
    }
}

// Graphs ------------------------------------------------------------------------------------------

/// (name, formula, x from, x to, y range or None to fit the graph)
///
/// The formula is in `formula.rs`'s syntax; `t` in it is time and makes the graph move.
pub type GraphDef = (&'static str, &'static str, f64, f64, Option<(f64, f64)>);

/// The built-in graphs offered on the page, in its order. Each x range shows the interesting part
/// (two waves of a sine, the bend of a cubic); fixed y ranges are given where fitting would look wrong
/// (tan's infinite jumps, waves that should keep their size as they move).
pub const GRAPHS: [GraphDef; 20] = [
    ("Sine", "sin(x)", -2.0 * PI, 2.0 * PI, Some((-1.2, 1.2))),
    ("Cosine", "cos(x)", -2.0 * PI, 2.0 * PI, Some((-1.2, 1.2))),
    ("Tangent", "tan(x)", -PI, PI, Some((-4.0, 4.0))),
    ("Parabola", "x^2", -3.0, 3.0, None),
    ("Cubic", "x^3 - 3*x", -2.5, 2.5, None),
    ("Exponential", "exp(x)", -3.0, 2.0, None),
    ("Logarithm", "log(x)", 0.05, 6.0, None),
    ("Square root", "sqrt(x)", 0.0, 9.0, None),
    ("Tanh", "tanh(x)", -4.0, 4.0, Some((-1.2, 1.2))),
    ("Sinc", "sinc(x)", -12.0, 12.0, Some((-0.4, 1.1))),
    ("Bell curve", "exp(-x^2)", -3.0, 3.0, Some((-0.1, 1.1))),
    ("Absolute value", "abs(x)", -3.0, 3.0, None),
    ("Damped wave", "exp(-x/4) * sin(2*x)", 0.0, 12.0, Some((-1.0, 1.0))),
    ("Beats", "sin(5*x) + sin(5.6*x)", 0.0, 12.0, Some((-2.1, 2.1))),
    ("1/x", "1/x", -4.0, 4.0, Some((-4.0, 4.0))),
    ("Arcsin", "asin(x)", -1.0, 1.0, None),
    ("Steps", "floor(x)", -4.0, 4.0, None),
    ("Sawtooth", "x - floor(x)", -3.0, 3.0, Some((-0.2, 1.2))),
    ("Moving wave", "sin(x - t)", -2.0 * PI, 2.0 * PI, Some((-1.2, 1.2))),
    ("Breathing wave", "sin(t) * sin(x)", -2.0 * PI, 2.0 * PI, Some((-1.2, 1.2))),
];

/// Parses one of the built-in formulas above. They are fixed text checked by the tests, so a parse
/// failure is a programming mistake and panics.
fn formula(text: &str) -> Formula {
    Formula::parse(text).expect("the built-in graphs are valid")
}

/// The graphs that don't move (no t), for starting the simulations from.
fn still_graphs() -> Vec<GraphDef> {
    GRAPHS.iter().copied().filter(|g| !formula(g.1).uses_t).collect()
}

/// numpy's percentile (linear).
///
/// The value `p` percent of the way through the sorted list, interpolating between the two nearest
/// entries (so the 50th percentile of [1, 2, 3, 4] is 2.5). `sorted` must not be empty.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let idx = p / 100.0 * (sorted.len() - 1) as f64;
    let (lo, hi) = (idx.floor() as usize, idx.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (idx - lo as f64)
}

/// A y range that shows most of the graph, ignoring the odd point that shoots off.
///
/// Works out a robust range from the 2nd to the 98th percentile of the finite values. If the full
/// range is at most 3 times that, the full range is used (nothing is cut off); otherwise the robust
/// one (so 1/x near 0 doesn't squash everything else flat). A flat graph gets ±1 around its value,
/// and 8% of margin is added on each side. With no finite values at all it gives -1..1.
pub fn fit_range(ys: &[f64]) -> (f64, f64) {
    let mut finite: Vec<f64> = ys.iter().copied().filter(|y| y.is_finite()).collect();
    if finite.is_empty() {
        return (-1.0, 1.0);
    }
    finite.sort_by(|a, b| a.total_cmp(b));
    let (mut low, mut high) = (percentile(&finite, 2.0), percentile(&finite, 98.0));
    let (min, max) = (finite[0], finite[finite.len() - 1]);
    // Show the whole graph unless a few points shoot far off (like 1/x near 0)
    if max - min <= 3.0 * (high - low) {
        low = min;
        high = max;
    }
    if high - low < 1e-9 {
        low -= 1.0;
        high += 1.0;
    }
    let pad = (high - low) * 0.08;
    (low - pad, high + pad)
}

/// A picture of y = f(x). x runs down the panel (one row per step), y runs across it.
///
/// The panel is tall and thin, so the graph is turned on its side: x goes down the 34 rows, y across
/// the columns (`low` in column 0, `high` in the last). The formula is sampled 16 times per row
/// (545 points in all). In each row the samples are split into unbroken runs, and each run lights the
/// columns from its smallest to its largest y, so a steep part of the curve shows as a solid bar
/// instead of scattered dots. A run breaks at a value that isn't a number, is outside the y range, or
/// jumps by more than half the range (like tan at its asymptotes), so no false wall is drawn.
/// With `axes`, the line y = 0 is a dim column and x = 0 a dim row, where they are in range.
pub fn draw_graph(
    f: &Formula,
    x_from: f64,
    x_to: f64,
    y_range: Option<(f64, f64)>,
    width: usize,
    t: f64,
    axes: bool,
) -> Pic {
    // Samples per row: enough to catch fast wiggles (like "Beats") between rows.
    const SAMPLES: usize = 16;
    let mut frame = Pic::new(width);
    let n = HEIGHT * SAMPLES + 1;
    let ys: Vec<f64> = (0..n).map(|i| f.at(x_from + (x_to - x_from) * i as f64 / (n - 1) as f64, t)).collect();
    let (low, high) = y_range.unwrap_or_else(|| fit_range(&ys));
    let span = high - low;
    // y value to column: a straight scale of low..high onto 0..width-1, rounded to the nearest
    // column (ties to even, like numpy's rounding in the Python version) and kept on the picture.
    let column = |y: f64| (((y - low) / span * (width - 1) as f64).round_ties_even().max(0.0) as usize).min(width - 1);
    if axes {
        // The y = 0 axis is a column (y runs across); x = 0 below is a row.
        if low <= 0.0 && 0.0 <= high {
            let c = column(0.0);
            for y in 0..HEIGHT {
                frame.set(c, y, DIM);
            }
        }
        if x_from <= 0.0 && 0.0 <= x_to {
            let r = ((0.0 - x_from) / (x_to - x_from) * (HEIGHT - 1) as f64).round_ties_even() as usize;
            for x in 0..width {
                frame.set(x, r, DIM);
            }
        }
    }
    for row in 0..HEIGHT {
        // This row's 17 samples: its own 16 and the first of the next row, so neighbouring rows
        // join without a gap.
        let part = &ys[row * SAMPLES..((row + 1) * SAMPLES + 1).min(n)];
        let mut segment: Vec<f64> = Vec::new();
        let flush = |segment: &Vec<f64>, frame: &mut Pic| {
            if !segment.is_empty() {
                let lo = segment.iter().copied().fold(f64::INFINITY, f64::min);
                let hi = segment.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                for x in column(lo)..=column(hi) {
                    frame.set(x, row, FULL);
                }
            }
        };
        // A `None` is added at the end so the last run is always drawn.
        for y in part.iter().map(|y| Some(*y)).chain([None]) {
            let usable = y.is_some_and(|y| y.is_finite() && low <= y && y <= high);
            // A jump of more than half the height is a break (like tan), not a steep line
            // (`y.unwrap()` is safe here: `usable` is only true when y is Some.)
            let jump = usable && segment.last().is_some_and(|last| (y.unwrap() - last).abs() > span / 2.0);
            if !usable || jump {
                flush(&segment, &mut frame);
                segment = if usable { vec![y.unwrap()] } else { Vec::new() };
            } else {
                segment.push(y.unwrap());
            }
        }
    }
    frame
}

/// One graph. If the formula uses t, it moves.
///
/// `t` starts at 0 and goes up 0.15 per frame, so at 20 frames a second a sin(x - t) wave moves about
/// 3 units of x a second. A formula without t gives the same picture every frame.
pub fn graph(f: Formula, x_from: f64, x_to: f64, y_range: Option<(f64, f64)>, axes: bool) -> Step {
    let mut t = 0.0;
    Box::new(move |g| {
        let frame = draw_graph(&f, x_from, x_to, y_range, g.width, t, axes);
        if f.uses_t {
            t += 0.15;
        }
        vec![frame]
    })
}

/// A random still graph (no axes) and its name, used as a starting picture for the simulations.
fn random_graph_picture(rng: &mut Rng, width: usize) -> (&'static str, Pic) {
    let still = still_graphs();
    let (name, text, x_from, x_to, y_range) = still[rng.below(still.len())];
    (name, draw_graph(&formula(text), x_from, x_to, y_range, width, 0.0, false))
}

/// A different graph every few seconds. frames_each is read when each graph starts.
///
/// Never the same graph twice in a row. `frames_each` is shared with the page so its speed control
/// can change how long each graph stays; it is read once per graph. Each graph's frames are made
/// all at once (moving ones with t stepping 0.15 per frame), and its name and formula go to the note.
pub fn random_graphs(frames_each: std::sync::Arc<std::sync::atomic::AtomicUsize>, axes: bool) -> Step {
    let mut last = "";
    Box::new(move |g| {
        let choices: Vec<GraphDef> = GRAPHS.iter().copied().filter(|c| c.0 != last).collect();
        let (name, text, x_from, x_to, y_range) = choices[g.rng.below(choices.len())];
        last = name;
        g.note = Some(format!("{name}: y = {text}"));
        let f = formula(text);
        let each = frames_each.load(std::sync::atomic::Ordering::Relaxed).max(1);
        (0..each)
            .map(|i| draw_graph(&f, x_from, x_to, y_range, g.width, if f.uses_t { i as f64 * 0.15 } else { 0.0 }, axes))
            .collect()
    })
}

// Starting pictures for the simulations -----------------------------------------------------------

/// Well-known Game of Life patterns as (row, column) cells
///
/// Glider: the smallest pattern that travels. R-pentomino, Acorn: tiny starts that grow for a long
/// time. Diehard: vanishes after 130 generations. Lightweight spaceship: travels sideways.
/// Pentadecathlon: repeats every 15 generations. Pulsar quarter: one quarter of the period-3 pulsar.
/// Any other name gives None (the caller then fills the grid at random).
pub fn life_pattern(name: &str) -> Option<Vec<(usize, usize)>> {
    Some(match name {
        "Glider" => vec![(0, 1), (1, 2), (2, 0), (2, 1), (2, 2)],
        "R-pentomino" => vec![(0, 1), (0, 2), (1, 0), (1, 1), (2, 1)],
        "Acorn" => vec![(0, 1), (1, 3), (2, 0), (2, 1), (2, 4), (2, 5), (2, 6)],
        "Diehard" => vec![(0, 6), (1, 0), (1, 1), (2, 1), (2, 5), (2, 6), (2, 7)],
        "Lightweight spaceship" => vec![(0, 1), (0, 4), (1, 0), (2, 0), (2, 4), (3, 0), (3, 1), (3, 2), (3, 3)],
        "Pentadecathlon" => {
            let mut v: Vec<(usize, usize)> = (0..10).filter(|r| *r != 2 && *r != 7).map(|r| (r, 1)).collect();
            v.extend([(2, 0), (2, 2), (7, 0), (7, 2)]);
            v
        }
        "Pulsar quarter" => {
            vec![(0, 2), (0, 3), (0, 4), (2, 0), (3, 0), (4, 0), (2, 5), (3, 5), (4, 5), (5, 2), (5, 3), (5, 4)]
        }
        _ => return None,
    })
}

/// The starting pictures offered for Life, in the page's order: random, a graph, or a named pattern.
pub const STARTS: [&str; 9] = [
    "Random",
    "Math graph",
    "Glider",
    "R-pentomino",
    "Acorn",
    "Diehard",
    "Lightweight spaceship",
    "Pentadecathlon",
    "Pulsar quarter",
];

/// A grid with the pattern centred. Cells that would fall off the right edge (a picture narrower than
/// the pattern) are left out.
fn place_pattern(cells: &[(usize, usize)], width: usize) -> Vec<bool> {
    let mut grid = vec![false; HEIGHT * width];
    let rows = cells.iter().map(|c| c.0).max().unwrap_or(0) + 1;
    let cols = cells.iter().map(|c| c.1).max().unwrap_or(0) + 1;
    let (top, left) = ((HEIGHT - rows) / 2, width.saturating_sub(cols) / 2);
    for &(r, c) in cells {
        if left + c < width {
            grid[(top + r) * width + left + c] = true;
        }
    }
    grid
}

/// A true/false picture to start a simulation from.
///
/// "Math graph" uses the lit LEDs of a random still graph (and says which in the note); a pattern name
/// places that pattern; anything else ("Random") makes each cell alive with probability `density`.
fn starting_grid(start: &str, g: &mut Gen, density: f64) -> Vec<bool> {
    let w = g.width;
    match start {
        "Math graph" => {
            let (name, picture) = random_graph_picture(&mut g.rng, w);
            g.note = Some(format!("Started from the {} graph", name.to_lowercase()));
            picture.v.iter().map(|v| *v > 0.0).collect()
        }
        _ => match life_pattern(start) {
            Some(cells) => place_pattern(&cells, w),
            None => (0..HEIGHT * w).map(|_| g.rng.random() < density).collect(),
        },
    }
}

// Game of Life and other Life-like rules ----------------------------------------------------------

/// (name, birth counts, survive counts)
///
/// A Life-like rule says how many of a cell's 8 neighbours must be alive for a dead cell to be born
/// ("B") and for a live cell to survive ("S"); Conway's rule is B3/S23. The page also accepts a custom
/// rule typed as two lists of digits.
pub const LIFE_RULES: [(&str, &str, &str); 9] = [
    ("Conway's Game of Life (B3/S23)", "3", "23"),
    ("HighLife (B36/S23)", "36", "23"),
    ("Seeds (B2/S)", "2", ""),
    ("Day & Night (B3678/S34678)", "3678", "34678"),
    ("Maze (B3/S12345)", "3", "12345"),
    ("Replicator (B1357/S1357)", "1357", "1357"),
    ("Life without death (B3/S012345678)", "3", "012345678"),
    ("Diamoeba (B35678/S5678)", "35678", "5678"),
    ("2x2 (B36/S125)", "36", "125"),
];

/// '3,6' or '36' -> the neighbour counts. Refuses anything that isn't 0 to 8.
///
/// The result has `true` at each listed count. Commas and spaces are ignored; an empty text means no
/// counts (as in Seeds' "S" part). The error is a sentence for the page.
pub fn parse_counts(text: &str) -> Result<[bool; 9], String> {
    let mut counts = [false; 9];
    for ch in text.chars().filter(|c| *c != ',' && *c != ' ') {
        match ch.to_digit(10) {
            Some(d) if d <= 8 => counts[d as usize] = true,
            _ => return Err("Use the numbers 0 to 8, like 3 or 23.".into()),
        }
    }
    Ok(counts)
}

/// The opposite of `parse_counts`: the listed counts as digits, e.g. "23".
pub fn counts_text(counts: &[bool; 9]) -> String {
    (0..9).filter(|i| counts[*i]).map(|i| i.to_string()).collect()
}

/// One generation; the edges wrap around.
///
/// The picture is a torus: the left edge's neighbours are on the right edge, the top's on the bottom,
/// so patterns that leave one side come back on the other. Counting uses `+ (size - 1)` modulo size
/// for "one back", which avoids negative numbers with unsigned indexes. Every cell is computed from the
/// old grid, so the order cells are visited in doesn't matter.
pub fn life_step(grid: &[bool], w: usize, birth: &[bool; 9], survive: &[bool; 9]) -> Vec<bool> {
    let mut out = vec![false; grid.len()];
    for r in 0..HEIGHT {
        for c in 0..w {
            let mut n = 0;
            for dr in [HEIGHT - 1, 0, 1] {
                for dc in [w - 1, 0, 1] {
                    if (dr != 0 || dc != 0) && grid[((r + dr) % HEIGHT) * w + (c + dc) % w] {
                        n += 1;
                    }
                }
            }
            let alive = grid[r * w + c];
            out[r * w + c] = if alive { survive[n] } else { birth[n] };
        }
    }
    out
}

/// Runs a Life-like rule. When it dies out, settles or loops, it pauses a moment and starts again.
///
/// It restarts when every cell is dead, when the grid repeats a generation seen before (it settled
/// or entered a loop; every generation is remembered in a set), or after 600 generations. The last
/// picture is held for 12 frames first. One generation per frame.
pub fn life(birth: [bool; 9], survive: [bool; 9], start: String, density: f64) -> Step {
    const PAUSE: usize = 12;
    const MAX_GENERATIONS: usize = 600;
    let mut grid: Vec<bool> = Vec::new();
    let mut seen: HashSet<Vec<bool>> = HashSet::new();
    let mut generation = 0;
    let mut pause = 0;
    Box::new(move |g| {
        let w = g.width;
        if pause > 0 {
            pause -= 1;
            let frame = bools(&grid, w, FULL);
            if pause == 0 {
                grid.clear();
            }
            return vec![frame];
        }
        if grid.len() != HEIGHT * w {
            grid = starting_grid(&start, g, density);
            seen.clear();
            generation = 0;
        }
        let frame = bools(&grid, w, FULL);
        if !grid.iter().any(|c| *c) || seen.contains(&grid) {
            pause = PAUSE;
        } else {
            // Remember this generation (to spot a repeat later), then compute the next one.
            seen.insert(grid.clone());
            grid = life_step(&grid, w, &birth, &survive);
            generation += 1;
            if generation >= MAX_GENERATIONS {
                pause = PAUSE;
            }
        }
        vec![frame]
    })
}

// Biham-Middleton-Levine traffic ------------------------------------------------------------------

/// Traffic cell contents: empty road.
const BML_EMPTY: u8 = 0;
/// A car that drives left to right.
const BML_ACROSS: u8 = 1;
/// A car that drives top to bottom.
const BML_DOWN: u8 = 2;

/// Every car of one kind moves one cell (across or down, wrapping) if that cell is free.
///
/// All cars of that kind move at the same moment: each looks at the old grid, so a car moves into a
/// cell only if that cell was free before anyone moved (no chain-following within a step). The edges
/// wrap round. Returns the new grid and how many cars moved.
fn bml_half_step(grid: &[u8], w: usize, kind: u8) -> (Vec<u8>, usize) {
    let mut new = grid.to_vec();
    let mut moved = 0;
    for r in 0..HEIGHT {
        for c in 0..w {
            if grid[r * w + c] != kind {
                continue;
            }
            let ahead = if kind == BML_ACROSS { r * w + (c + 1) % w } else { ((r + 1) % HEIGHT) * w + c };
            if grid[ahead] == BML_EMPTY {
                new[r * w + c] = BML_EMPTY;
                new[ahead] = kind;
                moved += 1;
            }
        }
    }
    (new, moved)
}

/// Empty road dark, across cars bright, down cars mid-grey.
fn bml_pic(grid: &[u8], w: usize) -> Pic {
    Pic { w, v: grid.iter().map(|c| [0.0, FULL, MID][*c as usize]).collect() }
}

/// Cars going across (bright) and cars going down (dim) take turns. A full jam starts it over.
///
/// The Biham-Middleton-Levine model: the road is filled at random with `density` cars (half each
/// kind). Each frame one kind moves (across on even steps, down on odd ones). Low densities flow
/// freely, high ones lock into a jam; near the threshold it does both. When a full round moves no car
/// the jam is total: it is held for 15 frames and restarted. It also restarts after 2000 steps.
pub fn traffic(density: f64) -> Step {
    const JAM_PAUSE: usize = 15;
    const MAX_STEPS: usize = 2000;
    let mut grid: Vec<u8> = Vec::new();
    let mut step = 0;
    let mut first_moved = 0;
    let mut pause = 0;
    Box::new(move |g| {
        let w = g.width;
        if pause > 0 {
            pause -= 1;
            let frame = bml_pic(&grid, w);
            if pause == 0 {
                grid.clear();
            }
            return vec![frame];
        }
        if grid.len() != HEIGHT * w {
            grid = (0..HEIGHT * w)
                .map(|_| {
                    let r = g.rng.random();
                    // One random number decides each cell: below density/2 an across car, below
                    // density a down car, otherwise empty road.
                    if r < density / 2.0 {
                        BML_ACROSS
                    } else if r < density {
                        BML_DOWN
                    } else {
                        BML_EMPTY
                    }
                })
                .collect();
            step = 0;
        }
        let frame = bml_pic(&grid, w);
        let (new, moved) = bml_half_step(&grid, w, if step % 2 == 0 { BML_ACROSS } else { BML_DOWN });
        grid = new;
        if step % 2 == 0 {
            first_moved = moved;
        } else if first_moved == 0 && moved == 0 {
            // A full round (across then down) in which no car moved: total jam.
            pause = JAM_PAUSE;
        }
        step += 1;
        if step >= MAX_STEPS && pause == 0 {
            pause = JAM_PAUSE;
        }
        vec![frame]
    })
}

// Hardy-Pomeau-Pazzis lattice gas -----------------------------------------------------------------

/// Lattice-gas cell bits: a particle moving north (towards row 0).
const N: u8 = 1;
/// A particle moving east (to the right).
const EAST: u8 = 2;
/// A particle moving south (down).
const S: u8 = 4;
/// A particle moving west (to the left). A cell holds 0 to 4 particles, at most one per direction.
const W: u8 = 8;
/// The gas's starting pictures, in the page's order.
pub const GAS_STARTS: [&str; 3] = ["Random", "Gas in a corner", "Math graph"];
/// Brightness of a cell by its number of particles (0 to 4).
const HPP_BRIGHTNESS: [f64; 5] = [0.0, 90.0 / 255.0, 160.0 / 255.0, 215.0 / 255.0, 1.0];

/// Particles moving N/E/S/W. Two meeting head on leave at right angles. Walls bounce them back.
///
/// Two halves. Collision: a cell holding exactly a north and a south particle (and nothing else)
/// turns them into east and west, and the other way round; every other cell is unchanged. This keeps
/// the number of particles and the momentum. Streaming: every particle moves one cell its way; one
/// that would leave the picture instead stays and reverses (the walls are mirrors). Because no two
/// particles share a direction in a cell, the bits never collide and particles are never lost.
pub fn hpp_step(grid: &[u8], w: usize) -> Vec<u8> {
    let g: Vec<u8> = grid
        .iter()
        .map(|c| match *c {
            // Collision rule (only exact head-on pairs; a third particle in the cell blocks it).
            c if c == N | S => EAST | W,
            c if c == EAST | W => N | S,
            c => c,
        })
        .collect();
    let mut new = vec![0u8; g.len()];
    for r in 0..HEIGHT {
        for c in 0..w {
            let cell = g[r * w + c];
            // Moving north = towards row 0
            if cell & N != 0 {
                if r > 0 {
                    new[(r - 1) * w + c] |= N;
                } else {
                    new[c] |= S;
                }
            }
            if cell & S != 0 {
                if r + 1 < HEIGHT {
                    new[(r + 1) * w + c] |= S;
                } else {
                    new[r * w + c] |= N;
                }
            }
            if cell & EAST != 0 {
                if c + 1 < w {
                    new[r * w + c + 1] |= EAST;
                } else {
                    new[r * w + c] |= W;
                }
            }
            if cell & W != 0 {
                if c > 0 {
                    new[r * w + c - 1] |= W;
                } else {
                    new[r * w + c] |= EAST;
                }
            }
        }
    }
    new
}

///
/// Each of the four directions is filled independently with probability `density / 2` in the allowed
/// area. "Gas in a corner": the top third of the rows, the left half of the columns (at least 3),
/// at least 0.7 dense, so you can watch it spread out. "Math graph": the lit LEDs of a random graph,
/// each given at least one particle. "Random": the whole picture.
fn hpp_grid(g: &mut Gen, start: &str, density: f64) -> Vec<u8> {
    let w = g.width;
    let (where_, density) = match start {
        "Gas in a corner" => {
            let cols = 3.max(w / 2);
            ((0..HEIGHT * w).map(|i| i / w < HEIGHT / 3 && i % w < cols).collect::<Vec<_>>(), density.max(0.7))
        }
        "Math graph" => (starting_grid("Math graph", g, 0.0), 1.0),
        _ => (vec![true; HEIGHT * w], density),
    };
    let mut grid = vec![0u8; HEIGHT * w];
    for bit in [N, EAST, S, W] {
        for (i, cell) in grid.iter_mut().enumerate() {
            if g.rng.random() < density / 2.0 && where_[i] {
                *cell |= bit;
            }
        }
    }
    if start == "Math graph" {
        // Every lit cell of the graph has at least one particle
        for (i, cell) in grid.iter_mut().enumerate() {
            if where_[i] && *cell == 0 {
                *cell = [N, EAST, S, W][g.rng.below(4)];
            }
        }
    }
    grid
}

/// Runs the lattice gas one step per frame, starting over with a new random fill every 300 steps.
pub fn gas(start: String, density: f64) -> Step {
    const STEPS_EACH: usize = 300;
    let mut grid: Vec<u8> = Vec::new();
    let mut steps = 0;
    Box::new(move |g| {
        let w = g.width;
        if grid.len() != HEIGHT * w || steps >= STEPS_EACH {
            grid = hpp_grid(g, &start, density);
            steps = 0;
        }
        // `count_ones` counts the set direction bits, i.e. the particles in the cell.
        let frame = Pic { w, v: grid.iter().map(|c| HPP_BRIGHTNESS[c.count_ones() as usize]).collect() };
        grid = hpp_step(&grid, w);
        steps += 1;
        vec![frame]
    })
}

// One-dimensional automata, one generation per row ------------------------------------------------

/// The one-dimensional automaton kinds, in the page's order, with the number range each takes.
pub const AUTOMATA_KINDS: [&str; 2] = ["Elementary (rule 0-255)", "Totalistic, 3 colours (code 0-2186)"];
/// The automata's first rows: one live cell in the middle, or random cells.
pub const AUTOMATA_STARTS: [&str; 2] = ["Single cell", "Random"];

/// One generation of an "elementary" cellular automaton (Wolfram's numbering), cells 0 or 1.
///
/// Each new cell depends on itself and its two neighbours (the row wraps round). Those three cells,
/// read as a 3-bit number left-centre-right (0..7), pick a bit of `rule`: the new cell is bit
/// number `index` of the rule. So rule 30 = 0b00011110 turns pattern 1 (001) into 1 and 0 into 0.
pub fn elementary_step(row: &[u8], rule: u32) -> Vec<u8> {
    let w = row.len();
    (0..w)
        .map(|i| {
            // left·4 + centre·2 + right, with the row wrapping round at both ends.
            let index = (row[(i + w - 1) % w] << 2) | (row[i] << 1) | row[(i + 1) % w];
            ((rule >> index) & 1) as u8
        })
        .collect()
}

/// One generation of a 3-colour totalistic automaton, cells 0, 1 or 2.
///
/// "Totalistic" means the new cell depends only on the sum of itself and its two neighbours (0..6).
/// The code, written in base 3, has 7 digits (so codes go up to 3^7 - 1 = 2186); digit number `sum`
/// (counting from the least significant) is the new cell.
pub fn totalistic_step(row: &[u8], code: u32) -> Vec<u8> {
    let w = row.len();
    // The code's base-3 digits, least significant first: digit s is the new cell for sum s.
    let digits: Vec<u8> = (0..7).map(|i| ((code / 3u32.pow(i)) % 3) as u8).collect();
    (0..w).map(|i| digits[(row[(i + w - 1) % w] + row[i] + row[(i + 1) % w]) as usize]).collect()
}

/// Each new generation appears at the bottom and the older ones scroll up. Once the rows start
/// repeating, it runs one more screenful and starts again.
///
/// The picture keeps the last 34 rows. A set remembers every row seen; once a row repeats the pattern
/// is periodic, so only one more screenful is shown. In any case it restarts after 400 rows.
pub fn automaton(totalistic: bool, number: u32, random_start: bool) -> Step {
    const ROWS_BEFORE_RESTART: usize = 400;
    let shades: Vec<f64> = if totalistic { vec![0.0, MID, FULL] } else { vec![0.0, FULL] };
    let colours = if totalistic { 3 } else { 2 };
    let mut row: Vec<u8> = Vec::new();
    let mut history: VecDeque<Vec<u8>> = VecDeque::new();
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    let mut left = 0;
    Box::new(move |g| {
        let w = g.width;
        if left == 0 || row.len() != w {
            row = if random_start {
                (0..w).map(|_| g.rng.below(colours) as u8).collect()
            } else {
                let mut r = vec![0u8; w];
                // Single live cell in the middle: rules then grow their famous triangles.
                r[w / 2] = 1;
                r
            };
            history = (0..HEIGHT).map(|_| vec![0u8; w]).collect();
            seen.clear();
            left = ROWS_BEFORE_RESTART;
        }
        // Scroll: drop the oldest row at the top and add the current one at the bottom.
        history.pop_front();
        history.push_back(row.clone());
        let frame = Pic { w, v: history.iter().flat_map(|r| r.iter().map(|c| shades[*c as usize])).collect() };
        // A repeated row means it cycles from here: show at most one more screenful.
        if seen.contains(&row) {
            left = left.min(HEIGHT);
        }
        seen.insert(row.clone());
        left -= 1;
        row = if totalistic { totalistic_step(&row, number) } else { elementary_step(&row, number) };
        vec![frame]
    })
}

/// Random grey.
/// Every LED gets a new random brightness (256 levels) every frame: a check that all LEDs work.
pub fn noise() -> Step {
    Box::new(|g| vec![Pic { w: g.width, v: (0..HEIGHT * g.width).map(|_| g.rng.below(256) as f64 / 255.0).collect() }])
}

/// A dot going back and forth across the whole picture, on a dim line, to set the keyboard gap.
///
/// A 3 x 4 bright dot on rows 15 to 18 slides one column per frame and bounces at the ends, over a dim
/// line on rows 16 and 17. Watching it cross from one panel to the other shows whether the gap set for
/// the keyboard looks right.
pub fn gap_test() -> Step {
    let mut x: i64 = 0;
    let mut step: i64 = 1;
    Box::new(move |g| {
        let w = g.width as i64;
        let mut frame = Pic::new(g.width);
        for c in 0..g.width {
            frame.set(c, 16, 40.0 / 255.0);
            frame.set(c, 17, 40.0 / 255.0);
        }
        for y in 15..19 {
            for c in (x - 1).max(0)..(x + 2).min(w) {
                frame.set(c as usize, y, 1.0);
            }
        }
        // Reverse at the ends (the 3-wide dot's centre stays between column 1 and width-2 when it can).
        if !(0 < x + step && x + step < w - 1) {
            step = -step;
        }
        x = (x + step).clamp(0, w - 1);
        vec![frame]
    })
}

// More maths ------------------------------------------------------------------------------------

/// An ant turns right on a dark square and left on a lit one, flipping it as it leaves.
///
/// The ant starts in the middle facing a random way; the edges wrap. It does 1 move a frame for the
/// first 60 frames, then 3 a frame, then 12, so you can see the rule and then the pattern it builds.
/// Starts over every 330 frames. The ant itself is the full-brightness LED; flipped cells are dimmer.
fn langton() -> Step {
    const STEPS_EACH: usize = 330;
    const TURNS: [(i64, i64); 4] = [(-1, 0), (0, 1), (1, 0), (0, -1)];
    let mut grid: Vec<bool> = Vec::new();
    let (mut y, mut x, mut d, mut f) = (0i64, 0i64, 0usize, STEPS_EACH);
    Box::new(move |g| {
        let w = g.width;
        if f >= STEPS_EACH || grid.len() != HEIGHT * w {
            grid = vec![false; HEIGHT * w];
            y = HEIGHT as i64 / 2;
            x = w as i64 / 2;
            d = g.rng.below(4);
            f = 0;
        }
        // Speed up after a while: 1 move a frame, then 3, then 12.
        let moves = if f < 60 {
            1
        } else if f < 140 {
            3
        } else {
            12
        };
        for _ in 0..moves {
            let i = y as usize * w + x as usize;
            // TURNS lists up, right, down, left as (dy, dx), so +1 is a right turn and +3 a left turn.
            d = if !grid[i] { (d + 1) % 4 } else { (d + 3) % 4 };
            grid[i] = !grid[i];
            y = (y + TURNS[d].0).rem_euclid(HEIGHT as i64);
            x = (x + TURNS[d].1).rem_euclid(w as i64);
        }
        let mut img = bools(&grid, w, 0.45);
        img.set(x as usize, y as usize, 1.0);
        f += 1;
        vec![img]
    })
}

/// The Collatz ("3n + 1") sequence from `n` down to 1, both ends included. `n` must be at least 1
/// (0 would loop forever). Every start ever tried reaches 1, though nobody has proved it always does.
pub fn collatz_sequence(mut n: u64) -> Vec<u64> {
    let mut seq = vec![n];
    while n != 1 {
        n = if n.is_multiple_of(2) { n / 2 } else { 3 * n + 1 };
        seq.push(n);
    }
    seq
}

/// Halve even numbers, triple odd ones and add 1, until reaching 1. Each number is a bar.
///
/// Each call makes a whole sequence's frames: the newest number is a bright bar on the right and the
/// older ones scroll left. Bar height is 34·(n/largest)^0.35: the 0.35 power squashes the huge peaks
/// so small values stay visible, and the bar's top LED shows the fractional part dimly. After the
/// sequence, bars of 1 push it off the picture. Starts with five famous long sequences (27 takes 111
/// steps), then random starts from 30 to 2999.
fn collatz() -> Step {
    const STARTS: [u64; 5] = [27, 97, 871, 703, 6171];
    let mut i = 0;
    Box::new(move |g| {
        let w = g.width;
        // 27, 97, 871, 703 and 6171 are known for very long sequences for their size.
        let start = if i < STARTS.len() { STARTS[i] } else { g.rng.range(30, 3000) as u64 };
        i += 1;
        let seq = collatz_sequence(start);
        let top = *seq.iter().max().unwrap_or(&1) as f64;
        let mut bars: Vec<f64> = Vec::new();
        let mut out = Vec::new();
        for n in seq.iter().copied().chain(std::iter::repeat_n(1, w)) {
            bars.push((HEIGHT as f64 * (n as f64 / top).powf(0.35)).max(1.0));
            let shown = &bars[bars.len().saturating_sub(w)..];
            let mut img = Pic::new(w);
            for (c, h) in shown.iter().enumerate() {
                // Bars are right-aligned: the newest at the right edge.
                let col = w - shown.len() + c;
                let full = *h as usize;
                for y in HEIGHT - full.min(HEIGHT)..HEIGHT {
                    img.set(col, y, 0.55);
                }
                if full < HEIGHT {
                    // The LED above a bar shows its fractional part, for smoother heights.
                    img.set(col, HEIGHT - full - 1, (h - full as f64) * 0.55);
                }
            }
            let last = *shown.last().unwrap_or(&1.0) as usize;
            for y in HEIGHT - last.min(HEIGHT)..HEIGHT {
                img.set(w - 1, y, 1.0);
            }
            out.push(img);
        }
        out
    })
}

/// Where each number 1..count lands on a square spiral from the middle of the picture.
///
/// Index k of the result is the (x, y) of number k (index 0 is a placeholder). 1 is in the middle;
/// the spiral then goes right 1, up 1, left 2, down 2, right 3, up 3... Positions may be off the
/// picture (the spiral is square, the picture is not); the caller skips those.
fn ulam_positions(width: usize, count: usize) -> Vec<(i64, i64)> {
    let (mut x, mut y) = (width as i64 / 2 - if width.is_multiple_of(2) { 1 } else { 0 }, HEIGHT as i64 / 2);
    let mut pos = vec![(0, 0), (x, y)];
    const TURNS: [(i64, i64); 4] = [(1, 0), (0, -1), (-1, 0), (0, 1)];
    let (mut step, mut d) = (1, 0);
    while pos.len() <= count {
        for _ in 0..2 {
            for _ in 0..step {
                x += TURNS[d].0;
                y += TURNS[d].1;
                pos.push((x, y));
            }
            d = (d + 1) % 4;
        }
        step += 1;
    }
    pos.truncate(count + 1);
    pos
}

/// `table[k]` is true when k is prime, for k up to n: the sieve of Eratosthenes (cross out every
/// multiple of each prime, starting at its square, since smaller multiples were crossed out already).
fn is_prime_table(n: usize) -> Vec<bool> {
    let mut t = vec![true; n + 1];
    t[0] = false;
    if n >= 1 {
        t[1] = false;
    }
    let mut p = 2;
    while p * p <= n {
        if t[p] {
            for m in (p * p..=n).step_by(p) {
                t[m] = false;
            }
        }
        p += 1;
    }
    t
}

/// The sieve of Eratosthenes on every LED (numbered left to right, top to bottom), then the primes
/// again on a spiral from the middle (the Ulam spiral).
///
/// Numbers 1 to 34·width each have an LED. Each prime p in turn is bright while its multiples are
/// crossed out in about 10 batches (crossed ones flash dim, then go dark). The sieve can stop at the
/// square root of the largest number. Then the primes alone are shown, then the Ulam spiral is drawn 25
/// numbers per frame: primes on a spiral line up on diagonals, which nobody fully understands.
fn primes() -> Step {
    Box::new(|g| {
        let w = g.width;
        let n = HEIGHT * w;
        let mut state = vec![1.0; n + 1];
        state[0] = 0.0;
        state[1] = 0.0;
        let frame = |state: &[f64], current: Option<usize>, crossing: &[usize]| {
            let mut img = Pic { w, v: state[1..].iter().map(|v| v * 0.5).collect() };
            for k in crossing {
                img.v[k - 1] = 0.18;
            }
            if let Some(c) = current {
                img.v[c - 1] = 1.0;
            }
            img
        };
        let mut out = Vec::new();
        for _ in 0..8 {
            out.push(frame(&state, None, &[]));
        }
        let mut p = 2;
        while p * p <= n {
            if state[p] != 0.0 {
                let multiples: Vec<usize> = (p * p..=n).step_by(p).collect();
                // Cross out in about 10 batches so big primes don't take long and small ones are
                // still visible.
                let chunk = (multiples.len() / 10).max(1);
                for part in multiples.chunks(chunk) {
                    out.push(frame(&state, Some(p), part));
                    for k in part {
                        state[*k] = 0.0;
                    }
                }
                out.push(frame(&state, Some(p), &[]));
            }
            p += 1;
        }
        for _ in 0..20 {
            out.push(Pic { w, v: state[1..].to_vec() });
        }
        // Enough numbers for the square spiral to cover the whole picture, wide or tall.
        let count = (HEIGHT.max(w) + 2).pow(2);
        let prime = is_prime_table(count);
        let mut img = Pic::new(w);
        for (k, &(x, y)) in ulam_positions(w, count).iter().enumerate().skip(1) {
            if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < HEIGHT {
                img.set(
                    x as usize,
                    y as usize,
                    if prime[k] {
                        1.0
                    } else if k == 1 {
                        0.3
                    } else {
                        0.0
                    },
                );
            }
            if k % 25 == 0 {
                out.push(img.clone());
            }
        }
        for _ in 0..40 {
            out.push(img.clone());
        }
        out
    })
}

/// A dot following the Lorenz weather equations, leaving a fading trail (the butterfly).
///
/// The Lorenz system: dx/dt = σ(y - x), dy/dt = x(ρ - z) - y, dz/dt = xy - βz, with the classic values
/// σ = 10, ρ = 28, β = 8/3 that make it chaotic. It is stepped by Euler's method (new = old + rate·dt)
/// with dt = 0.004, 8 small steps a frame. x (about -21..21) maps across the picture and z (about
/// 2..52) up it, the classic butterfly view. The trail fades to 96.5% each frame and is drawn at 75%,
/// with the current point at full brightness. The start is nudged at random so every run differs.
fn lorenz() -> Step {
    let (sigma, rho, beta) = (10.0, 28.0, 8.0 / 3.0);
    let mut started = false;
    let (mut x, mut y, mut z) = (1.0, 1.0, 20.0);
    let mut trail: Option<Pic> = None;
    Box::new(move |g| {
        let w = g.width;
        if !started {
            x = 1.0 + g.rng.random();
            started = true;
        }
        let trail = trail.get_or_insert_with(|| Pic::new(w));
        let dt = 0.004;
        *trail = trail.scaled(0.965);
        let (mut px, mut py) = (0.0, 0.0);
        for _ in 0..4 {
            for _ in 0..2 {
                // The three Lorenz equations, then one Euler step.
                let (dx, dy, dz) = (sigma * (y - x), x * (rho - z) - y, x * y - beta * z);
                x += dx * dt;
                y += dy * dt;
                z += dz * dt;
            }
            // Map x from about -21..21 across the picture and z from about 2..52 up it (row 0 at the top).
            px = (x + 21.0) / 42.0 * (w - 1) as f64;
            py = (HEIGHT - 1) as f64 - (z - 2.0) / 50.0 * (HEIGHT - 1) as f64;
            splat(trail, px, py, 0.3);
        }
        let mut img = trail.scaled(0.75);
        splat(&mut img, px, py, 1.0);
        vec![img]
    })
}

/// One small step of a double pendulum with equal arms and weights.
///
/// `a1`, `a2` are the arms' angles from straight down (radians), `w1`, `w2` their angular speeds
/// (radians per second); arm lengths and masses are 1 and gravity 9.8. The two acceleration formulas
/// are the standard equations of motion for this case (from Lagrange's method) with m = l = 1. It
/// uses the "semi-implicit" Euler step: speeds are updated first and the new speeds move the angles,
/// which keeps the energy from drifting up the way plain Euler would.
pub fn pendulum_step(a1: f64, a2: f64, w1: f64, w2: f64, dt: f64) -> (f64, f64, f64, f64) {
    let g = 9.8;
    let d = a1 - a2;
    let den = 3.0 - (2.0 * d).cos();
    // Angular accelerations of the two arms (the standard double pendulum equations with equal
    // lengths and masses of 1, simplified; `den` is their shared denominator).
    let acc1 = (-3.0 * g * a1.sin() - g * (a1 - 2.0 * a2).sin() - 2.0 * d.sin() * (w2 * w2 + w1 * w1 * d.cos())) / den;
    let acc2 = 2.0 * d.sin() * (2.0 * w1 * w1 + 2.0 * g * a1.cos() + w2 * w2 * d.cos()) / den;
    let (w1, w2) = (w1 + acc1 * dt, w2 + acc2 * dt);
    (a1 + w1 * dt, a2 + w2 * dt, w1, w2)
}

/// A pendulum hanging from another pendulum. The end leaves a trail and never repeats.
///
/// Starts with the first arm almost upside down (about 2.6 to 2.8 radians) and the second at 2.2, which
/// is chaotic. 10 steps of 0.006 s a frame. The pivot is the picture's centre and both arms are 4.2 LEDs
/// (less on a narrow picture so it fits). The arms are drawn as soft lines, the joints as dots, and
/// the tip leaves a trail that fades to 92% each frame.
fn pendulum() -> Step {
    let mut s: Option<(f64, f64, f64, f64)> = None;
    let mut trail: Option<Pic> = None;
    Box::new(move |g| {
        let w = g.width;
        let (mut a1, mut a2, mut w1, mut w2) = *s.get_or_insert_with(|| (2.6 + g.rng.random() * 0.2, 2.2, 0.0, 0.0));
        let (cx, cy) = ((w - 1) as f64 / 2.0, (HEIGHT - 1) as f64 / 2.0);
        let arm = 4.2f64.min((w - 1) as f64 / 4.0);
        let trail = trail.get_or_insert_with(|| Pic::new(w));
        *trail = trail.scaled(0.92);
        let (mut x1, mut y1, mut x2, mut y2) = (cx, cy, cx, cy);
        for _ in 0..10 {
            (a1, a2, w1, w2) = pendulum_step(a1, a2, w1, w2, 0.006);
            // Angle 0 hangs straight down; y grows downwards, so the first joint is at (sin, cos).
            x1 = cx + arm * a1.sin();
            y1 = cy + arm * a1.cos();
            x2 = x1 + arm * a2.sin();
            y2 = y1 + arm * a2.cos();
            splat(trail, x2, y2, 0.12);
        }
        s = Some((a1, a2, w1, w2));
        let mut img = trail.scaled(0.8);
        line(&mut img, cx, cy, x1, y1, 0.35);
        line(&mut img, x1, y1, x2, y2, 0.35);
        splat(&mut img, cx, cy, 0.4);
        splat(&mut img, x1, y1, 0.8);
        splat(&mut img, x2, y2, 1.0);
        vec![img]
    })
}

// Sorting: each sort records (the bars, the bars that are bright) for every step.

/// One frame of a sort: the bar heights in their current order, and the indexes of the bars to light
/// brightly (the ones being compared or moved).
type Scene = (Vec<usize>, Vec<usize>);

/// Bubble sort: walk the list comparing neighbours and swapping any pair in the wrong order; after
/// pass i the largest i+1 bars are in place at the end. Records a frame per comparison and per swap.
fn bubble_sort(a: &mut [usize], out: &mut Vec<Scene>) {
    let n = a.len();
    for i in 0..n {
        for j in 0..n.saturating_sub(1 + i) {
            out.push((a.to_vec(), vec![j, j + 1]));
            if a[j] > a[j + 1] {
                a.swap(j, j + 1);
                out.push((a.to_vec(), vec![j, j + 1]));
            }
        }
    }
}

/// Quick sort of `a[lo..=hi]` (Lomuto's partition): the last bar is the pivot; smaller bars are
/// swapped to the front, the pivot goes between the two groups, and each group is sorted the same
/// way. Records a frame per comparison and per swap.
fn quick_sort(a: &mut [usize], lo: usize, hi: usize, out: &mut Vec<Scene>) {
    if lo >= hi {
        return;
    }
    let (pivot, mut i) = (a[hi], lo);
    for j in lo..hi {
        out.push((a.to_vec(), vec![j, hi]));
        if a[j] < pivot {
            a.swap(i, j);
            out.push((a.to_vec(), vec![i, j]));
            i += 1;
        }
    }
    a.swap(i, hi);
    out.push((a.to_vec(), vec![i, hi]));
    // Guard against `i - 1` going below zero (usize can't be negative); nothing is left of i then.
    if i > 0 {
        quick_sort(a, lo, i - 1, out);
    }
    quick_sort(a, i + 1, hi, out);
}

/// Merge sort of `a[lo..hi]`: sort each half, then merge them by repeatedly taking the smaller front
/// bar from the two halves (copies of them, so `a` can be overwritten). Records a frame per bar placed.
fn merge_sort(a: &mut [usize], lo: usize, hi: usize, out: &mut Vec<Scene>) {
    if hi - lo < 2 {
        return;
    }
    let mid = (lo + hi) / 2;
    merge_sort(a, lo, mid, out);
    merge_sort(a, mid, hi, out);
    let (left, right) = (a[lo..mid].to_vec(), a[mid..hi].to_vec());
    let (mut i, mut j) = (0, 0);
    for k in lo..hi {
        if j >= right.len() || (i < left.len() && left[i] <= right[j]) {
            a[k] = left[i];
            i += 1;
        } else {
            a[k] = right[j];
            j += 1;
        }
        out.push((a.to_vec(), vec![k]));
    }
}

/// The sorts shown, in turn.
const SORTS: [&str; 3] = ["Bubble sort", "Quick sort", "Merge sort"];

/// Bars of different heights put in order by bubble sort, then quick sort, then merge sort. The
/// bars being compared or moved are bright.
///
/// One bar per column, heights spread evenly from 2 to 34 and shuffled. Each sort is recorded in full
/// first (6 frames of the shuffled bars, every step, then a sweep lighting the sorted bars left to
/// right and a short hold), and the frames are then handed out one per call. The sort's name goes to
/// the note.
fn sorting() -> Step {
    let mut which = 0;
    let mut scenes: VecDeque<Scene> = VecDeque::new();
    Box::new(move |g| {
        let w = g.width;
        if scenes.is_empty() {
            let name = SORTS[which % 3];
            which += 1;
            g.note = Some(name.into());
            let mut a: Vec<usize> = (0..w)
                .map(|i| (2.0 + (HEIGHT - 2) as f64 * i as f64 / (w.max(2) - 1) as f64).round_ties_even() as usize)
                .collect();
            g.rng.shuffle(&mut a);
            let mut out: Vec<Scene> = (0..6).map(|_| (a.clone(), Vec::new())).collect();
            match name {
                "Bubble sort" => bubble_sort(&mut a, &mut out),
                "Quick sort" => {
                    let hi = a.len() - 1;
                    quick_sort(&mut a, 0, hi, &mut out)
                }
                _ => {
                    let hi = a.len();
                    merge_sort(&mut a, 0, hi, &mut out)
                }
            }
            for i in 0..w {
                out.push((a.clone(), (0..=i).collect()));
            }
            for _ in 0..10 {
                out.push((a.clone(), (0..w).collect()));
            }
            scenes = out.into();
        }
        let Some((bars, bright)) = scenes.pop_front() else { return Vec::new() };
        let mut img = Pic::new(w);
        for (i, v) in bars.iter().enumerate() {
            let level = if bright.contains(&i) { 1.0 } else { 0.45 };
            for y in HEIGHT - (*v).min(HEIGHT)..HEIGHT {
                img.set(i, y, level);
            }
        }
        vec![img]
    })
}

/// Carves a maze by wandering and backing up from dead ends, then floods it from the top-left corner
/// until the flood reaches the bottom-right, and lights the shortest path.
///
/// The maze lives on a grid of "rooms" at odd (row, column) positions, with wall LEDs between them:
/// `cw` x `ch` rooms on the picture (on an even width the last column stays wall). Building is the
/// "recursive backtracker": from the current room, knock through the wall to a random unvisited
/// neighbour and go there; at a dead end, back up (pop the stack) until a room has an unvisited
/// neighbour. Every room ends up reachable by exactly one route. A frame is recorded every 2 moves,
/// with the digger lit.
///
/// Solving is a breadth-first flood from the top-left room: each frame adds every open cell next to
/// the flood (the front shown brighter), so the flood reaches each cell by a shortest route.
/// Remembering where each cell was reached from (`came_from`) gives the path back from the
/// bottom-right goal, which is then drawn two cells a frame and held for 25 frames.
/// A picture under 3 columns has no room for a maze and gives nothing (a dark frame).
fn maze() -> Step {
    const STEPS: [(i64, i64); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    Box::new(|g| {
        let w = g.width;
        let (cw, ch) = ((w - 1) / 2, (HEIGHT - 1) / 2);
        if cw == 0 {
            return Vec::new();
        }
        let mut wall = vec![1.0f64; HEIGHT * w];
        let mut seen = vec![false; ch * cw];
        let mut stack = vec![(0usize, 0usize)];
        seen[0] = true;
        wall[w + 1] = 0.0;
        let mut out = Vec::new();
        let mut k = 0;
        while let Some(&(cy, cx)) = stack.last() {
            let options: Vec<(usize, usize)> = STEPS
                .iter()
                .map(|(dy, dx)| (cy as i64 + dy, cx as i64 + dx))
                .filter(|(y, x)| {
                    *y >= 0
                        && *x >= 0
                        && (*y as usize) < ch
                        && (*x as usize) < cw
                        && !seen[*y as usize * cw + *x as usize]
                })
                .map(|(y, x)| (y as usize, x as usize))
                .collect();
            if options.is_empty() {
                stack.pop();
            } else {
                let (ny, nx) = options[g.rng.below(options.len())];
                seen[ny * cw + nx] = true;
                // Room (cy, cx) is the LED (2cy+1, 2cx+1); the wall between it and its neighbour is
                // halfway between them, at (cy+ny+1, cx+nx+1). Open that and the new room.
                wall[(cy + ny + 1) * w + cx + nx + 1] = 0.0;
                wall[(2 * ny + 1) * w + 2 * nx + 1] = 0.0;
                stack.push((ny, nx));
            }
            k += 1;
            if k % 2 == 0 {
                let mut img = Pic { w, v: wall.iter().map(|v| v * 0.4).collect() };
                if let Some(&(y, x)) = stack.last() {
                    img.set(2 * x + 1, 2 * y + 1, 1.0);
                }
                out.push(img);
            }
        }
        let start = (1usize, 1usize);
        let goal = (2 * ch - 1, 2 * cw - 1);
        let mut dist: HashMap<(usize, usize), usize> = HashMap::from([(start, 0)]);
        let mut order = vec![start];
        let mut came_from: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
        let mut edge = vec![start];
        while !edge.is_empty() && !dist.contains_key(&goal) {
            let mut new_edge = Vec::new();
            for &(y, x) in &edge {
                for (dy, dx) in STEPS {
                    // Open cells are never on the outer edge, so y + dy and x + dx can't be negative.
                    let p = ((y as i64 + dy) as usize, (x as i64 + dx) as usize);
                    if p.0 < HEIGHT && p.1 < w && wall[p.0 * w + p.1] == 0.0 && !dist.contains_key(&p) {
                        dist.insert(p, dist[&(y, x)] + 1);
                        order.push(p);
                        came_from.insert(p, (y, x));
                        new_edge.push(p);
                    }
                }
            }
            edge = new_edge;
            let mut img = Pic { w, v: wall.iter().map(|v| v * 0.4).collect() };
            for &(y, x) in &order {
                img.set(x, y, 0.15);
            }
            for &(y, x) in &edge {
                img.set(x, y, 0.7);
            }
            out.push(img);
        }
        if !dist.contains_key(&goal) {
            return out;
        }
        let mut path = vec![goal];
        while *path.last().unwrap() != start {
            path.push(came_from[path.last().unwrap()]);
        }
        path.reverse();
        let mut img = Pic::new(w);
        for i in (0..=path.len()).step_by(2) {
            img = Pic { w, v: wall.iter().map(|v| v * 0.4).collect() };
            for &(y, x) in &order {
                img.set(x, y, 0.12);
            }
            for &(y, x) in path.iter().take(i + 1) {
                img.set(x, y, 1.0);
            }
            out.push(img.clone());
        }
        for _ in 0..25 {
            out.push(img.clone());
        }
        out
    })
}

/// The outline of a sitting rabbit, drawn for a picture 18 wide and 34 tall.
///
/// (x, y) corners of a closed outline, going round once: ears at the top, the head, the back, the
/// feet at the bottom. It is stretched sideways to the picture's width for other widths.
const RABBIT: [(f64, f64); 31] = [
    (7.5, 11.0),
    (6.2, 5.0),
    (6.3, 1.5),
    (7.6, 1.2),
    (8.5, 4.0),
    (8.8, 9.5),
    (9.6, 9.5),
    (10.0, 4.0),
    (11.0, 1.3),
    (12.2, 1.8),
    (12.0, 5.5),
    (11.0, 10.5),
    (12.6, 12.5),
    (13.5, 15.0),
    (12.5, 17.5),
    (13.6, 20.0),
    (15.0, 24.0),
    (15.4, 28.0),
    (16.6, 29.5),
    (16.2, 31.8),
    (14.5, 32.5),
    (9.0, 32.6),
    (5.5, 32.5),
    (4.2, 31.2),
    (5.0, 29.8),
    (6.3, 28.6),
    (6.6, 25.0),
    (7.6, 21.0),
    (6.0, 18.5),
    (4.6, 16.2),
    (4.8, 13.6),
];

/// A complex number (real, imaginary), here also a point (x, y). Just the few operations the Fourier
/// circles need, so no maths crate is needed.
#[derive(Clone, Copy, Debug)]
struct C(f64, f64);

impl C {
    /// Sum.
    fn add(self, o: C) -> C {
        C(self.0 + o.0, self.1 + o.1)
    }
    /// Product: (a + bi)(c + di) = (ac - bd) + (ad + bc)i. Multiplying by `turn(angle)` rotates.
    fn mul(self, o: C) -> C {
        C(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
    /// Length (distance from 0).
    fn abs(self) -> f64 {
        self.0.hypot(self.1)
    }
    /// The unit complex number at `angle` radians, cos + i·sin (e^(i·angle)).
    fn turn(angle: f64) -> C {
        C(angle.cos(), angle.sin())
    }
}

/// n points evenly spaced along a closed outline.
///
/// The outline is closed by joining the last corner back to the first. Points are spaced by distance
/// along it (not one per corner): `lengths` holds the running length at each corner, and each sample's
/// distance is found on its segment and interpolated between that segment's two ends.
fn outline(points: &[(f64, f64)], n: usize) -> Vec<C> {
    let mut pts = points.to_vec();
    pts.push(points[0]);
    let mut lengths = vec![0.0];
    for p in pts.windows(2) {
        let l = lengths.last().unwrap() + (p[1].0 - p[0].0).hypot(p[1].1 - p[0].1);
        lengths.push(l);
    }
    let total = *lengths.last().unwrap();
    (0..n)
        .map(|i| {
            let at = total * i as f64 / n as f64;
            let seg = lengths.windows(2).position(|l| at >= l[0] && at <= l[1]).unwrap_or(0);
            let (l0, l1) = (lengths[seg], lengths[seg + 1]);
            let f = if l1 > l0 { (at - l0) / (l1 - l0) } else { 0.0 };
            C(pts[seg].0 + (pts[seg + 1].0 - pts[seg].0) * f, pts[seg].1 + (pts[seg + 1].1 - pts[seg].1) * f)
        })
        .collect()
}

/// Spinning circles on spinning circles, whose tip draws the White Rabbit.
///
/// The maths: the outline, sampled as 256 complex numbers p_j, is written as a sum of circles,
/// p_j = Σ c_k · e^(2πi·k·j/256), with c_k from the discrete Fourier transform. Each term is a
/// circle of radius |c_k| turning k times per trip round the outline (negative k turns the other
/// way); c_0 is the centre. Keeping only the 24 biggest circles still traces a recognisable rabbit.
/// Each frame advances t one 220th of the way round: the circles are chained end to end at angle
/// 2π·k·t, and the end of the chain is the pen. Circles larger than 0.6 LED and arms over 0.3 LED are
/// drawn faintly; the drawn path stays. When done, the drawing is held 40 frames and starts again.
fn epicycles() -> Step {
    const SAMPLES: usize = 256;
    const CIRCLES: usize = 24;
    const STEPS: usize = 220;
    let mut setup: Option<(Vec<(C, i64)>, C)> = None;
    let mut s = 0;
    let mut drawn: Vec<C> = Vec::new();
    Box::new(move |g| {
        let w = g.width;
        let (order, centre) = setup.get_or_insert_with(|| {
            // Worked out once per effect run: stretch the outline to the width, resample, transform.
            let scaled: Vec<(f64, f64)> = RABBIT.iter().map(|(x, y)| (x * w as f64 / 18.0, *y)).collect();
            let path = outline(&scaled, SAMPLES);
            // The discrete Fourier transform, done the slow way (256 points is quick enough)
            let coeffs: Vec<(C, i64)> = (0..SAMPLES)
                .map(|k| {
                    let mut sum = C(0.0, 0.0);
                    for (j, p) in path.iter().enumerate() {
                        sum = sum.add(p.mul(C::turn(-2.0 * PI * (k * j) as f64 / SAMPLES as f64)));
                    }
                    // Coefficients past the halfway point stand for negative frequencies (k - 256),
                    // which turn the other way; using the small signed frequency gives smooth circles.
                    let freq = if k < SAMPLES / 2 { k as i64 } else { k as i64 - SAMPLES as i64 };
                    (C(sum.0 / SAMPLES as f64, sum.1 / SAMPLES as f64), freq)
                })
                .collect();
            // c_0 (frequency 0) is the outline's average point: the fixed centre of the first circle.
            let centre = coeffs[0].0;
            let mut order: Vec<(C, i64)> = coeffs.into_iter().filter(|(_, f)| *f != 0).collect();
            order.sort_by(|a, b| b.0.abs().total_cmp(&a.0.abs()));
            order.truncate(CIRCLES);
            (order, centre)
        });
        if s >= STEPS + 40 {
            s = 0;
            drawn.clear();
        }
        let mut img = Pic::new(w);
        if s <= STEPS {
            let t = s as f64 / STEPS as f64;
            let mut pos = *centre;
            for (c, freq) in order.iter() {
                let r = c.abs();
                if r > 0.6 {
                    // Enough dots round the circle for its size (bigger circles get more).
                    let n = (r * 8.0) as usize + 6;
                    for i in 0..n {
                        let a = 2.0 * PI * i as f64 / n as f64;
                        splat(&mut img, pos.0 + r * a.cos(), pos.1 + r * a.sin(), 0.06);
                    }
                }
                // The next circle's centre: this circle's arm rotated to angle 2π·freq·t.
                let next = pos.add(c.mul(C::turn(2.0 * PI * *freq as f64 * t)));
                if r > 0.3 {
                    line(&mut img, pos.0, pos.1, next.0, next.1, 0.25);
                }
                pos = next;
            }
            drawn.push(pos);
            splat(&mut img, pos.0, pos.1, 1.0);
        }
        for p in &drawn {
            splat(&mut img, p.0, p.1, 0.7);
        }
        s += 1;
        vec![img]
    })
}

/// Specks wander at random, drifting down, and freeze when they touch the frost, which grows up from
/// the bottom edge in branches. Starts over when it nearly reaches the top.
///
/// This is diffusion-limited aggregation (DLA), the process behind frost and mineral branches. 14
/// specks start in the top 6 rows; each moves one step at random (left/right wraps round, up/down stops
/// at the edges) 3 times a frame, with a 15% chance per move of an extra step down. A speck next to
/// (or diagonal to) a frozen cell freezes there, glows briefly (fading by 10% a frame to 0.5), and a
/// new speck starts at the top. The frost starts as 2 cells in the middle of the bottom row. It starts
/// over (after a 30-frame hold) once frost reaches row 3, or after 900 frames.
fn frost() -> Step {
    const WALKERS: usize = 14;
    let mut stuck: Vec<bool> = Vec::new();
    let mut specks: Vec<(usize, usize)> = Vec::new();
    let mut glow: Vec<f64> = Vec::new();
    let mut frames = 0;
    let mut pause = 0;
    Box::new(move |g| {
        let w = g.width;
        if pause > 0 {
            pause -= 1;
            let frame = bools(&stuck, w, 0.5);
            if pause == 0 {
                stuck.clear();
            }
            return vec![frame];
        }
        if stuck.len() != HEIGHT * w {
            stuck = vec![false; HEIGHT * w];
            stuck[(HEIGHT - 1) * w + w / 2 - 1] = true;
            stuck[(HEIGHT - 1) * w + w / 2] = true;
            specks = (0..WALKERS).map(|_| (g.rng.below(w), g.rng.below(6))).collect();
            glow = vec![0.0; HEIGHT * w];
            frames = 0;
        }
        for _ in 0..3 {
            for s in specks.iter_mut() {
                // Random step of -1, 0 or +1 sideways (wrapping) and up/down (plus the drift).
                s.0 = (s.0 as i64 + g.rng.range(-1, 2)).rem_euclid(w as i64) as usize;
                let drift = if g.rng.random() < 0.15 { 1 } else { 0 };
                s.1 = (s.1 as i64 + g.rng.range(-1, 2) + drift).clamp(0, HEIGHT as i64 - 1) as usize;
                let (x, y) = *s;
                let near = (y.saturating_sub(1)..(y + 2).min(HEIGHT))
                    .any(|yy| (x.saturating_sub(1)..(x + 2).min(w)).any(|xx| stuck[yy * w + xx]));
                if !stuck[y * w + x] && near {
                    stuck[y * w + x] = true;
                    glow[y * w + x] = 1.0;
                    *s = (g.rng.below(w), 0);
                }
            }
        }
        for (i, v) in glow.iter_mut().enumerate() {
            // Fresh frost glows and fades to the steady 0.5 of old frost.
            *v = (*v * 0.9).max(if stuck[i] { 0.5 } else { 0.0 });
        }
        let mut img = Pic { w, v: glow.clone() };
        for &(x, y) in &specks {
            if !stuck[y * w + x] {
                img.set(x, y, img.get(x, y).max(0.25));
            }
        }
        frames += 1;
        if stuck[3 * w..4 * w].iter().any(|c| *c) || frames >= 900 {
            pause = 30;
        }
        vec![img]
    })
}

/// The first digits of pi (3, 1, 4, 1, 5...), from Machin's formula worked out in fixed point, so no
/// big-number library is needed.
///
/// Machin's formula: π = 16·arctan(1/5) - 4·arctan(1/239), with arctan(1/x) = 1/x - 1/(3x³) +
/// 1/(5x⁵) - … Numbers are kept as fixed-point arrays of base-10⁹ "limbs" (9 decimal digits each,
/// most significant first; limb 0 is the whole-number part), and only need division by a small
/// number, addition and subtraction, each done limb by limb with carries like long division on paper.
/// 10 extra digits are worked out and dropped, so rounding errors in the last limbs never show.
pub fn pi_digits(count: usize) -> Vec<u8> {
    let digits = count + 10;
    let limbs = digits / 9 + 2;
    let base: u64 = 1_000_000_000;
    // arctan(1/x) * scale, as base-1e9 limbs (most significant first)
    let arctan = |x: u64| -> Vec<u64> {
        let mut sum = vec![0u64; limbs];
        let mut term = vec![0u64; limbs];
        term[0] = 1;
        // Long division by a small number, from the most significant limb down. `rem * base + limb`
        // can't overflow: rem < d, d is at most 239² = 57121, so the value stays under about 6·10¹³.
        let div = |v: &mut Vec<u64>, d: u64| {
            let mut rem = 0u64;
            for limb in v.iter_mut() {
                let cur = rem * base + *limb;
                *limb = cur / d;
                rem = cur % d;
            }
        };
        div(&mut term, x);
        let mut k = 0u64;
        loop {
            // term = 1/x^(2k+1); part = term / (2k+1) is the k-th series term. Stop once it is zero
            // at this precision.
            let mut part = term.clone();
            div(&mut part, 2 * k + 1);
            if part.iter().all(|l| *l == 0) {
                break;
            }
            // Even k adds, odd k subtracts (the series alternates).
            if k.is_multiple_of(2) {
                let mut carry = 0;
                for i in (0..limbs).rev() {
                    let v = sum[i] + part[i] + carry;
                    sum[i] = v % base;
                    carry = v / base;
                }
            } else {
                let mut borrow = 0i64;
                for i in (0..limbs).rev() {
                    let mut v = sum[i] as i64 - part[i] as i64 - borrow;
                    borrow = 0;
                    if v < 0 {
                        v += base as i64;
                        borrow = 1;
                    }
                    sum[i] = v as u64;
                }
            }
            div(&mut term, x * x);
            k += 1;
        }
        sum
    };
    let (a, b) = (arctan(5), arctan(239));
    // pi = 16 arctan(1/5) - 4 arctan(1/239)
    let mut pi = vec![0i64; limbs];
    for i in 0..limbs {
        pi[i] = 16 * a[i] as i64 - 4 * b[i] as i64;
    }
    // Normalise: move each limb's overflow (or a negative value's borrow) into the limb above.
    for i in (1..limbs).rev() {
        let carry = pi[i].div_euclid(base as i64);
        pi[i] = pi[i].rem_euclid(base as i64);
        pi[i - 1] += carry;
    }
    let mut out = vec![pi[0] as u8];
    for limb in &pi[1..] {
        // Each limb is exactly 9 digits, with leading zeros.
        let s = format!("{:09}", limb);
        out.extend(s.bytes().map(|b| b - b'0'));
    }
    out.truncate(count);
    out
}

/// The first digits of e (2, 7, 1, 8, 2, 8...), by the spigot method.
///
/// e = 2 + 1/2!·(1 + 1/3·(1 + 1/4·(1 + …))): written in a "mixed radix" where position i counts in
/// units of 1/(i+2), all its fractional digits are 1. Multiplying the fraction by 10 and carrying from
/// the right end leftwards (each position keeps `value mod (i+2)` and passes on `value / (i+2)`) pushes
/// the next decimal digit out on the left. 10 more positions than digits keep the carries exact.
pub fn e_digits(count: usize) -> Vec<u8> {
    let mut a = vec![1u32; count + 10];
    let mut out = vec![2u8];
    for _ in 0..count.saturating_sub(1) {
        let mut carry = 0u32;
        for i in (0..a.len()).rev() {
            let v = a[i] * 10 + carry;
            a[i] = v % (i as u32 + 2);
            carry = v / (i as u32 + 2);
        }
        out.push(carry as u8);
    }
    out
}

/// Digits stream down as bars, one digit per row. With both panels, pi is on the left (growing out
/// from the keyboard) and e is on the right; with one panel it's just pi.
///
/// Each frame a new row appears at the top and the rows move down. Its bars are 0.6 bright and the
/// newest row is shown at full brightness. With a picture 18 or more wide, π's bar grows leftwards from
/// the middle and e's rightwards; narrower, only π from the left edge. Both digit lists (1000 digits
/// each) are worked out once when the effect starts and then repeat.
fn digits() -> Step {
    const COUNT: usize = 1000;
    let (pi, e) = (pi_digits(COUNT), e_digits(COUNT));
    let mut rows: VecDeque<Vec<f64>> = VecDeque::new();
    let mut i = 0;
    Box::new(move |g| {
        let w = g.width;
        let half = if w >= 18 { w / 2 } else { 0 };
        if rows.len() != HEIGHT || rows[0].len() != w {
            rows = (0..HEIGHT).map(|_| vec![0.0; w]).collect();
        }
        let mut row = vec![0.0; w];
        // This frame's digits: bar lengths of 0 to 9 LEDs.
        let (p, d) = (pi[i % COUNT] as usize, e[i % COUNT] as usize);
        if half > 0 {
            for v in &mut row[half.saturating_sub(p)..half] {
                *v = 0.6;
            }
            for v in &mut row[half..(half + d).min(w)] {
                *v = 0.6;
            }
        } else {
            for v in &mut row[..p.min(w)] {
                *v = 0.6;
            }
        }
        rows.pop_back();
        rows.push_front(row);
        let mut img = Pic { w, v: rows.iter().flatten().copied().collect() };
        for x in 0..w {
            if img.get(x, 0) > 0.0 {
                img.set(x, 0, 1.0);
            }
        }
        i += 1;
        vec![img]
    })
}

/// The (across, up-down) frequency pairs, one figure each; the figure's shape depends on their ratio.
const LISSAJOUS_RATIOS: [(f64, f64); 5] = [(1.0, 2.0), (3.0, 2.0), (3.0, 4.0), (5.0, 4.0), (1.0, 3.0)];

/// A dot swinging side to side and up and down at different speeds, drawing a looping figure that
/// slowly turns.
///
/// x = centre + rx·sin(a·t + shift), y = centre + ry·sin(b·t), with t going up 0.012 ten times a frame.
/// `shift` grows slowly within each figure, which makes the figure appear to turn in 3D. Each ratio in
/// `LISSAJOUS_RATIOS` is shown for 140 frames. The trail fades to 93% each frame.
fn lissajous() -> Step {
    const FRAMES_EACH: usize = 140;
    let mut trail: Option<Pic> = None;
    let mut t = 0.0;
    let mut n = 0usize;
    Box::new(move |g| {
        let w = g.width;
        let trail = trail.get_or_insert_with(|| Pic::new(w));
        let (cx, cy) = ((w - 1) as f64 / 2.0, (HEIGHT - 1) as f64 / 2.0);
        let (rx, ry) = (cx - 0.3, cy - 0.3);
        // Change figure every 140 frames; `shift` restarts at 0 for each.
        let (a, b) = LISSAJOUS_RATIOS[(n / FRAMES_EACH) % LISSAJOUS_RATIOS.len()];
        let shift = (n % FRAMES_EACH) as f64 * 0.012;
        *trail = trail.scaled(0.93);
        let (mut x, mut y) = (cx, cy);
        for _ in 0..10 {
            t += 0.012;
            x = cx + rx * (a * t + shift).sin();
            y = cy + ry * (b * t).sin();
            splat(trail, x, y, 0.18);
        }
        let mut img = trail.scaled(0.85);
        splat(&mut img, x, y, 1.0);
        n += 1;
        vec![img]
    })
}

/// (name, maker, suggested steps per second, what it is)
///
/// The "More maths" list on the page, in its order. The speed is what the page's speed control is set
/// to when the effect is chosen (frames a second); the text is shown under the list.
pub const EFFECTS: [(&str, fn() -> Step, u32, &str); 11] = [
    (
        "Langton's Ant",
        langton,
        15,
        "An ant turns right on dark squares and left on lit ones, flipping each square it leaves.",
    ),
    (
        "Collatz sequences",
        collatz,
        10,
        "Halve even numbers, triple odd ones and add 1. The bars bounce until they reach 1.",
    ),
    (
        "Prime sieve and spiral",
        primes,
        15,
        "Crosses out the multiples of each prime, then lays the primes out on a spiral (the Ulam spiral).",
    ),
    ("Lorenz attractor", lorenz, 20, "A dot following the Lorenz weather equations, tracing the butterfly."),
    ("Double pendulum", pendulum, 20, "A pendulum swinging from another pendulum. It never repeats."),
    ("Sorting", sorting, 20, "Bubble sort, quick sort and merge sort putting bars in order."),
    (
        "Maze: build and solve",
        maze,
        15,
        "Carves a maze, then floods it from one corner and lights the shortest way through.",
    ),
    (
        "Fourier circles draw a rabbit",
        epicycles,
        15,
        "Spinning circles on spinning circles, whose tip draws the White Rabbit.",
    ),
    ("Random walk frost", frost, 15, "Wandering specks freeze when they touch the frost, which grows up in branches."),
    ("Digits of pi and e", digits, 8, "Each digit is a bar that many lights long. Pi on the left, e on the right."),
    ("Lissajous curves", lissajous, 20, "A dot swinging two ways at once, drawing a looping figure that slowly turns."),
];

/// Tests of the maths (known digits, rules on tiny grids) and that every animation runs at every width.
#[cfg(test)]
mod tests {
    use super::*;

    /// The first `frames` pictures of an animation `width` wide, with a fixed seed.
    fn run(step: Step, width: usize, frames: usize) -> Vec<Pic> {
        let mut a = Anim::new(width, Rng::new(1), step);
        (0..frames).map(|_| a.next_frame()).collect()
    }

    /// π and e digits match the known values, including the Feynman point six 9s.
    #[test]
    fn digits_are_right() {
        assert_eq!(pi_digits(12), vec![3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5, 8]);
        let pi = pi_digits(1000);
        assert_eq!(pi.len(), 1000);
        // Digits 762-767 of pi are the Feynman point 999999
        assert_eq!(&pi[762..768], &[9, 9, 9, 9, 9, 9]);
        assert_eq!(e_digits(10), vec![2, 7, 1, 8, 2, 8, 1, 8, 2, 8]);
    }

    /// The sine graph lights at least one LED in every row, and starts in the middle column at x = -2π.
    #[test]
    fn sine_graph_crosses_the_middle() {
        let f = Formula::parse("sin(x)").unwrap();
        let p = draw_graph(&f, -2.0 * PI, 2.0 * PI, Some((-1.2, 1.2)), 9, 0.0, true);
        // The axis is dim down the middle column; the curve is bright in every row
        for y in 0..HEIGHT {
            assert!((0..9).any(|x| p.get(x, y) == FULL), "row {y}");
        }
        assert_eq!(p.get(4, 0), FULL);
    }

    /// tan's jumps are breaks: no row is lit all the way across.
    #[test]
    fn tangent_breaks_instead_of_drawing_a_wall() {
        let f = Formula::parse("tan(x)").unwrap();
        let p = draw_graph(&f, -PI, PI, Some((-4.0, 4.0)), 18, 0.0, false);
        assert!((0..HEIGHT).all(|y| (0..18).filter(|x| p.get(*x, y) == FULL).count() < 18));
    }

    /// A blinker (three in a row) flips upright and back; a 9 is refused as a count; traffic runs.
    #[test]
    fn life_blinker_and_traffic() {
        let w = 9;
        let mut grid = vec![false; HEIGHT * w];
        for x in 3..6 {
            grid[10 * w + x] = true;
        }
        let (birth, survive) = (parse_counts("3").unwrap(), parse_counts("23").unwrap());
        let next = life_step(&grid, w, &birth, &survive);
        assert!(next[9 * w + 4] && next[10 * w + 4] && next[11 * w + 4] && !next[10 * w + 3]);
        assert_eq!(life_step(&next, w, &birth, &survive), grid);
        assert!(parse_counts("39").is_err());
        let frames = run(traffic(0.35), 18, 50);
        assert!(frames.iter().all(|f| f.w == 18));
    }

    /// Rules 30 and 90 and totalistic code 3 give their textbook rows from a single cell.
    #[test]
    fn automata_rules() {
        let row = vec![0, 0, 0, 1, 0, 0, 0];
        assert_eq!(elementary_step(&row, 30), vec![0, 0, 1, 1, 1, 0, 0]);
        assert_eq!(elementary_step(&row, 90), vec![0, 0, 1, 0, 1, 0, 0]);
        assert_eq!(totalistic_step(&[0, 1, 0], 3), vec![1, 1, 1]);
    }

    /// 50 lattice-gas steps, collisions and wall bounces included, keep every particle.
    #[test]
    fn gas_keeps_its_particles() {
        let mut g = Gen { rng: Rng::new(3), width: 18, note: None };
        let grid = hpp_grid(&mut g, "Random", 0.3);
        let count = |g: &[u8]| g.iter().map(|c| c.count_ones()).sum::<u32>();
        let mut next = grid.clone();
        for _ in 0..50 {
            next = hpp_step(&next, 18);
        }
        assert_eq!(count(&grid), count(&next));
    }

    /// Every effect, plus noise and the gap test, makes pictures of the right size at 9, 18 and 48
    /// columns and isn't all dark; every Life and gas start and the other recipes run.
    #[test]
    fn every_animation_runs_at_every_width() {
        let mut steps: Vec<(String, fn() -> Step)> = EFFECTS.iter().map(|e| (e.0.to_string(), e.1)).collect();
        steps.push(("noise".into(), noise));
        steps.push(("gap".into(), gap_test));
        for (name, make) in steps {
            for w in [9, 18, 48] {
                let frames = run(make(), w, 400);
                assert!(frames.iter().all(|f| f.w == w && f.v.len() == w * HEIGHT), "{name} {w}");
                assert!(frames.iter().any(|f| f.v.iter().any(|v| *v > 0.0)), "{name} {w} stays dark");
            }
        }
        for start in STARTS {
            run(life(parse_counts("3").unwrap(), parse_counts("23").unwrap(), start.into(), 0.35), 18, 30);
        }
        for start in GAS_STARTS {
            run(gas(start.into(), 0.3), 9, 10);
        }
        run(automaton(true, 777, true), 18, 50);
        run(random_graphs(std::sync::Arc::new(8.into()), true), 18, 30);
    }
}
