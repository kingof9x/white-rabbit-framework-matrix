//! Apps beyond the CPU, memory and clock that `render.rs` draws itself.
//!
//! Apps that show your own content rather than readings: the Text app, the Picture app (built-in
//! pictures and Draw tab drawings), and the panel names shown by Identify panels.
//!
//! Apps that show readings from the computer: disk and network rates, the GPU graph, FPS, now
//! playing, weather, temperatures and fans. The drawing and the sums are here, so they can be
//! tested; reading the values from Windows is in `wr-monitor`.
//!
//! Where this sits: `render::render_panel` (called on the monitor's main loop for each panel, about
//! every frame) looks at each app's name. Names in `CONTENT_APPS` and `DATA_APPS` are recognised as
//! real apps (anything else is drawn as unknown); the data apps are then dispatched to
//! `render::draw_data_app`, which asks the `Readings` trait for the value and calls the drawing code
//! in these files. Nothing here reads the system, opens files (except `panel_names::load_panel_names`
//! and the picture loader) or starts threads, so every function can be unit-tested with fixed values.

pub mod gpu;
pub mod meters;
pub mod now_playing;
pub mod panel_names;
pub mod picture;
pub mod rates;
pub mod temp_fan;
pub mod text;
pub mod weather;

/// The app names drawn by this module, added to the ones `render.rs` draws itself.
/// Apps that show the user's own content: "text" (a scrolling or fixed text line) and "snap" (the
/// Picture app, which shows a built-in picture or a Draw tab drawing). `render.rs` checks this list
/// to decide whether an app name is known.
pub const CONTENT_APPS: [&str; 2] = ["text", "snap"];

/// The apps in this module, which `render_panel` can draw.
/// Each name is matched in `render::draw_data_app`, so a name added here must also get a branch there,
/// otherwise it would be accepted as known but draw nothing. The fuzz test feeds every name in this
/// list into random layouts.
pub const DATA_APPS: [&str; 10] =
    ["disk", "net", "disk-net", "gpu-graph", "fps", "fps-num", "now-playing", "weather", "temp", "fan"];

/// The fps and fps-num apps' reading: frames a second, the last 9 readings (newest first) for the
/// histogram, and whether it can be measured at all ("?" when not).
/// The value is filled in by `wr-monitor` (which counts presented frames of a process) and passed to
/// `drawing::draw_fps` / `draw_fps_half`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fps {
    /// Frames a second right now. Only meaningful when `available` is true.
    pub value: f64,
    /// Recent readings, newest first, at most 9 (one per column of the histogram on the 9-wide panel).
    pub history: Vec<f64>,
    /// False when no frame rate can be measured (no game running, or the counter is missing); the app
    /// then draws "?" instead of a number.
    pub available: bool,
}

#[cfg(test)]
mod tests {
    use super::now_playing::{Media, NowPlaying};
    use super::weather::Weather;
    use super::Fps;
    use crate::drawing::*;
    use crate::layout::{App, Layout, PanelApps, Rotation};
    use crate::render::{render_panel, Readings};
    use crate::Grid;

    /// A fake `Readings` with fixed values, so the tests below know exactly what each app must draw.
    struct Fixed {
        /// Whether the fake media player reports a song as playing (changes what the now-playing app draws).
        playing: bool,
    }
    impl Readings for Fixed {
        fn cpu_cores(&mut self) -> Vec<f64> {
            vec![]
        }
        fn cpu_history(&mut self) -> Vec<Option<f64>> {
            vec![]
        }
        fn memory(&mut self) -> f64 {
            0.0
        }
        fn battery(&mut self) -> Option<(f64, bool)> {
            None
        }
        fn clock(&mut self, app: &App) -> (String, bool) {
            assert!(app.arg_bool("fmt_24_hour"));
            ("2105".into(), false)
        }
        fn now(&mut self) -> f64 {
            0.0
        }
        fn disk(&mut self) -> (f64, f64) {
            (0.5, 0.25)
        }
        fn net(&mut self) -> (f64, f64) {
            (1.0, 0.1)
        }
        fn fps(&mut self, _: &str) -> Fps {
            Fps { value: 60.0, history: vec![60.0, 30.0], available: true }
        }
        fn fans(&mut self) -> Vec<f64> {
            vec![3000.0]
        }
        fn temperatures(&mut self, _: &App) -> [Option<f64>; 3] {
            [Some(40.0), None, Some(90.0)]
        }
        fn weather(&mut self, _: &App) -> Option<Weather> {
            None
        }
        fn now_playing(&mut self) -> NowPlaying {
            // Paused with a song name still shows the name, as in Python; nothing at all shows the clock
            let title = if self.playing { "A".into() } else { String::new() };
            let media = Media { title, playing: self.playing, ..Default::default() };
            NowPlaying { media, last_cover: None, levels: None }
        }
    }

