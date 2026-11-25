//! Runs every case from `scripts/golden_animations.py` with the Rust code (the panel animations, the
//! old-TV effect and the sand battery, with the same seeds and the same clock) and compares the
//! pictures with the ones Python saved in `tests/golden/animations.json`. Every pixel must match, and
//! the frames saved exactly (float64 bits) must match to within a few units in the last place: sin and
//! cos from the Windows C library can differ from Linux's in the last bit or two.
//!
//! Why both kinds: the LED pictures (0-255) are what the user sees, but a rounding difference can hide
//! there for a while and only show many frames later in an animation that feeds on itself (fire,
//! sand). Comparing some raw 0.0-1.0 frames bit for bit catches such drift early. The random looks use
//! `nprand::Generator`, a copy of numpy's random generator, so with the same seed both sides draw the
//! same random numbers.
//!
//! The JSON has two sections, "frames" (hex LED rows, as in `golden.rs`) and "exact" (16 hex digits,
//! the float64 bit pattern, per LED). Every name must exist on both sides. The file is read from the
//! repository, so run this with `cargo test` from the workspace.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use wr_core::animations::{self, Frame, Heat, LOOK_KEYS};
use wr_core::nprand::Generator;
use wr_core::sand::{self, SandPiles};
use wr_core::{Grid, HEIGHT, WIDTH};

/// The LED brightness used for the TV and sand cases.
const FOREGROUND: i32 = 160;
/// The time steps (seconds) between the 15 frames of each look: normal frame times, a pause of 0,
/// and long jumps of 0.5-3 s, to check the looks handle uneven timing exactly as Python does.
const LOOK_DTS: [f64; 15] = [0.04, 0.04, 0.04, 0.02, 0.02, 0.1, 0.0, 0.5, 1.0, 3.0, 1.0, 0.07, 1.0, 0.04, 1.0];
/// The random seed for every look, as in the Python script.
const LOOK_SEED: u64 = 7;
/// Which frame numbers of each look are also compared bit for bit.
const EXACT_FRAMES: [usize; 3] = [4, 9, 14];
/// The looks that use random numbers, run a second time with another seed.
const RANDOM_LOOKS: [&str; 3] = ["fire", "snow", "ball"];
/// The seed for that second run.
const SECOND_SEED: u64 = 2024;

/// The saved cases. Panics (failing the test) if the file is missing or not JSON.
fn golden() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/animations.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("animations.json")).expect("valid JSON")
}

/// An LED grid as Python saved it: one string per row, two hex digits per LED (clamped to 0-255).
fn encode(grid: &Grid) -> Vec<String> {
    let bytes = grid.to_bytes();
    (0..HEIGHT).map(|y| (0..WIDTH).map(|x| format!("{:02x}", bytes[x][y])).collect()).collect()
}

/// A 0.0-1.0 frame as the exact float64 bit patterns, 16 hex digits per LED, row by row.
fn encode_exact(frame: &Frame) -> Vec<String> {
    (0..HEIGHT).map(|y| (0..WIDTH).map(|x| format!("{:016x}", frame[x][y].to_bits())).collect()).collect()
}

// The inputs, made the same way as in the Python script ----------------------------------------------

/// A picture for the old-TV effect: a pattern over the top 30 rows with values from -10 to 219 (some
/// below zero to check clamping), the bottom 4 rows dark.
fn tv_picture() -> Grid {
    let mut g = Grid::new();
    for x in 0..WIDTH {
        for y in 0..30 {
            g.v[x][y] = ((x * 37 + y * 11) % 230) as i32 - 10;
        }
    }
    g
}

/// A single lit pixel, the smallest picture the TV effect can squeeze.
fn tv_dot() -> Grid {
    let mut g = Grid::new();
    g.v[3][5] = 90;
    g
}

