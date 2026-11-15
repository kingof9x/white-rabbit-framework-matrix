//! Drawings made on the Draw page (`led_mon/drawings.py`): one panel's worth of greyscale LEDs (9 wide,
//! 34 tall), one or more frames, saved as small JSON files in the settings folder (drawings\). The
//! Picture app plays them (`wr_core::apps::picture`), and the Draw page imports PNG and GIF files into
//! them.
//!
//! File format (version 1): `{"version": 1, "delay_ms": 100, "frames": [[[0-255 x 9] x 34], ...]}`

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
pub use wr_core::apps::picture::{clamp_delay, safe_name, Drawing, Frame, DEFAULT_DELAY_MS, MAX_FRAMES};
use wr_core::grid::{HEIGHT, WIDTH};

use crate::paths;

pub const EXTENSION: &str = ".json";
/// How a picture is fitted into 9 x 34 when it is imported
pub const FITS: [(&str, &str); 3] =
    [("whole", "Show the whole picture"), ("fill", "Fill the panel (cut the edges)"), ("stretch", "Stretch to fit")];

pub fn blank() -> Frame {
    [[0; WIDTH]; HEIGHT]
}

pub fn new_drawing() -> Drawing {
    Drawing { frames: vec![blank()], delay_ms: DEFAULT_DELAY_MS }
}

pub fn to_json(d: &Drawing) -> Value {
    json!({
        "version": 1,
        "delay_ms": d.delay_ms,
        "frames": d.frames.iter().map(|f| f.iter().map(|r| r.to_vec()).collect::<Vec<_>>()).collect::<Vec<_>>(),
    })
}

pub fn folder() -> PathBuf {
    paths::drawings_dir()
}

pub fn path_for(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}{EXTENSION}", safe_name(name)))
}

/// The saved drawings' names, sorted.
pub fn list_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(EXTENSION)).map(String::from))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