    /// Builds a one-panel layout with `top` in the top-left quarter and `bottom` in the bottom-left
    /// quarter (the right quarters show nothing), and returns what the left panel shows right now.
    fn panel(top: &str, bottom: &str) -> PanelApps {
        let text = format!(
            "duration: 10\nquadrants:\n  top-left:\n  - app:\n    name: {top}\n  bottom-left:\n  - app:\n    name: {bottom}\n  top-right:\n  - app:\n    name: none\n  bottom-right:\n  - app:\n    name: none\n"
        );
        Rotation::new(Layout::parse(&text).unwrap()).panel(false)
    }

    /// Draws the left panel of that layout with the fake readings, at foreground 160 and background 35
    /// (the brightness values used throughout the tests).
    fn draw(top: &str, bottom: &str, playing: bool) -> Grid {
        let mut g = Grid::new();
        render_panel(&mut g, &panel(top, bottom), &mut Fixed { playing }, 160, 35);
        g
    }

    /// The disk app draws two bars (read on the left, written on the right) in the top half and the net
    /// app does the same in the bottom half, each with its own border.
    #[test]
    fn disk_on_top_and_net_below() {
        let mut expected = Grid::new();
        draw_bar(&mut expected, 0.5, 160, 1, 0);
        draw_bar(&mut expected, 0.25, 160, 5, 0);
        draw_app_border("disk", &mut expected, 35, 0, 8);
        draw_bar(&mut expected, 1.0, 160, 1, 16);
        draw_bar(&mut expected, 0.1, 160, 5, 16);
        draw_app_border("net", &mut expected, 35, 16, 8);
        assert_eq!(draw("disk", "net", false), expected);
    }

    /// Fan bars, temperature numbers, the disk-net combined app and the fps-num number each come out
    /// exactly as the drawing functions draw them on their own.
    #[test]
    fn fans_temps_and_fps() {
        let mut expected = Grid::new();
        draw_bar(&mut expected, 0.5, 160, 1, 0);
        draw_bar(&mut expected, 0.0, 160, 5, 0);
        draw_app_border("fan", &mut expected, 35, 0, 8);
        draw_temp_numbers(&mut expected, &[Some(40.0), None, Some(90.0)], 160, 16);
        assert_eq!(draw("fan", "temp", false), expected);

        let mut expected = Grid::new();
        // Network on top, disk under it; no drive space reading here, so no meter
        super::meters::draw_net_disk_space(&mut expected, (1.0, 0.1), (0.5, 0.25), None, 160, 0);
        super::meters::draw_net_disk_space_border(&mut expected, 35, 0);
        draw_fps_half(&mut expected, 60.0, true, 160, 16);
        assert_eq!(draw("disk-net", "fps-num", false), expected);
    }

    /// Without a weather reading (no internet, or not fetched yet) the weather app shows two "?".
    #[test]
    fn no_weather_shows_question_marks() {
        let mut expected = Grid::new();
        draw_chars_list(&mut expected, &["?", "?"], 160, 0);
        assert_eq!(draw("weather", "none", false), expected);
    }

    /// With nothing playing the now-playing app falls back to a 24-hour clock; with a song playing it
    /// draws something else that is not blank.
    #[test]
    fn now_playing_shows_the_clock_when_nothing_plays() {
        let mut clock = Grid::new();
        draw_chars_list(&mut clock, &["2", "1", "horiz_colon", "0", "5"], 160, 0);
        assert_eq!(draw("now-playing", "none", false), clock);
        let playing = draw("now-playing", "none", true);
        assert_ne!(playing, clock);
        assert!(!playing.is_dark());
    }
}
