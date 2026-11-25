//! Audio reactive light shows: turn the loudness of each frequency band (bass to treble) into
//! colors for every LED across the keyboard and side module, drawn as one surface.
//!
//! The sound itself is captured and measured by White Rabbit's `wr-music` code in the app; this
//! module only gets band levels (0..1) and is the same on every platform.
//!
//! Who calls it: the lighting engine ([`crate::engine`], thread `hid-engine`) about 30 times a
//! second while the audio show is on. Each frame it calls [`AudioShow::update`] with the latest
//! band levels from [`crate::audio_src::Listener`] and the time step, passes on key presses with
//! [`AudioShow::press`], then [`AudioShow::render`]s every LED of both devices from their positions
//! on the combined layout (key units, y down). The same code draws the previews in the window.
//!
//! State kept between frames: smoothed band levels (fast up, slow down), overall loudness and a
//! falling peak, a running bass average for beat detection, the beat flash and hue, bass rings
//! and key presses still showing. All times are in seconds of the show's own clock (sum of `dt`).

use crate::effects::{hsv_to_rgb, Rgb};
use crate::layout::Point;

/// The different shows. Each has a name and a one-line explanation for the Audio page, and a
/// short key for the settings file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioLook {
    #[default]
    Spectrum,
    CenterOut,
    Meter,
    BeatPulse,
    BassRipples,
    ColorWash,
    Strobe,
    Typing,
}

impl AudioLook {
    /// Every show, in the order the Audio page lists them.
    pub const ALL: [AudioLook; 8] = [
        AudioLook::Spectrum,
        AudioLook::CenterOut,
        AudioLook::Meter,
        AudioLook::BeatPulse,
        AudioLook::BassRipples,
        AudioLook::ColorWash,
        AudioLook::Strobe,
        AudioLook::Typing,
    ];

    /// Name shown in the window.
    pub fn name(self) -> &'static str {
        match self {
            AudioLook::Spectrum => "Spectrum bars",
            AudioLook::CenterOut => "Centre out",
            AudioLook::Meter => "Level meter",
            AudioLook::BeatPulse => "Beat pulse",
            AudioLook::BassRipples => "Bass ripples",
            AudioLook::ColorWash => "Color wash",
            AudioLook::Strobe => "Strobe",
            AudioLook::Typing => "Typing",
        }
    }

    /// One-sentence explanation shown under the name.
    pub fn about(self) -> &'static str {
        match self {
            AudioLook::Spectrum => "Bars rise from the bottom row: bass on the left, treble on the right of the macropad.",
            AudioLook::CenterOut => "Bass in the middle of the keyboard, treble at both ends.",
            AudioLook::Meter => "The whole surface fills from the bottom row up with the loudness, with a peak mark that falls back slowly.",
            AudioLook::BeatPulse => "Everything flashes on each beat, and the color moves on with every beat.",
            AudioLook::BassRipples => "Each bass hit sends a ring of color out across the keys.",
            AudioLook::ColorWash => "Slow flowing colors that brighten with the music and shift with the treble.",
            AudioLook::Strobe => "White flashes on the strongest beats.",
            AudioLook::Typing => "Only the keys you press light up, in a color the music picks: bass makes them red, treble blue. Louder music makes them bigger.",
        }
    }

    /// Stable word for the settings file (never change these: saved settings use them).
    pub fn key(self) -> &'static str {
        match self {
            AudioLook::Spectrum => "spectrum",
            AudioLook::CenterOut => "center",
            AudioLook::Meter => "meter",
            AudioLook::BeatPulse => "pulse",
            AudioLook::BassRipples => "ripples",
            AudioLook::ColorWash => "wash",
            AudioLook::Strobe => "strobe",
            AudioLook::Typing => "typing",
        }
    }

    /// The show for a settings word; None for an unknown word.
    pub fn from_key(k: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|l| l.key() == k)
    }
}

