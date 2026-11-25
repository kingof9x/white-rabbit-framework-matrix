//! The games on the Games & patterns page: Snake, Ledris, Breakout and Rabbit run (`white_rabbit/games.py`).
//!
//! Each game is plain logic with no window or panel code, so it can be tested on its own. It draws a
//! 34-row picture as wide as the panels it is played on (9 for one panel, 18 for both) with
//! brightness from 0.0 to 1.0. `Session` adds what every game shares: a count-in, pause, the score
//! screen at the end and high scores.
//!
//! Where it runs: `pages/games.rs` builds a `Session` and moves it to a game-loop thread, which calls
//! `Session::update` with the time since the last frame (in seconds, capped at 0.1 s) and
//! `Session::frame` about 30 times a second, and sends the picture to the LED panels. Key presses
//! from the window reach the loop and are passed to `Session::press`; held keys are passed with each
//! update as `Held`. The Math page (`pages/math.rs`) reuses `Pic` and `Rng` for its effects.
//!
//! Coordinates: x is the column from the left (0 .. width-1), y the row from the top (0 .. 33), as
//! on the panels, which are 9 LEDs wide and 34 tall. Game positions that move smoothly (the ball, the
//! falling rocks) are kept as `f64` in LED units and rounded only when drawn or tested for a hit.
//! Every game is time-based (speeds in LEDs or rows per second, waits in seconds), so it plays the
//! same whatever the frame rate.
//!
//! Nothing here touches files except `save_high_score`, which rewrites the app's settings.json.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// Rows on an LED panel (and so in every game picture).
pub const HEIGHT: usize = 34;
/// Columns on one LED panel; a picture for two panels is twice this.
pub const PANEL_WIDTH: usize = 9;

/// A key press passed to a game: the arrow keys, and space as `Action` (launch, drop).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Action,
}

/// The keys held down.
///
/// Only the keys a game repeats while held: left/right (Breakout's paddle, Rabbit run) and down
/// (Ledris' faster fall). Up and space only count when pressed.
#[derive(Clone, Copy, Debug, Default)]
pub struct Held {
    pub left: bool,
    pub right: bool,
    pub down: bool,
}

/// A picture, `HEIGHT` rows of `w` columns.
///
/// `v` holds the brightness of each LED, 0.0 (off) to 1.0 (full), row by row from the top: the LED at
/// column x, row y is `v[y * w + x]`. Its length is always `w * HEIGHT`.
#[derive(Clone, Debug, PartialEq)]
pub struct Pic {
    pub w: usize,
    pub v: Vec<f64>,
}

impl Pic {
    /// An all-dark picture `w` columns wide.
    pub fn new(w: usize) -> Pic {
        Pic { w, v: vec![0.0; w * HEIGHT] }
    }

    /// The brightness at (x, y), or 0.0 (dark) when that is off the picture.
    pub fn get(&self, x: usize, y: usize) -> f64 {
        if x < self.w && y < HEIGHT {
            self.v[y * self.w + x]
        } else {
            0.0
        }
    }

    /// Sets the brightness at (x, y); a point off the picture is silently ignored, so games can draw
    /// shapes that run over the edge without checking.
    pub fn set(&mut self, x: usize, y: usize, level: f64) {
        if x < self.w && y < HEIGHT {
            self.v[y * self.w + x] = level;
        }
    }

    /// Set if x, y are on the picture (they may be negative).
    pub fn put(&mut self, x: i64, y: i64, level: f64) {
        if x >= 0 && y >= 0 {
            self.set(x as usize, y as usize, level);
        }
    }

    /// A copy with every LED's brightness multiplied by `f` (used to dim the picture behind the pause
    /// sign and for the end-of-game blink).
    pub fn scaled(&self, f: f64) -> Pic {
        Pic { w: self.w, v: self.v.iter().map(|v| v * f).collect() }
    }

    /// 0-255 for the panels.
    ///
    /// Each value is multiplied by 255, rounded and clamped, so out-of-range values can't wrap round.
    pub fn bytes(&self) -> Vec<u8> {
        self.v.iter().map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8).collect()
    }
}

/// A small random number maker, so the games don't need another crate (and tests can fix the seed).
///
/// It is splitmix64: a 64-bit counter is advanced by a fixed odd constant and each value is scrambled
/// by two xor-shift-multiply rounds. That is fast, has a long period (2^64) and passes common
/// statistical tests, which is plenty for game randomness (it is not for anything secret).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator from a fixed seed: the same seed always gives the same numbers (used by the tests).
    /// The seed is mixed with the golden-ratio constant so small seeds like 0 or 1 start well apart.
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    /// A generator seeded from the operating system's random source, or from the clock (nanoseconds
    /// since 1970) if that fails, so every game plays differently.
    pub fn from_time() -> Rng {
        let mut b = [0u8; 8];
        if getrandom::fill(&mut b).is_err() {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            b = (t.as_nanos() as u64).to_le_bytes();
        }
        Rng::new(u64::from_le_bytes(b))
    }

    /// The next raw 64-bit random number.
    fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0.0 <= x < 1.0
    ///
    /// Uses the top 53 bits (an `f64` has a 53-bit mantissa), so every result is exactly representable
    /// and evenly spread.
    pub fn random(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// 0 <= x < n (n > 0)
    ///
    /// By remainder; the tiny bias this gives for n not a power of two is irrelevant for a game.
    /// An `n` of 0 is treated as 1 (always returns 0) instead of dividing by zero.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    /// lo <= x < hi
    /// (If hi <= lo it returns lo.)
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo).max(1) as usize) as i64
    }

    /// Puts `v` in a random order (the Fisher-Yates shuffle: each element in turn, from the end, is
    /// swapped with a random one at or before it, which makes every order equally likely).
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

// 3 x 5 digits for scores
/// Digits 0 to 9 as 3 x 5 bitmaps, one string per row, '1' for a lit LED.
const DIGITS: [[&str; 5]; 10] = [
    ["111", "101", "101", "101", "111"],
    ["010", "110", "010", "010", "111"],
    ["111", "001", "111", "100", "111"],
    ["111", "001", "011", "001", "111"],
    ["101", "101", "111", "001", "001"],
    ["111", "100", "111", "001", "111"],
    ["111", "100", "111", "101", "111"],
    ["111", "001", "010", "010", "010"],
    ["111", "101", "111", "101", "111"],
    ["111", "101", "111", "001", "111"],
];

/// The White Rabbit, 5 x 6, for Rabbit run
/// Rows top to bottom, '1' for a lit LED: two ears, the head, the eyes (the 0s in "10101") and body.
const RABBIT: [&str; 6] = ["10010", "10010", "11110", "10101", "11110", "01110"];

