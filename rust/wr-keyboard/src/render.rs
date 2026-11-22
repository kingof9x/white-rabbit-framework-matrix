//! Draws the keyboard picture with tiny-skia (keycaps, their glow, the laptop deck), the way White
//! Rabbit draws its LED panel previews in Rust. Key legends are not drawn here: they are returned as
//! text lines the window puts on top, so they use the window's own font.

use crate::deck::{KbType, PlacedKey, Scene, Shell, R};
use crate::effects::Rgb;
use crate::keys::SideModule;
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, Shader, SpreadMode, Stroke,
    Transform,
};

/// One line of legend, positions and sizes as fractions of the picture's width.
#[derive(Debug, Clone, PartialEq)]
pub struct LegendLine {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
    pub color: [u8; 4],
    pub fs: f32,
    pub left: bool,
}

pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA
    pub pixels: Vec<u8>,
    pub legends: Vec<LegendLine>,
}

/// A rounded rectangle; `corners` = which corners are round: [top-left, top-right, bottom-right, bottom-left].
fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32, corners: [bool; 4]) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let c = |i: usize| if corners[i] { r } else { 0.0 };
    let mut pb = PathBuilder::new();
    pb.move_to(x + c(0), y);
    pb.line_to(x + w - c(1), y);
    pb.quad_to(x + w, y, x + w, y + c(1));
    pb.line_to(x + w, y + h - c(2));
    pb.quad_to(x + w, y + h, x + w - c(2), y + h);
    pb.line_to(x + c(3), y + h);
    pb.quad_to(x, y + h, x, y + h - c(3));
    pb.line_to(x, y + c(0));
    pb.quad_to(x, y, x + c(0), y);
    pb.close();
    pb.finish()
}

fn solid(r: u8, g: u8, b: u8, a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(r, g, b, a);
    p.anti_alias = true;
    p
}

fn fill(pm: &mut Pixmap, path: Option<tiny_skia::Path>, paint: &Paint) {
    if let Some(path) = path {
        pm.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
    }
}

/// A soft glow just around a key's edge: thin rings that fade out quickly, so the dark gaps between
/// keys stay dark and every key keeps its own shape.
#[allow(clippy::too_many_arguments)]
fn glow(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, radius: f32, c: Rgb, alpha: f32, blur: f32) {
    if alpha <= 0.02 || blur <= 0.5 {
        return;
    }
    const RINGS: usize = 4;
    for i in (1..=RINGS).rev() {
        let t = i as f32 / RINGS as f32;
        let e = blur * t;
        let a = alpha * 0.16 * (1.0 - t) * (1.0 - t) + alpha * 0.04;
        fill(
            pm,
            rrect(x - e, y - e, w + 2.0 * e, h + 2.0 * e, radius + e, [true; 4]),
            &solid(c[0], c[1], c[2], (a * 255.0) as u8),
        );
    }
}

