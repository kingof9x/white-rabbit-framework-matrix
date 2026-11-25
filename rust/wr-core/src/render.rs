//! Drawing one panel's frame from the apps it shows now: a port of `render_panel` in
//! `led_mon/rendering.py` and the app-name display of the main loop.
//!
//! Where it sits: the monitor's main loop (`wr-monitor/src/monitor.rs`) asks the `layout::Rotation` what
//! each panel shows, then calls `render_panel_on` once per panel about ten times a second and puts the
//! resulting grid in that panel's `queue::FrameQueue`. The window calls the same functions for its
//! previews, so the preview and the panel always match.
//!
//! How it works: for each shown app (one whole-panel app, or a top and a bottom app) it picks the drawing
//! function by the app's name, asks the `Readings` for the numbers that app needs, draws in the
//! foreground brightness, then draws the app's border in the dimmer background brightness. The numbers
//! come in through the `Readings` trait so this crate never touches Windows: the monitor implements it
//! with real sensors, the tests with fixed values.
//!
//! State: most apps are drawn fresh from their readings every frame. The ones that move on their own
//! (the sand battery and the animations) keep their state between frames in the global `MOVING`.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::animations::{self, Look};
use crate::apps::meters;
use crate::drawing::*;
use crate::layout::{App, PanelApps};
use crate::sand::{self, SandPiles};
use crate::Grid;

/// What the apps that move on their own keep between frames: the sand piles, and each running
/// animation with when it last drew ({(panel, look): (animation, seconds)}).
///
/// Kept per panel side so two panels showing the same look animate independently.
#[derive(Default)]
struct Moving {
    /// The sand battery's falling grains, per panel and half.
    sand: SandPiles,
    /// Each running animation and the `now` it last drew at, keyed by (panel side, look name).
    animations: HashMap<(String, String), (Box<dyn Look>, f64)>,
}

/// The single store of moving apps' state for this process. Behind a mutex because the monitor's main
/// loop and the window's previews may draw from different threads; the lock is held only while one app
/// draws. Entries are never removed, so an animation keeps its place if its app comes round again.
static MOVING: LazyLock<Mutex<Moving>> = LazyLock::new(Mutex::default);

/// What the apps show, read from the computer. The monitor exe reads Windows; tests use fixed values.
///
/// Called on the monitor's main loop for every frame, so implementations must be quick: the monitor reads
/// slow sources (weather, GPU, temperatures) on background threads and only hands over the latest values.
pub trait Readings {
    /// Each physical core's load, 0.0-1.0 (the cpu app).
    fn cpu_cores(&mut self) -> Vec<f64>;
    /// The whole CPU's load once a second, newest first (the cpu-graph app).
    fn cpu_history(&mut self) -> Vec<Option<f64>>;
    /// Memory in use, 0.0-1.0.
    fn memory(&mut self) -> f64;
    /// Battery charge 0.0-1.0 and whether it's plugged in; None without a battery.
    fn battery(&mut self) -> Option<(f64, bool)>;
    /// The clock's four digits ("0905") and whether to show the PM dot, for the time app's args.
    fn clock(&mut self, app: &App) -> (String, bool);
    /// Seconds, for the battery's flash and pulse.
    fn now(&mut self) -> f64;

    // The apps in `crate::apps`. Each has a default with no reading, so a source that can't read
    // something leaves it out.

    /// Which panel is drawn next (false: left, true: right). The monitor tells before each panel.
    fn set_panel(&mut self, _right: bool) {}
    /// The panel being drawn: true for the right one.
    fn panel_right(&mut self) -> bool {
        false
    }
    /// Disk (read, write) rates, 0.0-1.0 of the highest seen.
    fn disk(&mut self) -> (f64, f64) {
        (0.0, 0.0)
    }
    /// Network (upload, download) rates, 0.0-1.0 of the highest seen.
    fn net(&mut self) -> (f64, f64) {
        (0.0, 0.0)
    }
    /// A GPU's use once a second, newest first (the gpu-graph app). `gpu` is a name, or blank for the first.
    fn gpu_history(&mut self, _gpu: &str) -> Vec<Option<f64>> {
        Vec::new()
    }
    /// Frames a second of the watched app (blank: the game in front).
    fn fps(&mut self, _process: &str) -> crate::apps::Fps {
        crate::apps::Fps::default()
    }
    /// What is playing.
    fn now_playing(&mut self) -> crate::apps::now_playing::NowPlaying {
        Default::default()
    }
    /// The weather for the app's settings (location, units, forecast), or None while there is none.
    fn weather(&mut self, _app: &App) -> Option<crate::apps::weather::Weather> {
        None
    }
    /// The temperatures app's three values in its unit, None where there is no reading.
    fn temperatures(&mut self, _app: &App) -> [Option<f64>; 3] {
        [None; 3]
    }
    /// Fan speeds in RPM.
    fn fans(&mut self) -> Vec<f64> {
        Vec::new()
    }
    /// How much of a GPU's memory is in use, 0.0-1.0 (`gpu` as for `gpu_history`), or None
    /// without a reading.
    fn gpu_memory(&mut self, _gpu: &str) -> Option<f64> {
        None
    }
    /// How full a drive is, 0.0-1.0, for a drive such as "C:", or None when there is no such drive.
    fn drive_space(&mut self, _drive: &str) -> Option<f64> {
        None
    }
}

