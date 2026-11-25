//! A software copy of the 44 built-in RGB effects in Framework's firmware
//! (QMK's RGB matrix effects, in Framework's order), used for the preview.
//!
//! Effects work in QMK's LED space: x 0..=224, y 0..=64, centre (112, 32).
//! For a single device each LED keeps its own position; for a joined
//! keyboard + side module the caller squeezes each device into its slice of
//! that space, exactly as the firmware does (see [`span_slices`]).
//! Random effects use their own random numbers, so they will not match the
//! keys pixel for pixel; every other effect follows the firmware's formulas.
//!
//! Only the window's keyboard picture uses this ([`crate::deck::Preview`], on the UI thread): the
//! real devices run these effects themselves. The formulas use QMK's 8-bit helpers (`sin8`,
//! `scale8`, `atan2_8`), re-created below, and QMK's timer arithmetic: an effect's "time" is a
//! byte that counts up with the milliseconds since start, faster at higher speed settings, and
//! wraps at 256. Colors are worked out as QMK HSV (all 0..=255) and converted to RGB at the end.

use crate::effects::{hsv_to_rgb, Rgb};

/// Number of built-in effects (ids 1..=44).
pub const BUILTIN_COUNT: u8 = 44;

/// Effects that react to key presses (typing heatmap and the reactive family).
/// The preview pretends someone is typing while one of these is shown.
pub fn reacts_to_typing(id: u8) -> bool {
    id == 31 || (33..=44).contains(&id)
}

/// One device's built-in effect settings, as the device's sliders set them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectCfg {
    /// 1..=44; anything else renders black.
    pub effect: u8,
    /// Effect color, saturation, brightness and speed, QMK's 0..=255 scales.
    pub hue: u8,
    pub sat: u8,
    pub val: u8,
    pub speed: u8,
}

/// Effect 13 (cycle left-right), the app's default look.
impl Default for EffectCfg {
    fn default() -> Self {
        Self { effect: 13, hue: 11, sat: 220, val: 255, speed: 128 }
    }
}

/// One LED as the effect sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimLed {
    pub x: u8,
    pub y: u8,
    /// "Alphas / mods" colors modifier keys differently.
    pub modifier: bool,
}

/// One key press: which LED, where (QMK space) and when (seconds on the preview clock).
#[derive(Debug, Clone, Copy)]
struct Hit {
    led: usize,
    x: i32,
    y: i32,
    t: f64,
}

/// Memory that some effects keep from frame to frame, like the firmware's per-effect buffers.
/// Thrown away when the effect or the LED count changes.
#[derive(Debug, Clone)]
struct Mem {
    /// The effect this memory belongs to.
    id: u8,
    /// The number of LEDs it was made for.
    n: usize,
    /// The last random step carried out (-1 = none yet).
    step: i64,
    /// Per-LED HSV for the random pixel effects (23, 24, 28, 29, 30).
    px: Vec<(i32, i32, i32)>,
    /// Per-LED heat 0..=255 for the typing heatmap (31).
    heat: Vec<i32>,
    /// Per-LED brightness of the digital rain (32).
    rain: Vec<i32>,
}

/// One running effect (one device, or two joined devices).
///
/// Keeps effect memory and the last 8 key presses (presses older than 3 seconds are dropped).
#[derive(Debug, Clone, Default)]
pub struct BuiltinSim {
    mem: Option<Mem>,
    hits: Vec<Hit>,
}

/// QMK's `sin8`: a sine wave over one byte, input 0..=255 (one full turn), output 1..=255 round 128.
fn sin8(x: i32) -> i32 {
    (128.0 + 127.0 * (std::f64::consts::TAU * (x & 255) as f64 / 256.0).sin()).round() as i32
}
/// QMK's `cos8`: like [`sin8`], a quarter turn ahead.
fn cos8(x: i32) -> i32 {
    (128.0 + 127.0 * (std::f64::consts::TAU * (x & 255) as f64 / 256.0).cos()).round() as i32
}
/// QMK scale8: a * (1 + b) / 256
fn sc8(a: i32, b: i32) -> i32 {
    (a * (1 + b)).div_euclid(256)
}
/// QMK's `atan2_8`: the angle of (dx, dy) as a byte, 0..=255 for one full turn.
fn atan8(dy: i32, dx: i32) -> i32 {
    ((dy as f64).atan2(dx as f64) / std::f64::consts::TAU * 256.0).round() as i32 & 255
}
/// A random number in 0..=1 from `seed`: the same seed always gives the same number.
fn rnd(seed: u64) -> f64 {
    // splitmix64: deterministic, good enough for raindrops
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) as f64 / u64::MAX as f64
}

