//! The one-colour tray icon: two LED panels of dots, drawn to whole pixels at the tray's size
//! (a port of `make_tray_icon_image` in `led_mon/tray.py`). White on a dark taskbar, black on a light one.

/// (outline width, dot size, gap, gap between the two panels) by icon size.
const LAYOUTS: [(usize, (usize, usize, usize, usize)); 8] = [
    (16, (1, 1, 1, 2)),
    (20, (1, 1, 1, 4)),
    (24, (1, 2, 1, 2)),
    (28, (2, 2, 1, 4)),
    (32, (2, 2, 2, 4)),
    (36, (2, 2, 2, 4)),
    (40, (2, 3, 2, 6)),
    (48, (2, 3, 3, 6)),
];
/// Unlit dots, and every dot while the animation is stopped.
const OFF_ALPHA: u8 = 80;

/// RGBA pixels, `size` x `size`, top row first. Sizes under 16 are drawn at 16.
pub fn tray_icon_rgba(size: usize, lit: bool, light_taskbar: bool) -> (usize, Vec<u8>) {
    let ink: u8 = if light_taskbar { 0 } else { 255 };
    let (b, d, g, pg) = LAYOUTS.iter().rev().find(|(s, _)| *s <= size).map(|(_, l)| *l).unwrap_or(LAYOUTS[0].1);
    let side = size.max(16);
    let mut alpha = vec![0u8; side * side];
    let w = b + g + d + g + d + g + b;
    let h = 2 * b + 2 * g + 6 * d + 5 * g;
    let mut rect = |x: usize, y: usize, ww: usize, hh: usize, a: u8| {
        for yy in y..(y + hh).min(side) {
            for xx in x..(x + ww).min(side) {
                alpha[yy * side + xx] = a;
            }
        }
    };
    let x0 = (side - (2 * w + pg)) / 2;
    let y0 = (side - h) / 2;
    for (panel_x, left) in [(2usize, x0), (9, x0 + w + pg)] {
        rect(left + b, y0, w - 2 * b, b, 255);
        rect(left + b, y0 + h - b, w - 2 * b, b, 255);
        rect(left, y0 + b, b, h - 2 * b, 255);
        rect(left + w - b, y0 + b, b, h - 2 * b, 255);
        if b > 1 {
            for (cx, cy) in
                [(left + 1, y0 + 1), (left + w - 2, y0 + 1), (left + 1, y0 + h - 2), (left + w - 2, y0 + h - 2)]
            {
                rect(cx, cy, 1, 1, 255);
            }
        }
        for col in 0..2 {
            for row in 0..6 {
                let on = lit && (row + col + panel_x) % 3 != 0;
                rect(left + b + g + col * (d + g), y0 + b + g + row * (d + g), d, d, if on { 255 } else { OFF_ALPHA });
            }
        }
    }
    let rgba = alpha.iter().flat_map(|&a| [ink, ink, ink, a]).collect();
    (side, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_pixel_icon() {
        let (side, rgba) = tray_icon_rgba(16, true, false);
        assert_eq!(side, 16);
        let rows: Vec<String> = (0..16)
            .map(|y| {
                (0..16)
                    .map(|x| match rgba[(y * 16 + x) * 4 + 3] {
                        255 => '#',
                        OFF_ALPHA => '+',
                        _ => '.',
                    })
                    .collect()
            })
            .collect();
        // What the Python tray draws at 16 pixels
        let expected = [
            ".#####....#####.",
            "#.....#..#.....#",
            "#.#.+.#..#.+.#.#",
            "#.....#..#.....#",
            "#.+.#.#..#.#.#.#",
            "#.....#..#.....#",
            "#.#.#.#..#.#.+.#",
            "#.....#..#.....#",
            "#.#.+.#..#.+.#.#",
            "#.....#..#.....#",
            "#.+.#.#..#.#.#.#",
            "#.....#..#.....#",
            "#.#.#.#..#.#.+.#",
            "#.....#..#.....#",
            ".#####....#####.",
            "................",
        ];
        assert_eq!(rows, expected);
        assert!(rows.iter().all(|r| r.len() == 16));
    }

    #[test]
    fn stopped_icon_is_dim() {
        let (_, rgba) = tray_icon_rgba(16, false, true);
        assert!(rgba.chunks(4).all(|p| p[0] == 0 && (p[3] == 0 || p[3] == 255 || p[3] == OFF_ALPHA)));
        assert!(rgba.chunks(4).any(|p| p[3] == OFF_ALPHA));
    }
}
