//! Frames per second of a game or other app, for the fps apps, as `led_mon/fps.py` measures them.
//!
//! Windows has no simple "FPS of this app" reading, so this runs Intel's PresentMon console tool
//! (MIT licence, https://github.com/GameTechDev/PresentMon), which the installer puts next to
//! WhiteRabbit.exe, and reads the CSV it prints: one row per frame any app presents.
//!
//! PresentMon needs administrator rights, or the Windows account in the "Performance Log Users"
//! group. Without either it exits with "access denied", and the app shows a "?".
//!
//! # How it fits together
//!
//! `FpsReader` keeps one `Capture` per program asked for (blank = the game in front). A capture owns
//! one PresentMon process; two helper threads per process read its stdout (CSV rows into
//! `FrameStats`) and its stderr (to spot "access denied" when it exits). The main loop calls
//! `FpsReader::read` while an fps app is drawn, which starts PresentMon if needed and works out the
//! FPS from the rows collected so far. A separate `fps-idle` thread (`shared`) calls `tick` every
//! 2 s so PresentMon is stopped 15 s after the last fps app went off screen, even with the panels off.
//! Every PresentMon is put in a Windows "job object" that kills it if White Rabbit dies.
//!
//! The logic (CSV parsing, FPS maths, game picking, start/stop timing) is platform-neutral and
//! tested with a fake `Runner`; only `mod windows` touches the real exe and Win32.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use wr_core::apps::Fps;

/// Its own trace session, so the window's game picker (Python, "led_mon_fps") doesn't stop it.
pub const SESSION_NAME: &str = "WhiteRabbit_fps";
/// Stop PresentMon when no fps app has been drawn for this long.
const IDLE: Duration = Duration::from_secs(15);
/// Wait this long before trying again after PresentMon failed to start.
const RETRY: Duration = Duration::from_secs(30);
/// A process that presented nothing for this long shows 0 FPS.
const STALE: Duration = Duration::from_secs(2);
/// Readings kept for the histogram under the number, one a second.
const HISTORY: usize = 9;

/// Whether a capture can measure; anything but `Ok` shows as "?" on the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    /// PresentMon started (or is starting) normally.
    Ok,
    /// PresentMon.exe wasn't found or couldn't start.
    NotAvailable,
    /// PresentMon couldn't start its trace session.
    NoPermission,
}

/// One CSV line split into fields; PresentMon quotes fields that hold commas.
///
/// Inside quotes, a doubled quote `""` stands for one `"`. A trailing CR/LF is ignored. Never fails:
/// malformed input just gives odd fields, which the caller then fails to parse and skips.
pub fn split_csv(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.trim_end_matches(['\r', '\n']).chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    fields.push(field);
    fields
}

/// Recent frames per (process, swap chain), from PresentMon's CSV rows.
///
/// Fed one line at a time from PresentMon's stdout thread, read from the main loop; it sits behind a
/// mutex in `Capture`.
#[derive(Default)]
pub struct FrameStats {
    /// Column name -> index, from the header line. Empty until the header has been seen.
    columns: HashMap<String, usize>,
    /// (pid, swap chain) -> (present time in seconds, ms between presents)
    frames: HashMap<(u32, String), VecDeque<(f64, f64)>>,
    /// pid -> exe name as PresentMon reports it (e.g. "game.exe").
    names: HashMap<u32, String>,
    /// pid -> when its last frame row arrived (our clock, not PresentMon's).
    last_seen: HashMap<u32, Instant>,
}

impl FrameStats {
    /// Reads one line of output: the header, a frame row, or something to skip.
    /// Lines before the header (PresentMon's start-up messages) are ignored. The column order is
    /// taken from the header, so different PresentMon versions with other columns still work.
    pub fn add_line(&mut self, line: &str, now: Instant) {
        let row = split_csv(line);
        if self.columns.is_empty() {
            if row.iter().any(|c| c == "ProcessID") {
                self.columns = row.into_iter().enumerate().map(|(i, name)| (name, i)).collect();
            }
            return;
        }
        self.add_row(&row, now);
    }