/// A frame for `to_leds` with values below 0, above 1 and exactly on the rounding and 2% edges.
fn leds_frame() -> Frame {
    let mut f = [[0.0; HEIGHT]; WIDTH];
    for (x, column) in f.iter_mut().enumerate() {
        for (y, v) in column.iter_mut().enumerate() {
            // Python's % on floats; the left side is never below zero here
            *v = ((x * 34 + y) as f64 * 0.0137) % 1.3 - 0.1;
        }
    }
    // Hand-placed edge values: just around 2% (the "is it lit" threshold) and values that land
    // exactly on .5 after scaling by 160 or 255, where Python rounds halves to even.
    f[0][..6].copy_from_slice(&[0.02, 0.0200001, 0.019999, 2.5 / 160.0, 4.5 / 160.0, 5.5 / 255.0]);
    f[1][..4].copy_from_slice(&[0.5 / 160.0, 1.0, 1.5 / 160.0, 255.5 / 255.0]);
    f
}

/// A picture behind the sand battery (a diagonal pattern), for the case that draws sand over an app.
fn sand_base() -> Grid {
    let mut g = Grid::new();
    for x in 0..WIDTH {
        for y in 0..HEIGHT {
            if (x + y) % 5 == 0 {
                g.v[x][y] = if y % 2 == 1 { 200 } else { 60 };
            }
        }
    }
    g
}

/// A battery reading as the sand battery takes it: (charge 0.0-1.0, plugged in), or None for no battery.
type Battery = Option<(f64, bool)>;

/// `n` steps `step` seconds apart from `start`: the first with `first` and the rest with `then` (a
/// sudden change after the first frame, which is what makes the sand move).
fn run(first: Battery, then: Battery, n: usize, start: f64, step: f64) -> Vec<(Battery, f64)> {
    let mut steps = vec![(first, start)];
    steps.extend((1..n).map(|i| (then, start + i as f64 * step)));
    steps
}

/// One sand battery case, as in the Python script.
struct Scenario {
    /// Part of the case names ("sand-<name>-<step>").
    name: &'static str,
    /// The half: 0 top, 16 bottom.
    y: usize,
    /// The pile's random seed.
    seed: u64,
    /// The speed setting ("slow", "normal", "fast", or an unknown one that must act like normal).
    speed: &'static str,
    /// Which panel the pile belongs to ("left" / "right").
    key: &'static str,
    /// Each frame's battery reading and time in seconds.
    steps: Vec<(Battery, f64)>,
    /// Draw over `sand_base()` instead of a dark grid.
    on_base: bool,
}

/// Charging and draining in both halves and at every speed, a low battery that flashes, one that is
/// nearly full, no battery at first, an unknown speed, and sand over another picture.
fn sand_scenarios() -> Vec<Scenario> {
    let s = |name, y, seed, speed, key, steps, on_base| Scenario { name, y, seed, speed, key, steps, on_base };
    let mut nearly_full: Vec<(Battery, f64)> = (0..4).map(|i| (Some((0.999, true)), 1000.0 + i as f64 * 0.1)).collect();
    nearly_full.extend((0..8).map(|i| (Some((0.99, true)), 1000.4 + i as f64 * 0.1)));
    let mut no_battery = vec![(None, 1000.0), (None, 1000.1)];
    no_battery.extend(run(Some((0.5, false)), Some((0.45, false)), 10, 1000.2, 0.1));
    vec![
        s("charge-top", 0, 1, "normal", "left", run(Some((0.2, true)), Some((0.6, true)), 16, 1000.0, 0.1), false),
        s("drain-bottom", 16, 2, "normal", "left", run(Some((0.8, false)), Some((0.3, false)), 16, 1000.0, 0.1), false),
        s(
            "fast-charge-bottom",
            16,
            3,
            "fast",
            "right",
            run(Some((0.0, true)), Some((1.0, true)), 16, 1000.0, 0.1),
            false,
        ),
        s("slow-drain-top", 0, 4, "slow", "right", run(Some((1.0, false)), Some((0.5, false)), 16, 1000.0, 0.1), false),
        s(
            "low-flash-top",
            0,
            5,
            "normal",
            "left",
            run(Some((0.05, false)), Some((0.05, false)), 16, 1000.3, 0.25),
            false,
        ),
        s(
            "low-charging-bottom",
            16,
            6,
            "normal",
            "left",
            run(Some((0.05, true)), Some((0.07, true)), 16, 1000.0, 0.37),
            false,
        ),
        s("nearly-full-top", 0, 8, "normal", "left", nearly_full, false),
        s("no-battery-then-half-bottom", 16, 9, "normal", "left", no_battery, false),
        s(
            "unknown-speed-top",
            0,
            10,
            "warp",
            "left",
            run(Some((0.3, true)), Some((0.9, true)), 16, 1000.0, 0.1),
            false,
        ),
        s(
            "on-a-picture-bottom",
            16,
            11,
            "fast",
            "left",
            run(Some((0.4, true)), Some((0.1, false)), 16, 1000.0, 0.1),
            true,
        ),
    ]
}