pub fn save(dir: &Path, name: &str, d: &Drawing) -> Result<String, String> {
    let name = safe_name(name);
    if name.is_empty() {
        return Err("Type a name for the drawing first".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = path_for(dir, &name);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(&to_json(d)).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(name)
}

pub fn load(dir: &Path, name: &str) -> Result<Drawing, String> {
    let text = std::fs::read_to_string(path_for(dir, name)).map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Drawing::from_json(&value)
}

pub fn delete(dir: &Path, name: &str) -> Result<(), String> {
    std::fs::remove_file(path_for(dir, name)).map_err(|e| e.to_string())
}

// Editing

/// Fills the area of the same brightness around (row, col), like a paint bucket.
pub fn flood_fill(frame: &Frame, row: usize, col: usize, value: u8) -> Frame {
    let target = frame[row][col];
    let mut out = *frame;
    if target == value {
        return out;
    }
    let mut stack = vec![(row as i64, col as i64)];
    while let Some((r, c)) = stack.pop() {
        if r < 0 || c < 0 || r >= HEIGHT as i64 || c >= WIDTH as i64 {
            continue;
        }
        let (ru, cu) = (r as usize, c as usize);
        if out[ru][cu] == target {
            out[ru][cu] = value;
            stack.extend([(r + 1, c), (r - 1, c), (r, c + 1), (r, c - 1)]);
        }
    }
    out
}

pub fn mirror(f: &Frame) -> Frame {
    let mut out = *f;
    for row in out.iter_mut() {
        row.reverse();
    }
    out
}

pub fn upside_down(f: &Frame) -> Frame {
    let mut out = *f;
    out.reverse();
    out
}

pub fn invert(f: &Frame) -> Frame {
    let mut out = *f;
    for v in out.iter_mut().flatten() {
        *v = 255 - *v;
    }
    out
}

/// Moves the picture, wrapping around the edges.
pub fn roll(f: &Frame, dy: i64, dx: i64) -> Frame {
    let mut out = blank();
    for (y, row) in f.iter().enumerate() {
        for (x, v) in row.iter().enumerate() {
            let ny = (y as i64 + dy).rem_euclid(HEIGHT as i64) as usize;
            let nx = (x as i64 + dx).rem_euclid(WIDTH as i64) as usize;
            out[ny][nx] = *v;
        }
    }
    out
}

// Import

/// A greyscale picture, row by row.
struct Grey {
    w: usize,
    h: usize,
    v: Vec<f64>,
}

/// Shrinks (or grows) by averaging the area each new pixel covers, so there are no halos and pixel
/// art stays sharp.
fn resize_box(src: &Grey, nw: usize, nh: usize) -> Grey {
    let mut out = vec![0.0; nw * nh];
    let (sx, sy) = (src.w as f64 / nw as f64, src.h as f64 / nh as f64);
    for oy in 0..nh {
        let (y0, y1) = (oy as f64 * sy, (oy + 1) as f64 * sy);
        for ox in 0..nw {
            let (x0, x1) = (ox as f64 * sx, (ox + 1) as f64 * sx);
            let (mut sum, mut area) = (0.0, 0.0);
            let mut y = y0.floor() as usize;
            while (y as f64) < y1 && y < src.h {
                let hy = (y1.min(y as f64 + 1.0) - y0.max(y as f64)).max(0.0);
                let mut x = x0.floor() as usize;
                while (x as f64) < x1 && x < src.w {
                    let wx = (x1.min(x as f64 + 1.0) - x0.max(x as f64)).max(0.0);
                    sum += src.v[y * src.w + x] * wx * hy;
                    area += wx * hy;
                    x += 1;
                }
                y += 1;
            }
            out[oy * nw + ox] = if area > 0.0 { sum / area } else { 0.0 };
        }
    }
    Grey { w: nw, h: nh, v: out }
}

/// Fits a picture into 9 x 34.
fn fit(src: &Grey, how: &str) -> [[f64; WIDTH]; HEIGHT] {
    let mut out = [[0.0; WIDTH]; HEIGHT];
    if how == "stretch" || src.w == 0 || src.h == 0 {
        let r = resize_box(src, WIDTH, HEIGHT);
        for (y, row) in out.iter_mut().enumerate() {
            for (x, v) in row.iter_mut().enumerate() {
                *v = r.v[y * WIDTH + x];
            }
        }
        return out;
    }
    let (a, b) = (WIDTH as f64 / src.w as f64, HEIGHT as f64 / src.h as f64);
    let scale = if how == "fill" { a.max(b) } else { a.min(b) };
    let nw = ((src.w as f64 * scale).round() as usize).max(1);
    let nh = ((src.h as f64 * scale).round() as usize).max(1);
    let r = resize_box(src, nw, nh);
    let (left, top) = ((WIDTH as i64 - nw as i64).div_euclid(2), (HEIGHT as i64 - nh as i64).div_euclid(2));
    for y in 0..nh {
        for x in 0..nw {
            let (cx, cy) = (left + x as i64, top + y as i64);
            if cx >= 0 && cy >= 0 && (cx as usize) < WIDTH && (cy as usize) < HEIGHT {
                out[cy as usize][cx as usize] = r.v[y * nw + x];
            }
        }
    }
    out
}

/// One frame of a picture: see-through parts on black, then grey the way PIL's "L" makes it.
fn to_grey(img: &image::RgbaImage) -> Grey {
    let v = img
        .pixels()
        .map(|p| {
            let a = p[3] as f64 / 255.0;
            let (r, g, b) = (p[0] as f64 * a, p[1] as f64 * a, p[2] as f64 * a);
            (r * 299.0 + g * 587.0 + b * 114.0) / 1000.0
        })
        .collect();
    Grey { w: img.width() as usize, h: img.height() as usize, v }
}

/// A PNG, GIF (all its frames), JPEG or BMP turned into a drawing. The brightest part becomes full
/// brightness.
pub fn import(path: &Path, how: &str, swap: bool) -> Result<Drawing, String> {
    use image::AnimationDecoder;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut pictures: Vec<(Grey, i64)> = Vec::new();
    let is_gif = bytes.starts_with(b"GIF8");
    if is_gif {
        let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&bytes)).map_err(|e| e.to_string())?;
        for frame in decoder.into_frames().take(MAX_FRAMES) {
            let frame = frame.map_err(|e| e.to_string())?;
            let (num, den) = frame.delay().numer_denom_ms();
            let ms = if den == 0 { 0 } else { (num / den) as i64 };
            pictures.push((to_grey(frame.buffer()), if ms > 0 { ms } else { DEFAULT_DELAY_MS }));
        }
    } else {
        let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
        pictures.push((to_grey(&img.to_rgba8()), DEFAULT_DELAY_MS));
    }
    if pictures.is_empty() {
        return Err("there is no picture in that file".into());
    }
    let mut frames: Vec<[[f64; WIDTH]; HEIGHT]> = pictures.iter().map(|(g, _)| fit(g, how)).collect();
    if swap {
        for v in frames.iter_mut().flatten().flatten() {
            *v = 255.0 - *v;
        }
    }
    let top = frames.iter().flatten().flatten().copied().fold(0.0, f64::max);
    let out: Vec<Frame> = frames
        .iter()
        .map(|f| {
            let mut fr = blank();
            for (y, row) in f.iter().enumerate() {
                for (x, v) in row.iter().enumerate() {
                    let v = if top > 0.0 { v * 255.0 / top } else { *v };
                    fr[y][x] = v.round_ties_even().clamp(0.0, 255.0) as u8;
                }
            }
            fr
        })
        .collect();
    let mut delays: Vec<i64> = pictures.iter().map(|p| p.1).collect();
    delays.sort();
    let n = delays.len();
    let median = if n % 2 == 1 { delays[n / 2] as f64 } else { (delays[n / 2 - 1] + delays[n / 2]) as f64 / 2.0 };
    Ok(Drawing { frames: out, delay_ms: clamp_delay(Some(&json!(median as i64))) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("wr-drawings-{}", std::process::id()));
        let mut d = new_drawing();
        d.frames[0][3][4] = 200;
        d.frames.push(invert(&d.frames[0]));
        d.delay_ms = 250;
        assert_eq!(save(&dir, "My: rabbit?", &d).unwrap(), "My rabbit");
        assert_eq!(list_names(&dir), vec!["My rabbit".to_string()]);
        assert_eq!(load(&dir, "My rabbit").unwrap(), d);
        delete(&dir, "My rabbit").unwrap();
        assert!(list_names(&dir).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edits() {
        let mut f = blank();
        for row in f.iter_mut().take(10) {
            row[4] = 255;
        }
        let filled = flood_fill(&f, 0, 0, 80);
        assert_eq!(filled[0][0], 80);
        assert_eq!(filled[0][4], 255);
        // The wall at column 4 stops at row 10, so the fill goes round it
        assert_eq!(filled[0][8], 80);
        assert_eq!(mirror(&f)[0][4], 255);
        assert_eq!(upside_down(&f)[33][4], 255);
        assert_eq!(roll(&f, 0, 1)[0][5], 255);
        assert_eq!(roll(&f, -1, 0)[33][4], 255);
        assert_eq!(roll(&f, -1, 0)[9][4], 0);
        assert_eq!(roll(&f, -1, 0)[8][4], 255);
    }

    #[test]
    fn imports_a_png_and_a_gif() {
        let dir = std::env::temp_dir().join(format!("wr-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A white square on a see-through background, wider than tall
        let mut img = image::RgbaImage::new(40, 20);
        for y in 5..15 {
            for x in 15..25 {
                img.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let png = dir.join("a.png");
        img.save(&png).unwrap();
        let d = import(&png, "whole", false).unwrap();
        assert_eq!(d.frames.len(), 1);
        let lit: usize = d.frames[0].iter().flatten().filter(|v| **v > 128).count();
        assert!(lit > 0 && lit < 9 * 34 / 2);
        assert_eq!(*d.frames[0].iter().flatten().max().unwrap(), 255);
        assert_eq!(import(&png, "stretch", true).unwrap().frames[0][0][0], 255);

        let gif = dir.join("a.gif");
        {
            let file = std::fs::File::create(&gif).unwrap();
            let mut enc = image::codecs::gif::GifEncoder::new(file);
            let frames = (0..3).map(|i| {
                let mut f = image::RgbaImage::from_pixel(9, 34, image::Rgba([0, 0, 0, 255]));
                f.put_pixel(i, 0, image::Rgba([255, 255, 255, 255]));
                image::Frame::from_parts(f, 0, 0, image::Delay::from_numer_denom_ms(200, 1))
            });
            enc.encode_frames(frames).unwrap();
        }
        let d = import(&gif, "whole", false).unwrap();
        assert_eq!(d.frames.len(), 3);
        assert_eq!(d.delay_ms, 200);
        assert_eq!(d.frames[2][0][2], 255);
        let _ = std::fs::remove_dir_all(dir);
    }
}