    /// Stores one frame row. False (and nothing stored) when a needed column is missing or not a number.
    fn add_row(&mut self, row: &[String], now: Instant) -> bool {
        let col = |name: &str| self.columns.get(name).and_then(|&i| row.get(i));
        let (Some(pid), Some(ms)) = (col("ProcessID"), col("MsBetweenPresents")) else {
            return false;
        };
        let (Ok(pid), Ok(ms)) = (pid.trim().parse::<u32>(), ms.trim().parse::<f64>()) else {
            return false;
        };
        // The frame's time in seconds since the trace began; newer PresentMon versions call the column
        // CPUStartTime, older ones TimeInSeconds.
        let Some(t) = col("CPUStartTime").or(col("TimeInSeconds")).and_then(|t| t.trim().parse::<f64>().ok()) else {
            return false;
        };
        // A swap chain is one surface an app draws to; a game can have several (e.g. a launcher overlay),
        // so frames are kept per swap chain and the busiest one wins.
        let swapchain = col("SwapChainAddress").cloned().unwrap_or_default();
        if let Some(name) = col("Application") {
            self.names.insert(pid, name.clone());
        }
        let frames = self.frames.entry((pid, swapchain)).or_default();
        frames.push_back((t, ms));
        // Keep only the last second of frames for this swap chain.
        while frames.front().is_some_and(|&(first, _)| first < t - 1.0) {
            frames.pop_front();
        }
        self.last_seen.insert(pid, now);
        true
    }

    /// FPS of the process's busiest swap chain over the last second, 0 if it stopped presenting.
    ///
    /// Computed as 1000 / (average milliseconds between presents), which gives a steady number even
    /// when the one-second window holds a partial frame at each end.
    pub fn fps(&self, pid: u32, now: Instant) -> f64 {
        if self.last_seen.get(&pid).is_none_or(|&seen| now.duration_since(seen) > STALE) {
            return 0.0;
        }
        let mut best: f64 = 0.0;
        for ((p, _), frames) in &self.frames {
            if *p != pid {
                continue;
            }
            let gaps: Vec<f64> = frames.iter().map(|&(_, ms)| ms).filter(|&ms| ms > 0.0).collect();
            if !gaps.is_empty() {
                best = best.max(1000.0 / (gaps.iter().sum::<f64>() / gaps.len() as f64));
            }
        }
        best
    }

    /// The process ids whose exe name matches `name`, ignoring case (a game can run several processes).
    pub fn pids_named(&self, name: &str) -> Vec<u32> {
        let name = name.to_lowercase();
        self.names.iter().filter(|(_, n)| n.to_lowercase() == name).map(|(&pid, _)| pid).collect()
    }

    /// Drops everything about processes that presented nothing for a minute (closed apps), so the
    /// maps don't grow forever and a reused pid starts fresh.
    pub fn forget_old(&mut self, now: Instant) {
        let old: Vec<u32> = self
            .last_seen
            .iter()
            .filter(|(_, &seen)| now.duration_since(seen) > Duration::from_secs(60))
            .map(|(&pid, _)| pid)
            .collect();
        for pid in old {
            self.last_seen.remove(&pid);
            self.names.remove(&pid);
            self.frames.retain(|(p, _), _| *p != pid);
        }
    }
}

/// The window in front: its process, and whether it covers its whole screen like a game does.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Front {
    /// The process that owns the foreground window; None when there is no foreground window.
    pub pid: Option<u32>,
    /// The window covers its whole monitor (as full-screen and borderless games do).
    pub full_screen: bool,
}

/// Picks the game for an fps app that names no program.
///
/// It is the full-screen game in front, or the last one while another window is in front, so
/// Alt-Tabbing to a chat app or a browser keeps showing the game. With no game seen yet, or once
/// it stops drawing, it shows whatever window is in front.
#[derive(Default)]
pub struct GamePicker {
    /// The pid of the last full-screen app seen presenting frames.
    game: Option<u32>,
}