/// QMK-style HSV (all 0..=255, hue wraps) to RGB.
pub fn qmk_hsv(h: i32, s: i32, v: i32) -> Rgb {
    hsv_to_rgb(h.rem_euclid(256) as f32 / 256.0, s.clamp(0, 255) as f32 / 255.0, v.clamp(0, 255) as f32 / 255.0)
}

impl BuiltinSim {
    /// A key press at LED `led` (position in the same space as the LEDs), at `t` seconds.
    pub fn press(&mut self, led: usize, x: u8, y: u8, t: f64) {
        self.hits.push(Hit { led, x: x as i32, y: y as i32, t });
        if self.hits.len() > 8 {
            self.hits.remove(0);
        }
    }

    /// Forgets effect memory and key presses (called when the user picks another effect).
    pub fn clear(&mut self) {
        self.mem = None;
        self.hits.clear();
    }

    /// Colors of every LED at `t` seconds.
    ///
    /// `t` must come from one steadily rising clock; random effects catch up on the steps they
    /// missed. Unknown effect ids give black.
    pub fn render(&mut self, c: &EffectCfg, leds: &[SimLed], t: f64) -> Vec<Rgb> {
        let (id, h0, s0, v0, sp) = (c.effect, c.hue as i32, c.sat as i32, c.val as i32, c.speed as i32);
        let n = leds.len();
        if !(1..=BUILTIN_COUNT).contains(&id) {
            return vec![[0, 0, 0]; n];
        }
        let ms = t * 1000.0;
        let (cx, cy) = (112, 32);
        // QMK: time = scale16by8(timer, speed / 4 + 1) wrapped to a byte; breathing uses speed / 8.
        // `step` is how many "random ticks" have passed; faster speeds tick more often.
        let time = ((ms * ((sp / 4) + 1) as f64 / 256.0) as i64 & 255) as i32;
        let btime = ((ms * ((sp / 8) + 1) as f64 / 256.0) as i64 & 255) as i32;
        let step = (ms / (60.0 + (255 - sp) as f64 * 2.0)) as i64;
        self.hits.retain(|h| t - h.t < 3.0);

        // New effect or different LED count: start the memory again.
        let fresh = match &self.mem {
            Some(m) => m.id != id || m.n != n,
            None => true,
        };
        if fresh {
            self.mem = Some(Mem { id, n, step: -1, px: vec![(h0, s0, 0); n], heat: vec![0; n], rain: vec![0; n] });
        }
        let m = self.mem.as_mut().unwrap();
        // random effects advance in steps; catch up (at most 64 steps a frame)
        if m.step < 0 || step - m.step > 64 {
            m.step = step - 1;
        }
        while m.step < step && n > 0 {
            m.step += 1;
            // Random numbers depend only on the step, the effect and `k`, so they are repeatable.
            let r = |k: u64| rnd(m.step as u64 * 31 + k + id as u64 * 7919);
            let i = (r(1) * n as f64) as usize % n;
            match id {
                // Raindrops: one random LED takes a hue near the chosen one.
                23 => m.px[i] = (h0 + ((r(2) - 0.5) * 64.0) as i32, s0, v0),
                // Jellybean raindrops: any hue, fairly saturated.
                24 => m.px[i] = ((r(2) * 256.0) as i32, (r(3) * 128.0) as i32 + 127, v0),
                // Pixel rain: a random LED turns a random color or off.
                28 => m.px[i] = if r(4) < 0.5 { ((r(2) * 256.0) as i32, s0, v0) } else { (0, 0, 0) },
                29 => {
                    // Pixel flow: everything moves one LED along, a new color enters at the start.
                    m.px.rotate_right(1);
                    m.px[0] = if r(5) < 0.5 { ((r(2) * 256.0) as i32, s0, v0) } else { (0, 0, 0) };
                }
                30 => {
                    // Pixel fractal (simplified): LEDs flicker on and off at random.
                    for k in 0..n {
                        if rnd(m.step as u64 * 13 + k as u64) < 0.06 {
                            m.px[k] = if m.px[k].2 > 0 { (0, 0, 0) } else { (h0, s0, v0) };
                        }
                    }
                }
                32 => {
                    // Digital rain: everything dims, then a new drop lights a column above a random LED.
                    for v in m.rain.iter_mut() {
                        *v = (*v - 40).max(0);
                    }
                    let drop = leds[i];
                    for (rain, l) in m.rain.iter_mut().zip(leds) {
                        if (l.x as i32 - drop.x as i32).abs() < 8 && l.y <= drop.y {
                            *rain = 255 - ((drop.y - l.y) as i32 * 4).min(200);
                        }
                    }
                }
                31 => {
                    // Typing heatmap: heat cools down a little every step.
                    for v in m.heat.iter_mut() {
                        *v = (*v - 6).max(0);
                    }
                }
                _ => {}
            }
        }
        if id == 31 {
            // presses warm up the keys around them
            for h in self.hits.iter_mut() {
                if h.t.is_nan() {
                    continue;
                }
                for (k, l) in leds.iter().enumerate() {
                    let d = ((l.x as i32 - h.x).pow(2) as f64 + (l.y as i32 - h.y).pow(2) as f64).sqrt();
                    if d < 26.0 {
                        m.heat[k] = (m.heat[k] + if d < 4.0 { 40 } else { 12 }).min(255);
                    }
                }
                h.t = f64::NAN; // counted; NaN also drops it next frame
            }
        }
        let hits: Vec<(usize, i32, i32, i32)> = self
            .hits
            .iter()
            .filter(|h| !h.t.is_nan())
            // Press age in QMK "ticks" (faster at higher speed), used by the reactive effects.
            .map(|h| (h.led, h.x, h.y, ((t - h.t) * 1000.0 * (sp + 1) as f64 / 256.0) as i32))
            .collect();
        // The single-press effects only look at the latest press.
        let last: Vec<_> = hits.last().copied().into_iter().collect();

        // Per LED: start from the chosen color and let the effect change hue, saturation or value.
        let mut out = Vec::with_capacity(n);
        for (k, l) in leds.iter().enumerate() {
            let (x, y) = (l.x as i32, l.y as i32);
            let (dx, dy) = (x - cx, y - cy);
            // Distance from the centre (112, 32), used by the spiral and out-in effects.
            let dist = ((dx * dx + dy * dy) as f64).sqrt().round() as i32;
            let (mut h, mut s, mut v) = (h0, s0, v0);
            match id {
                1 => {}
                2 => {
                    if l.modifier {
                        h = h0 + sp
                    }
                }
                // 3..=22 follow QMK's effect files formula by formula: gradients, bands, pinwheels, spirals,
                // cycles and beacons, built from position, angle, distance and the time byte.
                3 => h = h0 + sc8(64, sp) * (y >> 4),
                4 => h = h0 + ((sc8(64, sp) * x) >> 5),
                // 5 breathing: brightness follows a sine. 6..=7 bands: a vertical stripe at x = time moves
                // across, fading saturation or brightness 8 per unit away. 8..=11: the same as pinwheels and
                // spirals, using the angle (and distance) from the centre instead of x.
                5 => v = sc8((sin8(btime) - 128).abs() * 2, v0),
                6 => s = sc8((s0 - (sc8(x, 228) + 28 - time).abs() * 8).max(0), s0),
                7 => v = sc8((v0 - (sc8(x, 228) + 28 - time).abs() * 8).max(0), v0),
                8 => s = sc8((s0 - time - atan8(dy, dx) * 3) & 255, s0),
                9 => v = sc8((v0 - time - atan8(dy, dx) * 3) & 255, v0),
                10 => s = sc8((s0 + dist - time - atan8(dy, dx)) & 255, s0),
                11 => v = sc8((v0 + dist - time - atan8(dy, dx)) & 255, v0),
                // 12..=14: the hue cycles over time, everywhere at once, across x, or down y.
                12 => h = time,
                13 => h = x - time,
                14 => h = y - time,
                15 => h = h0 + (y - cy).abs() + (x - time),
                16 => h = h0 + 3 * dist / 2 + time,
                17 => {
                    // Distance from two centres, a quarter of the width either side of the middle.
                    let ddx = cx / 2 - dx.abs();
                    h = h0 + 3 * ((ddx * ddx + dy * dy) as f64).sqrt().round() as i32 + time
                }
                18 => h = h0 + atan8(dy, dx) + time,
                19 => h = h0 + dist - time - atan8(dy, dx),
                // 20..=22 beacons: a hue gradient whose direction turns with time, like a rotating light.
                20 => h = h0 + (dy * (cos8(time) - 128) + dx * (sin8(time) - 128)).div_euclid(128),
                21 => h = h0 + (dy * 2 * (cos8(time) - 128) + dx * 2 * (sin8(time) - 128)).div_euclid(128),
                22 => h = h0 + (dy * 3 * (cos8(time) - 128) + (56 - dx.abs()) * 3 * (sin8(time) - 128)).div_euclid(128),
                // The random effects show what their steps above left in memory.
                23 | 24 | 28 | 29 | 30 => (h, s, v) = m.px[k],
                // 25..=27: hue breathing, pendulum and wave: small hue swings round the chosen color.
                25 => h = h0 + sc8((sin8(btime) - 128).abs(), 12),
                26 => h = h0 + sc8((((sin8(btime) + x - 128) & 255).abs() * 2) & 255, 12),
                27 => h = h0 + sc8((x - time) & 255, 24),
                31 => {
                    // Cold is blue (170), hot is red (0); keys never pressed stay off.
                    h = 170 - sc8(m.heat[k], 170);
                    s = 255;
                    v = if m.heat[k] > 0 { v0 } else { 0 };
                }
                32 => {
                    h = 85;
                    s = 255;
                    v = sc8(m.rain[k], v0);
                }
                33 => {
                    v = 0;
                    // Reactive (simple): only the pressed key, fading as the press ages.
                    for &(i, _, _, tick) in &last {
                        if i == k {
                            v = sc8(255 - tick.min(255), v0);
                        }
                    }
                }
                34 => {
                    // 34 Reactive: the pressed key's hue is shifted, settling back as the press ages.
                    for &(i, _, _, tick) in &last {
                        if i == k {
                            h = h0 + (130 - tick.min(255)).max(0);
                        }
                    }
                }
                _ => {
                    // 35..=44: splash family. Background off, light spreads from presses.
                    // For each press, `e` is how far that press's light has gone at this LED (0 = bright,
                    // 255 = off): wide uses distance, cross the distance along a row or column, nexus lights
                    // the row and column near the press, splash a ring growing with age. Presses add up.
                    // column near the press, splash a ring growing with age. Several presses add up.
                    let multi = matches!(id, 36 | 38 | 40 | 42 | 44);
                    v = 0;
                    for &(_, hx, hy, tick) in if multi { &hits } else { &last } {
                        let (ex, ey) = (x - hx, y - hy);
                        let dd = ((ex * ex + ey * ey) as f64).sqrt().round() as i32;
                        let mut e = match id {
                            35 | 36 => tick + dd * 5,
                            37 | 38 => tick + dd + (ex.abs() * 16).min(255).min((ey.abs() * 16).min(255)),
                            39 | 40 => {
                                let mut e = tick - dd;
                                if e < 0 || dd > 72 || (ex.abs() > 8 && ey.abs() > 8) {
                                    e = 255;
                                }
                                h = h0 + ey / 4;
                                e
                            }
                            _ => {
                                let mut e = tick - dd;
                                if e < 0 {
                                    e = 255;
                                }
                                if id <= 42 {
                                    h = h0 + e.min(255);
                                }
                                e
                            }
                        };
                        e = e.min(255);
                        v = (v + 255 - e).min(255);
                    }
                    // Scale the combined light by the chosen brightness.
                    v = sc8(v, v0);
                }
            }
            out.push(qmk_hsv(h, s, v));
        }
        out
    }
}