/// The apps this monitor can draw so far. The others show nothing until they are ported.
///
/// These are the apps drawn directly in `render_panel_on`; `is_supported` also accepts the ones in
/// `crate::apps`.
pub const SUPPORTED_APPS: [&str; 7] = ["cpu", "cpu-graph", "mem-bat", "battery", "time", "battery-sand", "animation"];

/// Whether `name` is an app this code can draw. Unknown names (a typo, or an app from a newer or
/// older layout) are skipped and leave their half dark rather than causing an error.
pub fn is_supported(name: &str) -> bool {
    SUPPORTED_APPS.contains(&name)
        || crate::apps::CONTENT_APPS.contains(&name)
        || crate::apps::DATA_APPS.contains(&name)
}

/// LED values for the screen brightness 0.0-1.0: (foreground, background).
///
/// The foreground (app pixels) goes from 24 to 160 and the background (borders) from 12 to 35, both
/// truncated like Python's `int` (the same numbers as the Python monitor). `ratio` is the screen
/// brightness or the user's level as 0.0-1.0; the results are clamped to 0-255.
pub fn brightness_values(ratio: f64) -> (i32, i32) {
    let fg = crate::pyround::int(ratio * (160.0 - 24.0) + 24.0) as i32;
    let bg = crate::pyround::int(ratio * (35.0 - 12.0) + 12.0) as i32;
    (fg.clamp(0, 255), bg.clamp(0, 255))
}

/// Draws the panel's apps into `grid`. Returns the `animate` setting of the last app drawn.
///
/// The same as `render_panel_on` with side "left"; used by the tests and callers that have one panel.
pub fn render_panel(grid: &mut Grid, apps: &PanelApps, readings: &mut dyn Readings, fg: i32, bg: i32) -> bool {
    render_panel_on(grid, apps, readings, fg, bg, "left")
}

