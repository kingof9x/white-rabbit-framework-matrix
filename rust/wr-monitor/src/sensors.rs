//! What the apps show, read from Windows: CPU load, memory, battery, the clock, and the screen's
//! brightness (the same sources as `led_mon/monitors.py`); disk and network rates, GPU use,
//! temperatures and fans, the weather and what is playing (the slower ones on their own threads,
//! see `background.rs`).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use std::collections::HashMap;

use sysinfo::{CpuRefreshKind, DiskRefreshKind, Disks, Networks, RefreshKind, System};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use wr_core::apps::now_playing::NowPlaying;
use wr_core::apps::rates::RateMonitor;
use wr_core::apps::temp_fan::{self, Thermal};
use wr_core::apps::weather::Weather;
use wr_core::apps::{gpu, Fps};
use wr_core::layout::App;
use wr_core::monitors::{core_groups, CpuSmoother, History};
use wr_core::render::Readings;

use wr_core::warn;

use crate::background::Background;
use crate::fps::FpsReader;
use crate::weather::WeatherSettings;

pub struct WindowsReadings {
    cores: System,
    overall: System,
    smoother: CpuSmoother,
    history: History,
    started: Instant,
    right: bool,
    disks: Disks,
    networks: Networks,
    disk_rates: RateMonitor,
    net_rates: RateMonitor,
    /// Every physical disk's bytes read and written, from the performance counters
    all_disks: Background<Option<(u64, u64)>>,
    /// The drives' sizes and free space, read every 10 seconds
    drives: Disks,
    drives_read: Option<Instant>,
    gpu_memory: Background<Vec<(String, Option<f64>)>>,
    gpus: Background<Vec<(String, Option<f64>)>>,
    gpu_histories: HashMap<String, History>,
    thermal: Background<Thermal>,
    weather: HashMap<String, Background<Option<Weather>>>,
    media: Background<NowPlaying>,
    fps: Arc<Mutex<FpsReader>>,
}

/// Seconds since 1970, as Python's time.time().
fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

impl WindowsReadings {
    pub fn new() -> WindowsReadings {
        let cpu_only = RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing().with_cpu_usage());
        let cores = System::new_with_specifics(cpu_only);
        let overall = System::new_with_specifics(cpu_only);
        let logical = cores.cpus().len();
        let physical = System::physical_core_count().unwrap_or(logical);
        WindowsReadings {
            cores,
            overall,
            smoother: CpuSmoother::new(core_groups(logical, physical)),
            history: History::new(),
            started: Instant::now(),
            right: false,
            disks: Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_io_usage()),
            networks: Networks::new_with_refreshed_list(),
            disk_rates: RateMonitor::new(),
            net_rates: RateMonitor::new(),
            all_disks: Background::new("disks", Duration::from_secs(1), None, |_| crate::counters::disk_reader()),
            drives: Disks::new(),
            drives_read: None,
            gpu_memory: Background::new("gpu-memory", Duration::from_secs(2), Vec::new(), |_| {
                crate::counters::gpu_memory_reader()
            }),
            gpus: Background::new("gpu", Duration::from_secs(1), Vec::new(), |_| crate::gpu::reader()),
            gpu_histories: HashMap::new(),
            thermal: Background::new("sensors", Duration::from_secs(2), Thermal::default(), crate::thermal::reader),
            weather: HashMap::new(),
            media: Background::new("now-playing", Duration::from_secs(1), NowPlaying::default(), |_| {
                crate::media::reader()
            }),
            fps: crate::fps::shared(),
        }
    }
}

impl Readings for WindowsReadings {
    fn cpu_cores(&mut self) -> Vec<f64> {
        self.cores.refresh_cpu_usage();
        let percent: Vec<f64> = self.cores.cpus().iter().map(|c| c.cpu_usage() as f64).collect();
        self.smoother.add(&percent)
    }

    fn cpu_history(&mut self) -> Vec<Option<f64>> {
        let now = self.started.elapsed().as_secs_f64();
        let overall = &mut self.overall;
        self.history.get(now, || {
            overall.refresh_cpu_usage();
            Some(overall.global_cpu_usage() as f64 / 100.0)
        })
    }

