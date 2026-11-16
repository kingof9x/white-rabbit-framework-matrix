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

#[cfg(test)]
pub fn new_drawing() -> Drawing {
    Drawing { frames: vec![blank()], delay_ms: DEFAULT_DELAY_MS, right: Vec::new() }
}

pub fn to_json(d: &Drawing) -> Value {
    let list =
        |frames: &[Frame]| frames.iter().map(|f| f.iter().map(|r| r.to_vec()).collect::<Vec<_>>()).collect::<Vec<_>>();
    let mut v = json!({
        "version": 1,
        "delay_ms": d.delay_ms,
        "frames": list(&d.frames),
    });
    // Made for both panels: the right panel's frames ("frames" is then the left panel's)
    if !d.right.is_empty() {
        v["right"] = json!(list(&d.right));
    }
    v
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

// Editing: the editor works on both panels side by side (left panel's 9 columns, then the right's).
// Each change works on columns c0..c1: one panel, or both when they are drawn as one picture.

/// Columns of both panels side by side.
pub const BOTH: usize = 2 * WIDTH;
/// One frame of both panels.
pub type Wide = [[u8; BOTH]; HEIGHT];

/// A drawing in the editor: frames of both panels, and how long each shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Pic {
    pub frames: Vec<Wide>,
    pub delay_ms: i64,
}

pub fn blank_wide() -> Wide {
    [[0; BOTH]; HEIGHT]
}

pub fn new_pic() -> Pic {
    Pic { frames: vec![blank_wide()], delay_ms: DEFAULT_DELAY_MS }
}

/// One panel's half of a frame (0 left, 1 right).
pub fn half(w: &Wide, panel: usize) -> Frame {
    let mut f = blank();
    for (y, row) in w.iter().enumerate() {
        f[y].copy_from_slice(&row[panel * WIDTH..(panel + 1) * WIDTH]);
    }
    f
}

fn join(left: &Frame, right: &Frame) -> Wide {
    let mut w = blank_wide();
    for y in 0..HEIGHT {
        w[y][..WIDTH].copy_from_slice(&left[y]);
        w[y][WIDTH..].copy_from_slice(&right[y]);
    }
    w
}

/// The drawing as saved: the right panel's frames only when something is drawn there.
pub fn to_drawing(p: &Pic) -> Drawing {
    let frames = p.frames.iter().map(|w| half(w, 0)).collect();
    let right: Vec<Frame> = p.frames.iter().map(|w| half(w, 1)).collect();
    let used = right.iter().flatten().flatten().any(|v| *v != 0);
    Drawing { frames, delay_ms: p.delay_ms, right: if used { right } else { Vec::new() } }
}

/// A saved drawing in the editor. One made for one panel goes on the left panel.
pub fn from_drawing(d: &Drawing) -> Pic {
    let frames = d.frames.iter().enumerate().map(|(i, f)| join(f, d.right.get(i).unwrap_or(&blank()))).collect();
    Pic { frames, delay_ms: d.delay_ms }
}

