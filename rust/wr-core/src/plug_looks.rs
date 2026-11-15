//! The plug animations you can pick on the Effects page besides the ripple and the drain: power surge,
//! pixel rain, old TV, ripple ring, zipper and glitch. Each has a way out (unplug: the picture goes) and a
//! way in (plug in: the picture comes back).
//!
//! Every frame is worked out from the time since the animation started, so it plays the same however
//! often the panel is redrawn. The pictures here have the port at the right-hand edge; for a port on the
//! left the caller flips the picture going in and the frame coming out, as it does for the drain.

use crate::animations::{tv_off_frames, tv_on_frames, TV_FRAME_SECONDS};
use crate::effects::Picture;
use crate::{Grid, HEIGHT, WIDTH};

/// One of the plug animations (the ripple and the drain are in `effects`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Surge,
    Rain,
    Tv,
    Ring,
    Zipper,
    Glitch,
}

impl Look {
    pub const ALL: [Look; 6] = [Look::Surge, Look::Rain, Look::Tv, Look::Ring, Look::Zipper, Look::Glitch];

    /// The name saved in settings.json.
    pub fn key(self) -> &'static str {
        match self {
            Look::Surge => "surge",
            Look::Rain => "rain",
            Look::Tv => "tv",
            Look::Ring => "ring",
            Look::Zipper => "zipper",
            Look::Glitch => "glitch",
        }
    }

    pub fn from_key(key: &str) -> Option<Look> {
        Look::ALL.into_iter().find(|l| l.key() == key)
    }

    /// The name on the Effects page.
    pub fn label(self) -> &'static str {
        match self {
            Look::Surge => "Power surge",
            Look::Rain => "Pixel rain",
            Look::Tv => "Old TV",
            Look::Ring => "Ripple ring",
            Look::Zipper => "Zipper",
            Look::Glitch => "Glitch",
        }
    }

    /// How long the picture takes to go, in seconds.
    pub fn out_seconds(self) -> f64 {
        match self {
            Look::Surge => 1.2,
            Look::Rain => 2.05,
            Look::Tv => tv_off_frames(None, 160).len() as f64 * TV_FRAME_SECONDS + 5.0 * TV_FRAME_SECONDS,
            Look::Ring => 1.0,
            Look::Zipper => 1.25,
            Look::Glitch => 1.1,
        }
    }

    /// How long the picture takes to come in, in seconds.
    pub fn in_seconds(self) -> f64 {
        match self {
            Look::Surge => 1.15,
            Look::Rain => 1.2,
            Look::Tv => (tv_on_frames(None, 160).len() + 5) as f64 * TV_FRAME_SECONDS,
            Look::Ring => 1.0,
            Look::Zipper => 1.2,
            Look::Glitch => 1.05,
        }
    }

    /// The picture going, `t` seconds in; None once it has gone.
    /// `port_row` is the row the port is level with, `bright` the panels' foreground brightness.
    pub fn out_frame(self, pic: &Picture, t: f64, port_row: f64, bright: f64) -> Option<Picture> {
        if t >= self.out_seconds() {
            return None;
        }
        let p = Px::from(pic);
        let k = Ctx { row: port_row, b: bright.max(1.0) / 160.0 };
        let f = match self {
            Look::Surge => surge_out(&p, t, &k),
            Look::Rain => rain_out(&p, t),
            Look::Tv => tv_at(&tv_off_frames(Some(&pic.to_grid()), bright as i32), t),
            Look::Ring => ring(&p, t, 1.0, false, &k),
            Look::Zipper => zip_out(&p, t, &k),
            Look::Glitch => glitch_out(&p, t, &k),
        };
        Some(f.to_picture())
    }

    /// The picture coming in, `t` seconds in; None once it is all there.
    pub fn in_frame(self, pic: &Picture, t: f64, port_row: f64, bright: f64) -> Option<Picture> {
        if t >= self.in_seconds() {
            return None;
        }
        let p = Px::from(pic);
        let k = Ctx { row: port_row, b: bright.max(1.0) / 160.0 };
        let f = match self {
            Look::Surge => surge_in(&p, t, &k),
            Look::Rain => rain_in(&p, t),
            Look::Tv => tv_at(&tv_on_frames(Some(&pic.to_grid()), bright as i32), t),
            Look::Ring => ring(&p, t, 1.0, true, &k),
            Look::Zipper => zip_in(&p, t, &k),
            Look::Glitch => glitch_in(&p, t, &k),
        };
        Some(f.to_picture())
    }
}