impl GamePicker {
    /// The FPS to show for an fps app with no program set (see the struct's doc for the rules).
    pub fn fps(&mut self, stats: &FrameStats, front: Front, now: Instant) -> f64 {
        let in_front = front.pid.map(|pid| stats.fps(pid, now)).unwrap_or(0.0);
        // A full-screen window in front that is drawing: that's the game; remember it.
        if front.pid.is_some() && in_front > 0.0 && front.full_screen {
            self.game = front.pid;
            return in_front;
        }
        if let Some(game) = self.game {
            if Some(game) == front.pid {
                return in_front;
            }
            // Another window is in front: keep showing the remembered game while it still draws.
            let fps = stats.fps(game, now);
            if fps > 0.0 {
                return fps;
            }
            self.game = None;
        }
        in_front
    }
}

/// The FPS readings once a second, newest first, for the histogram.
#[derive(Default)]
struct History {
    /// At most `HISTORY` values, newest at the front.
    samples: VecDeque<f64>,
    /// When the last value was added.
    last: Option<Instant>,
}

impl History {
    /// Adds `value` if a second has passed since the last one (the main loop calls this ~10 times a
    /// second), and returns the history, newest first.
    fn add(&mut self, now: Instant, value: f64) -> Vec<f64> {
        if self.last.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1)) {
            self.last = Some(now);
            self.samples.push_front(value);
            self.samples.truncate(HISTORY);
        }
        self.samples.iter().copied().collect()
    }
}

/// What starts and stops PresentMon. The Windows one runs the real exe; tests use a stand-in.
pub trait Runner: Send {
    /// Starts PresentMon for one exe name, or every process. Lines it prints go to `line`; when it
    /// exits, `ended` gets whether Windows refused it access.
    fn start(
        &mut self,
        process: Option<&str>,
        line: Box<dyn FnMut(&str) + Send>,
        ended: Box<dyn FnOnce(bool) + Send>,
    ) -> Result<(), String>;
    /// True while the PresentMon process is alive.
    fn running(&mut self) -> bool;
    /// Ends PresentMon and its trace session.
    fn stop(&mut self);
    /// Which window is in front now (used when no program is named).
    fn front(&self) -> Front;
}

/// One PresentMon process and what it has measured, for one program name (or "the game in front").
struct Capture {
    /// Starts/stops the PresentMon process.
    runner: Box<dyn Runner>,
    /// Frames collected, shared with the stdout reading thread.
    stats: std::sync::Arc<Mutex<FrameStats>>,
    /// Ok / not available / no permission, shared with the stderr thread that reports the exit.
    status: std::sync::Arc<Mutex<Status>>,
    /// When an fps app last asked for this capture; PresentMon stops `IDLE` after it.
    last_used: Instant,
    /// No new start before this time, so a failing PresentMon is tried only every `RETRY`.
    next_start: Option<Instant>,
    /// Picks the game when no program is named.
    picker: GamePicker,
    /// The once-a-second values for the histogram.
    history: History,
}

impl Capture {
    /// A capture that hasn't started PresentMon yet (that happens on the first `read`).
    fn new(runner: Box<dyn Runner>) -> Capture {
        Capture {
            runner,
            stats: Default::default(),
            status: std::sync::Arc::new(Mutex::new(Status::Ok)),
            last_used: Instant::now(),
            next_start: None,
            picker: GamePicker::default(),
            history: History::default(),
        }
    }

    /// Marks the capture as used and starts PresentMon if it isn't running and the retry wait is over.
    /// The old frames are cleared first, and the status is reset to Ok; a start error sets
    /// NotAvailable (logged only when it wasn't already), and a later "access denied" exit sets
    /// NoPermission from PresentMon's stderr thread.
    fn touch(&mut self, process: Option<&str>, now: Instant) {
        self.last_used = now;
        if self.runner.running() || self.next_start.is_some_and(|t| now < t) {
            return;
        }
        self.next_start = Some(now + RETRY);
        *self.stats.lock().unwrap() = FrameStats::default();
        let stats = self.stats.clone();
        let status = self.status.clone();
        // Runs on the stdout thread for every line PresentMon prints.
        let line = Box::new(move |l: &str| stats.lock().unwrap().add_line(l, Instant::now()));
        // Runs on the stderr thread once PresentMon has exited.
        let ended = Box::new(move |denied: bool| {
            if denied {
                let mut status = status.lock().unwrap();
                if *status != Status::NoPermission {
                    wr_core::warn!(
                        "FPS app: Windows refused PresentMon access to frame timing. Add your account to the \
                         'Performance Log Users' group, then sign out and back in."
                    );
                }
                *status = Status::NoPermission;
            }
        });
        let before = std::mem::replace(&mut *self.status.lock().unwrap(), Status::Ok);
        if let Err(e) = self.runner.start(process, line, ended) {
            if before != Status::NotAvailable {
                wr_core::warn!("FPS app: {e}");
            }
            *self.status.lock().unwrap() = Status::NotAvailable;
        }
    }

