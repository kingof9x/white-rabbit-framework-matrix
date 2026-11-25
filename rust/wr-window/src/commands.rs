//! The LED Matrix firmware's commands, as bytes: a port of the parts of Framework's `inputmodule` Python
//! package the window uses (`inputmodule/inputmodule/__init__.py` and `ledmatrix.py`).
//!
//! Every command the firmware understands has the same shape: the two magic bytes 0x32 0xAC, one
//! command id byte, then that command's parameters. Commands that answer (version, brightness, startup
//! setting) answer with exactly 32 bytes. This file only builds and reads those byte strings; it does
//! no I/O. The bytes are sent by `panels::Sender` (straight to the port, or hex-encoded through the
//! monitor), from worker threads.
//!
//! The panel is 9 LEDs wide and 34 tall. Two ways to draw: `draw_bits` sends one on/off bit per LED in
//! a single 42-byte command; `greyscale` sends a brightness byte (0..255) per LED, one "stage column"
//! command per column and then a "flush" that shows them all at once, so no half-drawn frame is seen.

use crate::font::FONT;

/// The two bytes every command starts with, so the firmware can tell a command from stray data.
pub const MAGIC: [u8; 2] = [0x32, 0xAC];
/// Length in bytes of every answer the firmware sends.
pub const RESPONSE_SIZE: usize = 32;
/// LED columns on one panel.
pub const WIDTH: usize = 9;
/// LED rows on one panel.
pub const HEIGHT: usize = 34;

/// Command ids (the third byte). Brightness: with a level, sets it (0..255); with none, asks for it.
pub const BRIGHTNESS: u8 = 0x00;
/// Shows one of the firmware's built-in patterns (first parameter picks which).
pub const PATTERN: u8 = 0x01;
/// Restarts the panel into its bootloader, for flashing new firmware.
pub const BOOTLOADER: u8 = 0x02;
/// Puts the panel to sleep (1) or wakes it (0).
pub const SLEEP: u8 = 0x03;
/// Turns the firmware's own scrolling animation on (1) or off (0).
pub const ANIMATE: u8 = 0x04;
/// A whole black and white frame: 39 bytes, one bit per LED.
pub const DRAW: u8 = 0x06;
/// Stores one column of greyscale values (column number, then 34 bytes) without showing it yet.
pub const STAGE_COL: u8 = 0x07;
/// Shows all the staged columns at once.
pub const FLUSH_COLS: u8 = 0x08;
/// Starts a game that the firmware plays by itself (first parameter: which game).
pub const START_GAME: u8 = 0x10;
/// Sends a key to the firmware's running game (or quits it with `GAME_QUIT`).
pub const GAME_CONTROL: u8 = 0x11;
/// Asks for the firmware version (answer read by `parse_version` and `parse_wr_build`).
pub const VERSION: u8 = 0x20;
/// Asks for (no parameters) or sets the startup animation; White Rabbit firmware only.
pub const STARTUP_ANIMATION: u8 = 0x21;

/// `START_GAME` id of the firmware's Game of Life.
pub const GAME_OF_LIFE: u8 = 0x03;
/// `GAME_CONTROL` value that ends the firmware's game, so it stops drawing over everything else.
pub const GAME_QUIT: u8 = 4;

/// Builds one command: magic bytes, id, parameters.
pub fn command(id: u8, params: &[u8]) -> Vec<u8> {
    let mut c = MAGIC.to_vec();
    c.push(id);
    c.extend_from_slice(params);
    c
}

/// Sets the panel's overall brightness (0 = off, 255 = brightest).
pub fn brightness(level: u8) -> Vec<u8> {
    command(BRIGHTNESS, &[level])
}

/// Turns the firmware's own animation on or off.
pub fn animate(on: bool) -> Vec<u8> {
    command(ANIMATE, &[on as u8])
}

/// Puts the panel to sleep (`true`) or wakes it.
pub fn sleep(on: bool) -> Vec<u8> {
    command(SLEEP, &[on as u8])
}

/// Reboots the panel into its bootloader (it then appears as a USB drive for flashing, not a port).
pub fn bootloader() -> Vec<u8> {
    command(BOOTLOADER, &[0])
}