// The cases ----------------------------------------------------------------------------------------

/// Every case: the LED pictures by name, and the frames compared bit for bit by name.
fn all_cases() -> (BTreeMap<String, Grid>, BTreeMap<String, Frame>) {
    let mut frames = BTreeMap::new();
    let mut exact = BTreeMap::new();

    // Each look (and an unknown name, which must fall back to Fire) for 15 uneven time steps.
    for key in LOOK_KEYS.iter().copied().chain(["unknown"]) {
        let mut look = animations::make_look(key, Some(LOOK_SEED));
        for (i, &dt) in LOOK_DTS.iter().enumerate() {
            let frame = look.frame(dt);
            let name = format!("look-{key}-{i:02}");
            frames.insert(name.clone(), animations::to_leds(&frame, 255));
            if EXACT_FRAMES.contains(&i) && key != "unknown" {
                exact.insert(name, frame);
            }
        }
    }
    for key in RANDOM_LOOKS {
        let mut look = animations::make_look(key, Some(SECOND_SEED));
        let mut frame = [[0.0; HEIGHT]; WIDTH];
        for i in 0..15 {
            frame = look.frame(0.1);
            frames.insert(format!("look-{key}-seed{SECOND_SEED}-{i:02}"), animations::to_leds(&frame, 255));
        }
        exact.insert(format!("look-{key}-seed{SECOND_SEED}-14"), frame);
    }
    // One step of the fire's heat spreading, on random heat from seed 5 with random cooling from
    // seed 9. The heat has two extra fuel rows, so it is compared as two 34-row windows.
    let mut heat: Heat = [[0.0; HEIGHT + 2]; WIDTH];
    let r = Generator::new(5).random_n(WIDTH * (HEIGHT + 2));
    for (x, column) in heat.iter_mut().enumerate() {
        column.copy_from_slice(&r[x * (HEIGHT + 2)..(x + 1) * (HEIGHT + 2)]);
    }
    animations::spread_heat(&mut heat, &mut Generator::new(9), 1.7, 0.14);
    let mut top = [[0.0; HEIGHT]; WIDTH];
    let mut bottom = [[0.0; HEIGHT]; WIDTH];
    for x in 0..WIDTH {
        top[x].copy_from_slice(&heat[x][..HEIGHT]);
        bottom[x].copy_from_slice(&heat[x][2..]);
    }
    exact.insert("spread-heat-k1.7-top".to_string(), top);
    exact.insert("spread-heat-k1.7-bottom".to_string(), bottom);

    let leds = leds_frame();
    // Brightness values including 0, a negative one and one above 255.
    for fg in [160, 255, 1, 0, -5, 300] {
        frames.insert(format!("to-leds-fg{fg}"), animations::to_leds(&leds, fg));
    }

    let picture = tv_picture();
    let dark = Grid::new();
    // The old-TV switch off and on, with a picture, a single dot, no picture and a dark picture,
    // and brightness values that are out of range.
    let tv: [(&str, Vec<Grid>); 10] = [
        ("tv-off-picture-fg160", animations::tv_off_frames(Some(&picture), FOREGROUND)),
        ("tv-on-picture-fg160", animations::tv_on_frames(Some(&picture), FOREGROUND)),
        ("tv-off-picture-fg300", animations::tv_off_frames(Some(&picture), 300)),
        ("tv-on-picture-fg0", animations::tv_on_frames(Some(&picture), 0)),
        ("tv-off-dot-fg160", animations::tv_off_frames(Some(&tv_dot()), FOREGROUND)),
        ("tv-on-dot-fg160", animations::tv_on_frames(Some(&tv_dot()), FOREGROUND)),
        ("tv-off-none-fg160", animations::tv_off_frames(None, FOREGROUND)),
        ("tv-on-none-fg160", animations::tv_on_frames(None, FOREGROUND)),
        ("tv-off-dark-fg160", animations::tv_off_frames(Some(&dark), FOREGROUND)),
        ("tv-on-dark-fg160", animations::tv_on_frames(Some(&dark), FOREGROUND)),
    ];
    for (name, list) in tv {
        for (i, g) in list.into_iter().enumerate() {
            frames.insert(format!("{name}-{i:02}"), g);
        }
    }

    for sc in sand_scenarios() {
        // A fresh pile per scenario: 15 rows inside the top half's border, 16 in the bottom half.
        let mut piles = SandPiles::new();
        piles.pile_for(sc.key, sc.y, if sc.y == 0 { 15 } else { 16 }, Some(sc.seed));
        for (i, &(battery, now)) in sc.steps.iter().enumerate() {
            let mut grid = if sc.on_base { sand_base() } else { Grid::new() };
            sand::draw_sand_battery(&mut piles, &mut grid, battery, FOREGROUND, sc.y, sc.key, sc.speed, now);
            frames.insert(format!("sand-{}-{i:02}", sc.name), grid);
        }
    }
    (frames, exact)
}