/// Fills the area of the same brightness around (row, col), like a paint bucket, inside columns
/// c0..c1.
pub fn flood_fill(frame: &Wide, row: usize, col: usize, value: u8, c0: usize, c1: usize) -> Wide {
    let target = frame[row][col];
    let mut out = *frame;
    if target == value {
        return out;
    }
    let mut stack = vec![(row as i64, col as i64)];
    while let Some((r, c)) = stack.pop() {
        if r < 0 || c < c0 as i64 || r >= HEIGHT as i64 || c >= c1 as i64 {
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

pub fn mirror(f: &Wide, c0: usize, c1: usize) -> Wide {
    let mut out = *f;
    for row in out.iter_mut() {
        row[c0..c1].reverse();
    }
    out
}

pub fn upside_down(f: &Wide, c0: usize, c1: usize) -> Wide {
    let mut out = *f;
    for (y, row) in out.iter_mut().enumerate() {
        row[c0..c1].copy_from_slice(&f[HEIGHT - 1 - y][c0..c1]);
    }
    out
}

pub fn invert(f: &Wide, c0: usize, c1: usize) -> Wide {
    let mut out = *f;
    for row in out.iter_mut() {
        for v in &mut row[c0..c1] {
            *v = 255 - *v;
        }
    }
    out
}

/// Sets every LED in columns c0..c1 to `value`.
pub fn fill(f: &Wide, value: u8, c0: usize, c1: usize) -> Wide {
    let mut out = *f;
    for row in out.iter_mut() {
        row[c0..c1].fill(value);
    }
    out
}

/// Moves the picture in columns c0..c1, wrapping around the edges.
pub fn roll(f: &Wide, dy: i64, dx: i64, c0: usize, c1: usize) -> Wide {
    let mut out = *f;
    let w = (c1 - c0) as i64;
    for y in 0..HEIGHT {
        for x in c0..c1 {
            let ny = (y as i64 + dy).rem_euclid(HEIGHT as i64) as usize;
            let nx = c0 + (x as i64 - c0 as i64 + dx).rem_euclid(w) as usize;
            out[ny][nx] = f[y][x];
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

/// Fits a picture into `width` x 34.
fn fit(src: &Grey, how: &str, width: usize) -> Vec<Vec<f64>> {
    let mut out = vec![vec![0.0; width]; HEIGHT];
    if how == "stretch" || src.w == 0 || src.h == 0 {
        let r = resize_box(src, width, HEIGHT);
        for (y, row) in out.iter_mut().enumerate() {
            for (x, v) in row.iter_mut().enumerate() {
                *v = r.v[y * width + x];
            }
        }
        return out;
    }
    let (a, b) = (width as f64 / src.w as f64, HEIGHT as f64 / src.h as f64);
    let scale = if how == "fill" { a.max(b) } else { a.min(b) };
    let nw = ((src.w as f64 * scale).round() as usize).max(1);
    let nh = ((src.h as f64 * scale).round() as usize).max(1);
    let r = resize_box(src, nw, nh);
    let (left, top) = ((width as i64 - nw as i64).div_euclid(2), (HEIGHT as i64 - nh as i64).div_euclid(2));
    for y in 0..nh {
        for x in 0..nw {
            let (cx, cy) = (left + x as i64, top + y as i64);
            if cx >= 0 && cy >= 0 && (cx as usize) < width && (cy as usize) < HEIGHT {
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

/// An imported picture: frames of 34 rows of `width` values, and how long each shows.
pub struct Imported {
    pub frames: Vec<Vec<Vec<u8>>>,
    pub delay_ms: i64,
}

/// A PNG, GIF (all its frames), JPEG or BMP turned into frames `width` LEDs wide (one panel, or both).
/// The brightest part becomes full brightness.
pub fn import(path: &Path, how: &str, swap: bool, width: usize) -> Result<Imported, String> {
    use image::AnimationDecoder;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut pictures: Vec<(Grey, i64)> = Vec::new();
    let is_gif = bytes.starts_with(b"GIF8");
    if is_gif {
        let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&bytes)).map_err(|e| e.to_string())?;
        for frame in decoder.into_frames().take(MAX_FRAMES) {
            let frame = frame.map_err(|e| e.to_string())?;
            let (num, den) = frame.delay().numer_denom_ms();
            let ms = num.checked_div(den).unwrap_or(0) as i64;
            pictures.push((to_grey(frame.buffer()), if ms > 0 { ms } else { DEFAULT_DELAY_MS }));
        }
    } else {
        let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
        pictures.push((to_grey(&img.to_rgba8()), DEFAULT_DELAY_MS));
    }
    if pictures.is_empty() {
        return Err("there is no picture in that file".into());
    }
    let mut frames: Vec<Vec<Vec<f64>>> = pictures.iter().map(|(g, _)| fit(g, how, width)).collect();
    if swap {
        for v in frames.iter_mut().flatten().flatten() {
            *v = 255.0 - *v;
        }
    }
    let top = frames.iter().flatten().flatten().copied().fold(0.0, f64::max);
    let out: Vec<Vec<Vec<u8>>> = frames
        .iter()
        .map(|f| {
            f.iter()
                .map(|row| {
                    row.iter()
                        .map(|v| {
                            let v = if top > 0.0 { v * 255.0 / top } else { *v };
                            v.round_ties_even().clamp(0.0, 255.0) as u8
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    let mut delays: Vec<i64> = pictures.iter().map(|p| p.1).collect();
    delays.sort();
    let n = delays.len();
    let median = if n % 2 == 1 { delays[n / 2] as f64 } else { (delays[n / 2 - 1] + delays[n / 2]) as f64 / 2.0 };
    Ok(Imported { frames: out, delay_ms: clamp_delay(Some(&json!(median as i64))) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("wr-drawings-{}", std::process::id()));
        let mut d = new_drawing();
        d.frames[0][3][4] = 200;
        d.frames.push(half(&invert(&from_drawing(&d).frames[0], 0, WIDTH), 0));
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
        let mut f = blank_wide();
        for row in f.iter_mut().take(10) {
            row[4] = 255;
        }
        let filled = flood_fill(&f, 0, 0, 80, 0, WIDTH);
        assert_eq!(filled[0][0], 80);
        assert_eq!(filled[0][4], 255);
        // The wall at column 4 stops at row 10, so the fill goes round it, but not into the right panel
        assert_eq!(filled[0][8], 80);
        assert_eq!(filled[0][9], 0);
        assert_eq!(flood_fill(&f, 0, 0, 80, 0, BOTH)[0][17], 80);
        assert_eq!(mirror(&f, 0, WIDTH)[0][4], 255);
        assert_eq!(mirror(&f, 0, BOTH)[0][13], 255);
        assert_eq!(upside_down(&f, 0, WIDTH)[33][4], 255);
        assert_eq!(roll(&f, 0, 1, 0, WIDTH)[0][5], 255);
        assert_eq!(roll(&f, -1, 0, 0, WIDTH)[33][4], 255);
        assert_eq!(roll(&f, -1, 0, 0, WIDTH)[9][4], 0);
        assert_eq!(roll(&f, -1, 0, 0, WIDTH)[8][4], 255);
        // Rolling the left panel leaves the right one alone; rolling both carries it across
        assert_eq!(roll(&f, 0, 5, 0, WIDTH)[0][0], 255);
        assert_eq!(roll(&f, 0, 5, 0, BOTH)[0][9], 255);
        assert_eq!(invert(&f, WIDTH, BOTH)[0][9], 255);
        assert_eq!(invert(&f, WIDTH, BOTH)[0][0], 0);
        assert_eq!(fill(&f, 7, WIDTH, BOTH)[5][12], 7);
    }

    #[test]
    fn both_panels_save_and_open() {
        let mut p = new_pic();
        p.frames[0][2][1] = 10;
        // Only the left panel drawn on: saved as before, for either panel
        let d = to_drawing(&p);
        assert!(d.right.is_empty());
        assert_eq!(d.frames[0][2][1], 10);
        p.frames[0][3][12] = 20;
        let d = to_drawing(&p);
        assert_eq!(d.right[0][3][3], 20);
        let back = Drawing::from_json(&to_json(&d)).unwrap();
        assert_eq!(back, d);
        assert_eq!(from_drawing(&back), p);
        assert_eq!(back.frame_on(0.0, "right")[3][3], 20);
        assert_eq!(back.frame_on(0.0, "left")[2][1], 10);
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
        let d = import(&png, "whole", false, WIDTH).unwrap();
        assert_eq!(d.frames.len(), 1);
        let lit: usize = d.frames[0].iter().flatten().filter(|v| **v > 128).count();
        assert!(lit > 0 && lit < 9 * 34 / 2);
        assert_eq!(*d.frames[0].iter().flatten().max().unwrap(), 255);
        assert_eq!(import(&png, "stretch", true, WIDTH).unwrap().frames[0][0][0], 255);
        assert_eq!(import(&png, "whole", false, BOTH).unwrap().frames[0][0].len(), BOTH);

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
        let d = import(&gif, "whole", false, WIDTH).unwrap();
        assert_eq!(d.frames.len(), 3);
        assert_eq!(d.delay_ms, 200);
        assert_eq!(d.frames[2][0][2], 255);
        let _ = std::fs::remove_dir_all(dir);
    }
}