/// One key press (or `GAME_QUIT`) for the firmware's game.
pub fn game_control(value: u8) -> Vec<u8> {
    command(GAME_CONTROL, &[value])
}

/// Starts the firmware's Game of Life with starting pattern number `start`.
pub fn game_of_life(start: u8) -> Vec<u8> {
    command(START_GAME, &[GAME_OF_LIFE, start])
}

/// Fill a percentage of the panel, bottom to top.
///
/// `p` is 0..100. Pattern 0x00 is the firmware's percentage bar.
pub fn percentage(p: u8) -> Vec<u8> {
    command(PATTERN, &[0x00, p])
}

/// The panel's 34 rows of 9 greyscale LEDs, as the commands that draw it: one per column, then show.
/// `grid[x][y]`.
pub fn greyscale(grid: &[[u8; HEIGHT]; WIDTH]) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(WIDTH + 1);
    for (x, col) in grid.iter().enumerate() {
        // Stage column x: its number, then its 34 brightness bytes, top to bottom.
        let mut params = vec![x as u8];
        params.extend_from_slice(col);
        out.push(command(STAGE_COL, &params));
    }
    out.push(command(FLUSH_COLS, &[0]));
    out
}

/// A black and white picture in one command (39 bytes, one bit per LED, row by row).
///
/// `lit(x, y)` says whether the LED in column x (0..9), row y (0..34, top first) is on. LED number
/// i = x + 9*y goes in bit i%8 of byte i/8, lowest bit first: 306 LEDs fit in 39 bytes (312 bits).
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
///
/// Counted in the same order as `draw_bits` (row by row from the top-left). Whole bytes are filled
/// with 0xFF, then the low bits of the next byte; any count past 312 is capped by the 39-byte frame.
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
///
/// One value per column (extra values ignored), in LEDs. Each bar grows from the middle: half the
/// value (rounded down) from row 17 downwards, the rest from row 16 upwards, clipped at the edges.
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

/// The 5x6 bitmap of a character from `font.rs` (30 bytes, row by row, non-zero = lit). Characters
/// not in the font show as '?', or as blank if even '?' is missing.
pub fn glyph(c: char) -> [u8; 30] {
    FONT.iter()
        .find(|(k, _)| *k == c)
        .or_else(|| FONT.iter().find(|(k, _)| *k == '?'))
        .map(|(_, g)| *g)
        .unwrap_or([0; 30])
}

/// Up to five letters, one under the other (`show_string`).
///
/// Each letter takes 7 rows (6 of glyph plus a gap) and columns 2..7, centred on the 9-wide panel;
/// letters past the fifth are cut off because 5 x 7 = 35 rows already fills the 34-row panel.
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

/// The pattern names offered in the window, in the order shown. Some are the firmware's own patterns,
/// the rest are drawn here in greyscale (see `pattern`).
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

/// A full panel grid (`grid[x][y]`) with each LED's brightness from `f(x, y)`.
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
///
/// An unknown name gives no commands (nothing is sent).
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
        // LED number x + 9*y gets brightness equal to its number (0..255); LEDs past 255 stay off.
        "All brightness levels (1 LED each)" => greyscale(&grid_from(&|x, y| u8::try_from(x + WIDTH * y).unwrap_or(0))),
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

/// The names of the startup animations White Rabbit firmware knows, by number (the `animation` byte of
/// `set_startup_animation`, and bit numbers in its `set`).
pub const STARTUP_ANIMATIONS: [&str; 34] = [
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
    "Pixel rain",
    "Pixel drop",
    "Pixel swirl",
    "Code wall",
    "Warp rings",
    "Rabbit hole rain",
];