/// Writes digits into the picture, 3 x 5 each, side by side or stacked.
///
/// (x, y) is the top-left of the first digit. Side by side each digit takes 4 columns (3 plus a gap);
/// stacked each takes 6 rows (5 plus a gap), which is how scores fit on a panel only 9 wide.
/// Characters that aren't digits are skipped but still take their place. Parts off the picture are
/// clipped.
pub fn draw_digits(grid: &mut Pic, text: &str, x: usize, y: usize, level: f64, vertical: bool) {
    for (i, ch) in text.chars().enumerate() {
        let Some(d) = ch.to_digit(10) else { continue };
        let (gx, gy) = if vertical { (x, y + i * 6) } else { (x + i * 4, y) };
        for (dy, row) in DIGITS[d as usize].iter().enumerate() {
            for (dx, c) in row.chars().enumerate() {
                if c == '1' {
                    grid.set(gx + dx, gy + dy, level);
                }
            }
        }
    }
}

/// What every game has. update(dt, held) moves it on by dt seconds.
///
/// `Send` because the session is built on the UI thread and then moved to the game-loop thread.
pub trait Game: Send {
    /// A key was pressed (once per press; key repeat is not used).
    fn press(&mut self, key: Key);
    /// Moves the game on by `dt` seconds with `held` keys down. Does nothing once the game is over.
    fn update(&mut self, dt: f64, held: Held);
    /// The current picture, at the width the game was made with.
    fn draw(&self) -> Pic;
    /// Points so far.
    fn score(&self) -> u32;
    /// True once the game has ended (it then ignores keys and updates).
    fn over(&self) -> bool;
    /// The level, for games that have levels (shown by the page); None otherwise.
    fn level(&self) -> Option<u32> {
        None
    }
}

/// Which game. The order of `KINDS` is the order of the page's game buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Snake,
    Ledris,
    Breakout,
    RabbitRun,
}

/// All games, in the page's order (the window passes an index into this).
pub const KINDS: [Kind; 4] = [Kind::Snake, Kind::Ledris, Kind::Breakout, Kind::RabbitRun];

impl Kind {
    /// The name used as the key in settings.json's "high_scores".
    pub fn key(self) -> &'static str {
        match self {
            Kind::Snake => "snake",
            Kind::Ledris => "ledris",
            Kind::Breakout => "breakout",
            Kind::RabbitRun => "rabbit-run",
        }
    }

    /// The name shown on the page.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Snake => "Snake",
            Kind::Ledris => "Ledris",
            Kind::Breakout => "Breakout",
            Kind::RabbitRun => "Rabbit run",
        }
    }

    /// One sentence on how to play, shown under the picture.
    pub fn controls(self) -> &'static str {
        match self {
            Kind::Snake => "Arrow keys to steer. Eat the blinking dot; don't hit the edge or yourself.",
            Kind::Ledris => "Left and right to move, up to turn, down to drop faster, space to drop at once.",
            Kind::Breakout => "Left and right to move the paddle, space to launch the ball. Three balls.",
            Kind::RabbitRun => "Left and right to dodge the falling rocks and catch the blinking carrots.",
        }
    }

    /// Follows the "Both panels" choice. Ledris doesn't: its board is the left panel and the next
    /// piece is on the right.
    pub fn spans(self) -> bool {
        self != Kind::Ledris
    }

    /// A new game of this kind, `width` LEDs wide (9 per panel).
    pub fn make(self, width: usize, rng: Rng) -> Box<dyn Game> {
        match self {
            Kind::Snake => Box::new(Snake::new(width, rng)),
            Kind::Ledris => Box::new(Ledris::new(width, rng)),
            Kind::Breakout => Box::new(Breakout::new(width, rng)),
            Kind::RabbitRun => Box::new(RabbitRun::new(width, rng)),
        }
    }
}

// Snake

/// Snake: steer a growing line round the panel, eating dots. Each dot is a point and makes the snake
/// one longer and a little faster. Hitting the edge or itself ends the game (there is no wrap-round).
pub struct Snake {
    width: usize,
    rng: Rng,
    score: u32,
    over: bool,
    /// The snake's cells, head first, as (column, row).
    pub body: Vec<(i64, i64)>,
    /// The step taken each move, as (dx, dy): (0, -1) is up.
    direction: (i64, i64),
    /// Turns pressed but not used yet (at most two), so a quick "left, up" between two moves isn't lost.
    turns: Vec<(i64, i64)>,
    /// Seconds collected towards the next move.
    wait: f64,
    /// Seconds since the start, for blinking the food.
    clock: f64,
    /// Where the dot to eat is; None when the snake fills the whole board.
    pub food: Option<(i64, i64)>,
}

impl Snake {
    /// A three-cell snake in the middle column, rows 20 to 22, heading up, and a first dot.
    pub fn new(width: usize, rng: Rng) -> Snake {
        let x = (width / 2) as i64;
        let mut s = Snake {
            width,
            rng,
            score: 0,
            over: false,
            body: vec![(x, 20), (x, 21), (x, 22)],
            direction: (0, -1),
            turns: Vec::new(),
            wait: 0.0,
            clock: 0.0,
            food: None,
        };
        s.food = s.place_food();
        s
    }

    /// A random free cell (not under the snake) for the next dot, each free cell equally likely.
    /// None if there is no free cell left, which means the snake filled the board.
    fn place_food(&mut self) -> Option<(i64, i64)> {
        let mut free = Vec::new();
        for x in 0..self.width as i64 {
            for y in 0..HEIGHT as i64 {
                if !self.body.contains(&(x, y)) {
                    free.push((x, y));
                }
            }
        }
        if free.is_empty() {
            None
        } else {
            Some(free[self.rng.below(free.len())])
        }
    }

    /// Seconds between moves: 0.16 s at the start, 4 ms quicker per point, never below 0.06 s
    /// (reached at 25 points). So the snake goes from about 6 to about 17 moves a second.
    fn tick_seconds(&self) -> f64 {
        (0.16 - self.score as f64 * 0.004).max(0.06)
    }

    /// One move: take the next queued turn, move the head one cell, and either grow (on food) or drop
    /// the tail.
    fn step(&mut self) {
        if !self.turns.is_empty() {
            self.direction = self.turns.remove(0);
        }
        let (hx, hy) = self.body[0];
        let head = (hx + self.direction.0, hy + self.direction.1);
        let grows = Some(head) == self.food;
        // When not growing the tail moves away this same move, so the head may enter the tail's cell:
        // the collision test only looks at the cells that stay.
        let keep = if grows { self.body.len() } else { self.body.len() - 1 };
        let hits = !(0..self.width as i64).contains(&head.0)
            || !(0..HEIGHT as i64).contains(&head.1)
            || self.body[..keep].contains(&head);
        if hits {
            self.over = true;
            return;
        }
        self.body.truncate(keep);
        self.body.insert(0, head);
        if grows {
            self.score += 1;
            self.food = self.place_food();
            // Board full: nothing left to eat, so the game ends (a perfect game).
            if self.food.is_none() {
                self.over = true;
            }
        }
    }
}