/// [`render_panel`] for the panel on this side ("left" or "right"), which the apps that move on
/// their own keep apart.
///
/// `fg` and `bg` are the foreground and border brightness from `brightness_values`. `grid` is normally
/// a new dark grid; apps only light pixels. The return value is the last drawn app's `animate` flag,
/// which the monitor passes to the panel so it scrolls the picture itself.
pub fn render_panel_on(
    grid: &mut Grid,
    apps: &PanelApps,
    readings: &mut dyn Readings,
    fg: i32,
    bg: i32,
    side: &str,
) -> bool {
    // Whole-panel apps draw no border of their own.
    let whole = matches!(apps, PanelApps::Whole(_));
    let mut animate = false;
    for (j, app) in apps.apps().into_iter().enumerate() {
        if !app.display {
            continue;
        }
        // The first app is the top half (row 0), the second the bottom half (row 16). A whole-panel app is
        // the only one, so it gets y = 0.
        let y = if j == 0 { 0 } else { 16 };
        if !is_supported(&app.name) {
            continue;
        }
        // The cpu app's core count decides its border (eight boxes, or a plain outline for bars).
        let mut cores = 0;
        match app.name.as_str() {
            "cpu" => {
                let values = readings.cpu_cores();
                cores = values.len();
                draw_cpu_vals(grid, &values, fg, y);
            }
            "cpu-graph" => {
                let (bars_from, newest) =
                    (app.arg_str("bars_from").unwrap_or_default(), app.arg_str("newest").unwrap_or_default());
                let history = readings.cpu_history();
                if memory_meter(app) {
                    let memory = readings.memory();
                    meters::draw_graph_with_meter(grid, &history, memory, fg, y, &bars_from, &newest);
                } else {
                    draw_history_graph_with(grid, &history, fg, y, &bars_from, &newest);
                }
            }
            "mem-bat" => {
                draw_memory(grid, readings.memory(), fg, y);
                if let Some((ratio, plugged)) = readings.battery() {
                    let now = readings.now();
                    // The battery sits under the two memory rows: y 3 on the top half, 19 on the bottom.
                    draw_battery(grid, ratio, plugged, fg, y + 3, now);
                }
            }
            "battery" => {
                let look = app.arg_str("look").unwrap_or_else(|| "bolt".into());
                // The battery outline is drawn at twice the border brightness so its shape reads clearly.
                let outline = (bg * 2).min(255);
                match readings.battery() {
                    Some((ratio, plugged)) => {
                        let now = readings.now();
                        meters::draw_battery_shape(grid, ratio, plugged, &look, fg, outline, y, now);
                    }
                    None => meters::draw_battery_outline(grid, outline, y),
                }
            }
            "time" => {
                let (digits, pm) = readings.clock(app);
                // Always four characters ("0905"); anything else from a reading is padded or cut, so a
                // bad clock reading can't stop the main loop
                let mut d: Vec<String> = digits.chars().take(4).map(|c| c.to_string()).collect();
                d.resize(4, "0".to_string());
                // Two hour digits, a short line as the colon, two minute digits, top to bottom.
                let chars: Vec<&str> = vec![&d[0], &d[1], "horiz_colon", &d[2], &d[3]];
                draw_chars_list(grid, &chars, fg, y);
                if pm {
                    draw_pm_indicator(grid, fg);
                }
            }
            name if crate::apps::DATA_APPS.contains(&name) => draw_data_app(grid, app, readings, fg, y),
            "text" => crate::apps::text::draw_app(grid, app, readings, fg),
            "snap" => crate::apps::picture::draw_app(grid, app, fg, side),
            "battery-sand" => {
                let speed = app.arg_str("sand_speed").unwrap_or_else(|| "normal".into());
                let (battery, now) = (readings.battery(), readings.now());
                // Panics only if a thread panicked while drawing sand (poisoned lock).
                let moving = &mut MOVING.lock().unwrap();
                sand::draw_sand_battery(&mut moving.sand, grid, battery, fg, y, side, &speed, now);
            }
            "animation" => {
                let look = app.arg_str("look").unwrap_or_else(|| "fire".into());
                let panel = app.arg_str("panel").unwrap_or_else(|| side.to_string());
                let now = readings.now();
                let moving = &mut MOVING.lock().unwrap();
                let entry = moving
                    .animations
                    .entry((panel, look.clone()))
                    // A new animation is made with a random seed, and its "last drew" time is set one step
                    // back so its first frame already moves.
                    .or_insert_with(|| (animations::make_look(&look, None), now - animations::LOOK_STEP));
                // Advance by the real time since this animation last drew, so its speed does not depend
                // on how often the main loop runs.
                let frame = entry.0.frame(now - entry.1);
                entry.1 = now;
                *grid = animations::to_leds(&frame, fg);
            }
            _ => {}
        }
        // A hidden (`display: false`) or unknown app does not reach here, so it does not change the flag.
        animate = app.animate;
        if app.border() && !whole {
            match app.name.as_str() {
                "cpu-graph" | "gpu-graph" if memory_meter(app) => meters::draw_graph_with_meter_border(grid, bg, y),
                "disk-net" => meters::draw_net_disk_space_border(grid, bg, y),
                "battery" => {} // the battery's outline is its border
                name => draw_app_border(name, grid, bg, y, cores),
            }
        }
    }
    animate
}

/// Whether a graph app shows the memory meter above it (its `memory` setting, on unless turned off).
///
/// `memory: false` (or `0`) turns the meter off; any other value, or no value, leaves it on.
fn memory_meter(app: &App) -> bool {
    app.arg("memory").map(|_| app.arg_bool("memory")).unwrap_or(true)
}

