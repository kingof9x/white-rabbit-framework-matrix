//! Stress test for the Top row setting (keyboard Fn lock).
//!
//! A small copy of how the keyboard firmware (QMK) picks what a key sends drives the
//! pretend keyboard through thousands of random steps: the app's Top row setting, Fn + Esc
//! on the keyboard, key changes on the Key mapping page, resets, and keymaps scrambled the
//! way old firmware or hand changes can leave them. After every step it checks what the top
//! row really sends, with and without Fn.

use crate::device::{self, Device};
use crate::keycodes::{is_f_key, Board, Keymap, KeyWrite, KC_F1, KC_TRNS};
use crate::keymap_data::QK_MOMENTARY;
use crate::protocol::{self as proto, DeviceKind};
use crate::sim::SimPort;

const FN_LOCK: u16 = 0x7E41;
const ESC: usize = 0;
const FN_KEY: usize = 68;

/// What the firmware sends for `key` (app order), like QMK: the highest active layer that
/// isn't see-through wins, falling back to layer 0. Holding Fn turns on the layer its key
/// asks for (MO(n)) on top of the default layer.
fn sends(km: &Keymap, default_layer: u8, fn_held: bool, key: usize) -> u16 {
    let mut active = vec![default_layer as usize];
    if fn_held {
        let fn_code = lookup(km, &active, FN_KEY);
        if (QK_MOMENTARY..QK_MOMENTARY + 4).contains(&fn_code) {
            active.push((fn_code - QK_MOMENTARY) as usize);
        }
    }
    lookup(km, &active, key)
}

fn lookup(km: &Keymap, active: &[usize], key: usize) -> u16 {
    let mut layers = active.to_vec();
    layers.sort_unstable();
    for &l in layers.iter().rev() {
        let c = km.layers[l][key];
        if c != KC_TRNS {
            return c;
        }
    }
    km.layers[0][key]
}

/// The pretend keyboard, as the app sees it over USB.
struct Rig {
    dev: Device,
    /// The app's copy of the keymap, kept the way the app keeps it.
    km: Keymap,
}

impl Rig {
    fn new() -> Self {
        let dev = Device::new(DeviceKind::Keyboard, Box::new(SimPort::keyboard()));
        let km = dev.keymap().unwrap();
        Rig { dev, km }
    }
    fn fn_lock(&self) -> u8 {
        self.dev.info().unwrap().layer.expect("keyboard reports Fn lock")
    }
    /// What the device itself holds (not the app's copy).
    fn device_keymap(&self) -> Keymap {
        self.dev.keymap().unwrap()
    }
    /// Top row: (sends on its own, sends with Fn) for keys F1..F12.
    fn top_row(&self) -> Vec<(u16, u16)> {
        let km = self.device_keymap();
        let dl = if self.fn_lock() == 1 { 2 } else { 0 };
        (1..=12).map(|k| (sends(&km, dl, false, k), sends(&km, dl, true, k))).collect()
    }
    fn write(&mut self, writes: &[KeyWrite]) {
        let reqs: Vec<_> = writes.iter().map(|&(l, r, c, code)| proto::set_keycode(l, r, c, code)).collect();
        self.dev.transact_many(&reqs).unwrap();
        self.km.apply(writes);
    }
    /// Fn + Esc pressed on the keyboard: the firmware's FN_LOCK key.
    fn press_fn_esc(&mut self) {
        let km = self.device_keymap();
        let dl = if self.fn_lock() == 1 { 2u8 } else { 0 };
        if sends(&km, dl, true, ESC) != FN_LOCK {
            return; // Esc was changed to something else
        }
        // QMK: with _FN held lock on, with _FM held lock off (see process_record_keymap)
        let fn_layer = lookup(&km, &[dl as usize], FN_KEY).wrapping_sub(QK_MOMENTARY);
        let lock = match fn_layer {
            1 => 1,
            3 => 0,
            _ => return,
        };
        self.dev.transact(&proto::set_layer(lock)).unwrap();
    }
    /// The app's Top row setting.
    fn choose(&mut self, f_keys_first: bool) -> device::FnLockResult {
        device::set_fn_lock(&self.dev, Some(&mut self.km), f_keys_first).unwrap()
    }
}