impl Game for Snake {
    fn press(&mut self, key: Key) {
        // Arrow keys become a one-cell step (dx, dy); y grows downwards, so Up is (0, -1).
        let step = match key {
            Key::Left => (-1, 0),
            Key::Right => (1, 0),
            Key::Up => (0, -1),
            Key::Down => (0, 1),
            Key::Action => return,
        };
        // Remember up to two quick turns; a turn straight back into the body is ignored
        let last = *self.turns.last().unwrap_or(&self.direction);
        if step != last && step != (-last.0, -last.1) && self.turns.len() < 2 {
            self.turns.push(step);
        }
    }

    // Moves happen on a fixed beat; a long frame can do several moves so speed doesn't depend on
    // the frame rate.
    fn update(&mut self, dt: f64, _held: Held) {
        self.clock += dt;
        self.wait += dt;
        while self.wait >= self.tick_seconds() && !self.over {
            self.wait -= self.tick_seconds();
            self.step();
        }
    }

    fn draw(&self) -> Pic {
        let mut g = Pic::new(self.width);
        for (i, &(x, y)) in self.body.iter().enumerate() {
            // The head is full brightness; the body fades from 0.8 towards the tail, never below 0.35.
            g.put(x, y, if i == 0 { 1.0 } else { (0.8 - i as f64 * 0.015).max(0.35) });
        }
        if let Some((x, y)) = self.food {
            // The dot blinks twice a second (bright for 0.25 s, dim for 0.25 s).
            g.put(x, y, if (self.clock * 4.0) % 2.0 < 1.0 { 1.0 } else { 0.45 });
        }
        g
    }

    fn score(&self) -> u32 {
        self.score
    }

    fn over(&self) -> bool {
        self.over
    }
}

// Ledris: falling blocks. The board is the left panel; with both panels the right one shows the next
// piece, the score and the level.

/// A piece's cells as (column, row) offsets from its top-left corner.
type Cells = Vec<(i64, i64)>;

/// The seven four-cell pieces, as (column, row): I, O, T, S, Z, J and L (the last two drawn upright).
const SHAPES: [[(i64, i64); 4]; 7] = [
    [(0, 0), (1, 0), (2, 0), (3, 0)],
    [(0, 0), (1, 0), (0, 1), (1, 1)],
    [(0, 0), (1, 0), (2, 0), (1, 1)],
    [(1, 0), (2, 0), (0, 1), (1, 1)],
    [(0, 0), (1, 0), (1, 1), (2, 1)],
    [(0, 0), (0, 1), (0, 2), (1, 2)],
    [(1, 0), (1, 1), (1, 2), (0, 2)],
];
/// Points for clearing 0, 1, 2, 3 or 4 rows at once (more rows at once pay more).
const POINTS: [u32; 5] = [0, 1, 3, 5, 8];
/// How long full rows blink before they are removed.
const FLASH_SECONDS: f64 = 0.3;

/// Moves cells so the smallest column and row are 0, and sorts them. Used after a turn so a turned
/// piece is again measured from its top-left, and so equal shapes compare equal.
fn normalised(cells: &[(i64, i64)]) -> Cells {
    let mx = cells.iter().map(|c| c.0).min().unwrap_or(0);
    let my = cells.iter().map(|c| c.1).min().unwrap_or(0);
    let mut out: Cells = cells.iter().map(|(x, y)| (x - mx, y - my)).collect();
    out.sort();
    out
}

/// Ledris: the falling-blocks game. The board is always 9 x 34 on the left panel; with both panels
/// the right one shows the next piece, the score and the level. A full row disappears and the rows
/// above drop down; a new piece that can't fit at the top ends the game.
pub struct Ledris {
    width: usize,
    rng: Rng,
    score: u32,
    over: bool,
    /// The settled cells, `board[row][column]`, row 0 at the top.
    pub board: [[bool; PANEL_WIDTH]; HEIGHT],
    /// Rows cleared so far; every 5 rows the level goes up.
    lines: u32,
    level: u32,
    /// Index into `SHAPES` of the piece that comes next (shown on the right panel).
    next: usize,
    /// Seconds collected towards the next one-row drop.
    fall: f64,
    /// Full rows blinking before removal; while not empty the game waits and ignores keys.
    flash_rows: Vec<usize>,
    /// Seconds of blinking left.
    flash: f64,
    /// The falling piece's cells, offsets from (x, y).
    pub cells: Cells,
    /// The falling piece's column on the board (its leftmost cell).
    pub x: i64,
    /// The falling piece's row on the board (its top cell).
    pub y: i64,
}

impl Ledris {
    /// A new game with an empty board; `width` only matters for drawing (9: board only, 18: side info).
    pub fn new(width: usize, mut rng: Rng) -> Ledris {
        let next = rng.below(SHAPES.len());
        let mut l = Ledris {
            width,
            rng,
            score: 0,
            over: false,
            board: [[false; PANEL_WIDTH]; HEIGHT],
            lines: 0,
            level: 1,
            next,
            fall: 0.0,
            flash_rows: Vec::new(),
            flash: 0.0,
            cells: Vec::new(),
            x: 0,
            y: 0,
        };
        l.spawn();
        l
    }

    /// Brings in the next piece, centred at the top, and picks a new "next". If it overlaps settled
    /// cells the board is full and the game ends.
    fn spawn(&mut self) {
        self.cells = normalised(&SHAPES[self.next]);
        self.next = self.rng.below(SHAPES.len());
        let w = self.cells.iter().map(|c| c.0).max().unwrap_or(0) + 1;
        self.x = (PANEL_WIDTH as i64 - w) / 2;
        self.y = 0;
        if !self.fits(&self.cells, self.x, self.y) {
            self.over = true;
        }
    }

    ///
    /// Whether the cells placed at (ox, oy) are inside the board's sides and bottom and don't overlap
    /// settled cells. Cells above the top (negative rows) count as fitting, so a turn near the top
    /// works.
    fn fits(&self, cells: &[(i64, i64)], ox: i64, oy: i64) -> bool {
        cells.iter().all(|(x, y)| {
            let (cx, cy) = (ox + x, oy + y);
            (0..PANEL_WIDTH as i64).contains(&cx)
                && cy < HEIGHT as i64
                && (cy < 0 || !self.board[cy as usize][cx as usize])
        })
    }