    /// The FPS to show now: the busiest process with that exe name, or the picked game. While
    /// PresentMon is missing or refused, `available` is false (the app draws "?") and 0 goes in the history.
    fn read(&mut self, process: Option<&str>, now: Instant) -> Fps {
        self.touch(process, now);
        let status = *self.status.lock().unwrap();
        if status != Status::Ok {
            let history = self.history.add(now, 0.0);
            return Fps { value: 0.0, history, available: false };
        }
        let value = {
            let mut stats = self.stats.lock().unwrap();
            stats.forget_old(now);
            match process {
                Some(name) => stats.pids_named(name).into_iter().map(|p| stats.fps(p, now)).fold(0.0, f64::max),
                None => self.picker.fps(&stats, self.runner.front(), now),
            }
        };
        let history = self.history.add(now, value);
        Fps { value, history, available: true }
    }

    /// Stops PresentMon when the app hasn't been drawn for a while (another layout, panels off).
    fn stop_if_idle(&mut self, now: Instant) {
        if now.duration_since(self.last_used) > IDLE && self.runner.running() {
            self.runner.stop();
            self.next_start = None;
        }
    }
}

/// One PresentMon per watched program (blank: whichever app is in front).
pub struct FpsReader {
    /// Makes a new runner for each new capture (the real PresentMon, or a fake in tests).
    make: Box<dyn Fn() -> Box<dyn Runner> + Send>,
    /// Program name in lower case ("" = the game in front) -> its capture. Entries stay; idle ones
    /// only have their PresentMon stopped.
    captures: HashMap<String, Capture>,
}

impl FpsReader {
    /// A reader with no captures yet.
    pub fn new(make: impl Fn() -> Box<dyn Runner> + Send + 'static) -> FpsReader {
        FpsReader { make: Box::new(make), captures: HashMap::new() }
    }

    /// The FPS of `process` (an exe name such as "game.exe", any case; blank for the game in front).
    /// Also stops the PresentMon of any other capture that has gone idle.
    pub fn read(&mut self, process: &str, now: Instant) -> Fps {
        let key = process.trim().to_lowercase();
        for (other, capture) in self.captures.iter_mut() {
            if *other != key {
                capture.stop_if_idle(now);
            }
        }
        let make = &self.make;
        let capture = self.captures.entry(key.clone()).or_insert_with(|| Capture::new(make()));
        capture.read((!key.is_empty()).then_some(key.as_str()), now)
    }

    /// Called every loop, so PresentMon stops even when no fps app is drawn any more.
    pub fn tick(&mut self, now: Instant) {
        for capture in self.captures.values_mut() {
            capture.stop_if_idle(now);
        }
    }

    /// Stops every running PresentMon (on exit).
    pub fn stop_all(&mut self) {
        for capture in self.captures.values_mut() {
            if capture.runner.running() {
                capture.runner.stop();
            }
        }
    }
}