/// Draws the deck at `width` pixels wide.
pub fn draw(scene: &Scene, keys: &[PlacedKey], colors: &[Rgb], shell: &Shell, width: u32, accent: [u8; 3]) -> Picture {
    let width = width.max(200);
    let scale = width as f32 / shell.size.0;
    let height = (shell.size.1 * scale).round().max(1.0) as u32;
    let mut pm = Pixmap::new(width, height).expect("picture size");
    let u = |v: f32| v * scale;
    let px = |r: &R| (u(r.x), u(r.y), u(r.w), u(r.h));

    // laptop deck, module plates, spacers, seam
    fill(&mut pm, rrect(0.0, 0.0, width as f32, height as f32, u(0.4), [true; 4]), &solid(0x64, 0x66, 0x6c, 255));
    for (r, corners) in &shell.plates {
        let (x, y, w, h) = px(r);
        fill(&mut pm, rrect(x, y, w, h, u(0.22), *corners), &solid(0x0c, 0x0c, 0x0e, 255));
    }
    for r in &shell.spacers {
        let (x, y, w, h) = px(r);
        fill(&mut pm, rrect(x, y, w, h, u(0.22), [true; 4]), &solid(0x72, 0x74, 0x7a, 255));
    }
    if let (Some(sx), Some((plate, _))) = (shell.seam, shell.plates.first()) {
        let mut pb = PathBuilder::new();
        pb.move_to(u(sx), u(plate.y));
        pb.line_to(u(sx), u(plate.y + plate.h));
        if let Some(path) = pb.finish() {
            let stroke = Stroke { width: 1.5, ..Stroke::default() };
            pm.stroke_path(&path, &solid(0, 0, 0, 255), &stroke, Transform::identity(), None);
        }
    }

    let mut legends = Vec::new();
    let fs = u(0.3).max(8.0);
    let gap = u(0.05);
    let radius = u(0.17);
    for (k, &c) in keys.iter().zip(colors) {
        let material = match k.dev {
            crate::deck::Dev::Keyboard => scene.setup.kb,
            crate::deck::Dev::Module => {
                if scene.setup.side == SideModule::Macropad {
                    KbType::ClearRgb
                } else {
                    KbType::White
                }
            }
        };
        let (x, y, w, h) = px(&k.rect);
        let (x, y, w, h) = (x + gap, y + gap, w - 2.0 * gap, h - 2.0 * gap);
        let lvl = c.iter().copied().max().unwrap_or(0) as f32 / 255.0;
        let (fx, fy, fw, fh) = (x + u(0.1), y + u(0.075), w - u(0.2), h - u(0.25));
        let legend: [u8; 4] = match material {
            KbType::ClearRgb => {
                glow(&mut pm, x, y, w, h, radius, c, 0.2 + 0.5 * lvl, u(0.04) + u(0.1) * lvl);
                // frosted white when dark, LED color when lit
                let frost = 150.0 * (1.0 - lvl) + 40.0;
                let f = |q: u8| (frost + q as f32 * 0.85).min(255.0) as u8;
                let fc = [f(c[0]), f(c[1]), f(c[2])];
                let dim = |q: u8| (q as f32 * 0.62) as u8;
                fill(&mut pm, rrect(x, y, w, h, radius, [true; 4]), &solid(dim(fc[0]), dim(fc[1]), dim(fc[2]), 255));
                if let Some(path) = rrect(x, y, w, h, radius, [true; 4]) {
                    let stroke = Stroke { width: 1.0, ..Stroke::default() };
                    pm.stroke_path(&path, &solid(0, 0, 0, 140), &stroke, Transform::identity(), None);
                }
                fill(&mut pm, rrect(fx, fy, fw, fh, radius * 0.7, [true; 4]), &solid(fc[0], fc[1], fc[2], 255));
                fill(
                    &mut pm,
                    rrect(fx + u(0.1), fy + 0.5, fw - u(0.2), 1.2, 0.5, [true; 4]),
                    &solid(255, 255, 255, 90),
                );
                let lum = 0.299 * fc[0] as f32 + 0.587 * fc[1] as f32 + 0.114 * fc[2] as f32;
                if lum > 125.0 {
                    [20, 20, 24, 215]
                } else {
                    [255, 255, 255, 235]
                }
            }
            KbType::BlackRgb | KbType::White => {
                if material == KbType::BlackRgb && lvl > 0.05 {
                    glow(&mut pm, x, y, w, h, radius, c, 0.3 + 0.5 * lvl, u(0.04) + u(0.1) * lvl);
                }
                fill(&mut pm, rrect(x, y, w, h, radius, [true; 4]), &solid(0x14, 0x14, 0x16, 255));
                let face = Paint {
                    anti_alias: true,
                    shader: LinearGradient::new(
                        Point::from_xy(0.0, fy),
                        Point::from_xy(0.0, fy + fh),
                        vec![
                            GradientStop::new(0.0, Color::from_rgba8(0x2e, 0x2e, 0x33, 255)),
                            GradientStop::new(1.0, Color::from_rgba8(0x22, 0x22, 0x26, 255)),
                        ],
                        SpreadMode::Pad,
                        Transform::identity(),
                    )
                    .unwrap_or(Shader::SolidColor(Color::from_rgba8(0x29, 0x29, 0x2e, 255))),
                    ..Paint::default()
                };
                fill(&mut pm, rrect(fx, fy, fw, fh, radius * 0.7, [true; 4]), &face);
                if lvl < 0.06 {
                    [0x55, 0x55, 0x5d, 255]
                } else {
                    [c[0], c[1], c[2], 255]
                }
            }
        };
        if scene.show_paint_marks && scene.paint.contains_key(&(k.dev, k.index)) {
            let r = u(0.065);
            if let Some(path) = PathBuilder::from_circle(x + w - u(0.15), y + u(0.15), r) {
                pm.fill_path(&path, &solid(255, 255, 255, 230), FillRule::Winding, Transform::identity(), None);
            }
        }
        if scene.selected == Some((k.dev, k.index)) {
            if let Some(path) = rrect(x + 1.0, y + 1.0, w - 2.0, h - 2.0, radius, [true; 4]) {
                let stroke = Stroke { width: 2.5, ..Stroke::default() };
                pm.stroke_path(
                    &path,
                    &solid(accent[0], accent[1], accent[2], 255),
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }
        }

        // legends: one or two lines
        let main = &k.def.main;
        let long = main.chars().count() > 2 && w > fs * 5.0;
        let lx = if long { fx + u(0.12) } else { fx };
        let lw = if long { fw - u(0.12) } else { fw };
        let cy = fy + fh / 2.0;
        let mut line = |text: &str, cy: f32, size: f32, color: [u8; 4]| {
            let lh = size * 1.4;
            legends.push(LegendLine {
                x: lx / width as f32,
                y: (cy - lh / 2.0) / width as f32,
                w: lw / width as f32,
                h: lh / width as f32,
                text: text.to_string(),
                color,
                fs: size / width as f32,
                left: long,
            });
        };
        if !k.def.top.is_empty() && h >= fs * 2.6 {
            let faded = [legend[0], legend[1], legend[2], (legend[3] as f32 * 0.8) as u8];
            line(&k.def.top, cy - fs * 0.6, fs * 0.78, faded);
            let fit = lw / (main.chars().count().max(1) as f32 * 0.62);
            line(main, cy + fs * 0.55, fs.min(fit), legend);
        } else {
            let size = if h < fs * 2.0 {
                fs * 0.72
            } else if main.chars().count() <= 2 {
                fs * 1.12
            } else {
                fs * 0.82
            };
            // shrink long labels so they fit on narrow keys
            let fit = lw / (main.chars().count().max(1) as f32 * 0.62);
            line(main, cy, size.min(fit), legend);
        }
    }
    Picture { width, height, pixels: pm.take(), legends }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::{self, Choice, Look, Preview, Setup};
    use std::collections::HashMap;

    fn scene(paint: &HashMap<(deck::Dev, usize), (u8, u8)>) -> Scene<'_> {
        let look = Look { choice: Choice::Builtin(1), hue: 0, sat: 255, val: 255, speed: 128 };
        Scene {
            setup: Setup::default(),
            joined: true,
            kb: look,
            module: look,
            paint,
            paint_mode: crate::protocol::PaintMode::Follow,
            macropad_layer: 0,
            show_paint_marks: true,
            selected: Some((crate::deck::Dev::Module, 0)),
        }
    }

    #[test]
    #[ignore]
    fn timing() {
        let paint = HashMap::new();
        let sc = scene(&paint);
        let keys = deck::place_keys(&sc.setup, 0);
        let shell = deck::shell(&sc.setup);
        let mut p = Preview::default();
        let t = std::time::Instant::now();
        for _ in 0..30 {
            let colors = p.colors(&sc, &keys);
            let _ = draw(&sc, &keys, &colors, &shell, 1400, [0, 95, 184]);
        }
        eprintln!("per frame: {:?}", t.elapsed() / 30);
    }

    #[test]
    fn draws_every_key_and_legend() {
        let paint = HashMap::new();
        let sc = scene(&paint);
        let keys = deck::place_keys(&sc.setup, 0);
        let colors = Preview::default().colors(&sc, &keys);
        let shell = deck::shell(&sc.setup);
        let pic = draw(&sc, &keys, &colors, &shell, 1200, [0, 95, 184]);
        assert_eq!(pic.pixels.len(), (pic.width * pic.height * 4) as usize);
        assert_eq!(pic.width, 1200);
        // 78 keyboard + 24 macropad keys, some with two legend lines
        assert!(pic.legends.len() >= 102, "{}", pic.legends.len());
        assert!(pic.legends.iter().any(|l| l.text == "Super"));
        // every legend sits inside the picture
        let aspect = pic.height as f32 / pic.width as f32;
        for l in &pic.legends {
            assert!(l.x >= 0.0 && l.x + l.w <= 1.0 && l.y >= 0.0 && l.y + l.h <= aspect + 1e-3, "{l:?}");
        }
        // solid red: a clear key's face is reddish
        let k = &keys[30];
        let (x, y) = (
            ((k.rect.x + k.rect.w / 2.0) * 1200.0 / shell.size.0) as u32,
            ((k.rect.y + k.rect.h / 2.0) * 1200.0 / shell.size.0) as u32,
        );
        let i = ((y * pic.width + x) * 4) as usize;
        assert!(pic.pixels[i] > 200 && pic.pixels[i + 1] < 120, "{:?}", &pic.pixels[i..i + 4]);
    }
}