/// The apps in `crate::apps`: disk and network, GPU, FPS, now playing, weather, temperatures and fans.
///
/// Called by `render_panel_on` for names in `crate::apps::DATA_APPS`. Each app reads only what it needs,
/// so a slow or missing reading for one app does not affect the others. `y` is the half's top row.
fn draw_data_app(grid: &mut Grid, app: &App, readings: &mut dyn Readings, fg: i32, y: usize) {
    use crate::apps::{now_playing, temp_fan, weather};
    let arg = |key: &str| app.arg_str(key).unwrap_or_default();
    match app.name.as_str() {
        // Two tall bars: read / write for "disk", upload / download for "net" (columns 1-3 and 5-7).
        "disk" | "net" => {
            let (a, b) = if app.name == "disk" { readings.disk() } else { readings.net() };
            draw_bar(grid, a, fg, 1, y);
            draw_bar(grid, b, fg, 5, y);
        }
        "disk-net" => {
            let (disk, net) = (readings.disk(), readings.net());
            // The drive whose space shows at the bottom: C: unless set; blank hides the meter
            let drive = app.arg_str("drive").unwrap_or_else(|| "C:".into());
            let space = if drive.trim().is_empty() { None } else { readings.drive_space(drive.trim()) };
            meters::draw_net_disk_space(grid, net, disk, space, fg, y);
        }
        "gpu-graph" => {
            let history = readings.gpu_history(&arg("gpu"));
            if memory_meter(app) {
                let memory = readings.gpu_memory(&arg("gpu")).unwrap_or(0.0);
                meters::draw_graph_with_meter(grid, &history, memory, fg, y, &arg("bars_from"), &arg("newest"));
            } else {
                draw_history_graph_with(grid, &history, fg, y, &arg("bars_from"), &arg("newest"));
            }
        }
        "fps" => {
            let fps = readings.fps(&arg("process"));
            draw_fps(grid, fps.value, &fps.history, fps.available, fg);
        }
        "fps-num" => {
            let fps = readings.fps(&arg("process"));
            draw_fps_half(grid, fps.value, fps.available, fg, y);
        }
        "temp" => draw_temp_numbers(grid, &readings.temperatures(app), fg, y),
        "fan" => {
            let [a, b] = temp_fan::fan_ratios(&readings.fans());
            draw_bar(grid, a, fg, 1, y);
            draw_bar(grid, b, fg, 5, y);
        }
        // The weather app cycles through its `measures` (temperature and condition by default), each
        // for `measures-duration` seconds (10 by default). With no weather yet (no key, offline, still
        // loading) it shows two question marks.
        "weather" => match readings.weather(app) {
            Some(w) => {
                let measures: Vec<String> = match app.arg("measures") {
                    Some(serde_yaml::Value::Sequence(list)) => {
                        list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
                    }
                    _ => vec!["temp_condition".to_string()],
                };
                let measures: Vec<&str> = measures.iter().map(String::as_str).collect();
                let duration = app.arg("measures-duration").and_then(serde_yaml::Value::as_f64).unwrap_or(10.0);
                let measure = weather::measure_at(&measures, duration, readings.now());
                let values = weather::weather_values(&w, measure);
                let chars: Vec<&str> = values.iter().map(String::as_str).collect();
                draw_chars_list(grid, &chars, fg, y);
                if app.arg_bool("forecast") {
                    // Accepts "2", 2 or "2.0"; anything unreadable gives the default.
                    let int_arg = |key: &str, default: i64| {
                        app.arg_str(key).and_then(|s| s.trim().parse::<f64>().ok()).map(|v| v as i64).unwrap_or(default)
                    };
                    weather::draw_forecast_period(grid, fg, int_arg("forecast_day", 1), int_arg("forecast_hour", 12));
                }
            }
            None => draw_chars_list(grid, &["?", "?"], fg, y),
        },
        "now-playing" => {
            // Each switch is on unless the layout turns it off; "when_idle" defaults to "skip".
            let opts = now_playing::Options {
                title: app.arg("title").map(|_| app.arg_bool("title")).unwrap_or(true),
                visualizer: app.arg("visualizer").map(|_| app.arg_bool("visualizer")).unwrap_or(true),
                drift: app.arg("drift").map(|_| app.arg_bool("drift")).unwrap_or(true),
                when_idle: app.arg_str("when_idle").unwrap_or_else(|| "skip".into()),
            };
            let right = match arg("side").as_str() {
                "left" => false,
                "right" => true,
                // Without a `side` option the app takes the side of the panel it is drawn on.
                _ => readings.panel_right(),
            };
            let np = readings.now_playing();
            let now = readings.now();
            // When nothing is playing and the app is set to show the clock instead, it draws a 24-hour
            // clock from the top of the panel.
            if !now_playing::draw_now_playing(grid, &np, &opts, right, now, fg) {
                // The clock, 24-hour, from the top
                let mut clock_app = app.clone();
                clock_app.args = serde_yaml::Mapping::new();
                clock_app.args.insert("fmt_24_hour".into(), true.into());
                let (digits, _) = readings.clock(&clock_app);
                // Always four characters ("0905"); anything else from a reading is padded or cut, so a
                // bad clock reading can't stop the main loop
                let mut d: Vec<String> = digits.chars().take(4).map(|c| c.to_string()).collect();
                d.resize(4, "0".to_string());
                // Always true after the `resize` above (a harmless leftover check).
                if d.len() == 4 {
                    draw_chars_list(grid, &[&d[0], &d[1], "horiz_colon", &d[2], &d[3]], fg, 0);
                }
            }
        }
        _ => {}
    }
}