/// Make sure no PresentMon is left running when the reader goes away.
impl Drop for FpsReader {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// The real PresentMon runner.
#[cfg(windows)]
pub use self::windows::PresentMon;

/// The fps reader for the monitor, with a thread that stops PresentMon once no fps app is drawn,
/// even while the panels are off.
///
/// The `fps-idle` thread holds only a weak reference, so it ends by itself once every owner of the
/// reader has dropped it. The lock is released before sleeping so the main loop never waits 2 s.
#[cfg(windows)]
pub fn shared() -> std::sync::Arc<Mutex<FpsReader>> {
    let reader = std::sync::Arc::new(Mutex::new(FpsReader::new(|| Box::new(PresentMon::default()) as Box<dyn Runner>)));
    let weak = std::sync::Arc::downgrade(&reader);
    let _ = std::thread::Builder::new().name("fps-idle".into()).spawn(move || {
        while let Some(reader) = weak.upgrade() {
            reader.lock().unwrap_or_else(|e| e.into_inner()).tick(Instant::now());
            drop(reader);
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    reader
}

/// The Windows-only part: finding and running PresentMon.exe, and finding the window in front.
#[cfg(windows)]
mod windows {
    use super::{Front, Runner, SESSION_NAME};
    use std::io::{BufRead, BufReader, Read};
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{HANDLE, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetDesktopWindow, GetForegroundWindow, GetShellWindow, GetWindowRect, GetWindowThreadProcessId,
    };

    /// Process creation flag: start the console tool without opening a console window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// PresentMon.exe next to WhiteRabbit.exe (where the installer puts it), or on the PATH.
    fn find() -> Option<PathBuf> {
        let mut dirs: Vec<PathBuf> =
            std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)).into_iter().collect();
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        dirs.into_iter().map(|d| d.join("PresentMon.exe")).find(|p| p.is_file())
    }

    /// A job Windows closes when White Rabbit exits, even if it crashes, taking PresentMon with it.
    ///
    /// A job object groups processes; with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, Windows kills every
    /// process in it when the last handle to the job closes, which happens when our process ends for
    /// any reason. One job is made on first use and kept (as a number, since a raw handle can't be in
    /// a static) for the whole run. None if it can't be created; PresentMon then still runs, just
    /// without that safety net.
    fn job() -> Option<HANDLE> {
        static JOB: OnceLock<usize> = OnceLock::new();
        let job = *JOB.get_or_init(|| unsafe {
            // SAFETY: plain Win32 calls with a zeroed, correctly sized struct.
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return 0;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            job as usize
        });
        (job != 0).then_some(job as HANDLE)
    }

    /// Runs the real PresentMon.exe.
    #[derive(Default)]
    pub struct PresentMon {
        /// The exe that was last started, used to end its trace session on stop.
        exe: Option<PathBuf>,
        /// The running PresentMon process, if any.
        child: Option<Child>,
    }

    impl Runner for PresentMon {
        fn start(
            &mut self,
            process: Option<&str>,
            mut line: Box<dyn FnMut(&str) + Send>,
            ended: Box<dyn FnOnce(bool) + Send>,
        ) -> Result<(), String> {
            let exe = find().ok_or("PresentMon.exe wasn't found next to WhiteRabbit.exe, so FPS can't be measured")?;
            let mut command = Command::new(&exe);
            // --output_stdout: CSV rows on stdout instead of a file; --no_console_stats: no summary text;
            // our own --session_name, closing a leftover one from a crash (--stop_existing_session);
            // --no_track_input/--no_track_gpu: skip columns we don't use, which lowers PresentMon's cost.
            command.args([
                "--output_stdout",
                "--no_console_stats",
                "--session_name",
                SESSION_NAME,
                "--stop_existing_session",
                "--no_track_input",
                "--no_track_gpu",
            ]);
            if let Some(name) = process {
                command.args(["--process_name", name]);
            }
            let mut child = command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map_err(|e| format!("couldn't start PresentMon: {e}"))?;
            // There is a short moment between spawn and this call when a crash would leave PresentMon
            // running; that is accepted.
            if let Some(job) = job() {
                // SAFETY: the child's handle is valid while `child` lives.
                unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) };
            }
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            // The stdout reader: one call of `line` per CSV row until PresentMon exits (stdout closes).
            std::thread::spawn(move || {
                if let Some(out) = stdout {
                    for l in BufReader::new(out).lines() {
                        match l {
                            Ok(l) if !l.is_empty() => line(&l),
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                }
            });
            // The stderr reader: collect everything PresentMon complains about; when stderr closes (the
            // process has ended) report whether it was an access error.
            std::thread::spawn(move || {
                let mut text = String::new();
                if let Some(mut err) = stderr {
                    let _ = err.read_to_string(&mut text);
                }
                let text = text.to_lowercase();
                ended(text.contains("access denied") || text.contains("access is denied"));
            });
            self.exe = Some(exe);
            self.child = Some(child);
            Ok(())
        }

        fn running(&mut self) -> bool {
            match &mut self.child {
                Some(child) => matches!(child.try_wait(), Ok(None)),
                None => false,
            }
        }

        fn stop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            // Killing skips PresentMon's own clean-up, so end its trace session the proper way
            if let Some(exe) = &self.exe {
                let _ = Command::new(exe)
                    .args(["--terminate_existing_session", "--session_name", SESSION_NAME])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(CREATE_NO_WINDOW)
                    .status();
            }
        }

        fn front(&self) -> Front {
            // SAFETY: plain Win32 calls on the foreground window, with correctly sized structs.
            unsafe {
                let hwnd = GetForegroundWindow();
                if hwnd.is_null() {
                    return Front::default();
                }
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, &mut pid);
                let pid = (pid != 0).then_some(pid);
                // The desktop itself in front is never "full screen", even though it covers the monitor.
                if hwnd == GetDesktopWindow() || hwnd == GetShellWindow() {
                    return Front { pid, full_screen: false };
                }
                let mut rect: RECT = std::mem::zeroed();
                let mut info: MONITORINFO = std::mem::zeroed();
                info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                // Full screen: the window's rectangle covers the whole of the monitor it is on.
                let full_screen = GetWindowRect(hwnd, &mut rect) != 0
                    && !monitor.is_null()
                    && GetMonitorInfoW(monitor, &mut info) != 0
                    && rect.left <= info.rcMonitor.left
                    && rect.top <= info.rcMonitor.top
                    && rect.right >= info.rcMonitor.right
                    && rect.bottom >= info.rcMonitor.bottom;
                Front { pid, full_screen }
            }
        }
    }
}

