//! The LED Matrix firmware's commands, as bytes: a port of the parts of Framework's `inputmodule` Python
//! package the window uses (`inputmodule/inputmodule/__init__.py` and `ledmatrix.py`).

use crate::font::FONT;

pub const MAGIC: [u8; 2] = [0x32, 0xAC];
pub const RESPONSE_SIZE: usize = 32;
pub const WIDTH: usize = 9;
pub const HEIGHT: usize = 34;

pub const BRIGHTNESS: u8 = 0x00;
pub const PATTERN: u8 = 0x01;
pub const BOOTLOADER: u8 = 0x02;
pub const SLEEP: u8 = 0x03;
pub const ANIMATE: u8 = 0x04;
pub const DRAW: u8 = 0x06;
pub const STAGE_COL: u8 = 0x07;
pub const FLUSH_COLS: u8 = 0x08;
pub const START_GAME: u8 = 0x10;
pub const GAME_CONTROL: u8 = 0x11;
pub const VERSION: u8 = 0x20;
pub const STARTUP_ANIMATION: u8 = 0x21;

pub const GAME_OF_LIFE: u8 = 0x03;
pub const GAME_QUIT: u8 = 4;

pub fn command(id: u8, params: &[u8]) -> Vec<u8> {
    let mut c = MAGIC.to_vec();
    c.push(id);
    c.extend_from_slice(params);
    c
}

pub fn brightness(level: u8) -> Vec<u8> {
    command(BRIGHTNESS, &[level])
}

pub fn animate(on: bool) -> Vec<u8> {
    command(ANIMATE, &[on as u8])
}

pub fn sleep(on: bool) -> Vec<u8> {
    command(SLEEP, &[on as u8])
}

pub fn bootloader() -> Vec<u8> {
    command(BOOTLOADER, &[0])
}

pub fn game_control(value: u8) -> Vec<u8> {
    command(GAME_CONTROL, &[value])
}

pub fn game_of_life(start: u8) -> Vec<u8> {
    command(START_GAME, &[GAME_OF_LIFE, start])
}

/// Fill a percentage of the panel, bottom to top.
pub fn percentage(p: u8) -> Vec<u8> {
    command(PATTERN, &[0x00, p])
}

/// The panel's 34 rows of 9 greyscale LEDs, as the commands that draw it: one per column, then show.
/// `grid[x][y]`.
pub fn greyscale(grid: &[[u8; HEIGHT]; WIDTH]) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(WIDTH + 1);
    for (x, col) in grid.iter().enumerate() {
        let mut params = vec![x as u8];
        params.extend_from_slice(col);
        out.push(command(STAGE_COL, &params));
    }
    out.push(command(FLUSH_COLS, &[0]));
    out
}

/// A black and white picture in one command (39 bytes, one bit per LED, row by row).
pub fn draw_bits(lit: &dyn Fn(usize, usize) -> bool) -> Vec<u8> {
    let mut vals = [0u8; 39];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if lit(x, y) {
                let i = x + WIDTH * y;
                vals[i / 8] |= 1 << (i % 8);
            }
        }
    }
    command(DRAW, &vals)
}

/// Light the first `leds` LEDs (the countdown).
pub fn light_leds(leds: usize) -> Vec<u8> {
    let mut vals = [0u8; 39];
    for v in vals.iter_mut().take(leds / 8) {
        *v = 0xFF;
    }
    for i in 0..leds % 8 {
        if leds / 8 < 39 {
            vals[leds / 8] += 1 << i;
        }
    }
    command(DRAW, &vals)
}

/// Nine values as an equalizer from the middle, up and down.
pub fn eq(vals: &[usize]) -> Vec<u8> {
    let mut m = [[false; HEIGHT]; WIDTH];
    for (col, &val) in vals.iter().take(9).enumerate() {
        let row = HEIGHT / 2;
        let above = val / 2;
        let below = val - above;
        for i in 0..above {
            if row + i < HEIGHT {
                m[col][row + i] = true;
            }
        }
        for i in 0..below {
            if row > i {
                m[col][row - 1 - i] = true;
            }
        }
    }
    draw_bits(&|x, y| m[x][y])
}