const FPS: f64 = 30.0;
/// Just past the right-hand edge, where the port is.
const PORT_X: f64 = WIDTH as f64;

/// Where the port is and how bright the panels are (1.0 at full brightness).
struct Ctx {
    row: f64,
    b: f64,
}

impl Ctx {
    /// A brightness given for full brightness (160 is the normal foreground), scaled to the panels now.
    fn v(&self, full: f64) -> f64 {
        (full * self.b).min(255.0)
    }
}

/// A panel picture as `[x][y]`, 0-255.
#[derive(Clone, Copy)]
struct Px([[f64; HEIGHT]; WIDTH]);

impl Px {
    fn dark() -> Px {
        Px([[0.0; HEIGHT]; WIDTH])
    }

    fn from(p: &Picture) -> Px {
        let mut f = Px::dark();
        for y in 0..HEIGHT.min(p.rows) {
            for x in 0..WIDTH.min(p.cols) {
                f.0[x][y] = p.at(y, x).clamp(0.0, 255.0);
            }
        }
        f
    }

    fn from_grid(g: &Grid) -> Px {
        let mut f = Px::dark();
        for x in 0..WIDTH {
            for y in 0..HEIGHT {
                f.0[x][y] = g.get(x, y).clamp(0, 255) as f64;
            }
        }
        f
    }

    fn to_picture(self) -> Picture {
        let mut p = Picture::zeros(HEIGHT, WIDTH);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                p.set(y, x, self.0[x][y].clamp(0.0, 255.0));
            }
        }
        p
    }

    /// Lights a pixel at least this bright; off the panel is ignored.
    fn put(&mut self, x: i64, y: i64, v: f64) {
        if x >= 0 && (x as usize) < WIDTH && y >= 0 && (y as usize) < HEIGHT {
            let c = &mut self.0[x as usize][y as usize];
            *c = c.max(v);
        }
    }

    fn set(&mut self, x: i64, y: i64, v: f64) {
        if x >= 0 && (x as usize) < WIDTH && y >= 0 && (y as usize) < HEIGHT {
            self.0[x as usize][y as usize] = v;
        }
    }
}

