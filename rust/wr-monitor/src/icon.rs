//! The one-colour tray icon: White Rabbit's rabbit, drawn to whole pixels at the tray's size (no smoothing,
//! like Windows' own tray icons). White on a dark taskbar, black on a light one, and faded while stopped.
//!
//! `ui.rs` calls `tray_icon_rgba` on the tray thread whenever the icon must change (start, pause/resume,
//! taskbar theme or screen scaling change) and turns the pixels into a Windows icon. The rabbit comes in
//! three hand-drawn sizes (16, 12 and 10 pixels across); the biggest one that fits a whole number of
//! times is scaled up by repeating each pixel, then centred.

/// The 16 x 16 rabbit, used at 16-19 px and at 48 px and up. `#` is a lit pixel, `.` is see-through.
const R16: [&str; 16] = [
    "....##....##....",
    "...####..####...",
    "...####..####...",
    "...####..####...",
    "...####..####...",
    "...####..####...",
    "....###..###....",
    "....########....",
    "..############..",
    ".##############.",
    ".###.######.###.",
    ".###.######.###.",
    ".##############.",
    ".######..######.",
    "..############..",
    "....########....",
];

/// The 12 x 12 rabbit, used at 32-47 px (drawn 2 or 3 times bigger).
const R12: [&str; 12] = [
    "..##....##..",
    ".####..####.",
    ".####..####.",
    ".####..####.",
    "..##....##..",
    "..########..",
    ".##########.",
    ".##.####.##.",
    "############",
    "#####..#####",
    ".##########.",
    "...######...",
];

/// The 10 x 10 rabbit, used at 20-31 px (drawn 2 or 3 times bigger), where 16 would only fit once
/// with a wide empty border.
const R10: [&str; 10] = [
    ".##....##.",
    ".##....##.",
    ".##....##.",
    ".##....##.",
    "..######..",
    ".########.",
    ".#.####.#.",
    "##########",
    "####..####",
    ".########.",
];

/// Every pixel's opacity while the animation is stopped.
/// 0 is fully see-through and 255 fully solid, so 110 is a little under half.
const OFF_ALPHA: u8 = 110;

/// The rabbit for this icon size and how many screen pixels each of its pixels takes: the biggest
/// rabbit that fits in whole pixels.
fn bitmap(size: usize) -> (&'static [&'static str], usize) {
    // Integer division: the cell size is rounded down so the rabbit never spills over the edge.
    let fits = |rows: &'static [&'static str]| (rows, size / rows.len());
    match size {
        0..=19 => fits(&R16),
        20..=31 => fits(&R10),
        32..=47 => fits(&R12),
        _ => fits(&R16),
    }
}

/// RGBA pixels, `size` x `size`, top row first. Sizes under 16 are drawn at 16.
///
/// `lit` is false while the monitor is paused (the rabbit is drawn faded); `light_taskbar` picks black
/// ink instead of white. Returns the side actually drawn (at least 16) and 4 bytes per pixel
/// (red, green, blue, alpha). Only the alpha changes from pixel to pixel; the colour is the same everywhere.
pub fn tray_icon_rgba(size: usize, lit: bool, light_taskbar: bool) -> (usize, Vec<u8>) {
    let ink: u8 = if light_taskbar { 0 } else { 255 };
    let side = size.max(16);
    let (rows, cell) = bitmap(side);
    let cell = cell.max(1);
    // The empty margin left over after scaling, split evenly so the rabbit is centred (both axes,
    // since the icon is square).
    let off = (side - rows.len() * cell) / 2;
    let on = if lit { 255 } else { OFF_ALPHA };
    // Draw into an alpha-only mask first, one byte per pixel, then expand to RGBA at the end.
    let mut alpha = vec![0u8; side * side];
    for (y, row) in rows.iter().enumerate() {
        for (x, c) in row.bytes().enumerate() {
            if c != b'#' {
                continue;
            }
            for dy in 0..cell {
                for dx in 0..cell {
                    // Row-major index: (row * width + column). Each rabbit pixel becomes a
                    // `cell` x `cell` block.
                    alpha[(off + y * cell + dy) * side + off + x * cell + dx] = on;
                }
            }
        }
    }
    let rgba = alpha.iter().flat_map(|&a| [ink, ink, ink, a]).collect();
    (side, rgba)
}

/// Tests that the tray icon is drawn at every size the tray may ask for.
#[cfg(test)]
mod tests {
    use super::*;

    /// Turns the drawn icon back into text rows (`#` solid, `+` faded, `.` clear), to compare with
    /// the bitmaps.
    fn picture(size: usize, lit: bool) -> Vec<String> {
        let (side, rgba) = tray_icon_rgba(size, lit, false);
        (0..side)
            .map(|y| {
                (0..side)
                    .map(|x| match rgba[(y * side + x) * 4 + 3] {
                        255 => '#',
                        OFF_ALPHA => '+',
                        _ => '.',
                    })
                    .collect()
            })
            .collect()
    }

    /// At 16 px the icon is exactly the 16 x 16 bitmap, pixel for pixel.
    #[test]
    fn sixteen_pixel_icon_is_the_rabbit() {
        assert_eq!(picture(16, true), R16.iter().map(|r| r.to_string()).collect::<Vec<_>>());
    }

    /// At every common tray size the picture fills the icon and the space above and below differs by
    /// at most one pixel.
    #[test]
    fn every_size_fits_and_is_centred() {
        for size in [16, 20, 24, 28, 32, 40, 48, 64] {
            let rows = picture(size, true);
            assert_eq!(rows.len(), size);
            let lit: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| r.contains('#')).map(|(y, _)| y).collect();
            let (top, bottom) = (lit[0], size - 1 - lit[lit.len() - 1]);
            assert!(top.abs_diff(bottom) <= 1, "size {size}: {top} above, {bottom} below");
        }
    }

    /// A paused icon on a light taskbar uses black ink and only the faded opacity.
    #[test]
    fn stopped_icon_is_faded() {
        let (_, rgba) = tray_icon_rgba(16, false, true);
        assert!(rgba.chunks(4).all(|p| p[0] == 0 && (p[3] == 0 || p[3] == OFF_ALPHA)));
        assert!(rgba.chunks(4).any(|p| p[3] == OFF_ALPHA));
    }
}