/// How far apart (in units in the last place) an exact value may be from Python's.
/// A float64's bits read as an integer step by one per representable number, so this allows tiny
/// last-digit differences between C libraries while any real change is far larger.
const MAX_ULPS: u64 = 8;

/// True if every float64 in the two sets of rows (16 hex digits a value) is within `max` ulps.
/// False if the shapes differ. Values of different signs never count as close.
fn within_ulps(a: &[String], b: &[String], max: u64) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(ra, rb)| {
            ra.len() == rb.len()
                && (0..ra.len()).step_by(16).all(|i| {
                    let x = u64::from_str_radix(&ra[i..i + 16], 16).unwrap() as i64;
                    let y = u64::from_str_radix(&rb[i..i + 16], 16).unwrap() as i64;
                    // same sign: the bit patterns of nearby floats are nearby integers
                    (x < 0) == (y < 0) && x.abs_diff(y) <= max
                })
        })
}

/// Every LED picture matches Python exactly, every exact frame matches to within `MAX_ULPS`, both sides
/// have the same names, and the case counts are as expected (so a case cannot quietly disappear).
#[test]
fn animations_tv_and_sand_match_python() {
    let golden = golden();
    let (frames, exact) = all_cases();
    let mut problems = Vec::new();
    for (section, now) in [
        ("frames", frames.iter().map(|(n, g)| (n.clone(), encode(g))).collect::<BTreeMap<_, _>>()),
        ("exact", exact.iter().map(|(n, f)| (n.clone(), encode_exact(f))).collect()),
    ] {
        let saved = golden[section].as_object().expect("section");
        for name in saved.keys().filter(|n| !now.contains_key(*n)) {
            problems.push(format!("{section}: not drawn in Rust: {name}"));
        }
        for (name, rows) in &now {
            match saved.get(name) {
                None => problems.push(format!("{section}: not in animations.json: {name}")),
                Some(want) => {
                    let want: Vec<String> = serde_json::from_value(want.clone()).expect("rows");
                    if &want != rows && !(section == "exact" && within_ulps(&want, rows, MAX_ULPS)) {
                        // The first differing row, to point at in the message.
                        let first = (0..HEIGHT).find(|&y| want[y] != rows[y]).unwrap_or(0);
                        problems.push(format!(
                            "{section}: differs: {name} (row {first}: python {} rust {})",
                            want[first], rows[first]
                        ));
                    }
                }
            }
        }
    }
    assert!(problems.is_empty(), "{} differences:\n{}", problems.len(), problems.join("\n"));
    assert_eq!(frames.len(), 423);
    assert_eq!(exact.len(), 23);
}
