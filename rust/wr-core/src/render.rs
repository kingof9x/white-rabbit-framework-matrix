//! Drawing one panel's frame from the apps it shows now: a port of `render_panel` in
//! `led_mon/rendering.py` and the app-name display of the main loop.

use crate::drawing::*;
use crate::layout::{App, PanelApps};
use crate::Grid;

/// What the apps show, read from the computer. The monitor exe reads Windows; tests use fixed values.
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
}

/// The apps this monitor can draw so far. The others show nothing until they are ported.
pub const SUPPORTED_APPS: [&str; 4] = ["cpu", "cpu-graph", "mem-bat", "time"];

pub fn is_supported(name: &str) -> bool {
    SUPPORTED_APPS.contains(&name) || crate::apps::DATA_APPS.contains(&name)
}

/// LED values for the screen brightness 0.0-1.0: (foreground, background).
pub fn brightness_values(ratio: f64) -> (i32, i32) {
    let fg = crate::pyround::int(ratio * (160.0 - 24.0) + 24.0) as i32;
    let bg = crate::pyround::int(ratio * (35.0 - 12.0) + 12.0) as i32;
    (fg.clamp(0, 255), bg.clamp(0, 255))
}

/// Draws the panel's apps into `grid`. Returns the `animate` setting of the last app drawn.
pub fn render_panel(grid: &mut Grid, apps: &PanelApps, readings: &mut dyn Readings, fg: i32, bg: i32) -> bool {
    let whole = matches!(apps, PanelApps::Whole(_));
    let mut animate = false;
    for (j, app) in apps.apps().into_iter().enumerate() {
        if !app.display {
            continue;
        }
        let y = if j == 0 { 0 } else { 16 };
        if !is_supported(&app.name) {
            continue;
        }
        let mut cores = 0;
        match app.name.as_str() {
            "cpu" => {
                let values = readings.cpu_cores();
                cores = values.len();
                draw_cpu_vals(grid, &values, fg, y);
            }
            "cpu-graph" => draw_history_graph(grid, &readings.cpu_history(), fg, y),
            "mem-bat" => {
                draw_memory(grid, readings.memory(), fg, y);
                if let Some((ratio, plugged)) = readings.battery() {
                    let now = readings.now();
                    draw_battery(grid, ratio, plugged, fg, y + 3, now);
                }
            }
            "time" => {
                let (digits, pm) = readings.clock(app);
                let d: Vec<String> = digits.chars().map(|c| c.to_string()).collect();
                let chars: Vec<&str> = vec![&d[0], &d[1], "horiz_colon", &d[2], &d[3]];
                draw_chars_list(grid, &chars, fg, y);
                if pm {
                    draw_pm_indicator(grid, fg);
                }
            }
            name if crate::apps::DATA_APPS.contains(&name) => draw_data_app(grid, app, readings, fg, y),
            _ => {}
        }
        animate = app.animate;
        if app.border() && !whole {
            draw_app_border(&app.name, grid, bg, y, cores);
        }
    }
    animate
}

/// The apps in `crate::apps`: disk and network, GPU, FPS, now playing, weather, temperatures and fans.
fn draw_data_app(grid: &mut Grid, app: &App, readings: &mut dyn Readings, fg: i32, y: usize) {
    use crate::apps::{now_playing, temp_fan, weather};
    let arg = |key: &str| app.arg_str(key).unwrap_or_default();
    match app.name.as_str() {
        "disk" | "net" => {
            let (a, b) = if app.name == "disk" { readings.disk() } else { readings.net() };
            draw_bar(grid, a, fg, 1, y);
            draw_bar(grid, b, fg, 5, y);
        }
        "disk-net" => {
            let (disk, net) = (readings.disk(), readings.net());
            draw_disk_net(grid, disk, net, fg, y);
        }
        "gpu-graph" => draw_history_graph(grid, &readings.gpu_history(&arg("gpu")), fg, y),
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
                    let int_arg = |key: &str, default: i64| {
                        app.arg_str(key).and_then(|s| s.trim().parse::<f64>().ok()).map(|v| v as i64).unwrap_or(default)
                    };
                    weather::draw_forecast_period(grid, fg, int_arg("forecast_day", 1), int_arg("forecast_hour", 12));
                }
            }
            None => draw_chars_list(grid, &["?", "?"], fg, y),
        },
        "now-playing" => {
            let opts = now_playing::Options {
                title: app.arg("title").map(|_| app.arg_bool("title")).unwrap_or(true),
                visualizer: app.arg("visualizer").map(|_| app.arg_bool("visualizer")).unwrap_or(true),
                drift: app.arg("drift").map(|_| app.arg_bool("drift")).unwrap_or(true),
                when_idle: app.arg_str("when_idle").unwrap_or_else(|| "skip".into()),
            };
            let right = match arg("side").as_str() {
                "left" => false,
                "right" => true,
                _ => readings.panel_right(),
            };
            let np = readings.now_playing();
            let now = readings.now();
            if !now_playing::draw_now_playing(grid, &np, &opts, right, now, fg) {
                // The clock, 24-hour, from the top
                let mut clock_app = app.clone();
                clock_app.args = serde_yaml::Mapping::new();
                clock_app.args.insert("fmt_24_hour".into(), true.into());
                let (digits, _) = readings.clock(&clock_app);
                let d: Vec<String> = digits.chars().map(|c| c.to_string()).collect();
                if d.len() == 4 {
                    draw_chars_list(grid, &[&d[0], &d[1], "horiz_colon", &d[2], &d[3]], fg, 0);
                }
            }
        }
        _ => {}
    }
}

/// The names of the panel's apps, inside an outline (Ctrl+Alt+Shift+I, or Identify panels).
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

    fn panel(top: &str, bottom: &str) -> PanelApps {
        let text = format!(
            "duration: 10\nquadrants:\n  top-left:\n  - app:\n    name: {top}\n  bottom-left:\n  - app:\n    name: {bottom}\n  top-right:\n  - app:\n    name: none\n  bottom-right:\n  - app:\n    name: none\n"
        );
        crate::layout::Rotation::new(Layout::parse(&text).unwrap()).panel(false)
    }

    #[test]
    fn brightness_like_python() {
        assert_eq!(brightness_values(1.0), (160, 35));
        assert_eq!(brightness_values(0.0), (24, 12));
        assert_eq!(brightness_values(0.5), (92, 23));
    }

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

    #[test]
    fn unported_apps_stay_dark() {
        let mut g = Grid::new();
        render_panel(&mut g, &panel("no-such-app", "none"), &mut Fixed, 160, 35);
        assert!(g.is_dark());
    }

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
