//! The layout editor's live preview (`led_mon/editor/preview.py`): draws a page the way the monitor
//! does. CPU, memory, battery, disk, network and the clock are this computer's own; temperatures, fans,
//! GPU use, FPS, the weather and what is playing show made-up values, so the window doesn't have to
//! read the sensors itself (the panels show the real ones). The audio visualizer plays made-up music.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value;
use sysinfo::{CpuRefreshKind, DiskRefreshKind, Disks, Networks, RefreshKind, System};
use wr_core::apps::now_playing::{Media, NowPlaying};
use wr_core::apps::rates::RateMonitor;
use wr_core::apps::temp_fan;
use wr_core::apps::weather::Weather;
use wr_core::apps::Fps;
use wr_core::grid::Grid;
use wr_core::layout::{App, PanelApps};
use wr_core::monitors::{core_groups, CpuSmoother, History};
use wr_core::render::{render_panel_on, Readings};
use wr_music::{to_leds, Visualizer};

use super::catalog;
use super::model::{Entry, Page};

pub const FOREGROUND: i32 = 160;
pub const BACKGROUND: i32 = 35;

fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub struct PreviewReadings {
    cores: System,
    overall: System,
    memory: System,
    smoother: CpuSmoother,
    history: History,
    gpu_history: History,
    started: Instant,
    right: bool,
    disks: Disks,
    networks: Networks,
    disk_rates: RateMonitor,
    net_rates: RateMonitor,
}

impl PreviewReadings {
    pub fn new() -> PreviewReadings {
        let cpu_only = RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing().with_cpu_usage());
        let cores = System::new_with_specifics(cpu_only);
        let overall = System::new_with_specifics(cpu_only);
        let logical = cores.cpus().len().max(1);
        let physical = System::physical_core_count().unwrap_or(logical);
        PreviewReadings {
            cores,
            overall,
            memory: System::new(),
            smoother: CpuSmoother::new(core_groups(logical, physical)),
            history: History::new(),
            gpu_history: History::new(),
            started: Instant::now(),
            right: false,
            disks: Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_io_usage()),
            networks: Networks::new_with_refreshed_list(),
            disk_rates: RateMonitor::new(),
            net_rates: RateMonitor::new(),
        }
    }

    fn t(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
}

/// A slow made-up wave between 0 and 1.
fn wave(t: f64, speed: f64, phase: f64) -> f64 {
    0.5 + 0.4 * (t * speed + phase).sin()
}

impl Readings for PreviewReadings {
    fn cpu_cores(&mut self) -> Vec<f64> {
        self.cores.refresh_cpu_usage();
        let percent: Vec<f64> = self.cores.cpus().iter().map(|c| c.cpu_usage() as f64).collect();
        self.smoother.add(&percent)
    }

    fn cpu_history(&mut self) -> Vec<Option<f64>> {
        let now = self.t();
        let overall = &mut self.overall;
        self.history.get(now, || {
            overall.refresh_cpu_usage();
            Some(overall.global_cpu_usage() as f64 / 100.0)
        })
    }

    fn memory(&mut self) -> f64 {
        self.memory.refresh_memory();
        let total = self.memory.total_memory();
        if total == 0 {
            return 0.0;
        }
        (total - self.memory.available_memory()) as f64 / total as f64
    }