/// Where each device sits on the joined 0..=224 surface, left to right, in
/// proportion to its physical width. Returns (keyboard, side module) slices
/// as (x_start, x_end); the firmware squeezes each device's LEDs into its slice.
/// Widths and gap are in any one unit (the callers use key units).
pub fn span_slices(kb_width: f32, module_width: f32, gap: f32, module_left: bool) -> ((u8, u8), (u8, u8)) {
    let total = kb_width + gap + module_width;
    let to = |u: f32| (u / total * 224.0).round().clamp(0.0, 224.0) as u8;
    if module_left {
        ((to(module_width + gap), 224), (0, to(module_width)))
    } else {
        ((0, to(kb_width)), (to(kb_width + gap), 224))
    }
}

/// A device LED x position moved into its slice, the same way the firmware does it.
pub fn squeeze_x(x: u8, slice: (u8, u8)) -> u8 {
    // Linear map of 0..=224 onto slice.0..=slice.1, in integers like the firmware. Panics in debug
    // builds if slice.1 < slice.0, which `span_slices` never produces.
    (slice.0 as u16 + x as u16 * (slice.1 - slice.0) as u16 / 224) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` LEDs spread evenly along the middle row (n must be at least 2).
    fn row(n: usize) -> Vec<SimLed> {
        (0..n).map(|i| SimLed { x: (i * 224 / (n - 1)) as u8, y: 32, modifier: false }).collect()
    }

    /// The hue of an RGB color in degrees (0 for greys).
    fn hue_of(c: Rgb) -> f32 {
        let (r, g, b) = (c[0] as f32, c[1] as f32, c[2] as f32);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        if max == min {
            return 0.0;
        }
        let h = if max == r {
            (g - b) / (max - min)
        } else if max == g {
            2.0 + (b - r) / (max - min)
        } else {
            4.0 + (r - g) / (max - min)
        };
        (h * 60.0).rem_euclid(360.0)
    }

    /// Solid color gives the chosen color on every LED.
    #[test]
    fn solid_color_is_the_chosen_color_everywhere() {
        let mut sim = BuiltinSim::default();
        let cfg = EffectCfg { effect: 1, hue: 0, sat: 255, val: 255, speed: 0 };
        let out = sim.render(&cfg, &row(10), 1.0);
        assert!(out.iter().all(|&c| c == [255, 0, 0]));
    }

    /// Ids outside 1..=44 render black.
    #[test]
    fn unknown_effects_are_black() {
        let mut sim = BuiltinSim::default();
        for id in [0, 45, 46, 200] {
            let cfg = EffectCfg { effect: id, ..Default::default() };
            assert!(sim.render(&cfg, &row(5), 1.0).iter().all(|&c| c == [0, 0, 0]));
        }
    }

    /// Cycle all: every LED the same color, changing with time.
    #[test]
    fn cycle_all_is_one_color_and_changes_over_time() {
        let mut sim = BuiltinSim::default();
        let cfg = EffectCfg { effect: 12, ..Default::default() };
        let a = sim.render(&cfg, &row(8), 1.0);
        assert!(a.iter().all(|&c| c == a[0]));
        let b = sim.render(&cfg, &row(8), 2.0);
        assert_ne!(a[0], b[0]);
    }

    /// Cycle left-right: the hue steps evenly from LED to LED.
    #[test]
    fn cycle_left_right_flows_across_x() {
        let mut sim = BuiltinSim::default();
        let cfg = EffectCfg { effect: 13, ..Default::default() };
        let out = sim.render(&cfg, &row(9), 0.0);
        // hue = x - time: neighbouring LEDs differ by a small, steady step
        let hues: Vec<f32> = out.iter().map(|&c| hue_of(c)).collect();
        for w in hues.windows(2) {
            let d = (w[1] - w[0]).rem_euclid(360.0);
            assert!(d > 30.0 && d < 50.0, "{hues:?}");
        }
    }

    /// Breathing never exceeds the chosen brightness and does vary.
    #[test]
    fn breathing_stays_in_range_and_moves() {
        let mut sim = BuiltinSim::default();
        let cfg = EffectCfg { effect: 5, hue: 0, sat: 255, val: 200, speed: 128 };
        let mut seen = std::collections::BTreeSet::new();
        for i in 0..40 {
            let c = sim.render(&cfg, &row(2), i as f64 * 0.1)[0];
            assert!(c[0] <= 200);
            seen.insert(c[0]);
        }
        assert!(seen.len() > 5);
    }

    /// Reactive simple lights only the pressed key, then fades.
    #[test]
    fn reactive_simple_lights_only_the_pressed_key() {
        let mut sim = BuiltinSim::default();
        let cfg = EffectCfg { effect: 33, hue: 0, sat: 255, val: 255, speed: 128 };
        let leds = row(6);
        assert!(sim.render(&cfg, &leds, 1.0).iter().all(|&c| c == [0, 0, 0]));
        sim.press(2, leds[2].x, leds[2].y, 1.0);
        let out = sim.render(&cfg, &leds, 1.01);
        assert!(out[2][0] > 200);
        assert_eq!(out[0], [0, 0, 0]);
        // it fades
        let later = sim.render(&cfg, &leds, 1.5);
        assert!(later[2][0] < out[2][0]);
    }

    /// Solid splash light reaches further as time passes.
    #[test]
    fn splash_spreads_outwards() {
        let mut sim = BuiltinSim::default();
        let cfg = EffectCfg { effect: 43, hue: 0, sat: 255, val: 255, speed: 128 };
        let leds = row(15);
        sim.press(0, leds[0].x, leds[0].y, 0.0);
        let near = sim.render(&cfg, &leds, 0.1);
        let lit_near = near.iter().position(|c| c[0] > 0);
        let far = sim.render(&cfg, &leds, 0.4);
        let lit_far = far.iter().rposition(|c| c[0] > 0);
        assert!(lit_near.is_some() && lit_far.is_some());
        assert!(lit_far.unwrap() > lit_near.unwrap());
    }

    /// The heatmap lights a pressed key and leaves far keys off.
    #[test]
    fn typing_heatmap_warms_pressed_keys() {
        let mut sim = BuiltinSim::default();
        let cfg = EffectCfg { effect: 31, ..Default::default() };
        let leds = row(10);
        sim.press(5, leds[5].x, leds[5].y, 1.0);
        let out = sim.render(&cfg, &leds, 1.0);
        assert_ne!(out[5], [0, 0, 0]);
        assert_eq!(out[0], [0, 0, 0]);
    }

    /// All 44 effects render the right number of colors for many frames.
    #[test]
    fn every_effect_renders_without_panicking() {
        let leds = row(30);
        for id in 1..=BUILTIN_COUNT {
            let mut sim = BuiltinSim::default();
            let cfg = EffectCfg { effect: id, ..Default::default() };
            sim.press(3, leds[3].x, leds[3].y, 0.5);
            for i in 0..20 {
                let out = sim.render(&cfg, &leds, i as f64 * 0.07);
                assert_eq!(out.len(), 30, "effect {id}");
            }
        }
    }

    /// Slices touch the ends, keep a gap and swap with the side; squeezing maps the ends.
    #[test]
    fn joined_slices_follow_the_side() {
        let (kb, md) = span_slices(18.25, 3.89, 0.3, false);
        assert_eq!(kb.0, 0);
        assert_eq!(md.1, 224);
        assert!(kb.1 < md.0, "gap between {kb:?} and {md:?}");
        let (kb, md) = span_slices(18.25, 3.89, 0.3, true);
        assert_eq!(md.0, 0);
        assert_eq!(kb.1, 224);
        assert!(md.1 < kb.0);
        assert_eq!(squeeze_x(0, kb), kb.0);
        assert_eq!(squeeze_x(224, kb), kb.1);
    }

    /// A sweep doesn't jump in hue across the seam between joined devices.
    #[test]
    fn joined_sweep_continues_across_the_seam() {
        // keyboard right edge and macropad left edge get neighbouring hues
        let (kb, md) = span_slices(18.25, 3.89, 0.3, false);
        let leds = [
            SimLed { x: squeeze_x(224, kb), y: 32, modifier: false },
            SimLed { x: squeeze_x(0, md), y: 32, modifier: false },
        ];
        let mut sim = BuiltinSim::default();
        let out = sim.render(&EffectCfg { effect: 13, ..Default::default() }, &leds, 0.0);
        let d = (hue_of(out[1]) - hue_of(out[0])).rem_euclid(360.0);
        assert!(d < 15.0, "hue jump at the seam: {d}");
    }
}
