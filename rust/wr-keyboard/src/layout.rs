//! Combined physical layout of the keyboard and the macropad.
//!
//! QMK stores LED positions per device in its own 0..=224 x 0..=64 grid. For
//! the Linked effects both devices are placed side by side in one space
//! measured in "key units" (1u = one normal key, about 19 mm), so a wave
//! travels at the same physical speed across both.
//!
//! Used by the lighting engine ([`crate::engine`], on its `hid-engine` thread): it rebuilds a
//! [`CombinedLayout`] whenever a device comes or goes or the macropad changes side, then draws
//! Linked effects and the audio show at these points and places key-press ripples with
//! [`CombinedLayout::led`]. The picture in the window uses its own, more detailed geometry
//! ([`crate::deck`]); this one only needs to be physically about right.

use crate::protocol::{DeviceKind, LedPoint};

/// Keyboard width/height in key units (Framework 16 ANSI: 18.25u wide,
/// 5 full rows + the half-height function row).
///
/// This ignores the gaps between rows (unlike `keys::KEYBOARD_SIZE`, 6.75 high), which only
/// stretches effects a little vertically.
pub const KEYBOARD_SIZE: (f32, f32) = (18.25, 5.5);
/// Macropad: 4 columns, same module height as the keyboard.
pub const MACROPAD_SIZE: (f32, f32) = (4.0, 5.5);
/// Space between the two modules (touchpad spacer / bezel).
pub const GAP: f32 = 0.6;

/// QMK's LED coordinate range.
const QMK_MAX_X: f32 = 224.0;
const QMK_MAX_Y: f32 = 64.0;

/// Which side of the keyboard the macropad is fitted (set on the Setup page).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MacropadSide {
    Left,
    #[default]
    Right,
}

/// A position in key units: x to the right, y downwards, from the top-left of the combined area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    /// Straight-line distance to `o`, in key units.
    pub fn dist(self, o: Point) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

/// LED positions of all connected devices in one shared space.
#[derive(Debug, Clone, Default)]
pub struct CombinedLayout {
    /// The keyboard's LEDs, in the firmware's LED order (empty when not connected).
    pub keyboard: Vec<Point>,
    /// The macropad's LEDs, in the firmware's LED order (empty when not connected).
    pub macropad: Vec<Point>,
    /// Width of the whole area in key units (0 when nothing is connected).
    pub width: f32,
    /// Height in key units (always the keyboard's height; both modules are the same height).
    pub height: f32,
}

/// Maps one LED from QMK's grid onto a device `size` key units big, shifted right by `offset_x`.
fn scale(p: &LedPoint, size: (f32, f32), offset_x: f32) -> Point {
    Point { x: offset_x + p.x as f32 / QMK_MAX_X * size.0, y: p.y as f32 / QMK_MAX_Y * size.1 }
}

impl CombinedLayout {
    /// Pass an empty slice for a device that is not connected.
    pub fn new(keyboard: &[LedPoint], macropad: &[LedPoint], side: MacropadSide) -> Self {
        let has_kb = !keyboard.is_empty();
        let has_pad = !macropad.is_empty();
        // With both present, the one on the left starts at 0 and the other after it plus the gap.
        // With only one, it starts at 0.
        let (kb_x, pad_x) = match (has_kb && has_pad, side) {
            (true, MacropadSide::Right) => (0.0, KEYBOARD_SIZE.0 + GAP),
            (true, MacropadSide::Left) => (MACROPAD_SIZE.0 + GAP, 0.0),
            (false, _) => (0.0, 0.0),
        };
        let width = match (has_kb, has_pad) {
            (true, true) => KEYBOARD_SIZE.0 + GAP + MACROPAD_SIZE.0,
            (true, false) => KEYBOARD_SIZE.0,
            (false, true) => MACROPAD_SIZE.0,
            (false, false) => 0.0,
        };
        Self {
            keyboard: keyboard.iter().map(|p| scale(p, KEYBOARD_SIZE, kb_x)).collect(),
            macropad: macropad.iter().map(|p| scale(p, MACROPAD_SIZE, pad_x)).collect(),
            width,
            height: KEYBOARD_SIZE.1,
        }
    }

    /// All LED positions of one device, in LED order (what the engine draws for that device).
    pub fn points(&self, kind: DeviceKind) -> &[Point] {
        match kind {
            DeviceKind::Keyboard => &self.keyboard,
            DeviceKind::Macropad => &self.macropad,
        }
    }

    /// Position of a device's LED, if it exists.
    pub fn led(&self, kind: DeviceKind, index: u8) -> Option<Point> {
        self.points(kind).get(index as usize).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key-light LED at QMK position (x, y).
    fn lp(x: u8, y: u8) -> LedPoint {
        LedPoint { x, y, flags: 4 }
    }

    /// Keyboard from 0, macropad after the gap, corners map to the outline's corners.
    #[test]
    fn macropad_on_the_right() {
        let l = CombinedLayout::new(&[lp(0, 0), lp(224, 64)], &[lp(0, 0), lp(224, 64)], MacropadSide::Right);
        assert_eq!(l.keyboard[0], Point { x: 0.0, y: 0.0 });
        assert!((l.keyboard[1].x - 18.25).abs() < 1e-4);
        assert!((l.keyboard[1].y - 5.5).abs() < 1e-4);
        assert!((l.macropad[0].x - (18.25 + GAP)).abs() < 1e-4);
        assert!((l.macropad[1].x - l.width).abs() < 1e-4);
    }

    /// The macropad starts at 0 and the keyboard after it; everything stays inside the width.
    #[test]
    fn macropad_on_the_left() {
        let l = CombinedLayout::new(&[lp(0, 0)], &[lp(0, 0), lp(224, 0)], MacropadSide::Left);
        assert_eq!(l.macropad[0].x, 0.0);
        assert!((l.macropad[1].x - 4.0).abs() < 1e-4);
        assert!((l.keyboard[0].x - (4.0 + GAP)).abs() < 1e-4);
        // every LED stays inside the combined area
        for p in l.keyboard.iter().chain(&l.macropad) {
            assert!(p.x >= 0.0 && p.x <= l.width);
        }
    }

    /// A device on its own starts at 0, and the missing one has no LEDs.
    #[test]
    fn single_device_starts_at_zero() {
        let l = CombinedLayout::new(&[], &[lp(0, 0)], MacropadSide::Right);
        assert_eq!(l.macropad[0].x, 0.0);
        assert_eq!(l.width, MACROPAD_SIZE.0);
        assert!(l.led(DeviceKind::Keyboard, 0).is_none());
        assert!(l.led(DeviceKind::Macropad, 0).is_some());
    }
}