    /// Seconds per one-row drop: 0.55 s at level 1, 0.05 s quicker per level, never below 0.07 s.
    fn fall_seconds(&self) -> f64 {
        (0.55 - (self.level as f64 - 1.0) * 0.05).max(0.07)
    }

    ///
    /// The piece has landed: copy it into the board. A piece that sticks out above the top ends the
    /// game. Full rows score `POINTS` and start blinking; otherwise the next piece comes at once.
    fn lock(&mut self) {
        for &(x, y) in &self.cells {
            if y + self.y < 0 {
                self.over = true;
                return;
            }
            self.board[(self.y + y) as usize][(self.x + x) as usize] = true;
        }
        self.fall = 0.0;
        let full: Vec<usize> = (0..HEIGHT).filter(|&r| self.board[r].iter().all(|c| *c)).collect();
        if full.is_empty() {
            self.spawn();
        } else {
            self.lines += full.len() as u32;
            self.score += POINTS[full.len().min(4)];
            self.level = 1 + self.lines / 5;
            self.flash_rows = full;
            self.flash = FLASH_SECONDS;
        }
    }
}

impl Game for Ledris {
    fn press(&mut self, key: Key) {
        // No moves while rows blink: the piece has already landed and the next isn't in yet.
        if self.over || !self.flash_rows.is_empty() {
            return;
        }
        match key {
            Key::Left | Key::Right => {
                let dx = if key == Key::Left { -1 } else { 1 };
                // Move one column if the piece still fits there; otherwise ignore the key.
                if self.fits(&self.cells, self.x + dx, self.y) {
                    self.x += dx;
                }
            }
            Key::Up => {
                // (x, y) -> (-y, x) is a quarter turn (clockwise on screen, where y points down);
                // normalising moves the result back to start at (0, 0).
                let turned = normalised(&self.cells.iter().map(|&(x, y)| (-y, x)).collect::<Vec<_>>());
                // Turn, nudging away from a wall if needed
                // Try the turn in place, then shifted 1 and 2 columns either way (a "wall kick").
                // If none fits, the piece doesn't turn.
                for dx in [0, -1, 1, -2, 2] {
                    if self.fits(&turned, self.x + dx, self.y) {
                        self.cells = turned;
                        self.x += dx;
                        break;
                    }
                }
            }
            Key::Action => {
                // Hard drop: straight to the lowest place it fits, and land at once.
                while self.fits(&self.cells, self.x, self.y + 1) {
                    self.y += 1;
                }
                self.lock();
            }
            Key::Down => {}
        }
    }

    fn update(&mut self, dt: f64, held: Held) {
        if self.over {
            return;
        }
        // Blink, then remove the full rows: copy the other rows to the bottom of a fresh board,
        // in order, so everything above a removed row drops by the number of rows removed.
        if !self.flash_rows.is_empty() {
            self.flash -= dt;
            if self.flash <= 0.0 {
                let mut board = [[false; PANEL_WIDTH]; HEIGHT];
                let keep: Vec<_> = (0..HEIGHT).filter(|r| !self.flash_rows.contains(r)).collect();
                let offset = self.flash_rows.len();
                for (i, r) in keep.into_iter().enumerate() {
                    board[offset + i] = self.board[r];
                }
                self.board = board;
                self.flash_rows.clear();
                self.spawn();
            }
            return;
        }
        self.fall += dt;
        // Holding Down drops at least 20 rows a second (soft drop).
        let step = if held.down { self.fall_seconds().min(0.05) } else { self.fall_seconds() };
        // Drop one row per `step` seconds collected; a piece that can't go lower lands (`lock`).
        while self.fall >= step && !self.over && self.flash_rows.is_empty() {
            self.fall -= step;
            if self.fits(&self.cells, self.x, self.y + 1) {
                self.y += 1;
            } else {
                self.lock();
            }
        }
    }

    fn draw(&self) -> Pic {
        let mut g = Pic::new(self.width);
        for (y, row) in self.board.iter().enumerate() {
            for (x, on) in row.iter().enumerate() {
                if *on {
                    g.set(x, y, 0.7);
                }
            }
        }
        if !self.flash_rows.is_empty() {
            // Full rows blink 10 times a second (on and off every 0.05 s).
            let level = if (self.flash * 20.0) as i64 % 2 == 0 { 1.0 } else { 0.25 };
            for &r in &self.flash_rows {
                for x in 0..PANEL_WIDTH {
                    g.set(x, r, level);
                }
            }
        } else if !self.over {
            for &(x, y) in &self.cells {
                g.put(self.x + x, self.y + y, 1.0);
            }
        }
        // Side panel: dim lines across rows 0 and 7 frame the next piece (drawn from row 2),
        // then the score as 3 stacked digits from row 10, and the level (1 digit, dimmer) at row 28.
        if self.width >= 2 * PANEL_WIDTH {
            let right = PANEL_WIDTH;
            for x in right..self.width {
                g.set(x, 0, 0.2);
                g.set(x, 7, 0.2);
            }
            for (x, y) in normalised(&SHAPES[self.next]) {
                g.set(right + 2 + x as usize, 2 + y as usize, 1.0);
            }
            draw_digits(&mut g, &format!("{:03}", self.score.min(999)), right + 3, 10, 1.0, true);
            draw_digits(&mut g, &self.level.min(9).to_string(), right + 3, 28, 0.5, false);
        }
        g
    }

    fn score(&self) -> u32 {
        self.score
    }

    fn over(&self) -> bool {
        self.over
    }

    fn level(&self) -> Option<u32> {
        Some(self.level)
    }
}

// Breakout

/// The paddle's width in LEDs.
const PADDLE: f64 = 3.0;

/// Breakout: bounce the ball off the paddle into the bricks. A brick is 2 LEDs wide and gives one
/// point. Clearing all bricks starts the next level with new bricks and a faster ball. Three balls.
pub struct Breakout {
    width: usize,
    rng: Rng,
    score: u32,
    over: bool,
    level: u32,
    /// Balls left, including the one in play.
    pub lives: u32,
    /// The paddle's left edge, in LEDs from the left; it sits on the bottom row.
    pub paddle: f64,
    /// Seconds until a held arrow key moves the paddle again.
    move_wait: f64,
    /// `bricks[row][column]`: true where a brick still stands (rows 2 to 7 at the start).
    pub bricks: Vec<Vec<bool>>,
    /// The ball is resting on the paddle waiting to be launched.
    pub stuck: bool,
    /// Seconds until a resting ball launches by itself.
    launch_wait: f64,
    /// The ball's speed across, in LEDs per second (negative is left).
    pub vx: f64,
    /// The ball's speed down, in LEDs per second (negative is up).
    pub vy: f64,
    /// The ball's column, in LEDs (fractional; rounded to draw it).
    pub bx: f64,
    /// The ball's row, in LEDs from the top.
    pub by: f64,
}