/// Startup modes: 0 random from all, 1 always one, 2 random from a set, 3 none.
/// Older firmware only understands 3 bytes of the set, so the 4th and 5th are only sent when they
/// matter (and then the firmware must be new anyway).
///
/// `set` is a bit mask: bit n set = animation n may be picked (only used in mode 2). Sent as 3, 4 or 5
/// little-endian bytes depending on the highest animation in it.
pub fn set_startup_animation(mode: u8, animation: u8, set: u64) -> Vec<u8> {
    let mut params = vec![mode, animation];
    if mode == 2 && set >> 32 != 0 {
        params.extend_from_slice(&set.to_le_bytes()[..5]);
    } else if mode == 2 && set >> 24 != 0 {
        params.extend_from_slice(&set.to_le_bytes()[..4]);
    } else {
        params.extend_from_slice(&(set & 0xFF_FFFF).to_le_bytes()[..3]);
    }
    command(STARTUP_ANIMATION, &params)
}

/// The answer to the startup animation question: (mode, animation, set), None for old firmware.
///
/// The first byte is the mode; a value above 3 (or a short answer) means the firmware doesn't know
/// this command. Bytes 2..7 are the set, little endian.
pub fn parse_startup(answer: &[u8]) -> Option<(u8, u8, u64)> {
    if answer.len() < 5 || answer[0] > 3 {
        return None;
    }
    // Older firmware answers with 3 or 4 bytes of the set and 0s after them
    let mut set = [0u8; 8];
    for (i, b) in answer[2..answer.len().min(7)].iter().enumerate() {
        set[i] = *b;
    }
    Some((answer[0], answer[1], u64::from_le_bytes(set)))
}

/// "0.2.0" from the version answer.
///
/// Byte 0 is the major version, byte 1 holds minor (high 4 bits) and patch (low 4 bits), byte 2 is
/// non-zero for a pre-release. A short answer gives "Unknown".
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

/// White Rabbit firmware's build number from the version answer: bytes 3-4 are "WR", 5-6 the
/// number (little endian). None for Framework's firmware and for White Rabbit builds older than this.
pub fn parse_wr_build(answer: &[u8]) -> Option<u16> {
    (answer.len() >= 7 && &answer[3..5] == b"WR").then(|| u16::from_le_bytes([answer[5], answer[6]]))
}

/// Tests for building and reading firmware commands.
#[cfg(test)]
mod tests {
    use super::*;

    /// The build number is read only when bytes 3-4 say "WR", and not from a short answer.
    #[test]
    fn wr_builds() {
        assert_eq!(parse_wr_build(&[0, 0x20, 0, b'W', b'R', 15, 0, 0]), Some(15));
        assert_eq!(parse_wr_build(&[0, 0x20, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(parse_wr_build(&[0, 0x20, 0]), None);
    }

    /// Version bytes become "major.minor.patch", with the pre-release note, or "Unknown".
    #[test]
    fn versions() {
        assert_eq!(parse_version(&[0, 0x20, 0]), "0.2.0");
        assert_eq!(parse_version(&[1, 0x13, 1]), "1.1.3 (Pre-release)");
        assert_eq!(parse_version(&[]), "Unknown");
    }

    /// The startup command sends 3, 4 or 5 set bytes as needed, and answers of every length read back.
    #[test]
    fn startup_bytes() {
        assert_eq!(set_startup_animation(1, 5, 0x0FFF_FFFF), vec![0x32, 0xAC, 0x21, 1, 5, 0xFF, 0xFF, 0xFF]);
        assert_eq!(set_startup_animation(2, 0, 0x0100_0001), vec![0x32, 0xAC, 0x21, 2, 0, 1, 0, 0, 1]);
        assert_eq!(set_startup_animation(2, 0, 0x02_0000_0001), vec![0x32, 0xAC, 0x21, 2, 0, 1, 0, 0, 0, 2]);
        assert_eq!(parse_startup(&[2, 3, 1, 0, 0, 1]), Some((2, 3, 0x0100_0001)));
        assert_eq!(parse_startup(&[2, 3, 1, 0, 0, 1, 3, 0]), Some((2, 3, 0x03_0100_0001)));
        assert_eq!(parse_startup(&[9, 0, 0, 0, 0]), None);
    }

    /// The checkerboard patterns give exactly the bytes Framework's Python code sends.
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

    /// One letter is a single 42-byte draw command, and the font's 'A' is where it should be.
    #[test]
    fn letters() {
        let c = show_string("A");
        assert_eq!(c.len(), 3 + 39);
        assert_eq!(glyph('A')[1], 1);
    }
}