/// The colors a show is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AudioColors {
    /// What suits the show: green, yellow and red for the bars and the meter, rainbow otherwise.
    #[default]
    Auto,
    /// Round the color wheel (red to purple; the last fifth is left out so the ends differ).
    Rainbow,
    /// Green, then yellow, then red at the top, like a sound level meter.
    Meter,
    /// Red, orange and yellow.
    Fire,
    /// Deep blue to cyan.
    Ocean,
    /// One color: hue and saturation, 0..1.
    One(f32, f32),
}

impl AudioColors {
    /// Settings text: a word, or "one H S" with hue and saturation as 0..=255 whole numbers.
    pub fn key(self) -> String {
        match self {
            AudioColors::Auto => "auto".into(),
            AudioColors::Rainbow => "rainbow".into(),
            AudioColors::Meter => "meter".into(),
            AudioColors::Fire => "fire".into(),
            AudioColors::Ocean => "ocean".into(),
            AudioColors::One(h, s) => {
                format!("one {} {}", (h * 255.0).round(), (s * 255.0).round())
            }
        }
    }

    /// Parses [`AudioColors::key`]'s text; out-of-range numbers are clamped, junk gives None.
    pub fn from_key(k: &str) -> Option<Self> {
        let p: Vec<&str> = k.split_whitespace().collect();
        Some(match p.first().copied()? {
            "auto" => AudioColors::Auto,
            "rainbow" => AudioColors::Rainbow,
            "meter" => AudioColors::Meter,
            "fire" => AudioColors::Fire,
            "ocean" => AudioColors::Ocean,
            "one" => {
                let h: f32 = p.get(1)?.parse().ok()?;
                let s: f32 = p.get(2)?.parse().ok()?;
                AudioColors::One(h.clamp(0.0, 255.0) / 255.0, s.clamp(0.0, 255.0) / 255.0)
            }
            _ => return None,
        })
    }

    /// The colors `look` really uses.
    fn resolve(self, look: AudioLook) -> Self {
        match (self, look) {
            // Shows with bars use level-meter colors; the others use the rainbow.
            (AudioColors::Auto, AudioLook::Spectrum | AudioLook::CenterOut | AudioLook::Meter) => AudioColors::Meter,
            (AudioColors::Auto, _) => AudioColors::Rainbow,
            (c, _) => c,
        }
    }

    /// Whether these colors go up the keyboard (by height) rather than across it.
    fn by_height(self) -> bool {
        matches!(self, AudioColors::Meter | AudioColors::Fire)
    }

    /// Hue and saturation at `t` (0..1) along the palette.
    fn at(self, t: f32) -> (f32, f32) {
        let t = t.clamp(0.0, 1.0);
        match self {
            AudioColors::Auto | AudioColors::Rainbow => (t * 0.8, 1.0),
            // Green up to 55%, yellow up to 80%, then red, like a studio level meter.
            AudioColors::Meter => (
                if t < 0.55 {
                    0.33
                } else if t < 0.8 {
                    0.14
                } else {
                    0.0
                },
                1.0,
            ),
            AudioColors::Fire => (t * 0.14, 1.0),
            AudioColors::Ocean => (0.66 - t * 0.17, 1.0),
            AudioColors::One(h, s) => (h, s),
        }
    }

    /// The palette color at `t` (0..1) at brightness `v` (0..1).
    fn rgb(self, t: f32, v: f32) -> Rgb {
        let (h, s) = self.at(t);
        hsv_to_rgb(h, s, v)
    }
}

/// How key presses show on top of an audio show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyReact {
    #[default]
    Off,
    /// The pressed key flashes white and fades.
    Glow,
    /// A ring spreads out from the key.
    Ripple,
    /// A burst of light spreads out from the key.
    Splash,
}

impl KeyReact {
    /// Every style, in the order the Audio page lists them.
    pub const ALL: [KeyReact; 4] = [KeyReact::Off, KeyReact::Glow, KeyReact::Ripple, KeyReact::Splash];