pub fn glyph(c: char) -> [u8; 30] {
    FONT.iter().find(|(k, _)| *k == c).or_else(|| FONT.iter().find(|(k, _)| *k == '?')).map(|(_, g)| *g).unwrap_or([0; 30])
}

/// Up to five letters, one under the other (`show_string`).
pub fn show_string(text: &str) -> Vec<u8> {
    let glyphs: Vec<[u8; 30]> = text.chars().take(5).map(glyph).collect();
    draw_bits(&|x, y| {
        let digit = y / 7;
        let py = y % 7;
        if digit >= glyphs.len() || py >= 6 || !(2..7).contains(&x) {
            return false;
        }
        glyphs[digit][(x - 2) + py * 5] != 0
    })
}

pub const PATTERNS: [&str; 21] = [
    "All LEDs on",
    "\"LOTUS\" sideways",
    "Gradient (0-13% Brightness)",
    "Double Gradient (0-7-0% Brightness)",
    "Zigzag",
    "\"PANIC\"",
    "\"LOTUS\" Top Down",
    "All brightness levels (1 LED each)",
    "Every Second Row",
    "Every Third Row",
    "Every Fourth Row",
    "Every Fifth Row",
    "Every Sixth Row",
    "Every Second Col",
    "Every Third Col",
    "Every Fourth Col",
    "Every Fifth Col",
    "Checkerboard",
    "Double Checkerboard",
    "Triple Checkerboard",
    "Quad Checkerboard",
];

fn grid_from(f: &dyn Fn(usize, usize) -> u8) -> [[u8; HEIGHT]; WIDTH] {
    let mut g = [[0u8; HEIGHT]; WIDTH];
    for (x, col) in g.iter_mut().enumerate() {
        for (y, v) in col.iter_mut().enumerate() {
            *v = f(x, y);
        }
    }
    g
}

/// The commands for one of the patterns above.
pub fn pattern(name: &str) -> Vec<Vec<u8>> {
    let firmware = |id: u8| vec![command(PATTERN, &[id])];
    let nth_row = |n: usize| greyscale(&grid_from(&|_, y| if y % n == 0 { 0xFF } else { 0 }));
    let nth_col = |n: usize| greyscale(&grid_from(&|x, _| if x % n == 0 { 0xFF } else { 0 }));
    let checker = |n: usize| {
        greyscale(&grid_from(&|x, y| {
            // The Python version builds ([0xFF]*n + [0]*n) * 17 and rotates odd column groups by n
            let base = |i: usize| if i % (2 * n) < n { 0xFF } else { 0 };
            if x % (n * 2) < n {
                base(y + n)
            } else {
                base(y)
            }
        }))
    };
    match name {
        "All LEDs on" => firmware(0x05),
        "Gradient (0-13% Brightness)" => firmware(0x01),
        "Double Gradient (0-7-0% Brightness)" => firmware(0x02),
        "\"LOTUS\" sideways" => firmware(0x03),
        "Zigzag" => firmware(0x04),
        "\"PANIC\"" => firmware(0x06),
        "\"LOTUS\" Top Down" => firmware(0x07),
        "All brightness levels (1 LED each)" => {
            greyscale(&grid_from(&|x, y| u8::try_from(x + WIDTH * y).unwrap_or(0)))
        }
        "Every Second Row" => nth_row(2),
        "Every Third Row" => nth_row(3),
        "Every Fourth Row" => nth_row(4),
        "Every Fifth Row" => nth_row(5),
        "Every Sixth Row" => nth_row(6),
        "Every Second Col" => nth_col(2),
        "Every Third Col" => nth_col(3),
        // As in Framework's code, Fifth is the same as Fourth
        "Every Fourth Col" | "Every Fifth Col" => nth_col(4),
        "Checkerboard" => checker(1),
        "Double Checkerboard" => checker(2),
        "Triple Checkerboard" => checker(3),
        "Quad Checkerboard" => checker(4),
        _ => Vec::new(),
    }
}

