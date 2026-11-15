//! The games on the Games & patterns page: Snake, Ledris, Breakout and Rabbit run (`white_rabbit/games.py`).
//!
//! Each game is plain logic with no window or panel code, so it can be tested on its own. It draws a
//! 34-row picture as wide as the panels it is played on (9 for one panel, 18 for both) with
//! brightness from 0.0 to 1.0. `Session` adds what every game shares: a count-in, pause, the score
//! screen at the end and high scores.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

pub const HEIGHT: usize = 34;
pub const PANEL_WIDTH: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Action,
}

/// The keys held down.
#[derive(Clone, Copy, Debug, Default)]
pub struct Held {
    pub left: bool,
    pub right: bool,
    pub down: bool,
}

/// A picture, `HEIGHT` rows of `w` columns.
#[derive(Clone, Debug, PartialEq)]
pub struct Pic {
    pub w: usize,
    pub v: Vec<f64>,
}

impl Pic {
    pub fn new(w: usize) -> Pic {
        Pic { w, v: vec![0.0; w * HEIGHT] }
    }

    pub fn get(&self, x: usize, y: usize) -> f64 {
        if x < self.w && y < HEIGHT {
            self.v[y * self.w + x]
        } else {
            0.0
        }
    }

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

    pub fn scaled(&self, f: f64) -> Pic {
        Pic { w: self.w, v: self.v.iter().map(|v| v * f).collect() }
    }

    /// 0-255 for the panels.
    pub fn bytes(&self) -> Vec<u8> {
        self.v.iter().map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8).collect()
    }
}

/// A small random number maker, so the games don't need another crate (and tests can fix the seed).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub fn from_time() -> Rng {
        let mut b = [0u8; 8];
        if getrandom::fill(&mut b).is_err() {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            b = (t.as_nanos() as u64).to_le_bytes();
        }
        Rng::new(u64::from_le_bytes(b))
    }

    fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0.0 <= x < 1.0
    pub fn random(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// 0 <= x < n (n > 0)
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

// 3 x 5 digits for scores
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
const RABBIT: [&str; 6] = ["10010", "10010", "11110", "10101", "11110", "01110"];

/// Writes digits into the picture, 3 x 5 each, side by side or stacked.
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
pub trait Game: Send {
    fn press(&mut self, key: Key);
    fn update(&mut self, dt: f64, held: Held);
    fn draw(&self) -> Pic;
    fn score(&self) -> u32;
    fn over(&self) -> bool;
    fn level(&self) -> Option<u32> {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Snake,
    Ledris,
    Breakout,
    RabbitRun,
}

pub const KINDS: [Kind; 4] = [Kind::Snake, Kind::Ledris, Kind::Breakout, Kind::RabbitRun];

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Snake => "snake",
            Kind::Ledris => "ledris",
            Kind::Breakout => "breakout",
            Kind::RabbitRun => "rabbit-run",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Kind::Snake => "Snake",
            Kind::Ledris => "Ledris",
            Kind::Breakout => "Breakout",
            Kind::RabbitRun => "Rabbit run",
        }
    }

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

pub struct Snake {
    width: usize,
    rng: Rng,
    score: u32,
    over: bool,
    pub body: Vec<(i64, i64)>,
    direction: (i64, i64),
    turns: Vec<(i64, i64)>,
    wait: f64,
    clock: f64,
    pub food: Option<(i64, i64)>,
}

impl Snake {
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

    fn tick_seconds(&self) -> f64 {
        (0.16 - self.score as f64 * 0.004).max(0.06)
    }

    fn step(&mut self) {
        if !self.turns.is_empty() {
            self.direction = self.turns.remove(0);
        }
        let (hx, hy) = self.body[0];
        let head = (hx + self.direction.0, hy + self.direction.1);
        let grows = Some(head) == self.food;
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
            if self.food.is_none() {
                self.over = true;
            }
        }
    }
}

