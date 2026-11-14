//! The bytes sent to an LED Matrix panel (from Framework's inputmodule-rs `commands.md`), as the
//! Python monitor sends them.

use crate::grid::{Grid, HEIGHT, WIDTH};

pub const MAGIC: [u8; 2] = [0x32, 0xAC];
pub const SLEEP: u8 = 0x03;
pub const ANIMATE: u8 = 0x04;
pub const STAGE_COL: u8 = 0x07;
pub const FLUSH_COLS: u8 = 0x08;
pub const VERSION: u8 = 0x20;

/// USB IDs of the Framework LED Matrix Input Module.
pub const LED_MATRIX_VID: u16 = 0x32AC;
pub const LED_MATRIX_PID: u16 = 0x0020;
pub const BAUD_RATE: u32 = 115_200;

pub fn command(id: u8, params: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(3 + params.len());
    m.extend_from_slice(&MAGIC);
    m.push(id);
    m.extend_from_slice(params);
    m
}

/// A greyscale frame: one "stage column" per lit column (the panel starts every frame dark, so dark
/// columns needn't be sent), then "flush columns" to show them all at once.
pub fn frame_commands(grid: &Grid) -> Vec<Vec<u8>> {
    let bytes = grid.to_bytes();
    let mut out = Vec::new();
    for (x, column) in bytes.iter().enumerate().take(WIDTH) {
        if column.iter().all(|&v| v == 0) {
            continue;
        }
        let mut params = Vec::with_capacity(1 + HEIGHT);
        params.push(x as u8);
        params.extend_from_slice(column);
        out.push(command(STAGE_COL, &params));
    }
    out.push(command(FLUSH_COLS, &[]));
    out
}

/// Fades the panel out and turns its LEDs off until the next command.
pub fn sleep() -> Vec<u8> {
    command(SLEEP, &[1])
}

/// The panel's own downward scroll of what it shows, on or off.
pub fn animate(on: bool) -> Vec<u8> {
    command(ANIMATE, &[on as u8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_columns_are_skipped() {
        let mut g = Grid::new();
        g.set(2, 0, 300);
        g.set(2, 33, -5);
        let cmds = frame_commands(&g);
        assert_eq!(cmds.len(), 2);
        assert_eq!(&cmds[0][..4], &[0x32, 0xAC, STAGE_COL, 2]);
        assert_eq!(cmds[0].len(), 4 + HEIGHT);
        assert_eq!(cmds[0][4], 255);
        assert_eq!(cmds[0][4 + 33], 0);
        assert_eq!(cmds[1], vec![0x32, 0xAC, FLUSH_COLS]);
    }
}