    fn memory(&mut self) -> f64 {
        // SAFETY: the struct has its length set, as Windows needs.
        unsafe {
            let mut m: MEMORYSTATUSEX = std::mem::zeroed();
            m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut m) == 0 || m.ullTotalPhys == 0 {
                return 0.0;
            }
            // psutil's virtual_memory().percent, rounded to one decimal like it
            let percent =
                ((m.ullTotalPhys - m.ullAvailPhys) as f64 / m.ullTotalPhys as f64 * 100.0 * 10.0).round() / 10.0;
            percent / 100.0
        }
    }

    fn battery(&mut self) -> Option<(f64, bool)> {
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

    fn clock(&mut self, app: &App) -> (String, bool) {
        clock_digits(app)
    }

    fn now(&mut self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn set_panel(&mut self, right: bool) {
        self.right = right;
    }

    fn panel_right(&mut self) -> bool {
        self.right
    }

    fn disk(&mut self) -> (f64, f64) {
        // Every SSD in the laptop, with or without a drive letter
        if let Some((read, written)) = self.all_disks.get() {
            return self.disk_rates.add(read, written, unix_now());
        }
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

    fn gpu_history(&mut self, name: &str) -> Vec<Option<f64>> {
        let rows = self.gpus.get();
        let now = self.started.elapsed().as_secs_f64();
        self.gpu_histories.entry(name.to_string()).or_default().get(now, || gpu::pick(&rows, name))
    }

    fn fps(&mut self, process: &str) -> Fps {
        self.fps.lock().unwrap_or_else(|e| e.into_inner()).read(process, Instant::now())
    }

    fn now_playing(&mut self) -> NowPlaying {
        self.media.get()
    }

    fn weather(&mut self, app: &App) -> Option<Weather> {
        let settings = WeatherSettings::of(app);
        self.weather
            .entry(settings.key())
            .or_insert_with(|| {
                Background::new("weather", crate::weather::REFRESH, None, move |_| crate::weather::reader(settings))
            })
            .get()
    }

    fn temperatures(&mut self, app: &App) -> [Option<f64>; 3] {
        let thermal = self.thermal.get();
        let chosen: Vec<String> =
            ["sensor_1", "sensor_2", "sensor_3"].iter().map(|k| app.arg_str(k).unwrap_or_default()).collect();
        let unit = app.arg_str("unit").unwrap_or_else(|| "C".into());
        let values = temp_fan::pick(&thermal.named, &chosen, 3);
        let mut result = [None; 3];
        for (slot, v) in result.iter_mut().zip(values) {
            *slot = v.map(|c| temp_fan::to_unit(c, &unit));
        }
        result
    }

    fn fans(&mut self) -> Vec<f64> {
        self.thermal.get().fans
    }

    fn gpu_memory(&mut self, gpu: &str) -> Option<f64> {
        gpu::pick(&self.gpu_memory.get(), gpu)
    }

    fn drive_space(&mut self, drive: &str) -> Option<f64> {
        if self.drives_read.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
            self.drives_read = Some(Instant::now());
            self.drives.refresh_specifics(true, DiskRefreshKind::nothing().with_storage());
        }
        drive_share(&self.drives, drive)
    }
}

/// How full the drive is ("C:", "c", "C:\\" all work), 0.0-1.0.
pub fn drive_share(drives: &Disks, drive: &str) -> Option<f64> {
    let letter = drive.trim().trim_end_matches(['\\', '/', ':']).to_ascii_uppercase();
    drives
        .list()
        .iter()
        .find(|d| d.mount_point().to_string_lossy().trim_end_matches(['\\', '/', ':']).eq_ignore_ascii_case(&letter))
        .filter(|d| d.total_space() > 0)
        .map(|d| (d.total_space() - d.available_space()) as f64 / d.total_space() as f64)
}

/// The clock app's four digits and PM dot, in the app's timezone (local time when blank).
pub fn clock_digits(app: &App) -> (String, bool) {
    use chrono::Timelike;
    let fmt_24 = app.arg_bool("fmt_24_hour");
    let tz = app.arg_str("timezone").filter(|t| !t.trim().is_empty());
    let (hour, minute) = match tz.as_deref().map(str::parse::<chrono_tz::Tz>) {
        Some(Ok(tz)) => {
            let t = chrono::Utc::now().with_timezone(&tz);
            (t.hour(), t.minute())
        }
        _ => {
            let t = chrono::Local::now();
            (t.hour(), t.minute())
        }
    };
    if fmt_24 {
        (format!("{hour:02}{minute:02}"), false)
    } else {
        let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
        (format!("{h12:02}{minute:02}"), hour >= 12)
    }
}

/// The screen's brightness from WMI, read on its own thread every 2 seconds while it is wanted.
pub struct ScreenBrightness {
    value: Arc<AtomicU64>,
    wanted: Arc<AtomicBool>,
}

#[derive(serde::Deserialize)]
#[serde(rename = "WmiMonitorBrightness")]
struct WmiBrightness {
    #[serde(rename = "CurrentBrightness")]
    current: u8,
}

impl ScreenBrightness {
    pub fn start() -> ScreenBrightness {
        let value = Arc::new(AtomicU64::new(1.0f64.to_bits()));
        let wanted = Arc::new(AtomicBool::new(true));
        let (v, w) = (value.clone(), wanted.clone());
        std::thread::Builder::new()
            .name("brightness".into())
            .spawn(move || {
                let connection =
                    wmi::COMLibrary::new().and_then(|com| wmi::WMIConnection::with_namespace_path("root\\wmi", com));
                let connection = match connection {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("Can't read the screen brightness ({e}); the panels stay at full brightness");
                        return;
                    }
                };
                let mut warned = false;
                loop {
                    if w.swap(false, Ordering::SeqCst) {
                        let read: Result<Vec<WmiBrightness>, _> =
                            connection.raw_query("SELECT CurrentBrightness FROM WmiMonitorBrightness");
                        let ratio = match read {
                            Ok(list) if !list.is_empty() => list[0].current as f64 / 100.0,
                            Ok(_) => 1.0,
                            Err(e) => {
                                if !warned {
                                    warn!("Can't read the screen brightness: {e}");
                                    warned = true;
                                }
                                1.0
                            }
                        };
                        v.store(ratio.to_bits(), Ordering::SeqCst);
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
            .expect("brightness thread");
        ScreenBrightness { value, wanted }
    }

    /// The last reading, 0.0-1.0 (1.0 until the first one).
    pub fn get(&self) -> f64 {
        self.wanted.store(true, Ordering::SeqCst);
        f64::from_bits(self.value.load(Ordering::SeqCst))
    }
}
