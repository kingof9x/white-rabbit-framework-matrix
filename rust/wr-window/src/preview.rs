//! The LED previews on the Effects, Music, Math, Games and Draw pages: panels drawn as rows of round
//! LEDs, the way `led_color` and the canvases in `music.py` drew them.
//!
//! A preview is a plain RGBA picture (`slint::Image`) that a page sets on an Image element. It is drawn
//! pixel by pixel here on the CPU: a dark background, then for each LED a filled circle whose colour
//! goes from dark grey (off) to warm white (full), with a one-pixel soft edge so circles look round at
//! any scale. Sizes are in "points" (logical pixels); `scale` multiplies them so the picture stays sharp
//! on high-DPI screens. Images are made on the UI thread, since Slint images belong there; a picture of
//! two panels at scale 2 is about 364 x 584 pixels, quick enough to redraw many times a second.

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use wr_core::grid::{Grid, HEIGHT, WIDTH};

/// One panel's brightness, 0 to 1, indexed [x][y].
pub type Levels = [[f64; HEIGHT]; WIDTH];

/// Diameter of one LED circle, in points.
pub const DOT: f64 = 7.0;
/// Distance from one LED to the next, in points (so 1 point of space between circles).
pub const STEP: usize = 8;
/// Space between two panels, in points.
pub const PANEL_GAP: usize = 18;
/// Space around the edge of the picture, in points.
pub const MARGIN: usize = 10;
/// Background colour (RGB), almost black.
const BACKGROUND: [u8; 3] = [0x11, 0x11, 0x11];
/// The colour (RGB) of an LED that is off: a dim grey, so the unlit LEDs are still visible.
const LED_OFF: [f64; 3] = [34.0, 34.0, 34.0];
/// The colour (RGB) of an LED at full brightness: a slightly warm white, like the real LEDs.
const LED_ON: [f64; 3] = [250.0, 246.0, 232.0];

/// One panel with every LED off.
pub fn dark() -> Levels {
    [[0.0; HEIGHT]; WIDTH]
}

/// A panel picture (0-255) as levels.
///
/// 255 shows at full brightness.
pub fn levels(grid: &Grid) -> Levels {
    levels_of(grid, 255.0)
}

/// A panel picture as levels, `full` being the value shown at full brightness (the layout editor uses
/// 160, the brightest app pixels).
pub fn levels_of(grid: &Grid, full: f64) -> Levels {
    let mut out = dark();
    for (x, col) in out.iter_mut().enumerate() {
        for (y, v) in col.iter_mut().enumerate() {
            *v = (grid.get(x, y) as f64 / full).clamp(0.0, 1.0);
        }
    }
    out
}

/// The colour of an LED at this level, as `led_color` in `music.py`.
pub fn led_color(level: f64) -> [u8; 3] {
    // A 0.6 power brightens the dim end so faint LEDs still show on screen (the same curve as Python's
    // led_color). Levels are clamped to 0..1; NaN or a negative level counts as off.
    let level = if level > 0.0 { level.clamp(0.0, 1.0).powf(0.6) } else { 0.0 };
    let mut c = [0u8; 3];
    for i in 0..3 {
        c[i] = (LED_OFF[i] + (LED_ON[i] - LED_OFF[i]) * level) as u8;
    }
    c
}

/// How the LEDs are spaced: the pages' previews, or the layout editor's bigger panels.
///
/// All in points: `dot` the LED diameter, `step` from one LED to the next, `margin` around the
/// picture, `gap` between panels.
#[derive(Clone, Copy)]
pub struct Style {
    pub dot: f64,
    pub step: usize,
    pub margin: usize,
    pub gap: usize,
}

/// The pages' previews: 7-point LEDs, 8 points apart.
pub const SMALL: Style = Style { dot: DOT, step: STEP, margin: MARGIN, gap: PANEL_GAP };
/// The layout editor's panel: 9 point LEDs, 2 points apart
pub const LARGE: Style = Style { dot: 9.0, step: 11, margin: 10, gap: PANEL_GAP };

/// Size in pixels of a preview of `count` panels side by side, at scale 1.
pub fn size(count: usize) -> (usize, usize) {
    size_of(count, SMALL)
}

/// Size in points of `count` panels side by side in this style (at least one panel): margins, plus
/// 9 LED steps per panel, plus the gaps between panels; height is 34 LED steps plus margins.
pub fn size_of(count: usize, style: Style) -> (usize, usize) {
    let count = count.max(1);
    let (step, margin) = (style.step, style.margin);
    (2 * margin + count * WIDTH * step + (count - 1) * style.gap, 2 * margin + HEIGHT * step)
}

/// Panels side by side, `scale` pixels per point (2 looks sharp on most laptop screens).
pub fn render(panels: &[Levels], scale: usize) -> Image {
    render_styled(panels, scale, SMALL)
}

/// Draws the panels (left first) in this style, `scale` pixels per point.
pub fn render_styled(panels: &[Levels], scale: usize, style: Style) -> Image {
    let Style { dot, step, margin, gap } = style;
    let (w, h) = size_of(panels.len(), style);
    let (w, h) = (w * scale, h * scale);
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
    let pixels = buffer.make_mut_bytes();
    for px in pixels.chunks_exact_mut(4) {
        px.copy_from_slice(&[BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]);
    }
    let s = scale as f64;
    let radius = dot * s / 2.0;
    // Each LED is drawn inside a square of this many pixels starting at its top-left corner: the
    // circle's diameter rounded up, plus one pixel for the soft edge.
    let span = (dot * s).ceil() as usize + 1;
    for (p, panel) in panels.iter().enumerate() {
        // Left edge of panel p, in points.
        let x0 = margin + p * (WIDTH * step + gap);
        for (x, col) in panel.iter().enumerate() {
            for (y, level) in col.iter().enumerate() {
                let color = led_color(*level);
                let left = (x0 + x * step) * scale;
                let top = (margin + y * step) * scale;
                let (cx, cy) = (left as f64 + radius, top as f64 + radius);
                for py in top..(top + span).min(h) {
                    for px in left..(left + span).min(w) {
                        // Distance from this pixel's centre to the circle's centre.
                        let d = ((px as f64 + 0.5 - cx).powi(2) + (py as f64 + 0.5 - cy).powi(2)).sqrt();
                        // Soft edge one pixel wide
                        let cover = (radius - d + 0.5).clamp(0.0, 1.0);
                        if cover <= 0.0 {
                            continue;
                        }
                        // Blend the LED colour over what is already there by how much of the pixel the circle
                        // covers.
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

/// Tests for the LED previews.
#[cfg(test)]
mod tests {
    use super::*;

    /// The LED colours are the same numbers the Python app used.
    #[test]
    fn colours_match_python() {
        assert_eq!(led_color(0.0), [34, 34, 34]);
        assert_eq!(led_color(1.0), [250, 246, 232]);
        // int(34 + 216 * 0.5 ** 0.6) = 176
        assert_eq!(led_color(0.5)[0], 176);
    }

    /// Two panels make a preview the same size as the Python app's, and `scale` multiplies it.
    #[test]
    fn two_panels_are_as_wide_as_in_python() {
        assert_eq!(size(2), (182, 292));
        let image = render(&[dark(), dark()], 2);
        assert_eq!((image.size().width, image.size().height), (364, 584));
    }
}
