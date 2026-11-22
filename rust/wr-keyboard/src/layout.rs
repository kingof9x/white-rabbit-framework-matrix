//! Combined physical layout of the keyboard and the macropad.
//!
//! QMK stores LED positions per device in its own 0..=224 x 0..=64 grid. For
//! the Linked effects both devices are placed side by side in one space
//! measured in "key units" (1u = one normal key, about 19 mm), so a wave
//! travels at the same physical speed across both.

use crate::protocol::{DeviceKind, LedPoint};

/// Keyboard width/height in key units (Framework 16 ANSI: 18.25u wide,
/// 5 full rows + the half-height function row).
pub const KEYBOARD_SIZE: (f32, f32) = (18.25, 5.5);
/// Macropad: 4 columns, same module height as the keyboard.
pub const MACROPAD_SIZE: (f32, f32) = (4.0, 5.5);
/// Space between the two modules (touchpad spacer / bezel).
pub const GAP: f32 = 0.6;

/// QMK's LED coordinate range.
const QMK_MAX_X: f32 = 224.0;
const QMK_MAX_Y: f32 = 64.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MacropadSide {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub fn dist(self, o: Point) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

/// LED positions of all connected devices in one shared space.
#[derive(Debug, Clone, Default)]
pub struct CombinedLayout {
    pub keyboard: Vec<Point>,
    pub macropad: Vec<Point>,
    pub width: f32,
    pub height: f32,
}

fn scale(p: &LedPoint, size: (f32, f32), offset_x: f32) -> Point {
    Point { x: offset_x + p.x as f32 / QMK_MAX_X * size.0, y: p.y as f32 / QMK_MAX_Y * size.1 }
}

impl CombinedLayout {
    /// Pass an empty slice for a device that is not connected.
    pub fn new(keyboard: &[LedPoint], macropad: &[LedPoint], side: MacropadSide) -> Self {
        let has_kb = !keyboard.is_empty();
        let has_pad = !macropad.is_empty();
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

    fn lp(x: u8, y: u8) -> LedPoint {
        LedPoint { x, y, flags: 4 }
    }

    #[test]
    fn macropad_on_the_right() {
        let l = CombinedLayout::new(&[lp(0, 0), lp(224, 64)], &[lp(0, 0), lp(224, 64)], MacropadSide::Right);
        assert_eq!(l.keyboard[0], Point { x: 0.0, y: 0.0 });
        assert!((l.keyboard[1].x - 18.25).abs() < 1e-4);
        assert!((l.keyboard[1].y - 5.5).abs() < 1e-4);
        assert!((l.macropad[0].x - (18.25 + GAP)).abs() < 1e-4);
        assert!((l.macropad[1].x - l.width).abs() < 1e-4);
    }

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

    #[test]
    fn single_device_starts_at_zero() {
        let l = CombinedLayout::new(&[], &[lp(0, 0)], MacropadSide::Right);
        assert_eq!(l.macropad[0].x, 0.0);
        assert_eq!(l.width, MACROPAD_SIZE.0);
        assert!(l.led(DeviceKind::Keyboard, 0).is_none());
        assert!(l.led(DeviceKind::Macropad, 0).is_some());
    }
}
