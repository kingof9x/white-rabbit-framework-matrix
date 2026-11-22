//! The live keyboard + side module picture: layout and preview colors (drawn in render.rs).

use crate::builtin::{self, BuiltinSim, EffectCfg, SimLed};
use crate::effects::{EffectParams, LinkedEffect, Renderer, Rgb};
use crate::keys::{self, KeyDef, SideModule, KEYBOARD_SIZE, MODULE_GRID};
use crate::layout::Point;
use crate::protocol::PaintMode;
use std::collections::HashMap;

/// What kind of keyboard the user has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KbType {
    /// Clear RGB keycaps: the whole cap glows.
    #[default]
    ClearRgb,
    /// Black RGB keycaps: light comes through the legends and around the keys.
    BlackRgb,
    /// Black keyboard with a single white backlight (no per-key color).
    White,
}

impl KbType {
    pub fn rgb(self) -> bool {
        self != KbType::White
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setup {
    pub kb: KbType,
    pub side: SideModule,
    pub module_left: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self { kb: KbType::ClearRgb, side: SideModule::Macropad, module_left: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dev {
    Keyboard,
    Module,
}

/// What is shown on the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Everything off except painted keys.
    PaintOnly,
    /// Built-in effect 1..=44.
    Builtin(u8),
    /// Drawn by the app across both devices.
    Pc(LinkedEffect),
}

impl Choice {
    pub fn name(self) -> String {
        match self {
            Choice::PaintOnly => "Off (painted keys only)".into(),
            Choice::Builtin(id) => {
                format!("{id}. {}", crate::protocol::EFFECT_NAMES[id as usize])
            }
            Choice::Pc(e) => e.name().into(),
        }
    }
    pub fn reacts_to_typing(self) -> bool {
        match self {
            Choice::Builtin(id) => builtin::reacts_to_typing(id),
            Choice::Pc(e) => e == LinkedEffect::Ripple,
            Choice::PaintOnly => false,
        }
    }
}

/// One device's settings for the preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub choice: Choice,
    pub hue: u8,
    pub sat: u8,
    pub val: u8,
    pub speed: u8,
}

impl Look {
    pub fn cfg(&self) -> EffectCfg {
        EffectCfg {
            effect: if let Choice::Builtin(id) = self.choice { id } else { 0 },
            hue: self.hue,
            sat: self.sat,
            val: self.val,
            speed: self.speed,
        }
    }
    pub fn pc_params(&self, effect: LinkedEffect) -> EffectParams {
        EffectParams {
            effect,
            speed: (0.05 + self.speed as f32 / 255.0 * 1.95).clamp(0.05, 2.0),
            hue: self.hue as f32 / 256.0,
            saturation: self.sat as f32 / 255.0,
            brightness: self.val as f32 / 255.0,
            reverse: false,
        }
    }
}

/// What the preview should show this frame.
pub struct Scene<'a> {
    pub setup: Setup,
    /// One effect across both devices (else `kb` / `module` separately).
    pub joined: bool,
    pub kb: Look,
    pub module: Look,
    pub paint: &'a HashMap<(Dev, usize), (u8, u8)>,
    pub paint_mode: PaintMode,
    pub macropad_layer: u8,
    pub show_paint_marks: bool,
    /// The key drawn with an accent outline (Macropad and Keys pages).
    pub selected: Option<(Dev, usize)>,
}

/// A rectangle in deck key units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// A key placed on the deck.
#[derive(Debug, Clone)]
pub struct PlacedKey {
    pub dev: Dev,
    pub index: usize,
    pub def: KeyDef,
    /// In deck key units.
    pub rect: R,
    /// Centre in deck key units (for the app-drawn effects).
    pub deck: Point,
    /// Position on its own device in QMK's 0..224 x 0..64 space.
    pub qmk: (u8, u8),
    pub rgb: bool,
}

/// Deck geometry in key units (independent of screen size).
struct Geo {
    total_w: f32,
    total_h: f32,
    kb_x: f32,
    mod_x: f32,
    pad: f32,
    module_w: f32,
    spacer_w: f32,
}

const PAD: f32 = 0.45;
const SEAM: f32 = 0.05;

fn module_scale() -> f32 {
    KEYBOARD_SIZE.1 / MODULE_GRID.1
}

fn geo(setup: &Setup) -> Geo {
    let module_w = MODULE_GRID.0 * module_scale();
    let spacer_w = module_w / 2.0;
    let has = setup.side != SideModule::None;
    let inner = KEYBOARD_SIZE.0 + if has { module_w + SEAM } else { 2.0 * (spacer_w + SEAM) };
    let (kb_x, mod_x) = if !has {
        (PAD + spacer_w + SEAM, 0.0)
    } else if setup.module_left {
        (PAD + module_w + SEAM, PAD)
    } else {
        (PAD, PAD + KEYBOARD_SIZE.0 + SEAM)
    };
    Geo { total_w: inner + 2.0 * PAD, total_h: KEYBOARD_SIZE.1 + 2.0 * PAD, kb_x, mod_x, pad: PAD, module_w, spacer_w }
}

/// Joined-surface slices for the firmware: (keyboard, side module).
/// RGB -> QMK's HSV scale (hue 0..255 in six 43-wide sections), like the firmware's tint.
pub fn rgb_to_qmk_hsv(c: Rgb) -> (u8, u8, u8) {
    let (r, g, b) = (c[0] as i32, c[1] as i32, c[2] as i32);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max == 0 {
        return (0, 0, 0);
    }
    let delta = max - min;
    let s = delta * 255 / max;
    if delta == 0 {
        return (0, s as u8, max as u8);
    }
    let h = if max == r {
        43 * (g - b) / delta
    } else if max == g {
        85 + 43 * (b - r) / delta
    } else {
        171 + 43 * (r - g) / delta
    };
    (h.rem_euclid(256) as u8, s as u8, max as u8)
}

/// A painted key's color over the effect's color for that key (same rules as the firmware).
pub fn paint_color(effect: Rgb, paint: (u8, u8), look: &Look, mode: PaintMode) -> Rgb {
    let floor = 0.3 * look.val as f32;
    let (eh, es, ev) = rgb_to_qmk_hsv(effect);
    let v = (ev as f32).max(floor);
    if look.choice == Choice::PaintOnly || mode == PaintMode::Steady {
        return builtin::qmk_hsv(paint.0 as i32, paint.1 as i32, look.val as i32);
    }
    if mode == PaintMode::Follow {
        return builtin::qmk_hsv(paint.0 as i32, paint.1 as i32, v as i32);
    }
    let h = eh.wrapping_add(paint.0.wrapping_sub(look.hue));
    let s = if es == 0 { paint.1 } else { (es as u32 * paint.1 as u32 / 255) as u8 };
    builtin::qmk_hsv(h as i32, s as i32, v as i32)
}

pub fn span_slices(setup: &Setup) -> ((u8, u8), (u8, u8)) {
    let module_w = MODULE_GRID.0 * module_scale();
    builtin::span_slices(KEYBOARD_SIZE.0, module_w, SEAM, setup.module_left)
}

/// Every key on the deck, in deck key units.
pub fn place_keys(setup: &Setup, layer: u8) -> Vec<PlacedKey> {
    let g = geo(setup);
    let mut out = Vec::new();
    for (i, def) in keys::keyboard_keys().into_iter().enumerate() {
        let (cx, cy) = def.center();
        out.push(PlacedKey {
            dev: Dev::Keyboard,
            index: i,
            qmk: keys::qmk_point(&def, KEYBOARD_SIZE),
            deck: Point { x: g.kb_x + cx, y: g.pad + cy },
            rgb: setup.kb.rgb(),
            rect: R { x: g.kb_x + def.x, y: g.pad + def.y, w: def.w, h: def.h },
            def,
        });
    }
    let s = module_scale();
    for (i, def) in keys::module_keys(setup.side, layer).into_iter().enumerate() {
        let (cx, cy) = def.center();
        out.push(PlacedKey {
            dev: Dev::Module,
            index: i,
            qmk: keys::qmk_point(&def, MODULE_GRID),
            deck: Point { x: g.mod_x + cx * s, y: g.pad + cy * s },
            rgb: setup.side == SideModule::Macropad,
            rect: R { x: g.mod_x + def.x * s, y: g.pad + def.y * s, w: def.w * s, h: def.h * s },
            def,
        });
    }
    out
}

/// The deck's parts that are not keys, in key units: (total size, plates, spacers, seam x).
pub struct Shell {
    pub size: (f32, f32),
    pub plates: Vec<(R, [bool; 4])>,
    pub spacers: Vec<R>,
    pub seam: Option<f32>,
}

/// Rounded corners per plate: [top-left, top-right, bottom-right, bottom-left].
pub fn shell(setup: &Setup) -> Shell {
    let g = geo(setup);
    let plate = |x: f32, w: f32| R { x: x - 0.18, y: g.pad - 0.18, w: w + 0.36, h: KEYBOARD_SIZE.1 + 0.36 };
    let has = setup.side != SideModule::None;
    let (left, right, all) = ([true, false, false, true], [false, true, true, false], [true; 4]);
    let mut plates = vec![(
        plate(g.kb_x, KEYBOARD_SIZE.0),
        if !has {
            all
        } else if setup.module_left {
            right
        } else {
            left
        },
    )];
    let mut spacers = vec![];
    let mut seam = None;
    if has {
        let mp = plate(g.mod_x, g.module_w);
        seam = Some(if setup.module_left { plates[0].0.x } else { mp.x });
        plates.push((mp, if setup.module_left { left } else { right }));
    } else {
        for x in [g.pad, g.kb_x + KEYBOARD_SIZE.0 + SEAM] {
            spacers.push(R { x: x - 0.18, y: g.pad - 0.18, w: g.spacer_w, h: KEYBOARD_SIZE.1 + 0.36 });
        }
    }
    Shell { size: (g.total_w, g.total_h), plates, spacers, seam }
}

/// The key under a point in deck key units.
pub fn key_at(keys: &[PlacedKey], x: f32, y: f32) -> Option<&PlacedKey> {
    keys.iter().find(|k| k.rect.contains(x, y))
}

/// Runs the effects for the preview, including pretend typing.
pub struct Preview {
    t0: std::time::Instant,
    joined_sim: BuiltinSim,
    kb_sim: BuiltinSim,
    mod_sim: BuiltinSim,
    renderer: Renderer,
    next_press: f64,
    rng: u64,
    last_key: Option<(Dev, usize, Choice)>,
}

impl Default for Preview {
    fn default() -> Self {
        Self {
            t0: std::time::Instant::now(),
            joined_sim: BuiltinSim::default(),
            kb_sim: BuiltinSim::default(),
            mod_sim: BuiltinSim::default(),
            renderer: Renderer::default(),
            next_press: 0.0,
            rng: 0x2545_F491_4F6C_DD1D,
            last_key: None,
        }
    }
}

fn white(val: u8) -> Rgb {
    let q = (235.0 * val as f32 / 255.0) as u8;
    [q, q, (q as f32 * 0.96) as u8]
}

impl Preview {
    pub fn time(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    fn rand(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % 1_000_000) as f64 / 1_000_000.0
    }