/// Small repeatable random numbers (no extra crates).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn below(&mut self, n: usize) -> usize {
        self.next() as usize % n
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len())]
    }
}

fn default_shortcut(k: usize) -> u16 {
    Board::Keyboard.default_code(0, k)
}

/// Checks what the top row does right after the Top row setting was chosen.
fn check_choice(rig: &Rig, f_keys_first: bool, ctx: &str) {
    assert_eq!(rig.fn_lock(), f_keys_first as u8, "{ctx}: Fn lock not set");
    assert_eq!(rig.km, rig.device_keymap(), "{ctx}: app's keymap copy out of step with the keyboard");
    for (i, (alone, with_fn)) in rig.top_row().into_iter().enumerate() {
        let k = i + 1;
        let (shortcut, f_key) = if f_keys_first { (with_fn, alone) } else { (alone, with_fn) };
        assert!(
            !is_f_key(shortcut),
            "{ctx}: F{k}: the shortcut side sends an F-key ({shortcut:#06x}); alone={alone:#06x} fn={with_fn:#06x} layers={:?}",
            (0..4).map(|l| rig.km.layers[l][k]).collect::<Vec<_>>()
        );
        // The two jobs are the base and Fn layers, the right way round
        assert_eq!(shortcut, rig.km.layers[0][k], "{ctx}: F{k} shortcut side");
        let fn_code = match rig.km.layers[1][k] {
            KC_TRNS => rig.km.layers[0][k],
            c => c,
        };
        assert_eq!(f_key, fn_code, "{ctx}: F{k} Fn side");
    }
}

#[test]
fn default_keyboard_both_ways() {
    let mut rig = Rig::new();
    for &f_first in &[false, true, false, true, true, false] {
        let r = rig.choose(f_first);
        assert!(r.took);
        assert_eq!(r.fixed_keys, 0, "a default keymap needs no fixing");
        for (i, (alone, with_fn)) in rig.top_row().into_iter().enumerate() {
            let k = i + 1;
            let f = KC_F1 + i as u16;
            if f_first {
                assert_eq!((alone, with_fn), (f, default_shortcut(k)), "F{k} with F1 to F12 chosen");
            } else {
                assert_eq!((alone, with_fn), (default_shortcut(k), f), "F{k} with Shortcuts chosen");
            }
        }
    }
}

/// The likely state of Callum's keyboard: the top row's base and Fn layers swapped, so both
/// Fn lock settings ended up sending F-keys on their own. Choosing Shortcuts must fix it.
#[test]
fn swapped_top_row_is_put_back() {
    let mut rig = Rig::new();
    let mut writes = vec![];
    for k in 1..=12 {
        let (r, c) = Board::Keyboard.matrix()[k];
        let s = default_shortcut(k);
        let f = KC_F1 + (k - 1) as u16;
        writes.extend([(0, r, c, f), (1, r, c, s), (2, r, c, s), (3, r, c, f)]);
    }
    rig.write(&writes);
    // Before: Shortcuts gives F-keys on their own (the bug Callum saw)
    rig.dev.transact(&proto::set_layer(0)).unwrap();
    assert!(rig.top_row().iter().all(|&(alone, _)| is_f_key(alone)));

    let r = rig.choose(false);
    assert!(r.took);
    assert_eq!(r.fixed_keys, 12);
    check_choice(&rig, false, "after fixing");
    for (i, (alone, with_fn)) in rig.top_row().into_iter().enumerate() {
        assert_eq!((alone, with_fn), (default_shortcut(i + 1), KC_F1 + i as u16));
    }
    // Choosing again changes nothing more
    assert_eq!(rig.choose(false).fixed_keys, 0);
}

#[test]
fn old_firmware_says_it_did_not_take() {
    // A keyboard that answers "not supported" to the Fn lock command, like firmware before it
    let dev = Device::new(DeviceKind::Keyboard, Box::new(SimPort::keyboard_without_fn_lock()));
    let r = device::set_fn_lock(&dev, None, true).unwrap();
    assert!(!r.took);
    assert_eq!(dev.info().unwrap().layer, None);
}