pub const STARTUP_ANIMATIONS: [&str; 28] = [
    "Percentage",
    "Zigzag",
    "Game of Life: Pattern 1",
    "Game of Life: Beacon, Toad, Blinker",
    "Game of Life: Glider",
    "Breathing",
    "Pong",
    "Snake",
    "Game of Life: R-pentomino",
    "Game of Life: Acorn",
    "Game of Life: Diehard",
    "Game of Life: Lightweight spaceship",
    "Game of Life: Middleweight spaceship",
    "Game of Life: Heavyweight spaceship",
    "Game of Life: Spaceship fleet",
    "Game of Life: Octagon",
    "Game of Life: Tumbler",
    "Game of Life: Caterer",
    "Game of Life: Pattern 2",
    "Game of Life: Pattern 3",
    "Game of Life: Pattern 4",
    "Game of Life: Pattern 5",
    "Rabbit hole",
    "Digital rain",
    "Hourglass",
    "Warp speed",
    "Power on",
    "Campfire",
];

/// Startup modes: 0 random from all, 1 always one, 2 random from a set, 3 none.
pub fn set_startup_animation(mode: u8, animation: u8, set: u32) -> Vec<u8> {
    let mut params = vec![mode, animation];
    if mode == 2 && set >> 24 != 0 {
        params.extend_from_slice(&set.to_le_bytes());
    } else {
        params.extend_from_slice(&(set & 0xFF_FFFF).to_le_bytes()[..3]);
    }
    command(STARTUP_ANIMATION, &params)
}

/// The answer to the startup animation question: (mode, animation, set), None for old firmware.
pub fn parse_startup(answer: &[u8]) -> Option<(u8, u8, u32)> {
    if answer.len() < 5 || answer[0] > 3 {
        return None;
    }
    let mut set = [0u8; 4];
    for (i, b) in answer[2..answer.len().min(6)].iter().enumerate() {
        set[i] = *b;
    }
    Some((answer[0], answer[1], u32::from_le_bytes(set)))
}

/// "0.2.0" from the version answer.
pub fn parse_version(answer: &[u8]) -> String {
    if answer.len() < 3 {
        return "Unknown".into();
    }
    let mut v = format!("{}.{}.{}", answer[0], (answer[1] & 0xF0) >> 4, answer[1] & 0x0F);
    if answer[2] != 0 {
        v.push_str(" (Pre-release)");
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version(&[0, 0x20, 0]), "0.2.0");
        assert_eq!(parse_version(&[1, 0x13, 1]), "1.1.3 (Pre-release)");
        assert_eq!(parse_version(&[]), "Unknown");
    }

    #[test]
    fn startup_bytes() {
        assert_eq!(set_startup_animation(1, 5, 0x0FFF_FFFF), vec![0x32, 0xAC, 0x21, 1, 5, 0xFF, 0xFF, 0xFF]);
        assert_eq!(set_startup_animation(2, 0, 0x0100_0001), vec![0x32, 0xAC, 0x21, 2, 0, 1, 0, 0, 1]);
        assert_eq!(parse_startup(&[2, 3, 1, 0, 0, 1]), Some((2, 3, 0x0100_0001)));
        assert_eq!(parse_startup(&[9, 0, 0, 0, 0]), None);
    }

    #[test]
    fn checkerboard_matches_python() {
        // Python: vals = ([0xFF]*n + [0]*n) * 17; even column groups rotate by n
        for n in 1..=4 {
            let cmds = pattern(match n {
                1 => "Checkerboard",
                2 => "Double Checkerboard",
                3 => "Triple Checkerboard",
                _ => "Quad Checkerboard",
            });
            for x in 0..WIDTH {
                let mut vals: Vec<u8> = ([vec![0xFFu8; n], vec![0u8; n]].concat()).repeat(17);
                if x % (n * 2) < n {
                    vals = [vals[n..].to_vec(), vals[..n].to_vec()].concat();
                }
                assert_eq!(&cmds[x][4..], &vals[..HEIGHT], "n={n} x={x}");
            }
        }
    }

    #[test]
    fn letters() {
        let c = show_string("A");
        assert_eq!(c.len(), 3 + 39);
        assert_eq!(glyph('A')[1], 1);
    }
}