    fn battery(&mut self) -> Option<(f64, bool)> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
            // SAFETY: plain Win32 call filling a struct.
            let s = unsafe {
                let mut s: SYSTEM_POWER_STATUS = std::mem::zeroed();
                if GetSystemPowerStatus(&mut s) == 0 {
                    return None;
                }
                s
            };
            // 128: no battery; 255: unknown charge
            if s.BatteryFlag == 128 || s.BatteryLifePercent == 255 {
                return None;
            }
            Some((s.BatteryLifePercent as f64 / 100.0, s.ACLineStatus == 1))
        }
        #[cfg(not(windows))]
        {
            Some((0.72, true))
        }
    }

    fn clock(&mut self, app: &App) -> (String, bool) {
        use chrono::Timelike;
        // The preview shows local time; the panels follow the app's timezone setting
        let t = chrono::Local::now();
        let (hour, minute) = (t.hour(), t.minute());
        if app.arg_bool("fmt_24_hour") {
            (format!("{hour:02}{minute:02}"), false)
        } else {
            let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
            (format!("{h12:02}{minute:02}"), hour >= 12)
        }
    }

    fn now(&mut self) -> f64 {
        self.t()
    }

    fn set_panel(&mut self, right: bool) {
        self.right = right;
    }

    fn panel_right(&mut self) -> bool {
        self.right
    }

    fn disk(&mut self) -> (f64, f64) {
        self.disks.refresh_specifics(false, DiskRefreshKind::nothing().with_io_usage());
        let (read, written) = self.disks.list().iter().fold((0u64, 0u64), |(r, w), d| {
            let u = d.usage();
            (r.saturating_add(u.total_read_bytes), w.saturating_add(u.total_written_bytes))
        });
        self.disk_rates.add(read, written, unix_now())
    }

    fn net(&mut self) -> (f64, f64) {
        self.networks.refresh(true);
        let (sent, received) = self.networks.iter().fold((0u64, 0u64), |(s, r), (_, n)| {
            (s.saturating_add(n.total_transmitted()), r.saturating_add(n.total_received()))
        });
        self.net_rates.add(sent, received, unix_now())
    }

    fn gpu_history(&mut self, _gpu: &str) -> Vec<Option<f64>> {
        let now = self.t();
        self.gpu_history.get(now, || Some(wave(now, 0.7, 1.0)))
    }

    fn fps(&mut self, _process: &str) -> Fps {
        let t = self.t();
        let history: Vec<f64> = (0..9).map(|i| 60.0 + 25.0 * (t * 0.5 - i as f64 * 0.4).sin()).collect();
        Fps { value: history[0], history, available: true }
    }

    fn now_playing(&mut self) -> NowPlaying {
        // A made-up song and cover, so the preview shows what the app looks like
        let n = wr_core::apps::now_playing::COVER;
        let cover: Vec<f64> = (0..n * n)
            .map(|i| {
                let (x, y) = ((i % n) as f64 - 8.5, (i / n) as f64 - 8.5);
                let d = (x * x + y * y).sqrt();
                if d < 3.0 {
                    0.15
                } else if d < 8.5 {
                    0.35 + 0.6 * ((d * 1.3).sin() * 0.5 + 0.5)
                } else {
                    0.0
                }
            })
            .collect();
        let t = self.t();
        let levels: Vec<f64> = (0..9).map(|i| 3.5 + 3.0 * (t * 3.0 + i as f64).sin()).collect();
        NowPlaying {
            media: Media {
                title: "Song title".into(),
                artist: "Artist".into(),
                playing: true,
                cover: Some(cover.clone()),
            },
            last_cover: Some(cover),
            levels: Some(levels),
        }
    }

    fn weather(&mut self, app: &App) -> Option<Weather> {
        let metric = app.arg_str("units").map(|u| u != "imperial").unwrap_or(true);
        Some(Weather {
            temp: if metric { 18.0 } else { 64.0 },
            wind_chill: if metric { 16.0 } else { 61.0 },
            wind_speed: 12.0,
            wind_speed_symbol: if metric { "km".into() } else { "mi".into() },
            wind_dir: 225.0,
            temp_symbol: if metric { "degC".into() } else { "degF".into() },
            condition: "clouds".into(),
        })
    }

    fn temperatures(&mut self, app: &App) -> [Option<f64>; 3] {
        let unit = app.arg_str("unit").unwrap_or_else(|| "C".into());
        let t = self.t();
        [48.0, 55.0, 37.0].map(|c: f64| Some(temp_fan::to_unit(c + 3.0 * (t * 0.3 + c).sin(), &unit)))
    }

    fn fans(&mut self) -> Vec<f64> {
        let t = self.t();
        vec![2400.0 + 600.0 * (t * 0.4).sin(), 3100.0 + 500.0 * (t * 0.3).cos()]
    }
}