impl Game for Snake {
    fn press(&mut self, key: Key) {
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
            g.put(x, y, if i == 0 { 1.0 } else { (0.8 - i as f64 * 0.015).max(0.35) });
        }
        if let Some((x, y)) = self.food {
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

type Cells = Vec<(i64, i64)>;

const SHAPES: [[(i64, i64); 4]; 7] = [
    [(0, 0), (1, 0), (2, 0), (3, 0)],
    [(0, 0), (1, 0), (0, 1), (1, 1)],
    [(0, 0), (1, 0), (2, 0), (1, 1)],
    [(1, 0), (2, 0), (0, 1), (1, 1)],
    [(0, 0), (1, 0), (1, 1), (2, 1)],
    [(0, 0), (0, 1), (0, 2), (1, 2)],
    [(1, 0), (1, 1), (1, 2), (0, 2)],
];
const POINTS: [u32; 5] = [0, 1, 3, 5, 8];
const FLASH_SECONDS: f64 = 0.3;

fn normalised(cells: &[(i64, i64)]) -> Cells {
    let mx = cells.iter().map(|c| c.0).min().unwrap_or(0);
    let my = cells.iter().map(|c| c.1).min().unwrap_or(0);
    let mut out: Cells = cells.iter().map(|(x, y)| (x - mx, y - my)).collect();
    out.sort();
    out
}

pub struct Ledris {
    width: usize,
    rng: Rng,
    score: u32,
    over: bool,
    pub board: [[bool; PANEL_WIDTH]; HEIGHT],
    lines: u32,
    level: u32,
    next: usize,
    fall: f64,
    flash_rows: Vec<usize>,
    flash: f64,
    pub cells: Cells,
    pub x: i64,
    pub y: i64,
}

impl Ledris {
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

    fn fits(&self, cells: &[(i64, i64)], ox: i64, oy: i64) -> bool {
        cells.iter().all(|(x, y)| {
            let (cx, cy) = (ox + x, oy + y);
            (0..PANEL_WIDTH as i64).contains(&cx)
                && cy < HEIGHT as i64
                && (cy < 0 || !self.board[cy as usize][cx as usize])
        })
    }

    fn fall_seconds(&self) -> f64 {
        (0.55 - (self.level as f64 - 1.0) * 0.05).max(0.07)
    }

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
        if self.over || !self.flash_rows.is_empty() {
            return;
        }
        match key {
            Key::Left | Key::Right => {
                let dx = if key == Key::Left { -1 } else { 1 };
                if self.fits(&self.cells, self.x + dx, self.y) {
                    self.x += dx;
                }
            }
            Key::Up => {
                let turned = normalised(&self.cells.iter().map(|&(x, y)| (-y, x)).collect::<Vec<_>>());
                // Turn, nudging away from a wall if needed
                for dx in [0, -1, 1, -2, 2] {
                    if self.fits(&turned, self.x + dx, self.y) {
                        self.cells = turned;
                        self.x += dx;
                        break;
                    }
                }
            }
            Key::Action => {
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
        let step = if held.down { self.fall_seconds().min(0.05) } else { self.fall_seconds() };
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

const PADDLE: f64 = 3.0;

pub struct Breakout {
    width: usize,
    rng: Rng,
    score: u32,
    over: bool,
    level: u32,
    pub lives: u32,
    pub paddle: f64,
    move_wait: f64,
    pub bricks: Vec<Vec<bool>>,
    pub stuck: bool,
    launch_wait: f64,
    pub vx: f64,
    pub vy: f64,
    pub bx: f64,
    pub by: f64,
}

impl Breakout {
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

    fn new_bricks(&mut self) {
        self.bricks = (0..HEIGHT).map(|r| vec![(2..8).contains(&r); self.width]).collect();
    }

    fn serve(&mut self) {
        self.stuck = true;
        self.launch_wait = 1.5;
        self.vx = 0.0;
        self.vy = 0.0;
        self.follow_paddle();
    }

    fn follow_paddle(&mut self) {
        self.bx = self.paddle + PADDLE / 2.0 - 0.5;
        self.by = HEIGHT as f64 - 2.0;
    }

    fn speed(&self) -> f64 {
        14.0 + 3.0 * (self.level as f64 - 1.0)
    }

    fn launch(&mut self) {
        self.stuck = false;
        let direction = if self.rng.below(2) == 0 { -1.0 } else { 1.0 };
        let speed = self.speed();
        self.vx = direction * speed * 0.45;
        self.vy = -speed * 0.9;
    }

    fn move_paddle(&mut self, dx: f64) {
        self.paddle = (self.paddle + dx).max(0.0).min(self.width as f64 - PADDLE);
        if self.stuck {
            self.follow_paddle();
        }
    }

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
        let (col, row) = (self.bx.round() as usize, self.by.round() as i64);
        if (0..HEIGHT as i64).contains(&row) && self.bricks[row as usize][col] {
            // Bricks are 2 LEDs wide
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
            if self.paddle - 0.5 <= self.bx && self.bx <= self.paddle + PADDLE - 0.5 {
                // Where the ball hits the paddle sets its angle
                let offset = (self.bx - (self.paddle + PADDLE / 2.0 - 0.5)) / (PADDLE / 2.0);
                let speed = self.speed();
                self.vx = (offset * 0.7 + (self.rng.random() - 0.5) * 0.2).clamp(-0.85, 0.85) * speed;
                self.vy = -(speed * speed - self.vx * self.vx).sqrt();
                self.by = HEIGHT as f64 - 2.0;
            } else if self.by > HEIGHT as f64 {
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
            self.move_wait += 0.05;
        }
        if self.move_wait < 0.0 {
            self.move_wait = 0.0;
        }
        if self.stuck {
            self.launch_wait -= dt;
            if self.launch_wait <= 0.0 {
                self.launch();
            }
            return;
        }
        // Small steps so the ball can't jump over a brick
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

pub struct RabbitRun {
    width: usize,
    rng: Rng,
    score: u32,
    over: bool,
    pub x: f64,
    /// (column, row), the row moving down
    pub rocks: Vec<(i64, f64)>,
    pub carrots: Vec<(i64, f64)>,
    clock: f64,
    rock_wait: f64,
    carrot_wait: f64,
    move_wait: f64,
}

impl RabbitRun {
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

    fn step(&mut self, dx: f64) {
        self.x = (self.x + dx).max(0.0).min(self.width as f64 - 5.0);
    }

    /// Rows a second the rocks fall; faster the longer you last.
    fn speed(&self) -> f64 {
        9.0 + (self.clock * 0.25).min(16.0)
    }

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
            let x = self.rng.below(self.width - 1) as i64;
            self.rocks.push((x, -2.0));
            // More rocks the wider the field and the faster the game
            self.rock_wait += 7.0 / self.speed() * (18.0 / self.width as f64).sqrt();
        }
        self.carrot_wait -= dt;
        if self.carrot_wait <= 0.0 {
            let x = self.rng.below(self.width) as i64;
            self.carrots.push((x, -1.0));
            self.carrot_wait += 2.0 + self.rng.random();
        }
        let fall = self.speed() * dt;
        for t in self.rocks.iter_mut().chain(self.carrots.iter_mut()) {
            t.1 += fall;
        }
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
        self.rocks.retain(|r| r.1 < HEIGHT as f64);
        self.carrots.retain(|c| c.1 < HEIGHT as f64);
    }

    fn draw(&self) -> Pic {
        let mut g = Pic::new(self.width);
        // Marks at the edges that scroll by, to show the speed
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
                for dy in 0..2 {
                    g.put(x + dx, y + dy, 0.55);
                }
            }
        }
        for &(x, cy) in &self.carrots {
            let y = cy as i64;
            g.put(x, y, if (self.clock * 6.0) % 2.0 < 1.0 { 1.0 } else { 0.7 });
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

const COUNT_IN: f64 = 1.5;
const END_FLASH: f64 = 1.2;

/// One game being played: a 3-2-1 count-in, pause, and the score screen when it's over.
pub struct Session {
    pub game: Box<dyn Game>,
    /// Turns the game's picture into what the panels show (`span::Span::to_panels`)
    to_panels: Box<dyn Fn(&Pic) -> Pic + Send>,
    /// How wide the picture on the panels is (9 per panel)
    pub width: usize,
    pub best: u32,
    count_in: f64,
    pub paused: bool,
    ended: f64,
    pub new_best: bool,
}

impl Session {
    pub fn new(game: Box<dyn Game>, best: u32, to_panels: Box<dyn Fn(&Pic) -> Pic + Send>, width: usize) -> Session {
        Session { game, to_panels, width, best, count_in: COUNT_IN, paused: false, ended: 0.0, new_best: false }
    }

    /// The game is over and the score screen is showing.
    pub fn finished(&self) -> bool {
        self.game.over() && self.ended >= END_FLASH
    }

    pub fn press(&mut self, key: Key) {
        if self.game.over() || self.count_in > 0.0 || self.paused {
            return;
        }
        self.game.press(key);
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused && !self.game.over();
    }

    pub fn update(&mut self, dt: f64, held: Held) {
        if self.paused {
            return;
        }
        if self.count_in > 0.0 {
            self.count_in -= dt;
            return;
        }
        if self.game.over() {
            if self.ended == 0.0 && self.game.score() > self.best {
                self.best = self.game.score();
                self.new_best = true;
            }
            self.ended += dt;
            return;
        }
        self.game.update(dt, held);
    }

    fn panels(&self) -> usize {
        (self.width / PANEL_WIDTH).max(1)
    }

    /// The picture to show, brightness 0.0 - 1.0.
    pub fn frame(&self) -> Pic {
        if self.count_in > 0.0 {
            let mut g = Pic::new(self.width);
            let number = ((self.count_in / COUNT_IN * 3.0).ceil() as u32).to_string();
            for p in 0..self.panels() {
                draw_digits(&mut g, &number, p * PANEL_WIDTH + 3, 14, 1.0, false);
            }
            return g;
        }
        let g = (self.to_panels)(&self.game.draw());
        if self.paused {
            let mut g = g.scaled(0.25);
            for p in 0..self.panels() {
                let x = p * PANEL_WIDTH;
                for y in 13..21 {
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
        draw_digits(&mut g, &score, 3, HEIGHT.saturating_sub(score.len() * 6) / 2, 1.0, true);
        if self.width >= 2 * PANEL_WIDTH {
            let best = self.best.min(99999).to_string();
            let level = if !self.new_best || (self.ended * 3.0) as i64 % 2 == 0 { 1.0 } else { 0.3 };
            draw_digits(&mut g, &best, PANEL_WIDTH + 3, HEIGHT.saturating_sub(best.len() * 6) / 2, level * 0.6, true);
            // A crown over the best score
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
pub fn save_high_score(key: &str, score: u32) -> bool {
    let mut settings = crate::settings::load();
    let scores = high_scores(&settings);
    if score <= scores.get(key).copied().unwrap_or(0) {
        return false;
    }
    let mut map = match settings.remove("high_scores") {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    map.insert(key.into(), Value::from(score));
    settings.insert("high_scores".into(), Value::Object(map));
    crate::settings::save(&settings).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(game: &mut dyn Game, seconds: f64, held: Held) {
        let mut t = 0.0;
        while t < seconds && !game.over() {
            game.update(1.0 / 30.0, held);
            t += 1.0 / 30.0;
        }
    }

    #[test]
    fn snake_hits_the_top() {
        let mut s = Snake::new(18, Rng::new(1));
        s.food = None;
        run(&mut s, 10.0, Held::default());
        assert!(s.over());
        assert_eq!(s.body[0].1, 0);
    }

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

    #[test]
    fn rabbit_run_scores_or_ends() {
        let mut r = RabbitRun::new(18, Rng::new(6));
        run(&mut r, 120.0, Held::default());
        assert!(r.over());
        let g = r.draw();
        assert!(g.v.contains(&1.0));
    }

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

    #[test]
    fn high_scores_read_loosely() {
        let m: Map<String, Value> = serde_json::from_str(r#"{"high_scores": {"snake": 4, "ledris": "7", "x": null}}"#).unwrap();
        let h = high_scores(&m);
        assert_eq!(h.get("snake"), Some(&4));
        assert_eq!(h.get("ledris"), Some(&7));
        assert_eq!(h.len(), 2);
    }

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
