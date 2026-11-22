//! "Linked" effects rendered on the PC across keyboard + macropad.
//!
//! All positions are in key units (see [`crate::layout`]) and all times in
//! seconds, so effects are pure functions of (position, time) that are easy
//! to test.

use crate::layout::Point;
use std::f32::consts::TAU;

pub type Rgb = [u8; 3];

/// HSV (all 0..=1, hue wraps) to 8-bit RGB.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgb {
    let h = h.rem_euclid(1.0) * 6.0;
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to8 = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [to8(r), to8(g), to8(b)]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkedEffect {
    #[default]
    Wave,
    RainbowSweep,
    Ripple,
}

impl LinkedEffect {
    pub const ALL: [LinkedEffect; 3] = [Self::Wave, Self::RainbowSweep, Self::Ripple];
    pub fn name(self) -> &'static str {
        match self {
            Self::Wave => "Wave",
            Self::RainbowSweep => "Rainbow sweep",
            Self::Ripple => "Keypress ripple",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectParams {
    pub effect: LinkedEffect,
    /// 0.05..=2.0; roughly "cycles per second".
    pub speed: f32,
    /// Base color (0..=1). Not used by the rainbow sweep.
    pub hue: f32,
    pub saturation: f32,
    /// Overall brightness 0..=1 (the device's own brightness applies on top).
    pub brightness: f32,
    /// Sweep/wave direction: true = right to left.
    pub reverse: bool,
}

impl Default for EffectParams {
    fn default() -> Self {
        Self { effect: LinkedEffect::Wave, speed: 0.5, hue: 0.55, saturation: 1.0, brightness: 1.0, reverse: false }
    }
}

/// Wave length in key units.
pub const WAVE_LENGTH: f32 = 7.0;
/// Ripple ring thickness in key units.
pub const RIPPLE_WIDTH: f32 = 1.3;
/// Background level of the ripple effect (fraction of brightness).
pub const RIPPLE_BACKGROUND: f32 = 0.06;
pub const MAX_RIPPLES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ripple {
    pub center: Point,
    pub start: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Renderer {
    pub params: EffectParams,
    ripples: Vec<Ripple>,
}

impl Renderer {
    pub fn new(params: EffectParams) -> Self {
        Self { params, ripples: Vec::new() }
    }

    /// Ring expansion speed in key units per second.
    pub fn ripple_speed(&self) -> f32 {
        6.0 + 14.0 * self.params.speed
    }

    /// Seconds until a ripple has faded out.
    pub fn ripple_lifetime(&self) -> f32 {
        // long enough to cross most of the combined width
        (24.0 / self.ripple_speed()).clamp(0.6, 3.0)
    }

    pub fn add_ripple(&mut self, center: Point, now: f32) {
        if self.ripples.len() >= MAX_RIPPLES {
            self.ripples.remove(0);
        }
        self.ripples.push(Ripple { center, start: now });
    }

    pub fn ripple_count(&self) -> usize {
        self.ripples.len()
    }

    /// Drops faded ripples. Called by [`Renderer::render_all`].
    pub fn prune(&mut self, now: f32) {
        let life = self.ripple_lifetime();
        self.ripples.retain(|r| now - r.start < life);
    }

    /// Color of one LED at time `t`. `width` is the combined layout width.
    pub fn color_at(&self, p: Point, t: f32, width: f32) -> Rgb {
        let pr = &self.params;
        let dir = if pr.reverse { -1.0 } else { 1.0 };
        match pr.effect {
            LinkedEffect::Wave => {
                let phase = dir * p.x / WAVE_LENGTH - t * pr.speed;
                let wave = 0.5 + 0.5 * (TAU * phase).sin();
                hsv_to_rgb(pr.hue, pr.saturation, pr.brightness * (0.1 + 0.9 * wave))
            }
            LinkedEffect::RainbowSweep => {
                let w = if width > 0.0 { width } else { 1.0 };
                let hue = dir * p.x / w - t * pr.speed;
                hsv_to_rgb(hue, pr.saturation, pr.brightness)
            }
            LinkedEffect::Ripple => {
                let speed = self.ripple_speed();
                let life = self.ripple_lifetime();
                let mut level = RIPPLE_BACKGROUND;
                for r in &self.ripples {
                    let age = t - r.start;
                    if age < 0.0 || age >= life {
                        continue;
                    }
                    let ring = 1.0 - ((p.dist(r.center) - age * speed).abs() / RIPPLE_WIDTH);
                    let fade = 1.0 - age / life;
                    level = level.max(ring.max(0.0) * fade);
                }
                hsv_to_rgb(pr.hue, pr.saturation, pr.brightness * level)
            }
        }
    }

    /// Renders every LED in `points` at time `t`.
    pub fn render(&self, points: &[Point], t: f32, width: f32) -> Vec<Rgb> {
        points.iter().map(|&p| self.color_at(p, t, width)).collect()
    }

    /// Prunes old ripples, then renders.
    pub fn render_all(&mut self, points: &[Point], t: f32, width: f32) -> Vec<Rgb> {
        self.prune(t);
        self.render(points, t, width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    fn luma(c: Rgb) -> u32 {
        c.iter().map(|&v| v as u32).sum()
    }

    #[test]
    fn hsv_primaries() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), [255, 0, 0]);
        assert_eq!(hsv_to_rgb(1.0 / 3.0, 1.0, 1.0), [0, 255, 0]);
        assert_eq!(hsv_to_rgb(2.0 / 3.0, 1.0, 1.0), [0, 0, 255]);
        assert_eq!(hsv_to_rgb(1.0, 1.0, 1.0), [255, 0, 0]); // wraps
        assert_eq!(hsv_to_rgb(-1.0 / 3.0, 1.0, 1.0), [0, 0, 255]); // negative wraps
        assert_eq!(hsv_to_rgb(0.3, 0.0, 0.5), [128, 128, 128]);
        assert_eq!(hsv_to_rgb(0.7, 1.0, 0.0), [0, 0, 0]);
        assert_eq!(hsv_to_rgb(0.0, 2.0, 2.0), [255, 0, 0]); // clamps
    }

    #[test]
    fn wave_repeats_in_time_and_space() {
        let r = Renderer::new(EffectParams { speed: 0.5, ..Default::default() });
        let a = r.color_at(p(3.0, 1.0), 1.0, 22.0);
        // one period later (1 / speed seconds)
        assert_eq!(a, r.color_at(p(3.0, 1.0), 3.0, 22.0));
        // one wave length further along
        assert_eq!(a, r.color_at(p(3.0 + WAVE_LENGTH, 1.0), 1.0, 22.0));
        // half a wave length away is a different brightness
        assert_ne!(a, r.color_at(p(3.0 + WAVE_LENGTH / 4.0, 1.0), 1.0, 22.0));
        // never fully dark
        for i in 0..50 {
            assert!(luma(r.color_at(p(i as f32 * 0.37, 0.0), 0.0, 22.0)) > 0);
        }
    }

    #[test]
    fn rainbow_spans_the_whole_width() {
        let r = Renderer::new(EffectParams { effect: LinkedEffect::RainbowSweep, ..Default::default() });
        let w = 22.85;
        // both ends of the combined layout have the same hue (one full rainbow)
        assert_eq!(r.color_at(p(0.0, 0.0), 0.4, w), r.color_at(p(w, 0.0), 0.4, w));
        // the middle is the opposite color
        assert_ne!(r.color_at(p(0.0, 0.0), 0.4, w), r.color_at(p(w / 2.0, 0.0), 0.4, w));
        // it moves: one LED changes color over time and repeats after 1/speed s
        let c0 = r.color_at(p(5.0, 0.0), 0.0, w);
        assert_ne!(c0, r.color_at(p(5.0, 0.0), 0.5, w));
        assert_eq!(c0, r.color_at(p(5.0, 0.0), 2.0, w));
    }

    #[test]
    fn reverse_flips_direction() {
        let fwd = Renderer::new(EffectParams { effect: LinkedEffect::RainbowSweep, ..Default::default() });
        let rev =
            Renderer::new(EffectParams { effect: LinkedEffect::RainbowSweep, reverse: true, ..Default::default() });
        let w = 20.0;
        assert_eq!(fwd.color_at(p(5.0, 0.0), 0.0, w), rev.color_at(p(15.0, 0.0), 0.0, w));
    }

    #[test]
    fn ripple_ring_moves_outward_and_fades() {
        let mut r = Renderer::new(EffectParams { effect: LinkedEffect::Ripple, ..Default::default() });
        let w = 22.85;
        let center = p(10.0, 2.0);
        let bg = r.color_at(p(0.0, 0.0), 0.0, w);
        r.add_ripple(center, 0.0);
        assert_eq!(r.ripple_count(), 1);

        let t = 0.2;
        let radius = t * r.ripple_speed();
        let on_ring = r.color_at(p(center.x + radius, center.y), t, w);
        let inside = r.color_at(center, t, w);
        let far = r.color_at(p(center.x + radius + 3.0 * RIPPLE_WIDTH, center.y), t, w);
        assert!(luma(on_ring) > luma(inside));
        assert!(luma(on_ring) > luma(far));
        assert_eq!(far, bg);

        // later the ring is dimmer
        let t2 = r.ripple_lifetime() * 0.8;
        let on_ring_later = r.color_at(p(center.x + t2 * r.ripple_speed(), center.y), t2, w);
        assert!(luma(on_ring_later) < luma(on_ring));

        // after its lifetime it is gone
        let end = r.ripple_lifetime() + 0.01;
        let colors = r.render_all(&[center, p(1.0, 1.0)], end, w);
        assert_eq!(r.ripple_count(), 0);
        assert!(colors.iter().all(|&c| c == bg));
    }

    #[test]
    fn ripples_are_capped() {
        let mut r = Renderer::new(EffectParams { effect: LinkedEffect::Ripple, ..Default::default() });
        for i in 0..100 {
            r.add_ripple(p(i as f32, 0.0), 0.0);
        }
        assert_eq!(r.ripple_count(), MAX_RIPPLES);
    }

    #[test]
    fn zero_brightness_is_black() {
        for effect in LinkedEffect::ALL {
            let mut r = Renderer::new(EffectParams { effect, brightness: 0.0, ..Default::default() });
            r.add_ripple(p(1.0, 1.0), 0.0);
            let pts: Vec<Point> = (0..20).map(|i| p(i as f32, 1.0)).collect();
            assert!(r.render(&pts, 0.1, 20.0).iter().all(|&c| c == [0, 0, 0]), "{effect:?}");
        }
    }
}