/// A finalized entry as the monitor's App.
pub fn to_app(entry: &Entry, whole_panel: bool) -> App {
    let args = entry
        .get("args")
        .and_then(|a| serde_yaml::to_value(a).ok())
        .and_then(|v| v.as_mapping().cloned())
        .unwrap_or_default();
    let flag = |k: &str| super::model::is_true(entry.get(k));
    App {
        name: entry.get("name").and_then(Value::as_str).unwrap_or("none").to_string(),
        duration: 30,
        whole_panel,
        display: entry.get("display").map(|d| *d != Value::Bool(false)).unwrap_or(true),
        animate: flag("animate"),
        persistent_draw: flag("persistent-draw"),
        args,
    }
}

fn blank() -> App {
    App {
        name: "none".into(),
        duration: 30,
        whole_panel: false,
        display: false,
        animate: false,
        persistent_draw: false,
        args: Default::default(),
    }
}

/// Draws pages for the preview, keeping the audio visualizers it plays between frames.
pub struct Renderer {
    pub readings: PreviewReadings,
    visualizers: HashMap<String, (Visualizer, Instant)>,
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer { readings: PreviewReadings::new(), visualizers: HashMap::new() }
    }

    /// The 9 x 34 picture of one page.
    pub fn render(&mut self, page: Option<&Page>, side: &str) -> Grid {
        let mut grid = Grid::new();
        let Some(page) = page else { return grid };
        self.readings.set_panel(side == "right");
        if let Some(full) = &page.full {
            let entry = catalog::finalize(full.clone(), side);
            if entry.get("name").and_then(Value::as_str) == Some("equalizer") {
                return self.equalizer(&entry, side);
            }
            let apps = PanelApps::Whole(to_app(&entry, true));
            render_panel_on(&mut grid, &apps, &mut self.readings, FOREGROUND, BACKGROUND, side);
        } else {
            let half = |e: &Option<Entry>| {
                e.as_ref().map(|e| to_app(&catalog::finalize(e.clone(), side), false)).unwrap_or_else(blank)
            };
            let apps = PanelApps::Halves(half(&page.top), half(&page.bottom));
            render_panel_on(&mut grid, &apps, &mut self.readings, FOREGROUND, BACKGROUND, side);
        }
        for col in grid.v.iter_mut() {
            for v in col.iter_mut() {
                *v = (*v).clamp(0, 255);
            }
        }
        grid
    }

    /// The audio visualizer draws on the panel itself, so the preview plays made-up music through the look.
    fn equalizer(&mut self, entry: &Entry, side: &str) -> Grid {
        let args = entry.get("args").cloned().unwrap_or(Value::Null);
        let settings = wr_music::from_json(&args);
        let key = format!("{side}{}", args);
        if self.visualizers.len() > 16 {
            self.visualizers.clear();
        }
        let now = Instant::now();
        let (visualizer, last) = self.visualizers.entry(key).or_insert_with(|| {
            (Visualizer::new(&settings, side == "right", Some(0)), now - std::time::Duration::from_millis(30))
        });
        let dt = now.duration_since(*last).as_secs_f64().clamp(0.001, 0.5);
        *last = now;
        let t = self.readings.t();
        let frame = visualizer.frame(&wr_music::demo::demo_samples(t), dt);
        to_leds(&frame, FOREGROUND)
    }
}

#[cfg(test)]
mod tests {
    use super::super::model;
    use super::*;

    #[test]
    fn draws_every_app() {
        let mut renderer = Renderer::new();
        for info in catalog::offered() {
            let entry = catalog::new_app(info.key, "left", Some("drawing:none".into()));
            let page = if info.size == catalog::Size::Full {
                model::Page { full: Some(entry), ..model::Page::default() }
            } else {
                model::Page { top: Some(entry.clone()), bottom: Some(entry), ..model::Page::default() }
            };
            let grid = renderer.render(Some(&page), "left");
            let lit = grid.v.iter().flatten().filter(|v| **v > 0).count();
            // A drawing that doesn't exist and an empty text stay dark; the rest show something
            if !matches!(info.key, "snap" | "text") {
                assert!(lit > 0, "{} drew nothing", info.key);
            }
        }
    }
}