#[test]
fn stress_random_steps() {
    let shortcuts: Vec<u16> = (1..=12).map(default_shortcut).collect();
    let f_keys: Vec<u16> = (0..24).map(|i| if i < 12 { KC_F1 + i } else { 0x68 + (i - 12) }).collect();
    let others = [0x0004u16 /* A */, 0x0029 /* Esc */, crate::keycodes::app_code(0), crate::keycodes::app_code(5), 0x0000 /* nothing */];

    for seed in 0..40u64 {
        let mut rng = Lcg(seed * 7919 + 1);
        let mut rig = Rig::new();
        let mut steps = 0;
        while steps < 500 {
            steps += 1;
            let ctx = format!("seed {seed} step {steps}");
            let any_code = |rng: &mut Lcg| match rng.below(3) {
                0 => rng.pick(&shortcuts),
                1 => rng.pick(&f_keys),
                _ => rng.pick(&others),
            };
            match rng.below(10) {
                // The Top row setting
                0..=2 => {
                    let f_first = rng.below(2) == 1;
                    let r = rig.choose(f_first);
                    assert!(r.took, "{ctx}");
                    check_choice(&rig, f_first, &ctx);
                    // Doing it again finds nothing left to fix
                    assert_eq!(rig.choose(f_first).fixed_keys, 0, "{ctx}: second choice fixed more");
                    check_choice(&rig, f_first, &ctx);
                }
                // Fn + Esc on the keyboard swaps exactly the two jobs of every top-row key
                3..=4 => {
                    let before = rig.top_row();
                    let lock = rig.fn_lock();
                    rig.press_fn_esc();
                    assert_ne!(rig.fn_lock(), lock, "{ctx}: Fn + Esc didn't toggle");
                    let after = rig.top_row();
                    // Only a properly kept top row swaps exactly; that's what the app keeps
                    if consistent(&rig.device_keymap()) {
                        for (i, (b, a)) in before.iter().zip(&after).enumerate() {
                            assert_eq!((a.0, a.1), (b.1, b.0), "{ctx}: F{} after Fn + Esc", i + 1);
                        }
                    }
                    // The app sees the change
                    assert_eq!(rig.dev.info().unwrap().layer, Some(1 - lock), "{ctx}");
                }
                // A change on the Key mapping page, in either view
                5..=6 => {
                    let view = rng.below(2) as u8;
                    let k = 1 + rng.below(12);
                    let code = any_code(&mut rng);
                    let writes = rig.km.writes(view, k, Some(code));
                    rig.write(&writes);
                    assert_eq!(rig.km, rig.device_keymap(), "{ctx}");
                    // The change shows up where the page says: "Without Fn" is the base
                    // layer, which is what the key does alone with Fn lock off
                    let lock = rig.fn_lock();
                    let (alone, with_fn) = rig.top_row()[k - 1];
                    let shown = if (view == 0) == (lock == 0) { alone } else { with_fn };
                    assert_eq!(shown, code, "{ctx}: F{k} view {view} lock {lock}");
                }
                // Something else left the keymap in a mess (old firmware, hand edits)
                7..=8 => {
                    let k = 1 + rng.below(12);
                    let (r, c) = Board::Keyboard.matrix()[k];
                    let l = rng.below(4) as u8;
                    let code = if rng.below(5) == 0 { KC_TRNS } else { any_code(&mut rng) };
                    rig.write(&[(l, r, c, code)]);
                }
                // Reset all keys
                _ => {
                    let writes = rig.km.reset_writes();
                    rig.write(&writes);
                    assert_eq!(rig.km, Keymap::defaults(Board::Keyboard), "{ctx}");
                }
            }
        }
        // Whatever state it's in, both choices work at the end
        for f_first in [false, true, false] {
            assert!(rig.choose(f_first).took);
            check_choice(&rig, f_first, &format!("seed {seed} end"));
        }
    }
}

/// The top row the way Fn lock expects: layer 2 = layer 1 and layer 3 = layer 0, no see-through.
fn consistent(km: &Keymap) -> bool {
    (1..=12).all(|k| {
        km.layers[0][k] != KC_TRNS && km.layers[1][k] != KC_TRNS && km.layers[2][k] == km.layers[1][k] && km.layers[3][k] == km.layers[0][k]
    })
}