    /// Stable word for the settings file.
    pub fn key(self) -> &'static str {
        match self {
            KeyReact::Off => "off",
            KeyReact::Glow => "glow",
            KeyReact::Ripple => "ripple",
            KeyReact::Splash => "splash",
        }
    }

    /// The style for a settings word; None for an unknown word.
    pub fn from_key(k: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|r| r.key() == k)
    }

    /// How long a press shows, in seconds.
    fn life(self) -> f32 {
        match self {
            KeyReact::Off => 0.0,
            KeyReact::Glow => 0.5,
            KeyReact::Ripple => 0.9,
            KeyReact::Splash => 0.7,
        }
    }

    /// How strongly a press `age` seconds old shows at `d` key units from the key (0..1).
    fn weight(self, d: f32, age: f32) -> f32 {
        let fade = (1.0 - age / self.life()).max(0.0);
        match self {
            KeyReact::Off => 0.0,
            // Glow: a spot 0.7 units across. Ripple: a ring 0.9 wide moving out at 9 units/s.
            // Splash: a disc that grows by 6 units/s and fades faster (fade squared).
            KeyReact::Glow => (1.0 - d / 0.7).clamp(0.0, 1.0) * fade,
            KeyReact::Ripple => (1.0 - (d - age * 9.0).abs() / 0.9).max(0.0) * fade,
            KeyReact::Splash => (1.0 - d / (0.6 + age * 6.0)).clamp(0.0, 1.0) * fade * fade,
        }
    }
}

/// Bands the shows use (bass first).
/// Whatever number of bands the listener gives is resampled to this many.
pub const BANDS: usize = 16;

/// A ring of color sent out by a bass beat (BassRipples), alive for 1.6 s.
struct Ring {
    /// Show time when it started.
    born: f32,
    /// Palette position (0..1) it is drawn in.
    hue: f32,
}

/// A key press still showing: where, when, its color (None = white) and how big.
struct Press {
    at: Point,
    born: f32,
    color: Option<Rgb>,
    size: f32,
}

/// The show's state between frames: smoothed levels, beats, rings.
///
/// Created when the audio show starts; the engine changes `look`, `colors` and `keys` in place
/// when the user changes them, without losing the rest.
pub struct AudioShow {
    /// Which show is drawn.
    pub look: AudioLook,
    /// The palette.
    pub colors: AudioColors,
    /// How key presses show on top.
    pub keys: KeyReact,
    /// Key presses still showing.
    presses: Vec<Press>,
    /// Where the music's weight is, from bass (0) to treble (1), smoothed.
    centroid: f32,
    /// Smoothed level of each band, 0..1.
    levels: [f32; BANDS],
    /// Smoothed average of all bands, 0..1.
    loudness: f32,
    /// Highest recent loudness, falling back at 0.4 per second (the meter's peak mark).
    peak: f32,
    /// Slow running average of the bass, the baseline a beat must rise above.
    bass_avg: f32,
    /// Show time of the last beat (starts far in the past so the first one isn't blocked).
    last_beat: f32,
    /// Beat flash, 1 on a beat, dying away (about 1/e every 1/6 s).
    flash: f32,
    /// Palette position that moves on by 0.13 with every beat.
    hue: f32,
    /// Live bass rings.
    rings: Vec<Ring>,
    /// The show's clock, in seconds (the sum of every `dt`).
    t: f32,
}

impl AudioShow {
    /// A silent show with automatic colors and no key reactions.
    pub fn new(look: AudioLook) -> Self {
        Self {
            look,
            colors: AudioColors::Auto,
            keys: KeyReact::Off,
            presses: Vec::new(),
            centroid: 0.0,
            levels: [0.0; BANDS],
            loudness: 0.0,
            peak: 0.0,
            bass_avg: 0.0,
            last_beat: -10.0,
            flash: 0.0,
            hue: 0.0,
            rings: Vec::new(),
            t: 0.0,
        }
    }