    /// True when pretend typing is running (an effect that reacts to keys).
    pub fn typing(&self, scene: &Scene) -> bool {
        scene.kb.choice.reacts_to_typing() || (!scene.joined && scene.module.choice.reacts_to_typing())
    }

    /// Colors for every placed key.
    pub fn colors(&mut self, scene: &Scene, keys: &[PlacedKey]) -> Vec<Rgb> {
        let t = self.time();
        let kb_slice_mod = span_slices(&scene.setup);
        let squeeze = |k: &PlacedKey| -> SimLed {
            let slice = if k.dev == Dev::Keyboard { kb_slice_mod.0 } else { kb_slice_mod.1 };
            SimLed { x: builtin::squeeze_x(k.qmk.0, slice), y: k.qmk.1, modifier: k.def.is_modifier() }
        };
        let rgb_keys: Vec<usize> = (0..keys.len()).filter(|&i| keys[i].rgb).collect();
        let look_of = |dev: Dev| {
            if scene.joined || dev == Dev::Keyboard {
                scene.kb
            } else {
                scene.module
            }
        };

        // forget effect memory when the effect changes
        let sig = (Dev::Keyboard, 0usize, scene.kb.choice);
        if self.last_key.map(|l| l.2) != Some(sig.2) {
            self.joined_sim.clear();
            self.kb_sim.clear();
            self.mod_sim.clear();
            self.last_key = Some(sig);
        }

        // pretend typing
        if self.typing(scene) && t >= self.next_press && !rgb_keys.is_empty() {
            let pick = rgb_keys[(self.rand() * rgb_keys.len() as f64) as usize % rgb_keys.len()];
            let k = &keys[pick];
            let jl = squeeze(k);
            let joined_index = rgb_keys.iter().position(|&i| i == pick).unwrap_or(0);
            self.joined_sim.press(joined_index, jl.x, jl.y, t);
            match k.dev {
                Dev::Keyboard => self.kb_sim.press(k.index, k.qmk.0, k.qmk.1, t),
                Dev::Module => self.mod_sim.press(k.index, k.qmk.0, k.qmk.1, t),
            }
            self.renderer.add_ripple(k.deck, t as f32);
            self.next_press = t + 0.25 + self.rand() * 0.6;
        }

        let g = geo(&scene.setup);
        let mut out = vec![[0u8; 3]; keys.len()];
        if scene.joined {
            let look = scene.kb;
            let cols: Vec<Rgb> = match look.choice {
                Choice::PaintOnly => vec![[0, 0, 0]; rgb_keys.len()],
                Choice::Pc(e) => {
                    self.renderer.params = look.pc_params(e);
                    self.renderer.prune(t as f32);
                    rgb_keys.iter().map(|&i| self.renderer.color_at(keys[i].deck, t as f32, g.total_w)).collect()
                }
                Choice::Builtin(_) => {
                    let leds: Vec<SimLed> = rgb_keys.iter().map(|&i| squeeze(&keys[i])).collect();
                    self.joined_sim.render(&look.cfg(), &leds, t)
                }
            };
            for (j, &i) in rgb_keys.iter().enumerate() {
                out[i] = cols[j];
            }
        } else {
            for dev in [Dev::Keyboard, Dev::Module] {
                let idx: Vec<usize> = rgb_keys.iter().copied().filter(|&i| keys[i].dev == dev).collect();
                if idx.is_empty() {
                    continue;
                }
                let look = look_of(dev);
                let leds: Vec<SimLed> = idx
                    .iter()
                    .map(|&i| SimLed { x: keys[i].qmk.0, y: keys[i].qmk.1, modifier: keys[i].def.is_modifier() })
                    .collect();
                let sim = if dev == Dev::Keyboard { &mut self.kb_sim } else { &mut self.mod_sim };
                let cols = match look.choice {
                    Choice::Builtin(_) => sim.render(&look.cfg(), &leds, t),
                    _ => vec![[0, 0, 0]; idx.len()],
                };
                for (j, &i) in idx.iter().enumerate() {
                    out[i] = cols[j];
                }
            }
        }

        // white-backlight devices and painted keys
        for (i, k) in keys.iter().enumerate() {
            let look = look_of(k.dev);
            if !k.rgb {
                out[i] = white(look.val);
                continue;
            }
            if let Some(&(h, s)) = scene.paint.get(&(k.dev, k.index)) {
                out[i] = paint_color(out[i], (h, s), &look, scene.paint_mode);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(hue: u8) -> Look {
        Look { choice: Choice::Builtin(13), hue, sat: 255, val: 255, speed: 128 }
    }

    #[test]
    fn color_codes_turn_into_the_firmwares_hue() {
        assert_eq!(rgb_to_qmk_hsv([255, 0, 0]), (0, 255, 255));
        let (h, s, v) = rgb_to_qmk_hsv(builtin::qmk_hsv(170, 255, 255));
        assert!((h as i32 - 170).abs() <= 2 && s >= 250 && v == 255);
        assert_eq!(rgb_to_qmk_hsv([255, 255, 255]).1, 0);
    }

    #[test]
    fn tint_shifts_the_effects_color() {
        let effect = builtin::qmk_hsv(85, 255, 200); // green from the effect
                                                     // painted the effect's own hue: unchanged
        let same = paint_color(effect, (10, 255), &look(10), PaintMode::Tint);
        let (h, _, v) = rgb_to_qmk_hsv(same);
        assert!((h as i32 - 85).abs() <= 2 && (v as i32 - 200).abs() <= 2);
        // painted 85 round from the effect's hue: green becomes blue
        let shifted = paint_color(effect, (95, 255), &look(10), PaintMode::Tint);
        assert!((rgb_to_qmk_hsv(shifted).0 as i32 - 170).abs() <= 3);
        // white takes the color out
        assert_eq!(rgb_to_qmk_hsv(paint_color(effect, (0, 0), &look(10), PaintMode::Tint)).1, 0);
    }
}