impl Breakout {
    /// A new game: paddle centred, bricks up, ball resting on the paddle.
    pub fn new(width: usize, rng: Rng) -> Breakout {
        let mut b = Breakout {
            width,
            rng,
            score: 0,
            over: false,
            level: 1,
            lives: 3,
            paddle: (width as f64 - PADDLE) / 2.0,
            move_wait: 0.0,
            bricks: Vec::new(),
            stuck: true,
            launch_wait: 0.0,
            vx: 0.0,
            vy: 0.0,
            bx: 0.0,
            by: 0.0,
        };
        b.new_bricks();
        b.serve();
        b
    }

    /// Fills rows 2 to 7 with bricks across the whole width; every other row is empty.
    fn new_bricks(&mut self) {
        self.bricks = (0..HEIGHT).map(|r| vec![(2..8).contains(&r); self.width]).collect();
    }

    /// Puts a ball on the paddle; it launches on Space or by itself after 1.5 s.
    fn serve(&mut self) {
        self.stuck = true;
        self.launch_wait = 1.5;
        self.vx = 0.0;
        self.vy = 0.0;
        self.follow_paddle();
    }

    /// Puts the resting ball on the middle of the paddle, one row above it.
    fn follow_paddle(&mut self) {
        self.bx = self.paddle + PADDLE / 2.0 - 0.5;
        self.by = HEIGHT as f64 - 2.0;
    }

    /// The ball's speed in LEDs per second: 14 at level 1, 3 more each level.
    fn speed(&self) -> f64 {
        14.0 + 3.0 * (self.level as f64 - 1.0)
    }

    /// Sends the ball up and randomly left or right, at a steep angle (across 0.45, up 0.9 of the speed).
    fn launch(&mut self) {
        self.stuck = false;
        let direction = if self.rng.below(2) == 0 { -1.0 } else { 1.0 };
        let speed = self.speed();
        self.vx = direction * speed * 0.45;
        self.vy = -speed * 0.9;
    }

    /// Moves the paddle by `dx` LEDs, kept inside the picture; a resting ball moves with it.
    fn move_paddle(&mut self, dx: f64) {
        self.paddle = (self.paddle + dx).max(0.0).min(self.width as f64 - PADDLE);
        if self.stuck {
            self.follow_paddle();
        }
    }

    ///
    /// One small step of the ball over `dt` seconds: move, bounce off the sides and top, then check the
    /// cell it is in for a brick, then the bottom row for the paddle. `update` keeps each step under
    /// 0.4 LEDs so the ball can't pass through a brick.
    fn move_ball(&mut self, dt: f64) {
        self.bx += self.vx * dt;
        self.by += self.vy * dt;
        let right = self.width as f64 - 1.0;
        if self.bx < 0.0 || self.bx > right {
            self.vx = if self.bx < 0.0 { self.vx.abs() } else { -self.vx.abs() };
            self.bx = self.bx.clamp(0.0, right);
        }
        if self.by < 0.0 {
            self.vy = self.vy.abs();
            self.by = 0.0;
        }
        // The LED the ball is drawn on. `bx` was clamped above, so `col` is always a valid column.
        let (col, row) = (self.bx.round() as usize, self.by.round() as i64);
        if (0..HEIGHT as i64).contains(&row) && self.bricks[row as usize][col] {
            // Bricks are 2 LEDs wide
            // Clear both LEDs of the brick (bricks start on even columns), bounce back vertically, score.
            let left = col - col % 2;
            for c in left..(left + 2).min(self.width) {
                self.bricks[row as usize][c] = false;
            }
            self.vy = -self.vy;
            self.score += 1;
            if !self.bricks.iter().flatten().any(|b| *b) {
                self.level += 1;
                self.new_bricks();
                self.serve();
            }
            return;
        }
        if self.vy > 0.0 && row >= HEIGHT as i64 - 1 {
            // The ball is over the paddle (with half an LED of slack each side, because positions
            // are LED centres).
            if self.paddle - 0.5 <= self.bx && self.bx <= self.paddle + PADDLE - 0.5 {
                // Where the ball hits the paddle sets its angle
                // `offset` is -1 at the paddle's left end, 0 in the middle, +1 at the right end. The new
                // sideways speed is 70% of that (plus a little random spread so rallies don't repeat),
                // limited to 85% of the speed, and the upward speed is set so the total speed stays the
                // same (Pythagoras: vx² + vy² = speed²).
                let offset = (self.bx - (self.paddle + PADDLE / 2.0 - 0.5)) / (PADDLE / 2.0);
                let speed = self.speed();
                self.vx = (offset * 0.7 + (self.rng.random() - 0.5) * 0.2).clamp(-0.85, 0.85) * speed;
                self.vy = -(speed * speed - self.vx * self.vx).sqrt();
                self.by = HEIGHT as f64 - 2.0;
            } else if self.by > HEIGHT as f64 {
                // Missed: once the ball has left the bottom, lose a ball and serve again (or end).
                self.lives -= 1;
                if self.lives == 0 {
                    self.over = true;
                } else {
                    self.serve();
                }
            }
        }
    }
}

impl Game for Breakout {
    fn press(&mut self, key: Key) {
        match key {
            Key::Left | Key::Right => {
                self.move_paddle(if key == Key::Left { -1.0 } else { 1.0 });
                // A press moves at once; holding then repeats after 0.12 s (see `update`).
                self.move_wait = 0.12;
            }
            Key::Action if self.stuck => self.launch(),
            _ => {}
        }
    }

    fn update(&mut self, dt: f64, held: Held) {
        if self.over {
            return;
        }
        // Held keys keep the paddle moving, 20 steps a second
        self.move_wait -= dt;
        while self.move_wait <= 0.0 && held.left != held.right {
            self.move_paddle(if held.left { -1.0 } else { 1.0 });
            // Holding both arrows (or neither) does nothing. Each repeat adds 0.05 s, so a long frame
            // catches up with several moves; with no arrow held the wait is kept at 0 (below), so it
            // can't save up moves for later.
            self.move_wait += 0.05;
        }
        if self.move_wait < 0.0 {
            self.move_wait = 0.0;
        }
        if self.stuck {
            // A resting ball launches by itself when its wait runs out (so the game never stalls).
            self.launch_wait -= dt;
            if self.launch_wait <= 0.0 {
                self.launch();
            }
            return;
        }
        // Small steps so the ball can't jump over a brick
        // Split this frame's movement into steps of at most 0.4 LEDs along the faster axis.
        let steps = ((self.vx.abs().max(self.vy.abs()) * dt / 0.4).ceil() as usize).max(1);
        for _ in 0..steps {
            self.move_ball(dt / steps as f64);
            if self.stuck || self.over {
                break;
            }
        }
    }

