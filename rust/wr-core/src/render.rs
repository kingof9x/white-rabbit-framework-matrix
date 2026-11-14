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
}

/// The apps this monitor can draw so far. The others show nothing until they are ported.
pub const SUPPORTED_APPS: [&str; 4] = ["cpu", "cpu-graph", "mem-bat", "time"];

pub fn is_supported(name: &str) -> bool {
    SUPPORTED_APPS.contains(&name) || crate::apps::CONTENT_APPS.contains(&name)
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
            "text" => crate::apps::text::draw_app(grid, app, readings, fg),
            "snap" => crate::apps::picture::draw_app(grid, app, fg),
            _ => {}
        }
        animate = app.animate;
        if app.border() && !whole {
            draw_app_border(&app.name, grid, bg, y, cores);
        }
    }
    animate
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
        render_panel(&mut g, &panel("disk", "weather"), &mut Fixed, 160, 35);
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