/// Repeatable noise in 0..1.
fn h(a: i64, b: i64, c: i64) -> f64 {
    let mut x = (a as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (b as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (c as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

fn smooth(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn tv_at(frames: &[Grid], t: f64) -> Px {
    Px::from_grid(&frames[((t / TV_FRAME_SECONDS) as usize).min(frames.len() - 1)])
}

// Power surge: the picture squeezes into a bright line that is sucked into the port; a spark shoots
// out of the port and floods the panel with light, which settles into the picture.

fn squeeze_to_port(pic: &Px, share: f64, row: f64) -> Px {
    let mut f = Px::dark();
    let gain = (1.0 / share.max(0.05).sqrt()).min(2.2);
    let centre = row - 0.5;
    for y in 0..HEIGHT {
        let src = centre + (y as f64 - centre) / share.max(0.02);
        if src >= 0.0 && src <= (HEIGHT - 1) as f64 {
            for x in 0..WIDTH {
                f.0[x][y] = pic.0[x][src.round() as usize] * gain;
            }
        }
    }
    f
}

fn surge_out(pic: &Px, t: f64, k: &Ctx) -> Px {
    let row = k.row as i64;
    if t < 0.12 {
        let g = 1.0 + 0.8 * (t / 0.12);
        let mut f = *pic;
        f.0.iter_mut().flatten().for_each(|v| *v *= g);
        return f;
    }
    if t < 0.55 {
        let s = smooth((t - 0.12) / 0.43);
        let mut f = squeeze_to_port(pic, 1.0 - 0.97 * s, k.row);
        for x in 0..WIDTH as i64 {
            f.put(x, row, k.v(120.0 + 135.0 * s));
        }
        return f;
    }
    let mut f = Px::dark();
    if t < 0.85 {
        let s = smooth((t - 0.55) / 0.3);
        let left = s * WIDTH as f64;
        for x in 0..WIDTH {
            if x as f64 >= left {
                let tip = 1.0 - ((x as f64 - left) / 4.0).min(1.0) * 0.4;
                f.set(x as i64, row, k.v(255.0 * tip));
            }
        }
        return f;
    }
    // Sparks fizzle out at the port
    let s = (t - 0.85) / 0.35;
    let n = (t * FPS) as i64;
    for i in 0..3 {
        if h(n, i, 1) > s {
            let x = WIDTH as i64 - 1 - (h(n, i, 2) * 2.0) as i64;
            let y = row + (h(n, i, 3) * 3.0) as i64 - 1;
            f.put(x, y, k.v(255.0 * (1.0 - s)));
        }
    }
    f
}

fn surge_in(pic: &Px, t: f64, k: &Ctx) -> Px {
    let row = k.row as i64;
    let mut f = Px::dark();
    if t < 0.3 {
        // A spark races out of the port with a trail behind it
        let head = (WIDTH - 1) as f64 - t / 0.3 * 10.0;
        for x in 0..WIDTH as i64 {
            let behind = x as f64 - head;
            if behind >= 0.0 {
                let jitter = (h(x, (t * FPS) as i64, 4) * 3.0) as i64 - 1;
                let v = k.v(255.0 * (1.0 - behind / 6.0).max(0.25));
                f.put(x, row + if behind < 3.0 { jitter } else { 0 }, v);
            }
        }
        return f;
    }
    if t < 0.75 {
        // The charge floods up and down from the port's row, with a crackling edge
        let reach = smooth((t - 0.3) / 0.45) * (HEIGHT as f64 / 2.0 + 1.5 + (k.row - HEIGHT as f64 / 2.0).abs());
        let n = (t * FPS) as i64;
        for x in 0..WIDTH {
            for y in 0..HEIGHT {
                let d = (y as f64 - (k.row - 0.5)).abs();
                let edge = reach + (h(x as i64, n, 5) - 0.5) * 2.0;
                if d <= edge {
                    let front = ((edge - d) / 3.0).min(1.0);
                    f.0[x][y] = k.v(255.0 - 120.0 * front + 60.0 * h(x as i64, y as i64, n) * front);
                }
            }
        }
        return f;
    }
    // Settles from full charge into the picture
    let s = smooth((t - 0.75) / 0.4);
    for x in 0..WIDTH {
        for y in 0..HEIGHT {
            f.0[x][y] = k.v(170.0) * (1.0 - s) + pic.0[x][y] * s;
        }
    }
    f
}

// Pixel rain: the picture crumbles and falls away; it floats back up into place.

const GRAVITY: f64 = 70.0;

fn rain_out(pic: &Px, t: f64) -> Px {
    let mut f = Px::dark();
    for x in 0..WIDTH {
        for y in 0..HEIGHT {
            let v = pic.0[x][y];
            if v <= 0.0 {
                continue;
            }
            // Crumbles from the bottom up, a little at random
            let (xi, yi) = (x as i64, y as i64);
            let delay = (HEIGHT - 1 - y) as f64 / (HEIGHT - 1) as f64 * 0.7 + h(xi, yi, 7) * 0.3;
            let a = (t - delay).max(0.0);
            if a > 0.0 && a < 0.06 {
                // A little shake before it lets go
                f.put(xi + (h(xi, yi, 9) > 0.5) as i64, yi, v);
            } else {
                let fy = y as f64 + 0.5 * GRAVITY * a * a;
                let fx = x as f64 + (h(xi, yi, 8) - 0.5) * 1.6 * a;
                f.put(fx.round() as i64, fy.round() as i64, v);
            }
        }
    }
    f
}

fn rain_in(pic: &Px, t: f64) -> Px {
    let mut f = Px::dark();
    for x in 0..WIDTH {
        for y in 0..HEIGHT {
            let v = pic.0[x][y];
            if v <= 0.0 {
                continue;
            }
            // Floats up from below and slows to a stop in place; the top lands first
            let (xi, yi) = (x as i64, y as i64);
            let arrive = 0.45 + y as f64 / (HEIGHT - 1) as f64 * 0.55 + h(xi, yi, 10) * 0.19;
            let a = (arrive - t).max(0.0);
            let fy = y as f64 + 0.5 * GRAVITY * a * a;
            let fx = x as f64 + (h(xi, yi, 11) - 0.5) * 1.6 * a;
            f.put(fx.round() as i64, fy.round() as i64, v);
        }
    }
    f
}

// Ripple ring: a bright ring spreads from the port, wiping the picture away or showing it.

fn ring(pic: &Px, t: f64, secs: f64, reveal: bool, k: &Ctx) -> Px {
    let r = smooth(t / secs) * 19.0 - 1.0;
    let fade = 1.0 - smooth((t / secs - 0.75) / 0.2);
    let mut f = Px::dark();
    for x in 0..WIDTH {
        for y in 0..HEIGHT {
            let dx = x as f64 + 0.5 - PORT_X;
            let dy = y as f64 + 0.5 - k.row;
            // Rings a little taller than wide read better on the tall panel
            let d = dx.hypot(dy * 0.8);
            let mut v = 0.0;
            if (d < r) == reveal {
                // Near the front the picture is pushed along with the wave
                let push = (-(d - r).powi(2) / 4.0).exp() * 1.4 * if reveal { -1.0 } else { 1.0 };
                let n = dx.hypot(dy).max(0.01);
                let sx = (x as f64 + push * dx / n).round().clamp(0.0, (WIDTH - 1) as f64) as usize;
                let sy = (y as f64 + push * dy / n).round().clamp(0.0, (HEIGHT - 1) as f64) as usize;
                v = pic.0[sx][sy];
            }
            // A bright crest, and a fainter one behind it
            let crest = (-(d - r).powi(2) / 1.2).exp() + 0.35 * (-(d - (r - 4.0)).powi(2) / 1.5).exp();
            f.0[x][y] = v.max(k.v(230.0) * crest * fade);
        }
    }
    f
}

// Zipper: the two halves of the picture come apart below a zip pull and fall away to the sides; they
// slide back in and zip up.

fn zipper(pic: &Px, pull: f64, apart: f64, k: &Ctx) -> Px {
    let mut f = Px::dark();
    let mid = WIDTH / 2;
    for y in 0..HEIGHT {
        let s = ((y as f64 - pull).max(0.0) * 0.45 + apart).round() as i64;
        for x in 0..WIDTH {
            // The middle column is the teeth: even rows go with the left half, odd rows with the right
            let left = x < mid || (x == mid && y % 2 == 0);
            let nx = if left { x as i64 - s } else { x as i64 + s };
            f.put(nx, y as i64, pic.0[x][y]);
            if x == mid && s > 0 {
                // The teeth stay faintly lit where the picture is dark
                f.put(nx, y as i64, k.v(45.0));
            }
        }
    }
    if pull > -1.0 && pull < HEIGHT as f64 && apart < 0.5 {
        let p = pull.round() as i64;
        let m = mid as i64;
        for (dx, dy, v) in [(-1, 0, 255.0), (0, 0, 255.0), (1, 0, 255.0), (0, 1, 255.0), (0, 2, 200.0)] {
            f.set(m + dx, p + dy, k.v(v));
        }
    }
    f
}

fn zip_out(pic: &Px, t: f64, k: &Ctx) -> Px {
    if t < 0.9 {
        zipper(pic, HEIGHT as f64 + 1.0 - smooth(t / 0.9) * (HEIGHT as f64 + 3.0), 0.0, k)
    } else {
        let s = (t - 0.9) / 0.35;
        zipper(pic, -2.0, s * s * 8.0, k)
    }
}

fn zip_in(pic: &Px, t: f64, k: &Ctx) -> Px {
    if t < 0.3 {
        let s = 1.0 - t / 0.3;
        zipper(pic, -2.0, s * s * 8.0, k)
    } else {
        zipper(pic, -2.0 + smooth((t - 0.3) / 0.9) * (HEIGHT as f64 + 3.0), 0.0, k)
    }
}

// Glitch: the picture tears and fills with static until the signal cuts out; static clears into it.

fn glitched(pic: &Px, amount: f64, n: i64, k: &Ctx) -> Px {
    let mut f = Px::dark();
    let jump = if h(n, 0, 20) < amount * 0.6 { ((h(n, 0, 21) - 0.5) * 6.0).round() as i64 } else { 0 };
    let flicker = if h(n, 0, 22) < amount * 0.3 { 0.35 } else { 1.0 };
    for y in 0..HEIGHT as i64 {
        // Bands of rows torn sideways
        let band = y / 3;
        let shift = if h(n, band, 23) < amount * 0.55 { ((h(n, band, 24) - 0.5) * 8.0).round() as i64 } else { 0 };
        for x in 0..WIDTH as i64 {
            let sy = (y + jump).rem_euclid(HEIGHT as i64) as usize;
            let sx = (x - shift).rem_euclid(WIDTH as i64) as usize;
            let mut v = pic.0[sx][sy] * flicker;
            if h(n, x * 100 + y, 25) < amount * 0.18 {
                v = k.v(60.0 + 195.0 * h(n, x * 100 + y, 26));
            }
            f.0[x as usize][y as usize] = v;
        }
    }
    // Now and then a solid bright block
    if h(n, 1, 27) < amount * 0.5 {
        let bx = (h(n, 1, 28) * 7.0) as i64;
        let by = (h(n, 1, 29) * 30.0) as i64;
        let (bw, bh) = (2 + (h(n, 1, 30) * 4.0) as i64, 1 + (h(n, 1, 31) * 3.0) as i64);
        for x in bx..bx + bw {
            for y in by..by + bh {
                f.set(x, y, k.v(220.0));
            }
        }
    }
    f
}

fn glitch_out(pic: &Px, t: f64, k: &Ctx) -> Px {
    // The glitches hold for two frames, like a real signal
    let n = (t * FPS) as i64 / 2;
    if t < 0.75 {
        return glitched(pic, (t / 0.75).powf(0.7), n, k);
    }
    let mut f = Px::dark();
    let mid = HEIGHT as i64 / 2;
    if t < 0.82 {
        // The signal goes: a flash of a line, then a few last specks
        for x in 0..WIDTH as i64 {
            f.set(x, mid - 1, k.v(255.0));
            f.set(x, mid, k.v(140.0));
        }
    } else {
        let s = (t - 0.82) / 0.28;
        for i in 0..4 {
            if h(n, i, 40) > s {
                f.put((h(n, i, 41) * WIDTH as f64) as i64, (h(n, i, 42) * HEIGHT as f64) as i64, k.v(120.0));
            }
        }
    }
    f
}

fn glitch_in(pic: &Px, t: f64, k: &Ctx) -> Px {
    let n = (t * FPS) as i64 / 2 + 1000;
    if t < 0.25 {
        // Bursts of static while it finds the signal
        let mut f = Px::dark();
        if h(n, 0, 50) < 0.6 {
            for x in 0..WIDTH as i64 {
                for y in 0..HEIGHT as i64 {
                    if h(n, x * 100 + y, 51) < 0.25 {
                        f.set(x, y, k.v(40.0 + 160.0 * h(n, x * 100 + y, 52)));
                    }
                }
            }
        }
        return f;
    }
    glitched(pic, 1.0 - smooth((t - 0.25) / 0.8), n, k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> Picture {
        let mut p = Picture::zeros(HEIGHT, WIDTH);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                if (x + y) % 3 != 0 {
                    p.set(y, x, 160.0);
                }
            }
        }
        p
    }

    #[test]
    fn keys_round_trip() {
        for look in Look::ALL {
            assert_eq!(Look::from_key(look.key()), Some(look));
        }
        assert_eq!(Look::from_key("drain"), None);
    }

    #[test]
    fn every_look_ends_dark_going_out_and_on_the_picture_coming_in() {
        let pic = picture();
        for look in Look::ALL {
            // Just before the end of going out, nothing much is left
            let last = look.out_frame(&pic, look.out_seconds() - 0.01, 16.5, 160.0).unwrap();
            let lit = last.data.iter().filter(|v| **v >= 0.5).count();
            assert!(lit <= 4, "{look:?} still shows {lit} pixels at the end of going out");
            assert!(look.out_frame(&pic, look.out_seconds(), 16.5, 160.0).is_none());
            // Just before the end of coming in, it's the picture (give or take the last few pixels)
            let last = look.in_frame(&pic, look.in_seconds() - 0.01, 16.5, 160.0).unwrap();
            let off = last.data.iter().zip(&pic.data).filter(|(a, b)| (**a - **b).abs() > 20.0).count();
            assert!(off <= 6, "{look:?} is {off} pixels off the picture at the end of coming in");
            assert!(look.in_frame(&pic, look.in_seconds(), 16.5, 160.0).is_none());
        }
    }

    #[test]
    fn frames_stay_in_range_at_any_port_row_and_brightness() {
        let pic = picture();
        for look in Look::ALL {
            for row in [0.5, 5.5, 16.5, 33.5] {
                for bright in [1.0, 24.0, 160.0, 255.0] {
                    let mut t = 0.0;
                    while t < look.out_seconds().max(look.in_seconds()) {
                        for f in [look.out_frame(&pic, t, row, bright), look.in_frame(&pic, t, row, bright)]
                            .into_iter()
                            .flatten()
                        {
                            assert!(f.data.iter().all(|v| (0.0..=255.0).contains(v)));
                        }
                        t += 1.0 / 30.0;
                    }
                }
            }
        }
    }
}