    fn draw(&self) -> Pic {
        let mut g = Pic::new(self.width);
        for (y, row) in self.bricks.iter().enumerate() {
            for (x, on) in row.iter().enumerate() {
                if *on {
                    let mut level = if y % 2 == 0 { 0.8 } else { 0.5 };
                    // Alternate rows are brighter, and the right half of each brick is dimmer, so
                    // separate bricks can be told apart.
                    if (2..8).contains(&y) && x % 2 == 1 {
                        level *= 0.75;
                    }
                    g.set(x, y, level);
                }
            }
        }
        let p = self.paddle.round() as usize;
        for x in p..p + PADDLE as usize {
            g.set(x, HEIGHT - 1, 1.0);
        }
        g.put(self.bx.round() as i64, self.by.round() as i64, 1.0);
        // Balls left, in the top corner
        for i in 0..self.lives.saturating_sub(1) as i64 {
            // One dim dot per spare ball, every other LED leftwards from the top-right corner.
            g.put(self.width as i64 - 1 - 2 * i, 0, 0.4);
        }
        g
    }

    fn score(&self) -> u32 {
        self.score
    }

    fn over(&self) -> bool {
        self.over
    }

    fn level(&self) -> Option<u32> {
        Some(self.level)
    }
}

// Rabbit run

/// Rabbit run: move the White Rabbit (5 x 6, at the bottom) left and right to dodge falling 2 x 2
/// rocks and catch falling carrots (a point each). A rock touching the rabbit ends the game. Everything
/// falls faster the longer you last.
pub struct RabbitRun {
    width: usize,
    rng: Rng,
    score: u32,
    over: bool,
    /// The rabbit's left edge, in LEDs (0 .. width-5).
    pub x: f64,
    /// (column, row), the row moving down
    pub rocks: Vec<(i64, f64)>,
    /// (column, row) of each carrot, the row moving down.
    pub carrots: Vec<(i64, f64)>,
    /// Seconds since the start; sets the speed.
    clock: f64,
    /// Seconds until the next rock appears.
    rock_wait: f64,
    /// Seconds until the next carrot appears (the first after 1 s).
    carrot_wait: f64,
    /// Seconds until a held arrow key moves the rabbit again.
    move_wait: f64,
}

impl RabbitRun {
    /// A new game with the rabbit centred and nothing falling yet.
    pub fn new(width: usize, rng: Rng) -> RabbitRun {
        RabbitRun {
            width,
            rng,
            score: 0,
            over: false,
            x: (width as f64 - 5.0) / 2.0,
            rocks: Vec::new(),
            carrots: Vec::new(),
            clock: 0.0,
            rock_wait: 0.0,
            carrot_wait: 1.0,
            move_wait: 0.0,
        }
    }

    /// Moves the rabbit `dx` LEDs, kept fully on the picture.
    fn step(&mut self, dx: f64) {
        self.x = (self.x + dx).max(0.0).min(self.width as f64 - 5.0);
    }

    /// Rows a second the rocks fall; faster the longer you last.
    ///
    /// 9 rows a second at the start, a quarter row a second faster for every second played, up to 25
    /// (reached after 64 s).
    fn speed(&self) -> f64 {
        9.0 + (self.clock * 0.25).min(16.0)
    }

    /// The lit cells of the rabbit, in picture coordinates; it stands on the bottom 6 rows.
    fn rabbit_cells(&self) -> Vec<(i64, i64)> {
        let x = self.x.round() as i64;
        let mut out = Vec::new();
        for (dy, row) in RABBIT.iter().enumerate() {
            for (dx, c) in row.chars().enumerate() {
                if c == '1' {
                    out.push((x + dx as i64, HEIGHT as i64 - 6 + dy as i64));
                }
            }
        }
        out
    }
}

impl Game for RabbitRun {
    fn press(&mut self, key: Key) {
        if matches!(key, Key::Left | Key::Right) {
            self.step(if key == Key::Left { -1.0 } else { 1.0 });
            self.move_wait = 0.12;
        }
    }

    fn update(&mut self, dt: f64, held: Held) {
        if self.over {
            return;
        }
        self.clock += dt;
        self.move_wait -= dt;
        while self.move_wait <= 0.0 && held.left != held.right {
            self.step(if held.left { -1.0 } else { 1.0 });
            self.move_wait += 0.06;
        }
        if self.move_wait < 0.0 {
            self.move_wait = 0.0;
        }
        self.rock_wait -= dt;
        if self.rock_wait <= 0.0 {
            // Rocks are 2 wide, so their left column is 0 .. width-2; they start just above the top.
            let x = self.rng.below(self.width - 1) as i64;
            self.rocks.push((x, -2.0));
            // More rocks the wider the field and the faster the game
            // On the two-panel width (18) a rock comes every 7/speed seconds, i.e. one per 7 rows of
            // fall, so the gaps stay the same height as it speeds up. A narrower field gets fewer rocks
            // (scaled by the square root of the width ratio) so it stays playable.
            self.rock_wait += 7.0 / self.speed() * (18.0 / self.width as f64).sqrt();
        }
        self.carrot_wait -= dt;
        if self.carrot_wait <= 0.0 {
            let x = self.rng.below(self.width) as i64;
            self.carrots.push((x, -1.0));
            // A carrot every 2 to 3 seconds.
            self.carrot_wait += 2.0 + self.rng.random();
        }
        let fall = self.speed() * dt;
        for t in self.rocks.iter_mut().chain(self.carrots.iter_mut()) {
            t.1 += fall;
        }
        // Collisions use whole rows (the fractional row is truncated). A rock is hit if any of its
        // four cells overlaps the rabbit; a carrot (drawn with a dim leaf above it) is caught if its
        // cell or the one below is on the rabbit.
        let rabbit = self.rabbit_cells();
        for &(rx, ry) in &self.rocks {
            let y = ry as i64;
            if [(0, 0), (1, 0), (0, 1), (1, 1)].iter().any(|(dx, dy)| rabbit.contains(&(rx + dx, y + dy))) {
                self.over = true;
            }
        }
        let before = self.carrots.len();
        self.carrots.retain(|&(cx, cy)| {
            let y = cy as i64;
            !(rabbit.contains(&(cx, y)) || rabbit.contains(&(cx, y + 1)))
        });
        self.score += (before - self.carrots.len()) as u32;
        // Forget what has fallen off the bottom.
        self.rocks.retain(|r| r.1 < HEIGHT as f64);
        self.carrots.retain(|c| c.1 < HEIGHT as f64);
    }

