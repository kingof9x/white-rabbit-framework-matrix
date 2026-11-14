//! The apps that show readings from the computer besides the CPU and memory: disk and network
//! rates, the GPU graph, FPS, now playing, weather, temperatures and fans. The drawing and the
//! sums are here, so they can be tested; reading the values from Windows is in `wr-monitor`.

pub mod gpu;
pub mod now_playing;
pub mod rates;
pub mod temp_fan;
pub mod weather;

/// The apps in this module, which `render_panel` can draw.
pub const DATA_APPS: [&str; 10] =
    ["disk", "net", "disk-net", "gpu-graph", "fps", "fps-num", "now-playing", "weather", "temp", "fan"];

/// The fps and fps-num apps' reading: frames a second, the last 9 readings (newest first) for the
/// histogram, and whether it can be measured at all ("?" when not).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fps {
    pub value: f64,
    pub history: Vec<f64>,
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

    struct Fixed {
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

    fn panel(top: &str, bottom: &str) -> PanelApps {
        let text = format!(
            "duration: 10\nquadrants:\n  top-left:\n  - app:\n    name: {top}\n  bottom-left:\n  - app:\n    name: {bottom}\n  top-right:\n  - app:\n    name: none\n  bottom-right:\n  - app:\n    name: none\n"
        );
        Rotation::new(Layout::parse(&text).unwrap()).panel(false)
    }

    fn draw(top: &str, bottom: &str, playing: bool) -> Grid {
        let mut g = Grid::new();
        render_panel(&mut g, &panel(top, bottom), &mut Fixed { playing }, 160, 35);
        g
    }

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

    #[test]
    fn fans_temps_and_fps() {
        let mut expected = Grid::new();
        draw_bar(&mut expected, 0.5, 160, 1, 0);
        draw_bar(&mut expected, 0.0, 160, 5, 0);
        draw_app_border("fan", &mut expected, 35, 0, 8);
        draw_temp_numbers(&mut expected, &[Some(40.0), None, Some(90.0)], 160, 16);
        assert_eq!(draw("fan", "temp", false), expected);

        let mut expected = Grid::new();
        draw_disk_net(&mut expected, (0.5, 0.25), (1.0, 0.1), 160, 0);
        draw_app_border("disk-net", &mut expected, 35, 0, 8);
        draw_fps_half(&mut expected, 60.0, true, 160, 16);
        assert_eq!(draw("disk-net", "fps-num", false), expected);
    }

    #[test]
    fn no_weather_shows_question_marks() {
        let mut expected = Grid::new();
        draw_chars_list(&mut expected, &["?", "?"], 160, 0);
        assert_eq!(draw("weather", "none", false), expected);
    }

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