/// The names of the panel's apps, inside an outline (Ctrl+Alt+Shift+I, or Identify panels).
///
/// Unlike normal frames, every app is named, even unsupported or hidden ones, so the user can see what
/// the layout says each quarter holds. Names without a picture are left blank.
pub fn render_ids(grid: &mut Grid, apps: &PanelApps, fg: i32, bg: i32) {
    draw_outline_border(grid, bg);
    match apps {
        PanelApps::Whole(app) => draw_id(grid, &app.name, fg),
        PanelApps::Halves(top, bottom) => draw_ids(grid, &top.name, &bottom.name, fg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Layout;

    /// Test readings: eight cores at 50%, memory 40%, battery 66% on battery power, clock 09:05 PM.
    struct Fixed;
    impl Readings for Fixed {
        fn cpu_cores(&mut self) -> Vec<f64> {
            vec![0.5; 8]
        }
        fn cpu_history(&mut self) -> Vec<Option<f64>> {
            vec![Some(0.5), None]
        }
        fn memory(&mut self) -> f64 {
            0.4
        }
        fn battery(&mut self) -> Option<(f64, bool)> {
            Some((0.66, false))
        }
        fn clock(&mut self, _: &App) -> (String, bool) {
            ("0905".into(), true)
        }
        fn now(&mut self) -> f64 {
            0.0
        }
    }

    /// Test helper: the left panel's apps for a layout with `top` and `bottom` on the left quarters.
    fn panel(top: &str, bottom: &str) -> PanelApps {
        let text = format!(
            "duration: 10\nquadrants:\n  top-left:\n  - app:\n    name: {top}\n  bottom-left:\n  - app:\n    name: {bottom}\n  top-right:\n  - app:\n    name: none\n  bottom-right:\n  - app:\n    name: none\n"
        );
        crate::layout::Rotation::new(Layout::parse(&text).unwrap()).panel(false)
    }

    /// Proves the brightness range ends (24/12 and 160/35) and the truncation in the middle match Python.
    #[test]
    fn brightness_like_python() {
        assert_eq!(brightness_values(1.0), (160, 35));
        assert_eq!(brightness_values(0.0), (24, 12));
        assert_eq!(brightness_values(0.5), (92, 23));
    }

    /// Proves `render_panel` draws the same as calling the drawing functions by hand, borders included.
    #[test]
    fn cpu_on_top_and_memory_below_match_the_golden_drawing() {
        let mut g = Grid::new();
        render_panel(&mut g, &panel("cpu", "mem-bat"), &mut Fixed, 160, 35);
        let mut expected = Grid::new();
        draw_cpu_vals(&mut expected, &[0.5; 8], 160, 0);
        draw_app_border("cpu", &mut expected, 35, 0, 8);
        draw_memory(&mut expected, 0.4, 160, 16);
        draw_battery(&mut expected, 0.66, false, 160, 19, 0.0);
        draw_app_border("mem", &mut expected, 35, 16, 8);
        assert_eq!(g, expected);
    }

    /// Proves an unknown app name draws nothing instead of failing.
    #[test]
    fn unported_apps_stay_dark() {
        let mut g = Grid::new();
        render_panel(&mut g, &panel("no-such-app", "none"), &mut Fixed, 160, 35);
        assert!(g.is_dark());
    }

    /// Proves the time app draws its four digits with the colon between them and the PM dot.
    #[test]
    fn clock_draws_digits_and_pm() {
        let mut g = Grid::new();
        render_panel(&mut g, &panel("time", "none"), &mut Fixed, 160, 35);
        let mut expected = Grid::new();
        draw_chars_list(&mut expected, &["0", "9", "horiz_colon", "0", "5"], 160, 0);
        draw_pm_indicator(&mut expected, 160);
        assert_eq!(g, expected);
    }
}