/// Tests for the CSV parsing, FPS maths, game picking and PresentMon start/stop timing.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A minimal PresentMon CSV header with the columns the parser uses.
    const HEADER: &str = "Application,ProcessID,SwapChainAddress,PresentRuntime,CPUStartTime,MsBetweenPresents";

    /// Feeds `count` evenly spaced frame rows, `ms` milliseconds apart, for one process.
    fn frames(stats: &mut FrameStats, app: &str, pid: u32, ms: f64, count: usize, now: Instant) {
        for i in 0..count {
            let t = i as f64 * ms / 1000.0;
            stats.add_line(&format!("{app},{pid},0x1,DXGI,{t},{ms}"), now);
        }
    }

    /// Quoted commas, doubled quotes, a trailing empty field and CR/LF are handled.
    #[test]
    fn splits_quoted_fields() {
        assert_eq!(split_csv("a,\"b,c\",\"say \"\"hi\"\"\",\r\n"), vec!["a", "b,c", "say \"hi\"", ""]);
    }

    /// Lines before the header are skipped, 60 Hz frames give 60 FPS, names match in any case,
    /// a silent process shows 0 and is forgotten after a minute.
    #[test]
    fn counts_frames_per_second() {
        let now = Instant::now();
        let mut stats = FrameStats::default();
        stats.add_line("not the header", now);
        stats.add_line(HEADER, now);
        frames(&mut stats, "game.exe", 42, 1000.0 / 60.0, 120, now);
        assert!((stats.fps(42, now) - 60.0).abs() < 0.5);
        assert_eq!(stats.pids_named("GAME.EXE"), vec![42]);
        // Nothing presented for a while: 0
        assert_eq!(stats.fps(42, now + Duration::from_secs(3)), 0.0);
        stats.forget_old(now + Duration::from_secs(61));
        assert!(stats.pids_named("game.exe").is_empty());
    }

    /// The full-screen game stays picked while a chat window is in front, until the game stops drawing.
    #[test]
    fn keeps_the_game_while_alt_tabbed() {
        let now = Instant::now();
        let mut stats = FrameStats::default();
        stats.add_line(HEADER, now);
        frames(&mut stats, "game.exe", 1, 10.0, 100, now);
        frames(&mut stats, "chat.exe", 2, 1000.0 / 30.0, 30, now);
        let mut picker = GamePicker::default();
        let game = picker.fps(&stats, Front { pid: Some(1), full_screen: true }, now);
        assert!((game - 100.0).abs() < 1.0);
        let chat_in_front = picker.fps(&stats, Front { pid: Some(2), full_screen: false }, now);
        assert!((chat_in_front - 100.0).abs() < 1.0, "still the game: {chat_in_front}");
        // The game stopped drawing: whatever is in front
        let later = now + Duration::from_secs(3);
        assert_eq!(picker.fps(&stats, Front { pid: Some(2), full_screen: false }, later), 0.0);
        assert_eq!(picker.game, None);
    }

    /// Stands in for PresentMon: prints the given lines when started, or fails.
    struct Fake {
        /// Lines "printed" at start.
        lines: Vec<String>,
        /// Exit at once with "access denied".
        denied: bool,
        /// Fail to start, as if PresentMon.exe were missing.
        missing: bool,
        /// Whether this fake is "running".
        running: bool,
        /// How many fakes are running, across all captures.
        alive: Arc<Mutex<usize>>,
        /// How many starts were attempted, across all captures.
        starts: Arc<Mutex<usize>>,
    }

    impl Runner for Fake {
        fn start(
            &mut self,
            _process: Option<&str>,
            mut line: Box<dyn FnMut(&str) + Send>,
            ended: Box<dyn FnOnce(bool) + Send>,
        ) -> Result<(), String> {
            *self.starts.lock().unwrap() += 1;
            if self.missing {
                return Err("PresentMon.exe wasn't found".into());
            }
            for l in &self.lines {
                line(l);
            }
            if self.denied {
                ended(true);
            } else {
                self.running = true;
                *self.alive.lock().unwrap() += 1;
            }
            Ok(())
        }
        fn running(&mut self) -> bool {
            self.running
        }
        fn stop(&mut self) {
            self.running = false;
            *self.alive.lock().unwrap() -= 1;
        }
        fn front(&self) -> Front {
            Front { pid: Some(7), full_screen: true }
        }
    }

    /// An `FpsReader` using fakes, with counters of running fakes and start attempts.
    fn reader(lines: Vec<String>, denied: bool, missing: bool) -> (FpsReader, Arc<Mutex<usize>>, Arc<Mutex<usize>>) {
        let running = Arc::new(Mutex::new(0));
        let starts = Arc::new(Mutex::new(0));
        let (r, s) = (running.clone(), starts.clone());
        let reader = FpsReader::new(move || {
            Box::new(Fake {
                lines: lines.clone(),
                denied,
                missing,
                running: false,
                alive: r.clone(),
                starts: s.clone(),
            }) as Box<dyn Runner>
        });
        (reader, running, starts)
    }

    /// Blank and named programs get separate captures, and `tick` stops both once idle.
    #[test]
    fn reads_the_game_in_front_and_stops_when_idle() {
        let mut lines = vec![HEADER.to_string()];
        lines.extend((0..60).map(|i| format!("game.exe,7,0x1,DXGI,{},{}", i as f64 / 60.0, 1000.0 / 60.0)));
        let (mut reader, running, _) = reader(lines, false, false);
        let now = Instant::now();
        let fps = reader.read("", now);
        assert!(fps.available);
        assert!((fps.value - 60.0).abs() < 0.5);
        assert_eq!(fps.history.len(), 1);
        // A named program uses its own capture
        assert!((reader.read("Game.exe", now).value - 60.0).abs() < 0.5);
        assert_eq!(*running.lock().unwrap(), 2);
        reader.tick(now + IDLE + Duration::from_secs(1));
        assert_eq!(*running.lock().unwrap(), 0);
    }

    /// Denied or missing PresentMon reads as unavailable, and a missing one is retried only after `RETRY`.
    #[test]
    fn shows_a_question_mark_without_presentmon_or_permission() {
        let now = Instant::now();
        let (mut denied, _, _) = reader(vec![], true, false);
        assert!(!denied.read("", now).available);
        assert!(!denied.read("", now + Duration::from_secs(1)).available);
        let (mut missing, _, starts) = reader(vec![], false, true);
        assert!(!missing.read("", now).available);
        // Not retried every frame
        missing.read("", now + Duration::from_secs(1));
        assert_eq!(*starts.lock().unwrap(), 1);
        missing.read("", now + RETRY);
        assert_eq!(*starts.lock().unwrap(), 2);
    }
}
