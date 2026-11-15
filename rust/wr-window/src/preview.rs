//! The LED previews on the Effects, Music, Math, Games and Draw pages: panels drawn as rows of round
//! LEDs, the way `led_color` and the canvases in `music.py` drew them.

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use wr_core::grid::{Grid, HEIGHT, WIDTH};

/// One panel's brightness, 0 to 1, indexed [x][y].
pub type Levels = [[f64; HEIGHT]; WIDTH];

pub const DOT: f64 = 7.0;
pub const STEP: usize = 8;
pub const PANEL_GAP: usize = 18;
pub const MARGIN: usize = 10;
const BACKGROUND: [u8; 3] = [0x11, 0x11, 0x11];
const LED_OFF: [f64; 3] = [34.0, 34.0, 34.0];
const LED_ON: [f64; 3] = [250.0, 246.0, 232.0];

pub fn dark() -> Levels {
    [[0.0; HEIGHT]; WIDTH]
}

/// A panel picture (0-255) as levels.
pub fn levels(grid: &Grid) -> Levels {
    let mut out = dark();
    for (x, col) in out.iter_mut().enumerate() {
        for (y, v) in col.iter_mut().enumerate() {
            *v = (grid.get(x, y) as f64 / 255.0).clamp(0.0, 1.0);
        }
    }
    out
}

/// The colour of an LED at this level, as `led_color` in `music.py`.
pub fn led_color(level: f64) -> [u8; 3] {
    let level = if level > 0.0 { level.clamp(0.0, 1.0).powf(0.6) } else { 0.0 };
    let mut c = [0u8; 3];
    for i in 0..3 {
        c[i] = (LED_OFF[i] + (LED_ON[i] - LED_OFF[i]) * level) as u8;
    }
    c
}

/// Size in pixels of a preview of `count` panels side by side, at scale 1.
pub fn size(count: usize) -> (usize, usize) {
    let count = count.max(1);
    (2 * MARGIN + count * WIDTH * STEP + (count - 1) * PANEL_GAP, 2 * MARGIN + HEIGHT * STEP)
}

/// Panels side by side, `scale` pixels per point (2 looks sharp on most laptop screens).
pub fn render(panels: &[Levels], scale: usize) -> Image {
    let (w, h) = size(panels.len());
    let (w, h) = (w * scale, h * scale);
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
    let pixels = buffer.make_mut_bytes();
    for px in pixels.chunks_exact_mut(4) {
        px.copy_from_slice(&[BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]);
    }
    let s = scale as f64;
    let radius = DOT * s / 2.0;
    let span = (DOT * s).ceil() as usize + 1;
    for (p, panel) in panels.iter().enumerate() {
        let x0 = MARGIN + p * (WIDTH * STEP + PANEL_GAP);
        for (x, col) in panel.iter().enumerate() {
            for (y, level) in col.iter().enumerate() {
                let color = led_color(*level);
                let left = (x0 + x * STEP) * scale;
                let top = (MARGIN + y * STEP) * scale;
                let (cx, cy) = (left as f64 + radius, top as f64 + radius);
                for py in top..(top + span).min(h) {
                    for px in left..(left + span).min(w) {
                        let d = ((px as f64 + 0.5 - cx).powi(2) + (py as f64 + 0.5 - cy).powi(2)).sqrt();
                        // Soft edge one pixel wide
                        let cover = (radius - d + 0.5).clamp(0.0, 1.0);
                        if cover <= 0.0 {
                            continue;
                        }
                        let i = (py * w + px) * 4;
                        for c in 0..3 {
                            let bg = pixels[i + c] as f64;
                            pixels[i + c] = (bg + (color[c] as f64 - bg) * cover).round() as u8;
                        }
                    }
                }
            }
        }
    }
    Image::from_rgba8(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_match_python() {
        assert_eq!(led_color(0.0), [34, 34, 34]);
        assert_eq!(led_color(1.0), [250, 246, 232]);
        // int(34 + 216 * 0.5 ** 0.6) = 176
        assert_eq!(led_color(0.5)[0], 176);
    }

    #[test]
    fn two_panels_are_as_wide_as_in_python() {
        assert_eq!(size(2), (182, 292));
        let image = render(&[dark(), dark()], 2);
        assert_eq!((image.size().width, image.size().height), (364, 584));
    }
}