    fn draw(&self) -> Pic {
        let mut g = Pic::new(self.width);
        // Marks at the edges that scroll by, to show the speed
        // Every 6th row of the outer columns is dimly lit and the marks move down with the fall speed.
        let offset = (self.clock * self.speed()) as i64;
        for y in 0..HEIGHT {
            if (y as i64 - offset).rem_euclid(6) == 0 {
                g.set(0, y, 0.12);
                g.set(self.width - 1, y, 0.12);
            }
        }
        for &(x, ry) in &self.rocks {
            let y = ry as i64;
            for dx in 0..2 {
                // Rocks are 2 x 2 squares at a middle brightness, so they look different from carrots.
                for dy in 0..2 {
                    g.put(x + dx, y + dy, 0.55);
                }
            }
        }
        for &(x, cy) in &self.carrots {
            let y = cy as i64;
            g.put(x, y, if (self.clock * 6.0) % 2.0 < 1.0 { 1.0 } else { 0.7 });
            // The carrot blinks three times a second; the dim LED above it is its leaf.
            g.put(x, y - 1, 0.3);
        }
        for (x, y) in self.rabbit_cells() {
            g.put(x, y, 1.0);
        }
        g
    }

    fn score(&self) -> u32 {
        self.score
    }

    fn over(&self) -> bool {
        self.over
    }
}

// The session around a game

/// Seconds of the 3-2-1 count-in before a game starts (each number shows for 0.5 s).
const COUNT_IN: f64 = 1.5;
/// Seconds the last picture blinks at game over before the score screen.
const END_FLASH: f64 = 1.2;

/// One game being played: a 3-2-1 count-in, pause, and the score screen when it's over.
pub struct Session {
    pub game: Box<dyn Game>,
    /// Turns the game's picture into what the panels show (`span::Span::to_panels`)
    to_panels: Box<dyn Fn(&Pic) -> Pic + Send>,
    /// How wide the picture on the panels is (9 per panel)
    pub width: usize,
    /// The best score before this game (raised at game over if beaten).
    pub best: u32,
    /// Seconds of count-in left; while above 0 keys are ignored and the number shows.
    count_in: f64,
    /// Paused: the game stands still and a dimmed picture with a pause sign shows.
    pub paused: bool,
    /// Seconds since the game ended (0 while it runs).
    ended: f64,
    /// This game beat the old best (the best score blinks on the score screen). Saving it is the
    /// caller's job (`save_high_score`).
    pub new_best: bool,
}

impl Session {
    /// Wraps a new game; `best` is the stored high score for its kind.
    pub fn new(game: Box<dyn Game>, best: u32, to_panels: Box<dyn Fn(&Pic) -> Pic + Send>, width: usize) -> Session {
        Session { game, to_panels, width, best, count_in: COUNT_IN, paused: false, ended: 0.0, new_best: false }
    }

    /// The game is over and the score screen is showing.
    pub fn finished(&self) -> bool {
        self.game.over() && self.ended >= END_FLASH
    }

    /// Passes a key to the game, except during the count-in, while paused, or after the end.
    pub fn press(&mut self, key: Key) {
        if self.game.over() || self.count_in > 0.0 || self.paused {
            return;
        }
        self.game.press(key);
    }

    /// Pauses or resumes. A finished game can't be paused (so the score screen still shows).
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused && !self.game.over();
    }

    /// Moves time on by `dt` seconds: the count-in, then the game, then the end timer. Paused: nothing.
    pub fn update(&mut self, dt: f64, held: Held) {
        if self.paused {
            return;
        }
        if self.count_in > 0.0 {
            self.count_in -= dt;
            return;
        }
        if self.game.over() {
            // Only on the first frame after the end, so the best is compared with the old one once.
            if self.ended == 0.0 && self.game.score() > self.best {
                self.best = self.game.score();
                self.new_best = true;
            }
            self.ended += dt;
            return;
        }
        self.game.update(dt, held);
    }

    /// How many panels the picture covers (at least 1).
    fn panels(&self) -> usize {
        (self.width / PANEL_WIDTH).max(1)
    }

    /// The picture to show, brightness 0.0 - 1.0.
    pub fn frame(&self) -> Pic {
        if self.count_in > 0.0 {
            let mut g = Pic::new(self.width);
            // 1.5 s split into thirds: "3", then "2", then "1", centred on every panel (column 3, row 14).
            let number = ((self.count_in / COUNT_IN * 3.0).ceil() as u32).to_string();
            for p in 0..self.panels() {
                draw_digits(&mut g, &number, p * PANEL_WIDTH + 3, 14, 1.0, false);
            }
            return g;
        }
        // The game draws one wide picture; `to_panels` rearranges it for the chosen layout (for example
        // mirrored or the keyboard gap skipped) before it is shown.
        let g = (self.to_panels)(&self.game.draw());
        if self.paused {
            let mut g = g.scaled(0.25);
            for p in 0..self.panels() {
                let x = p * PANEL_WIDTH;
                for y in 13..21 {
                    // A pause sign: two 2-wide bars, rows 13 to 20, on each panel.
                    for dx in [2, 3, 5, 6] {
                        g.set(x + dx, y, 1.0);
                    }
                }
            }
            return g;
        }
        if self.game.over() {
            if self.ended < END_FLASH {
                // The last picture blinks
                // Blinks three times a second: full, then dimmed to 20%.
                return if (self.ended * 6.0) as i64 % 2 == 0 { g } else { g.scaled(0.2) };
            }
            return self.score_screen();
        }
        g
    }

    /// The score on the left panel (stacked digits); the best score on the right if there is one.
    fn score_screen(&self) -> Pic {
        let mut g = Pic::new(self.width);
        let score = self.game.score().min(99999).to_string();
        // Digits stacked down the panel (6 rows each), centred vertically, starting at column 3.
        draw_digits(&mut g, &score, 3, HEIGHT.saturating_sub(score.len() * 6) / 2, 1.0, true);
        if self.width >= 2 * PANEL_WIDTH {
            let best = self.best.min(99999).to_string();
            // A new best blinks (1.5 times a second); an old best is shown steadily, both at 60% brightness.
            let level = if !self.new_best || (self.ended * 3.0) as i64 % 2 == 0 { 1.0 } else { 0.3 };
            draw_digits(&mut g, &best, PANEL_WIDTH + 3, HEIGHT.saturating_sub(best.len() * 6) / 2, level * 0.6, true);
            // A crown over the best score
            // A five-LED band on row 1 with three points on row 0 above it, like a little crown.
            for x in PANEL_WIDTH + 2..PANEL_WIDTH + 7 {
                g.set(x, 1, 0.6 * level);
            }
            for x in [PANEL_WIDTH + 2, PANEL_WIDTH + 4, PANEL_WIDTH + 6] {
                g.set(x, 0, 0.6 * level);
            }
        }
        g
    }
}