    /// Feeds the latest band levels (0..1, any number of bands, bass first). Returns true on a beat.
    pub fn update(&mut self, bands: &[f32], dt: f32) -> bool {
        self.t += dt;
        let n = bands.len().max(1);
        for (i, l) in self.levels.iter_mut().enumerate() {
            // Pick the input band at the same relative position (nearest below, no averaging).
            let src = bands.get(i * n / BANDS).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            // fast up, slower down, so bars don't flicker
            // Rising: move 60% of the way each frame. Falling: close `dt x 6` of the gap per frame, a
            // time constant of about 1/6 s whatever the frame rate.
            let k = if src > *l { 0.6 } else { (dt * 6.0).min(1.0) };
            *l += (src - *l) * k;
        }
        let total: f32 = self.levels.iter().sum();
        if total > 0.05 {
            // The level-weighted average band number, as a 0..1 fraction: low = bass-heavy music.
            // Skipped in near silence, where it would jump around.
            let c = self.levels.iter().enumerate().map(|(i, l)| i as f32 * l).sum::<f32>() / total / (BANDS - 1) as f32;
            self.centroid += (c - self.centroid) * (dt * 8.0).min(1.0);
        }
        let loud = bands.iter().sum::<f32>() / n as f32;
        self.loudness += (loud - self.loudness) * (dt * 10.0).min(1.0);
        self.peak = (self.peak - dt * 0.4).max(self.loudness);

        // Beat: the bottom sixth of the bands jumps 35% (plus a little) above its running average,
        // is loud enough in itself, and the last beat was over 0.18 s ago (at most ~5 beats a second).
        let bass = bands.iter().take((n / 6).max(1)).sum::<f32>() / (n / 6).max(1) as f32;
        let beat = bass > self.bass_avg * 1.35 + 0.05 && bass > 0.25 && self.t - self.last_beat > 0.18;
        self.bass_avg += (bass - self.bass_avg) * (dt * 2.5).min(1.0);
        if beat {
            self.last_beat = self.t;
            self.flash = 1.0;
            self.hue = (self.hue + 0.13).fract();
            self.rings.push(Ring { born: self.t, hue: self.hue });
        }
        // Exponential decay that doesn't depend on the frame rate.
        self.flash *= (-dt * 6.0).exp();
        let t = self.t;
        self.rings.retain(|r| t - r.born < 1.6);
        let life = self.key_style().life();
        self.presses.retain(|p| t - p.born < life);
        beat
    }

    /// Colors for LEDs at `points` on a surface `width` x `height` (key units, y down).
    pub fn render(&self, points: &[Point], width: f32, height: f32) -> Vec<Rgb> {
        points.iter().map(|p| self.with_presses(*p, self.color(*p, width, height))).collect()
    }

    /// How key presses show: the chosen style, or a splash for the Typing show.
    fn key_style(&self) -> KeyReact {
        match (self.look, self.keys) {
            (AudioLook::Typing, KeyReact::Off) => KeyReact::Splash,
            (_, k) => k,
        }
    }

    /// A key was pressed at `at` (layout units).
    pub fn press(&mut self, at: Point) {
        if self.key_style() == KeyReact::Off {
            return;
        }
        // The Typing show sizes and colors presses by the music; other shows use plain white, size 1.
        let (color, size) = if self.look == AudioLook::Typing {
            // The music picks the color (bass to treble along the palette) and the size
            let pal = self.colors.resolve(self.look);
            let loud = (self.loudness * 1.5).min(1.0);
            (Some(pal.rgb(self.centroid, 0.5 + 0.5 * loud)), 1.0 + 1.5 * loud)
        } else {
            (None, 1.0)
        };
        self.presses.push(Press { at, born: self.t, color, size });
    }

