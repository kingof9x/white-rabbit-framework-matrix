//! What the apps show, read from Windows: CPU load, memory, battery, the clock, and the screen's
//! brightness (the same sources as `led_mon/monitors.py`).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sysinfo::{CpuRefreshKind, RefreshKind, System};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use wr_core::layout::App;
use wr_core::monitors::{core_groups, CpuSmoother, History};
use wr_core::render::Readings;

use wr_core::warn;

pub struct WindowsReadings {
    cores: System,
    overall: System,
    smoother: CpuSmoother,
    history: History,
    started: Instant,
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
