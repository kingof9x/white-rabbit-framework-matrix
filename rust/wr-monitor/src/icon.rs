//! The one-colour tray icon: White Rabbit's rabbit, drawn to whole pixels at the tray's size (no smoothing,
//! like Windows' own tray icons). White on a dark taskbar, black on a light one, and faded while stopped.

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
const OFF_ALPHA: u8 = 110;

/// The rabbit for this icon size and how many screen pixels each of its pixels takes: the biggest
/// rabbit that fits in whole pixels.
fn bitmap(size: usize) -> (&'static [&'static str], usize) {
    let fits = |rows: &'static [&'static str]| (rows, size / rows.len());
    match size {
        0..=19 => fits(&R16),
        20..=31 => fits(&R10),
        32..=47 => fits(&R12),
        _ => fits(&R16),
    }
}

/// RGBA pixels, `size` x `size`, top row first. Sizes under 16 are drawn at 16.
pub fn tray_icon_rgba(size: usize, lit: bool, light_taskbar: bool) -> (usize, Vec<u8>) {
    let ink: u8 = if light_taskbar { 0 } else { 255 };
    let side = size.max(16);
    let (rows, cell) = bitmap(side);
    let cell = cell.max(1);
    let off = (side - rows.len() * cell) / 2;
    let on = if lit { 255 } else { OFF_ALPHA };
    let mut alpha = vec![0u8; side * side];
    for (y, row) in rows.iter().enumerate() {
        for (x, c) in row.bytes().enumerate() {
            if c != b'#' {
                continue;
            }
            for dy in 0..cell {
                for dx in 0..cell {
                    alpha[(off + y * cell + dy) * side + off + x * cell + dx] = on;
                }
            }
        }
    }
    let rgba = alpha.iter().flat_map(|&a| [ink, ink, ink, a]).collect();
    (side, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn sixteen_pixel_icon_is_the_rabbit() {
        assert_eq!(picture(16, true), R16.iter().map(|r| r.to_string()).collect::<Vec<_>>());
    }

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

    #[test]
    fn stopped_icon_is_faded() {
        let (_, rgba) = tray_icon_rgba(16, false, true);
        assert!(rgba.chunks(4).all(|p| p[0] == 0 && (p[3] == 0 || p[3] == OFF_ALPHA)));
        assert!(rgba.chunks(4).any(|p| p[3] == OFF_ALPHA));
    }
}