    /// The show's color with key presses on top.
    fn with_presses(&self, p: Point, c: Rgb) -> Rgb {
        let style = self.key_style();
        let mut out = c;
        for press in &self.presses {
            let d = ((p.x - press.at.x).powi(2) + (p.y - press.at.y).powi(2)).sqrt() / press.size;
            let w = style.weight(d, self.t - press.born).min(1.0);
            if w <= 0.0 {
                continue;
            }
            // Blend towards the press color by `w`, but never darken a channel.
            let to = press.color.unwrap_or([255, 255, 255]);
            for k in 0..3 {
                let v = out[k] as f32 + (to[k] as f32 - out[k] as f32) * w;
                out[k] = out[k].max(v.round() as u8);
            }
        }
        out
    }

    /// The smoothed level at position `f` (0 = bass, 1 = treble), blending the two nearest bands.
    fn band_at(&self, f: f32) -> f32 {
        let x = f.clamp(0.0, 0.999) * (BANDS - 1) as f32;
        let i = x as usize;
        let frac = x - i as f32;
        self.levels[i] * (1.0 - frac) + self.levels[(i + 1).min(BANDS - 1)] * frac
    }

    /// The show's own color at point `p`, before key presses. `xn`/`yn` are the position as 0..1
    /// fractions of the surface.
    fn color(&self, p: Point, width: f32, height: f32) -> Rgb {
        let xn = (p.x / width.max(0.01)).clamp(0.0, 1.0);
        let yn = (p.y / height.max(0.01)).clamp(0.0, 1.0);
        // `up` is the height above the bottom row (0..1); `bar(level)` lights the LEDs below `level`,
        // with a soft edge about 1/6 of the height.
        let up = 1.0 - yn; // 0 bottom row, 1 top row
        let bar = |level: f32| ((level - up) * 6.0 + 0.3).clamp(0.0, 1.0);
        let pal = self.colors.resolve(self.look);
        match self.look {
            // Spectrum: bass at the left edge to treble at the right; each column is a bar of its band.
            AudioLook::Spectrum => {
                let l = self.band_at(xn);
                let t = if pal.by_height() { up } else { xn };
                pal.rgb(t, 0.04 + 0.96 * bar(l))
            }
            // Centre out: the same bars mirrored, bass in the middle and treble at both ends.
            AudioLook::CenterOut => {
                let d = ((xn - 0.5).abs() * 2.0).min(1.0);
                let l = self.band_at(d);
                let t = if pal.by_height() { up } else { d };
                pal.rgb(t, 0.04 + 0.96 * bar(l))
            }
            AudioLook::Meter => {
                // Fills from the bottom row up, with a peak mark that falls back slowly
                let level = (self.loudness * 1.4).min(1.0);
                let lit = ((level - up) * 6.0 + 0.3).clamp(0.0, 1.0);
                let peak = (self.peak * 1.4).min(1.0);
                // A thin bright line at the peak height (1/6 of the height wide), on top of the fill.
                let peak_mark = (1.0 - (up - peak).abs() * 6.0).max(0.0);
                pal.rgb(up, (0.03 + 0.97 * lit).max(peak_mark))
            }
            // Beat pulse: everything at one brightness (the beat flash, or some loudness between beats),
            // the color moving on with each beat and shading slightly across the width.
            AudioLook::BeatPulse => {
                let v = 0.12 + 0.88 * self.flash.max(self.loudness * 0.4);
                pal.rgb((self.hue + xn * 0.15).fract(), v)
            }
            // Bass ripples: each ring is a band 1.6 units wide at its radius, fading as it ages; rings
            // add up, over a faint background that follows the loudness.
            AudioLook::BassRipples => {
                // Rings start a bit below the middle of the surface and grow at 14 units/s for 1.6 s.
                let (cx, cy) = (0.5 * width, 0.55 * height);
                let d = ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt();
                let mut c = [0.0f32; 3];
                for r in &self.rings {
                    let age = self.t - r.born;
                    let radius = age * 14.0;
                    let w = (1.0 - (d - radius).abs() / 1.6).max(0.0) * (1.0 - age / 1.6).max(0.0);
                    let rgb = pal.rgb(r.hue, 1.0);
                    for k in 0..3 {
                        c[k] = (c[k] + rgb[k] as f32 * w).min(255.0);
                    }
                }
                let base = pal.rgb(self.hue, 0.04 + 0.1 * self.loudness);
                [(c[0] as u8).max(base[0]), (c[1] as u8).max(base[1]), (c[2] as u8).max(base[2])]
            }
            // Color wash: a slow drift through the palette across the surface, nudged by the treble and
            // brightened by the loudness.
            AudioLook::ColorWash => {
                let treble = self.levels[BANDS - 4..].iter().sum::<f32>() / 4.0;
                let t = (self.t * 0.04 + xn * 0.35 + yn * 0.1 + treble * 0.3).fract();
                // Palettes with an end (not the rainbow) go there and back instead of jumping
                let t = if matches!(pal, AudioColors::Rainbow) { t } else { 1.0 - (t * 2.0 - 1.0).abs() };
                pal.rgb(t, 0.15 + 0.85 * (self.loudness * 1.3).min(1.0))
            }
            // Typing: dark; only the key presses (drawn on top in `with_presses`) light anything.
            AudioLook::Typing => [0, 0, 0],
            AudioLook::Strobe => {
                // A 60 ms flash right after each beat; dim palette color otherwise.
                let since = self.t - self.last_beat;
                if since < 0.06 {
                    match pal {
                        AudioColors::One(..) => pal.rgb(0.0, 1.0),
                        _ => [255, 255, 255],
                    }
                } else {
                    pal.rgb(self.hue, 0.03 + 0.12 * self.loudness)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 20 x 6 grid of LED positions on a 20 x 6 surface.
    fn pts() -> Vec<Point> {
        let mut v = vec![];
        for y in 0..6 {
            for x in 0..20 {
                v.push(Point { x: x as f32 + 0.5, y: y as f32 + 0.5 });
            }
        }
        v
    }

    /// Silence draws (nearly) nothing; a sudden bass hit is a beat and lights the bass side.
    #[test]
    fn silence_is_dark_and_loud_bass_beats() {
        let mut show = AudioShow::new(AudioLook::Spectrum);
        for _ in 0..30 {
            show.update(&[0.0; 9], 1.0 / 30.0);
        }
        let quiet = show.render(&pts(), 20.0, 6.0);
        assert!(quiet.iter().all(|c| c.iter().all(|&v| v < 20)));
        // a sudden bass hit is a beat, and lights the bass bars on the left
        let mut bands = [0.1f32; 9];
        bands[0] = 1.0;
        bands[1] = 1.0;
        assert!(show.update(&bands, 1.0 / 30.0));
        let loud = show.render(&pts(), 20.0, 6.0);
        let left_bottom = loud[5 * 20];
        let right_bottom = loud[5 * 20 + 19];
        let lum = |c: Rgb| c.iter().map(|&v| v as u32).sum::<u32>();
        assert!(lum(left_bottom) > lum(right_bottom) * 2);
    }

    /// Spectrum colors run green to red upwards by default, across with Rainbow; color keys round-trip.
    #[test]
    fn spectrum_is_green_yellow_red_by_default_and_rainbow_on_request() {
        let mut show = AudioShow::new(AudioLook::Spectrum);
        for _ in 0..10 {
            show.update(&[1.0; 16], 0.03);
        }
        let c = show.render(&pts(), 20.0, 6.0);
        let bottom = c[5 * 20 + 10];
        let top = c[10];
        assert!(bottom[1] > 200 && bottom[0] < 60, "bottom row is green: {bottom:?}");
        assert!(top[0] > 200 && top[1] < 60, "top row is red: {top:?}");
        show.colors = AudioColors::Rainbow;
        let c = show.render(&pts(), 20.0, 6.0);
        assert_ne!(c[5 * 20], c[5 * 20 + 19], "rainbow changes across");
        for k in ["auto", "rainbow", "meter", "fire", "ocean", "one 170 255"] {
            assert_eq!(AudioColors::from_key(k).unwrap().key(), k);
        }
    }

    /// The meter lights the bottom row before the top and is even across a row.
    #[test]
    fn meter_fills_from_the_bottom() {
        let mut show = AudioShow::new(AudioLook::Meter);
        for _ in 0..20 {
            show.update(&[0.25; 16], 0.03);
        }
        let c = show.render(&pts(), 20.0, 6.0);
        let lum = |c: Rgb| c.iter().map(|&v| v as u32).sum::<u32>();
        assert!(lum(c[5 * 20 + 3]) > lum(c[3]) * 3, "bottom lit, top dark");
        assert_eq!(c[5 * 20 + 3], c[5 * 20 + 15], "same across a row");
    }

    /// Glow lights the pressed key white and fades; Ripple travels outwards; style keys round-trip.
    #[test]
    fn key_presses_light_up_on_top_of_the_show() {
        let mut show = AudioShow::new(AudioLook::Spectrum);
        show.keys = KeyReact::Glow;
        show.update(&[0.0; 16], 0.03);
        let key = Point { x: 3.5, y: 2.5 };
        show.press(key);
        let c = show.render(&[key, Point { x: 15.5, y: 2.5 }], 20.0, 6.0);
        assert!(c[0].iter().all(|&v| v > 200), "pressed key is white: {:?}", c[0]);
        assert!(c[1].iter().all(|&v| v < 30), "far key stays dark: {:?}", c[1]);
        // fades out
        for _ in 0..30 {
            show.update(&[0.0; 16], 0.03);
        }
        assert!(show.render(&[key], 20.0, 6.0)[0].iter().all(|&v| v < 30));
        // ripples travel outwards
        show.keys = KeyReact::Ripple;
        show.press(key);
        for _ in 0..10 {
            show.update(&[0.0; 16], 0.03);
        }
        let near_ring = Point { x: 3.5 + 2.7, y: 2.5 };
        assert!(show.render(&[near_ring], 20.0, 6.0)[0][0] > 100);
        for r in KeyReact::ALL {
            assert_eq!(KeyReact::from_key(r.key()), Some(r));
        }
    }

    /// In the Typing show, bass makes presses red and treble blue; unpressed keys stay dark.
    #[test]
    fn typing_colors_follow_the_music() {
        let key = Point { x: 3.5, y: 2.5 };
        let color_for = |bands: &[f32]| {
            let mut show = AudioShow::new(AudioLook::Typing);
            for _ in 0..30 {
                show.update(bands, 0.03);
            }
            show.press(key);
            show.update(bands, 0.01);
            show.render(&[key, Point { x: 15.5, y: 0.5 }], 20.0, 6.0)
        };
        let mut bass = [0.0f32; 16];
        bass[..3].fill(0.9);
        let mut treble = [0.0f32; 16];
        treble[12..].fill(0.9);
        let b = color_for(&bass);
        let t = color_for(&treble);
        assert!(b[0][0] > b[0][2], "bass is red: {:?}", b[0]);
        assert!(t[0][2] > t[0][0], "treble is blue/purple: {:?}", t[0]);
        assert_eq!(b[1], [0, 0, 0], "other keys stay dark");
    }

    /// Every show draws a color for every LED; look keys round-trip.
    #[test]
    fn every_look_draws_every_led() {
        for look in AudioLook::ALL {
            let mut show = AudioShow::new(look);
            show.update(&[0.5; 16], 0.03);
            assert_eq!(show.render(&pts(), 20.0, 6.0).len(), 120);
            assert_eq!(AudioLook::from_key(look.key()), Some(look));
        }
    }
}