/// The best score for each game, from settings.json ("high_scores").
///
/// Read loosely, because the settings file may have been written by the older Python app or by hand:
/// a whole number, a decimal (cut down) or a number in text all count; negatives become 0 and anything
/// else is skipped.
pub fn high_scores(settings: &Map<String, Value>) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    if let Some(Value::Object(scores)) = settings.get("high_scores") {
        for (k, v) in scores {
            let n = v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).or_else(|| v.as_str()?.trim().parse().ok());
            if let Some(n) = n {
                out.insert(k.clone(), n.max(0) as u32);
            }
        }
    }
    out
}

/// Keeps score as the best for this game if it beats the old one. Returns true if it did.
///
/// Loads settings.json fresh (so other settings changed meanwhile aren't overwritten), sets
/// "high_scores".key and saves the whole file. Returns false if the score didn't beat the old one, or
/// if saving failed (the error itself is dropped; the game simply doesn't announce a saved record).
pub fn save_high_score(key: &str, score: u32) -> bool {
    let mut settings = crate::settings::load();
    let scores = high_scores(&settings);
    if score <= scores.get(key).copied().unwrap_or(0) {
        return false;
    }
    // Keep any other games' scores; a "high_scores" value that isn't an object is replaced.
    let mut map = match settings.remove("high_scores") {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    map.insert(key.into(), Value::from(score));
    settings.insert("high_scores".into(), Value::Object(map));
    crate::settings::save(&settings).is_ok()
}

/// Tests that play each game with fixed seeds and check the rules above.
#[cfg(test)]
mod tests {
    use super::*;

    /// Plays `game` for up to `seconds` at 30 frames a second, or until it ends.
    fn run(game: &mut dyn Game, seconds: f64, held: Held) {
        let mut t = 0.0;
        while t < seconds && !game.over() {
            game.update(1.0 / 30.0, held);
            t += 1.0 / 30.0;
        }
    }

    /// With no food and no turns the snake runs straight up into the top edge and the game ends there.
    #[test]
    fn snake_hits_the_top() {
        let mut s = Snake::new(18, Rng::new(1));
        s.food = None;
        run(&mut s, 10.0, Held::default());
        assert!(s.over());
        assert_eq!(s.body[0].1, 0);
    }

    /// Pressing Down (straight back) is ignored, and moving onto the dot scores and grows the snake.
    #[test]
    fn snake_grows_on_food_and_ignores_reverse() {
        let mut s = Snake::new(18, Rng::new(2));
        s.food = Some((9, 19));
        s.press(Key::Down);
        s.update(0.17, Held::default());
        assert_eq!(s.score(), 1);
        assert_eq!(s.body.len(), 4);
        assert_eq!(s.body[0], (9, 19));
    }

    /// Dropping an I piece into a 4-cell gap completes the bottom row: 1 point, and after the blink
    /// the row is gone.
    #[test]
    fn ledris_clears_a_line() {
        let mut l = Ledris::new(18, Rng::new(3));
        for x in 0..PANEL_WIDTH {
            l.board[HEIGHT - 1][x] = x >= 4;
        }
        l.cells = vec![(0, 0), (1, 0), (2, 0), (3, 0)];
        l.x = 0;
        l.press(Key::Action);
        assert_eq!(l.score(), 1);
        run(&mut l, 0.5, Held::default());
        assert!(l.board[HEIGHT - 1].iter().all(|c| !c));
    }

    /// Hard-dropping pieces over and over fills the board and ends the game; drawing then still works.
    #[test]
    fn ledris_tops_out_eventually() {
        let mut l = Ledris::new(18, Rng::new(4));
        for _ in 0..200 {
            l.press(Key::Action);
        }
        assert!(l.over());
        let g = l.draw();
        assert_eq!(g.w, 18);
    }

    /// With the paddle always away from the ball, all three balls are lost and the game ends (after
    /// hitting at least one brick on the way).
    #[test]
    fn breakout_loses_three_balls_without_a_paddle() {
        let mut b = Breakout::new(18, Rng::new(5));
        b.press(Key::Action);
        assert!(!b.stuck);
        // Paddle in the far corner and the ball heading elsewhere
        for _ in 0..2000 {
            b.paddle = if b.bx < 9.0 { 15.0 } else { 0.0 };
            b.update(1.0 / 30.0, Held::default());
            if b.over() {
                break;
            }
        }
        assert!(b.over());
        assert!(b.score() > 0);
    }

    /// Standing still, a rock hits the rabbit within two minutes; the rabbit is drawn at full brightness.
    #[test]
    fn rabbit_run_scores_or_ends() {
        let mut r = RabbitRun::new(18, Rng::new(6));
        run(&mut r, 120.0, Held::default());
        assert!(r.over());
        let g = r.draw();
        assert!(g.v.contains(&1.0));
    }

    /// The count-in shows a number, pausing shows the pause sign, and a game left alone ends and shows
    /// the score screen without claiming a new best (5 is the old best here).
    #[test]
    fn session_counts_in_pauses_and_shows_score() {
        let kind = Kind::Snake;
        let mut s = Session::new(kind.make(18, Rng::new(7)), 5, Box::new(|p: &Pic| p.clone()), 18);
        assert!(s.frame().v.contains(&1.0));
        s.update(2.0, Held::default());
        s.set_paused(true);
        let paused = s.frame();
        assert_eq!(paused.get(2, 15), 1.0);
        s.set_paused(false);
        for _ in 0..400 {
            s.update(0.05, Held::default());
        }
        assert!(s.finished());
        assert!(!s.new_best);
        assert_eq!(s.frame().w, 18);
    }

    /// Numbers and numbers in text are read; null is skipped.
    #[test]
    fn high_scores_read_loosely() {
        let m: Map<String, Value> =
            serde_json::from_str(r#"{"high_scores": {"snake": 4, "ledris": "7", "x": null}}"#).unwrap();
        let h = high_scores(&m);
        assert_eq!(h.get("snake"), Some(&4));
        assert_eq!(h.get("ledris"), Some(&7));
        assert_eq!(h.len(), 2);
    }

    /// Every game copes with one, two and a wide row of panels (9, 18, 48 columns) while keys are used.
    #[test]
    fn every_game_draws_at_every_width() {
        for kind in KINDS {
            for w in [9, 18, 48] {
                let mut g = kind.make(w, Rng::new(8));
                for k in [Key::Left, Key::Up, Key::Action, Key::Right] {
                    g.press(k);
                    g.update(0.3, Held { left: true, right: false, down: true });
                }
                assert_eq!(g.draw().w, w);
            }
        }
    }
}
