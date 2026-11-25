//! Listening to sound for the audio visualizer: what the laptop plays, or a microphone (port of
//! `led_mon/equalizer_files/audio.py`, Windows path).
//!
//! "playback" records the default output device through WASAPI loopback, following it when it
//! changes (e.g. from speakers to headphones). "microphone" records a chosen capture device, or the
//! default one.
//!
//! One recording is shared by everything that listens to the same thing, so the two panels (and the
//! window's preview) see exactly the same sound. [`acquire`] starts or joins one, [`release`] leaves
//! it, and the last one to leave stops it.
//!
//! Everything but the WASAPI calls (in `capture/wasapi.rs`) is platform independent and tested on
//! Linux with a fake [`Backend`].
//!
//! # Threads
//!
//! Each shared `Capture` has one supervisor thread ("audio-visualizer-capture", `supervise`) that
//! wakes every `DEVICE_CHECK` (3 s) or when stopped. It works out which device should be recorded
//! and, when that changes (new default speaker, microphone came back, recording broke or stalled),
//! starts a new recorder thread ("audio-visualizer-record", `record`). Recorder threads can block
//! inside the sound system while nothing plays, so an old one is never waited for: each recording
//! has a `generation` number, and a recorder whose generation is no longer the current one drops
//! its samples and ends the next time it checks.
//!
//! The recorder pushes chunks of samples into a `SampleBuffer` (a ring of the last 4096 stereo
//! frames) under the `buffer` lock; the users (the monitor's `music-N` threads) copy it with
//! `snapshot` under the same lock, about 33 times a second each. Both hold the lock only for a copy.
//!
//! # Locks
//!
//! `Registry::captures` (the shared recordings), then per `Capture`: `buffer` and `state` (never held
//! together), and `stopped` (with the `wake` condition variable). None of them is held while calling
//! into the sound system, except that `acquire` holds `captures` while starting the supervisor thread.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use wr_core::{info, warn};

#[cfg(windows)]
mod wasapi;

/// Samples per second asked of the sound device (looks.py SAMPLE_RATE).
pub const SAMPLE_RATE: u32 = crate::analyzer::SAMPLE_RATE as u32;
/// Stereo frames kept for the FFT (looks.py FFT_SIZE).
pub const FFT_SIZE: usize = crate::analyzer::FFT_SIZE;
/// Frames handed to the buffer at a time.
/// (About 21 ms of sound at 48 kHz; the WASAPI reader collects packets until it has this many.)
pub const CHUNK_SIZE: usize = 1024;
/// How often to look for a new default output device (or a microphone plugged in).
pub const DEVICE_CHECK: Duration = Duration::from_secs(3);
/// Sound older than this counts as silence (WASAPI loopback delivers nothing while nothing plays).
pub const STALE_AUDIO: Duration = Duration::from_millis(250);
/// A microphone sends sound all the time, even in a quiet room. If its recording gives nothing for
/// this long it has stalled (e.g. after the screen turned off or another app took the microphone),
/// so it is opened again. Playback is not checked: it gives nothing whenever nothing plays.
pub const MIC_STALL: Duration = Duration::from_secs(5);
/// What the settings show for "no particular microphone".
pub const DEFAULT_MICROPHONE: &str = "Default microphone";

/// The error every call gives on Linux and macOS (used by the tests there).
#[cfg(not(windows))]
const UNSUPPORTED: &str = "sound capture is only available on Windows";

/// One stereo sample: [left, right].
pub type Frame = [f32; 2];

/// Called with the new device's name when the playback device changes (not on the first open).
pub type DeviceChangeFn = Arc<dyn Fn(&str) + Send + Sync>;

pub use crate::settings::InputMode;

/// The device in `names` that `wanted` means: the same name, else the first that contains it
/// (ignoring case). None if none, or if `wanted` is empty.
pub fn match_name<'a, S: AsRef<str>>(wanted: &str, names: &'a [S]) -> Option<&'a str> {
    if wanted.is_empty() {
        return None;
    }
    if let Some(name) = names.iter().find(|n| n.as_ref() == wanted) {
        return Some(name.as_ref());
    }
    let lower = wanted.to_lowercase();
    names.iter().map(|n| n.as_ref()).find(|n| n.to_lowercase().contains(&lower))
}

/// True when a microphone recording, open since `recording_since`, has given nothing for
/// [`MIC_STALL`] (measured from the later of the last sound and the start of the recording).
/// False when neither time is known (nothing has been opened yet).
pub fn is_stalled(updated: Option<Instant>, recording_since: Option<Instant>, now: Instant) -> bool {
    match updated.max(recording_since) {
        Some(last) => now.saturating_duration_since(last) > MIC_STALL,
        None => false,
    }
}

/// The last [`FFT_SIZE`] stereo frames, oldest first, in a ring.
/// Written by the recorder thread and read by the drawing threads, always behind a `Mutex` in
/// `Capture`. Fixed size, so it never allocates after it is made.
pub struct SampleBuffer {
    /// FFT_SIZE stereo frames.
    frames: Box<[Frame]>,
    /// Where the next frame goes, which is also the oldest frame.
    pos: usize,
    /// When sound last arrived (None before the first), for the staleness and stall checks.
    updated: Option<Instant>,
}

impl Default for SampleBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl SampleBuffer {
    /// An all-silent buffer that has never been updated.
    pub fn new() -> Self {
        SampleBuffer { frames: vec![[0.0; 2]; FFT_SIZE].into_boxed_slice(), pos: 0, updated: None }
    }

    /// Adds interleaved samples with `channels` channels per frame: a single channel goes to both
    /// sides, channels after the second are dropped. Only the last FFT_SIZE frames are kept.
    pub fn push_at(&mut self, samples: &[f32], channels: usize, now: Instant) {
        if channels == 0 {
            return;
        }
        let count = samples.len() / channels;
        // More than FFT_SIZE frames at once: only the newest FFT_SIZE can be kept, so the rest is skipped.
        let skip = count.saturating_sub(FFT_SIZE);
        // No whole frame (e.g. an empty chunk): nothing changes, not even `updated`.
        if count == skip {
            return;
        }
        for frame in samples.chunks_exact(channels).skip(skip) {
            let left = frame[0];
            let right = if channels >= 2 { frame[1] } else { left };
            self.frames[self.pos] = [left, right];
            self.pos = (self.pos + 1) % FFT_SIZE;
        }
        self.updated = Some(now);
    }

    /// The latest FFT_SIZE frames, oldest first; all zero when nothing has arrived for [`STALE_AUDIO`].
    pub fn snapshot_at(&self, now: Instant) -> Vec<Frame> {
        match self.updated {
            Some(updated) if now.saturating_duration_since(updated) <= STALE_AUDIO => {
                let mut out = Vec::with_capacity(FFT_SIZE);
                // `pos` is the oldest frame: copying from there to the end and then from the start gives the
                // frames oldest first.
                out.extend_from_slice(&self.frames[self.pos..]);
                out.extend_from_slice(&self.frames[..self.pos]);
                out
            }
            _ => vec![[0.0; 2]; FFT_SIZE],
        }
    }

    /// When sound last arrived.
    pub fn updated(&self) -> Option<Instant> {
        self.updated
    }
}

/// A sound device as the backend sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Device {
    /// Stable id (the WASAPI endpoint id).
    pub id: String,
    /// Friendly name, e.g. "Speakers (Realtek(R) Audio)".
    pub name: String,
}

/// The platform's sound devices. WASAPI on Windows, an error elsewhere, a fake in tests.
/// `Send + Sync + 'static` because one backend is shared (`Arc`) by every capture's threads.
pub(crate) trait Backend: Send + Sync + 'static {
    /// Err when recording can't work at all on this computer.
    fn check_available(&self) -> Result<(), String>;
    /// The default output device (recorded through loopback).
    fn default_speaker(&self) -> Result<Device, String>;
    /// The active recording inputs, without loopback ones.
    fn microphones(&self) -> Result<Vec<Device>, String>;
    /// The default recording input.
    fn default_microphone(&self) -> Result<Device, String>;
    /// Records from `device` (its output when `loopback`) until `keep_going` returns false, handing
    /// chunks of interleaved samples and their channel count to `sink`. May block while no sound
    /// arrives. Returns Ok when told to stop, Err when the recording broke.
    fn record(
        &self,
        device: &Device,
        loopback: bool,
        keep_going: &dyn Fn() -> bool,
        sink: &mut dyn FnMut(&[f32], usize),
    ) -> Result<(), String>;
}

/// The backend on anything but Windows: every call fails with `UNSUPPORTED`.
#[cfg(not(windows))]
struct Unsupported;

#[cfg(not(windows))]
impl Backend for Unsupported {
    fn check_available(&self) -> Result<(), String> {
        Err(UNSUPPORTED.into())
    }
    fn default_speaker(&self) -> Result<Device, String> {
        Err(UNSUPPORTED.into())
    }
    fn microphones(&self) -> Result<Vec<Device>, String> {
        Err(UNSUPPORTED.into())
    }
    fn default_microphone(&self) -> Result<Device, String> {
        Err(UNSUPPORTED.into())
    }
    fn record(
        &self,
        _: &Device,
        _: bool,
        _: &dyn Fn() -> bool,
        _: &mut dyn FnMut(&[f32], usize),
    ) -> Result<(), String> {
        Err(UNSUPPORTED.into())
    }
}

/// The one backend for this platform, made on first use and shared after that.
fn platform_backend() -> Arc<dyn Backend> {
    static BACKEND: LazyLock<Arc<dyn Backend>> = LazyLock::new(|| {
        #[cfg(windows)]
        {
            Arc::new(wasapi::Wasapi)
        }
        #[cfg(not(windows))]
        {
            Arc::new(Unsupported)
        }
    });
    BACKEND.clone()
}

/// Names of the microphones (and other recording inputs) this computer has, without duplicates.
pub fn list_microphones() -> Vec<String> {
    list_microphones_with(platform_backend().as_ref())
}

/// `list_microphones` with a given backend (for tests). An error is logged and gives an empty list.
fn list_microphones_with(backend: &dyn Backend) -> Vec<String> {
    let devices = backend.microphones().unwrap_or_else(|e| {
        warn!("Could not list the microphones: {e}");
        Vec::new()
    });
    let mut unique: Vec<String> = Vec::new();
    for device in devices {
        if !device.name.is_empty() && !unique.contains(&device.name) {
            unique.push(device.name);
        }
    }
    unique
}

/// Locks, ignoring poisoning: the guarded data (samples, device names) stays usable after a panic
/// elsewhere, and sound capture should keep going.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the supervisor knows about the recording; read by the users for the status display.
#[derive(Default)]
struct State {
    /// Friendly name of the device being recorded (or last recorded).
    device_name: Option<String>,
    /// A message for the user, or None when all is well.
    problem: Option<String>,
    /// Bumped for every new recording; an older recording thread ends when it sees a newer one.
    generation: u64,
    /// The device being recorded, None when a new recording should be opened.
    current_id: Option<String>,
    /// When the current recording was started, for the microphone stall check.
    recording_since: Option<Instant>,
}

/// Records sound into a buffer holding the last FFT_SIZE stereo frames.
/// One shared recording. Made by `Registry::acquire` and handed out as `Arc<Capture>`; its
/// supervisor and recorder threads hold their own `Arc`s until they end.
pub struct Capture {
    /// Playback (loopback of the default output) or microphone.
    input_mode: InputMode,
    /// The microphone asked for by name ("" for the default, and always "" for playback).
    input_device: String,
    /// Told the new device's name when playback moves to another device.
    on_device_change: Option<DeviceChangeFn>,
    /// WASAPI, or a fake in tests.
    backend: Arc<dyn Backend>,
    /// How often the supervisor looks at the devices (`DEVICE_CHECK`; shorter in tests).
    check_interval: Duration,
    /// The latest sound.
    buffer: Mutex<SampleBuffer>,
    /// Which device is recorded, and any problem.
    state: Mutex<State>,
    /// Set once by `stop`; never cleared.
    stopped: Mutex<bool>,
    /// Wakes the supervisor at once when stopped, instead of after its next interval.
    wake: Condvar,
}

impl Capture {
    /// Not started yet: `Registry::acquire` calls `start`.
    fn new(
        input_mode: InputMode,
        input_device: &str,
        on_device_change: Option<DeviceChangeFn>,
        backend: Arc<dyn Backend>,
        check_interval: Duration,
    ) -> Self {
        Capture {
            input_mode,
            input_device: input_device.to_string(),
            on_device_change,
            backend,
            check_interval,
            buffer: Mutex::new(SampleBuffer::new()),
            state: Mutex::new(State::default()),
            stopped: Mutex::new(false),
            wake: Condvar::new(),
        }
    }

    /// Playback or microphone, as asked for.
    pub fn input_mode(&self) -> InputMode {
        self.input_mode
    }

    /// The microphone asked for ("" for playback or the default microphone).
    pub fn input_device(&self) -> &str {
        &self.input_device
    }

    /// The name of the device being recorded, once one has been opened.
    pub fn device_name(&self) -> Option<String> {
        lock(&self.state).device_name.clone()
    }

    /// Something to tell the user, e.g. that the chosen microphone wasn't found.
    pub fn problem(&self) -> Option<String> {
        lock(&self.state).problem.clone()
    }

    /// Adds interleaved samples (see [`SampleBuffer::push_at`]).
    pub fn push(&self, samples: &[f32], channels: usize) {
        lock(&self.buffer).push_at(samples, channels, Instant::now());
    }

    /// The latest FFT_SIZE frames, oldest first, all zero when nothing new has arrived for a moment.
    pub fn snapshot(&self) -> Vec<Frame> {
        self.snapshot_at(Instant::now())
    }

    /// `snapshot` at a given time (lets a caller or test pick the time; `snapshot` uses now).
    pub fn snapshot_at(&self, now: Instant) -> Vec<Frame> {
        lock(&self.buffer).snapshot_at(now)
    }

    /// [`Capture::snapshot`] as the f64 samples the looks take (like Python's float32 -> float64).
    pub fn snapshot_f64(&self) -> Vec<crate::Stereo> {
        self.snapshot().into_iter().map(|[l, r]| [l as f64, r as f64]).collect()
    }

    /// True when the microphone's recording has given nothing for [`MIC_STALL`].
    /// Always false for playback, and false while no recording is open.
    pub fn stalled(&self, now: Instant) -> bool {
        // The buffer's lock is let go at the end of this statement, before `state` is locked.
        let updated = lock(&self.buffer).updated();
        let state = lock(&self.state);
        self.input_mode == InputMode::Microphone
            && state.current_id.is_some()
            && is_stalled(updated, state.recording_since, now)
    }

    /// True once `stop` has been called.
    pub fn is_stopped(&self) -> bool {
        *lock(&self.stopped)
    }

    /// Stops recording. The recording threads end at their next look.
    pub fn stop(&self) {
        *lock(&self.stopped) = true;
        self.wake.notify_all();
    }

    /// Starts the supervisor thread. Fails if recording can't work here (no Windows audio, or not
    /// Windows) or the thread can't be made.
    fn start(self: &Arc<Self>) -> Result<(), String> {
        self.backend.check_available()?;
        let capture = Arc::clone(self);
        std::thread::Builder::new()
            .name("audio-visualizer-capture".into())
            .spawn(move || capture.supervise())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// The supervisor thread's body: a `check` now, then one every `check_interval`, until stopped.
    /// `error_logged` keeps a device error from being logged again every 3 seconds.
    fn supervise(self: Arc<Self>) {
        let mut error_logged = false;
        loop {
            self.check(&mut error_logged);
            let stopped = lock(&self.stopped);
            // Sleeps for the interval, but `stop` wakes it at once (the condition is re-checked, so a
            // spurious wake-up just loops).
            let (stopped, _) =
                self.wake.wait_timeout_while(stopped, self.check_interval, |s| !*s).unwrap_or_else(|e| e.into_inner());
            if *stopped {
                return;
            }
        }
    }

    /// One look at the devices: reopen a stalled microphone, follow a new default device, retry
    /// after an error.
    fn check(self: &Arc<Self>, error_logged: &mut bool) {
        if self.stalled(Instant::now()) {
            let mut state = lock(&self.state);
            warn!(
                "Audio visualizer heard nothing from {} for {} seconds; opening it again",
                state.device_name.as_deref().unwrap_or("the microphone"),
                MIC_STALL.as_secs()
            );
            // Start a new recording below; the stalled one ends if it ever wakes up
            state.current_id = None;
        }
        match self.resolve_source() {
            Ok(device) => {
                let mut state = lock(&self.state);
                if state.current_id.as_deref() != Some(device.id.as_str()) {
                    // The very first device opened is not a "change"; reopening after an error or a stall is
                    // (as in Python), because `device_name` is still set then.
                    let first = state.current_id.is_none() && state.device_name.is_none();
                    state.current_id = Some(device.id.clone());
                    state.device_name = Some(device.name.clone());
                    state.generation += 1;
                    state.recording_since = Some(Instant::now());
                    let generation = state.generation;
                    // Unlocked before logging, starting the thread and calling the user's callback, none of which
                    // should run with `state` held (the callback could ask for the device name).
                    drop(state);
                    info!("Audio visualizer listening to {}", device.name);
                    let name = device.name.clone();
                    let capture = Arc::clone(self);
                    let spawned = std::thread::Builder::new()
                        .name("audio-visualizer-record".into())
                        .spawn(move || capture.record(device, generation));
                    if let Err(e) = spawned {
                        warn!("Audio visualizer could not start recording: {e}");
                        // No recorder thread: try again on the next check (the generation was already bumped, so
                        // the previous recording ends anyway).
                        lock(&self.state).current_id = None;
                    }
                    if !first && self.input_mode == InputMode::Playback {
                        if let Some(callback) = &self.on_device_change {
                            callback(&name);
                        }
                    }
                }
                *error_logged = false;
            }
            Err(e) => {
                if !*error_logged {
                    *error_logged = true;
                    warn!("Audio visualizer could not open the {} audio device: {e}", self.input_mode.as_str());
                }
                let mut state = lock(&self.state);
                state.problem = Some(format!("Couldn't open the sound device: {e}"));
                // The running recording (if any) is left alone; the next successful check starts a new one.
                state.current_id = None;
            }
        }
    }

    /// The device to record, setting or clearing `problem` on the way.
    /// Playback: the default output device. Microphone: the named one (matched with `match_name`), or the
    /// default microphone with a problem message when the named one isn't there. Errors come from the
    /// backend (no device, Windows audio not working).
    fn resolve_source(&self) -> Result<Device, String> {
        if self.input_mode == InputMode::Playback {
            let device = self.backend.default_speaker()?;
            lock(&self.state).problem = None;
            return Ok(device);
        }
        let mut problem = None;
        if !self.input_device.is_empty() {
            let mics = self.backend.microphones()?;
            let names: Vec<&str> = mics.iter().map(|m| m.name.as_str()).collect();
            if let Some(name) = match_name(&self.input_device, &names) {
                let device = mics.iter().find(|m| m.name == name).cloned().expect("name came from mics");
                lock(&self.state).problem = None;
                return Ok(device);
            }
            let message = format!("{} wasn't found, so the default microphone is used", self.input_device);
            // Logged only when the message is new, not on every 3-second check.
            if lock(&self.state).problem.as_deref() != Some(message.as_str()) {
                warn!("Microphone '{}' was not found, using the default microphone", self.input_device);
            }
            problem = Some(message);
        }
        let device = self.backend.default_microphone()?;
        lock(&self.state).problem = problem;
        Ok(device)
    }

    /// The current recording's generation number (locks `state` briefly).
    fn generation(&self) -> u64 {
        lock(&self.state).generation
    }

    /// Runs on its own thread: recording can block while nothing plays, so a device change starts a
    /// new thread and an outdated one ends when it next wakes up.
    fn record(self: Arc<Self>, device: Device, generation: u64) {
        // The backend asks this between packets: false once the capture is stopped or a newer
        // recording has started.
        let current = || !self.is_stopped() && self.generation() == generation;
        let loopback = self.input_mode == InputMode::Playback;
        let result = self.backend.record(&device, loopback, &current, &mut |samples, channels| {
            if self.generation() == generation {
                self.push(samples, channels);
            }
        });
        if let Err(e) = result {
            warn!("Audio visualizer recording stopped: {e}");
            let mut state = lock(&self.state);
            if state.generation == generation {
                // Let the supervisor open the device again on its next check
                state.current_id = None;
            }
        }
    }
}

/// What a recording listens to: the mode and, for a microphone, the name asked for.
type Key = (InputMode, String);

/// The shared recordings, by what they listen to, with how many users each has.
struct Registry {
    /// Each recording and how many users hold it.
    captures: Mutex<HashMap<Key, (Arc<Capture>, usize)>>,
}

impl Registry {
    /// An empty registry (the real one is `REGISTRY`; tests make their own).
    fn new() -> Self {
        Registry { captures: Mutex::new(HashMap::new()) }
    }

    fn acquire(
        &self,
        backend: &Arc<dyn Backend>,
        check_interval: Duration,
        input_mode: InputMode,
        input_device: &str,
        on_device_change: Option<DeviceChangeFn>,
    ) -> Result<Arc<Capture>, String> {
        // Playback has one recording whatever device name is passed.
        let device = if input_mode == InputMode::Microphone { input_device } else { "" };
        let key = (input_mode, device.to_string());
        let mut captures = lock(&self.captures);
        if let Some((capture, refs)) = captures.get_mut(&key) {
            *refs += 1;
            return Ok(Arc::clone(capture));
        }
        let capture = Arc::new(Capture::new(input_mode, device, on_device_change, Arc::clone(backend), check_interval));
        // On error the capture is not registered and simply dropped.
        capture.start()?;
        captures.insert(key, (Arc::clone(&capture), 1));
        Ok(capture)
    }

    /// Drops one user of `capture`; at zero it is removed and stopped. A capture not in the registry is
    /// just stopped.
    fn release(&self, capture: &Arc<Capture>) {
        let mut captures = lock(&self.captures);
        let key = captures.iter().find(|(_, (c, _))| Arc::ptr_eq(c, capture)).map(|(k, _)| k.clone());
        if let Some(key) = key {
            let refs = &mut captures.get_mut(&key).expect("key was just found").1;
            *refs -= 1;
            if *refs > 0 {
                return;
            }
            captures.remove(&key);
        }
        // The registry is unlocked before stopping, which takes the capture's own `stopped` lock.
        drop(captures);
        capture.stop();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        lock(&self.captures).len()
    }
}

/// The process-wide shared recordings.
static REGISTRY: LazyLock<Registry> = LazyLock::new(Registry::new);

/// Starts (or joins) the recording of this input. `input_device` only matters for a microphone ("" =
/// the default one). `on_device_change` is used only by the call that starts the recording. Call
/// [`release`] with the result when done. Errs on anything but Windows.
pub fn acquire(
    input_mode: InputMode,
    input_device: &str,
    on_device_change: Option<DeviceChangeFn>,
) -> Result<Arc<Capture>, String> {
    REGISTRY.acquire(&platform_backend(), DEVICE_CHECK, input_mode, input_device, on_device_change)
}

/// Leaves a recording; the last one to leave stops it.
pub fn release(capture: &Arc<Capture>) {
    REGISTRY.release(capture)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Proves the name matching: exact first, then the first that contains it in any case; "" matches none.
    #[test]
    fn match_name_prefers_exact_then_contains_ignoring_case() {
        let names = ["Microphone Array (Realtek)", "USB Mic", "Mic"];
        assert_eq!(match_name("Mic", &names), Some("Mic"));
        assert_eq!(match_name("usb", &names), Some("USB Mic"));
        assert_eq!(match_name("MIC", &names), Some("Microphone Array (Realtek)"));
        assert_eq!(match_name("Headset", &names), None);
        assert_eq!(match_name("", &names), None);
    }

    /// Proves that a mono chunk goes to both sides, the newest frames are last, and extra channels are cut.
    #[test]
    fn buffer_keeps_last_frames_and_spreads_mono() {
        let t0 = Instant::now();
        let mut buffer = SampleBuffer::new();
        assert!(buffer.snapshot_at(t0).iter().all(|f| *f == [0.0, 0.0]));
        buffer.push_at(&[1.0, 2.0], 1, t0);
        let snap = buffer.snapshot_at(t0);
        assert_eq!(snap.len(), FFT_SIZE);
        assert_eq!(snap[FFT_SIZE - 2..], [[1.0, 1.0], [2.0, 2.0]]);
        assert_eq!(snap[0], [0.0, 0.0]);
        // Extra channels are dropped
        buffer.push_at(&[3.0, 4.0, 9.0, 5.0, 6.0, 9.0], 3, t0);
        let snap = buffer.snapshot_at(t0);
        assert_eq!(snap[FFT_SIZE - 4..], [[1.0, 1.0], [2.0, 2.0], [3.0, 4.0], [5.0, 6.0]]);
    }

    /// Proves that an oversized chunk keeps only its newest FFT_SIZE frames and that the ring wraps in order.
    #[test]
    fn buffer_keeps_only_the_last_fft_size_frames() {
        let t0 = Instant::now();
        let mut buffer = SampleBuffer::new();
        let samples: Vec<f32> = (0..FFT_SIZE + 10).flat_map(|i| [i as f32, -(i as f32)]).collect();
        buffer.push_at(&samples, 2, t0);
        let snap = buffer.snapshot_at(t0);
        assert_eq!(snap[0], [10.0, -10.0]);
        assert_eq!(snap[FFT_SIZE - 1], [(FFT_SIZE + 9) as f32, -((FFT_SIZE + 9) as f32)]);
        // Wraps around the ring in order
        buffer.push_at(&[7.0, 8.0], 2, t0);
        let snap = buffer.snapshot_at(t0);
        assert_eq!(snap[0], [11.0, -11.0]);
        assert_eq!(snap[FFT_SIZE - 1], [7.0, 8.0]);
    }

    /// Proves that old sound reads as silence after `STALE_AUDIO`, and that empty pushes don't count as sound.
    #[test]
    fn buffer_goes_silent_when_stale() {
        let t0 = Instant::now();
        let mut buffer = SampleBuffer::new();
        buffer.push_at(&[0.5, 0.5], 2, t0);
        buffer.push_at(&[], 2, t0 + Duration::from_secs(1)); // empty pushes don't count
        assert_eq!(buffer.snapshot_at(t0 + STALE_AUDIO)[FFT_SIZE - 1], [0.5, 0.5]);
        let late = buffer.snapshot_at(t0 + STALE_AUDIO + Duration::from_millis(1));
        assert!(late.iter().all(|f| *f == [0.0, 0.0]));
        assert_eq!(buffer.updated(), Some(t0));
    }

    /// Proves the stall rule with every combination of known and unknown times.
    #[test]
    fn stall_is_measured_from_the_later_of_sound_and_start() {
        let t0 = Instant::now();
        let stall = MIC_STALL + Duration::from_millis(1);
        assert!(!is_stalled(None, None, t0 + stall));
        assert!(is_stalled(None, Some(t0), t0 + stall));
        assert!(!is_stalled(None, Some(t0), t0 + MIC_STALL));
        assert!(!is_stalled(Some(t0 + Duration::from_secs(2)), Some(t0), t0 + stall));
        assert!(is_stalled(Some(t0), Some(t0 + Duration::from_secs(1)), t0 + Duration::from_secs(1) + stall));
    }

    /// Devices and recordings the tests control.
    #[derive(Default)]
    struct Fake {
        speaker: Mutex<Option<Device>>,
        mics: Mutex<Vec<Device>>,
        default_mic: Mutex<Option<Device>>,
        /// How many recordings have been started.
        recordings: AtomicUsize,
        /// Recordings end (as stopped/outdated) when this many have been started.
        live: AtomicUsize,
    }

    /// A device with this id and name.
    fn dev(id: &str, name: &str) -> Device {
        Device { id: id.into(), name: name.into() }
    }

    impl Backend for Fake {
        fn check_available(&self) -> Result<(), String> {
            Ok(())
        }
        fn default_speaker(&self) -> Result<Device, String> {
            lock(&self.speaker).clone().ok_or_else(|| "no speaker".into())
        }
        fn microphones(&self) -> Result<Vec<Device>, String> {
            Ok(lock(&self.mics).clone())
        }
        fn default_microphone(&self) -> Result<Device, String> {
            lock(&self.default_mic).clone().ok_or_else(|| "no microphone".into())
        }
        fn record(
            &self,
            _: &Device,
            _: bool,
            keep_going: &dyn Fn() -> bool,
            sink: &mut dyn FnMut(&[f32], usize),
        ) -> Result<(), String> {
            self.recordings.fetch_add(1, Ordering::SeqCst);
            self.live.fetch_add(1, Ordering::SeqCst);
            sink(&[0.25, 0.75], 2);
            while keep_going() {
                std::thread::sleep(Duration::from_millis(1));
            }
            self.live.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Supervisor interval in the tests, so changes are noticed within milliseconds.
    const FAST: Duration = Duration::from_millis(5);

    /// Waits up to 5 s for `cond`, failing the test with `what` if it never becomes true.
    fn wait_for(what: &str, cond: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Proves that playback records the default speaker, follows a new default (ending the old recording
    /// and calling the callback, but not for the first open), reports a missing device as a problem and
    /// reopens it once it is back.
    #[test]
    fn playback_follows_the_default_device_and_reports_changes() {
        let fake = Arc::new(Fake::default());
        *lock(&fake.speaker) = Some(dev("1", "Speakers"));
        let changes = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&changes);
        let callback: DeviceChangeFn = Arc::new(move |name| lock(&seen).push(name.to_string()));
        let backend: Arc<dyn Backend> = fake.clone();
        let registry = Registry::new();
        let capture = registry.acquire(&backend, FAST, InputMode::Playback, "ignored", Some(callback)).unwrap();
        assert_eq!(capture.input_device(), "");
        wait_for("first recording", || capture.device_name().as_deref() == Some("Speakers"));
        wait_for("sound", || capture.snapshot()[FFT_SIZE - 1] == [0.25, 0.75]);
        assert!(lock(&changes).is_empty(), "no callback on the first open");

        *lock(&fake.speaker) = Some(dev("2", "Headphones"));
        wait_for("device change", || lock(&changes).len() == 1);
        assert_eq!(lock(&changes)[0], "Headphones");
        assert_eq!(capture.device_name().as_deref(), Some("Headphones"));
        wait_for("old recording to end", || fake.live.load(Ordering::SeqCst) == 1);

        // An error is reported, then the device is opened again once it is back
        *lock(&fake.speaker) = None;
        wait_for("problem", || capture.problem().is_some());
        assert_eq!(capture.problem().unwrap(), "Couldn't open the sound device: no speaker");
        *lock(&fake.speaker) = Some(dev("2", "Headphones"));
        wait_for("reopen", || fake.recordings.load(Ordering::SeqCst) == 3);
        wait_for("problem cleared", || capture.problem().is_none());

        registry.release(&capture);
        assert!(capture.is_stopped());
        wait_for("recordings to end", || fake.live.load(Ordering::SeqCst) == 0);
        assert_eq!(lock(&changes).len(), 2, "reopening after an error counts as a change, as in Python");
    }

    /// Proves that a named microphone is found by part of its name, and that an unknown one falls back to
    /// the default with a problem message.
    #[test]
    fn missing_microphone_falls_back_to_the_default() {
        let fake = Arc::new(Fake::default());
        *lock(&fake.mics) = vec![dev("a", "Microphone Array"), dev("b", "USB Mic")];
        *lock(&fake.default_mic) = Some(dev("a", "Microphone Array"));
        let backend: Arc<dyn Backend> = fake.clone();
        let registry = Registry::new();

        let usb = registry.acquire(&backend, FAST, InputMode::Microphone, "usb", None).unwrap();
        wait_for("usb", || usb.device_name().as_deref() == Some("USB Mic"));
        assert_eq!(usb.problem(), None);

        let headset = registry.acquire(&backend, FAST, InputMode::Microphone, "Headset", None).unwrap();
        wait_for("default", || headset.device_name().as_deref() == Some("Microphone Array"));
        assert_eq!(headset.problem().unwrap(), "Headset wasn't found, so the default microphone is used");
        assert_eq!(list_microphones_with(fake.as_ref()), ["Microphone Array", "USB Mic"]);
        registry.release(&usb);
        registry.release(&headset);
    }

    /// Proves that a microphone that has given nothing for `MIC_STALL` is detected and reopened, and the
    /// stalled recording ends.
    #[test]
    fn stalled_microphone_is_reopened() {
        let fake = Arc::new(Fake::default());
        *lock(&fake.default_mic) = Some(dev("a", "Mic"));
        let backend: Arc<dyn Backend> = fake.clone();
        let capture = Arc::new(Capture::new(InputMode::Microphone, "", None, backend, FAST));
        let mut logged = false;
        capture.check(&mut logged);
        wait_for("recording", || fake.recordings.load(Ordering::SeqCst) == 1);
        assert!(!capture.stalled(Instant::now()));
        assert!(capture.stalled(Instant::now() + MIC_STALL + Duration::from_secs(1)));
        // Pretend it stalled: move the clocks back
        let long_ago = Instant::now().checked_sub(MIC_STALL * 2).unwrap();
        lock(&capture.buffer).updated = Some(long_ago);
        lock(&capture.state).recording_since = Some(long_ago);
        assert!(capture.stalled(Instant::now()));
        capture.check(&mut logged);
        wait_for("reopen", || fake.recordings.load(Ordering::SeqCst) == 2);
        wait_for("old recording to end", || fake.live.load(Ordering::SeqCst) == 1);
        capture.stop();
        wait_for("stop", || fake.live.load(Ordering::SeqCst) == 0);
    }

    /// Proves that users of the same input share one recording (any name for playback), different
    /// microphones get their own, and a recording stops only when its last user releases it.
    #[test]
    fn recordings_are_shared_and_stop_with_the_last_release() {
        let fake = Arc::new(Fake::default());
        *lock(&fake.speaker) = Some(dev("1", "Speakers"));
        *lock(&fake.default_mic) = Some(dev("a", "Mic"));
        let backend: Arc<dyn Backend> = fake.clone();
        let registry = Registry::new();
        let a = registry.acquire(&backend, FAST, InputMode::Playback, "", None).unwrap();
        let b = registry.acquire(&backend, FAST, InputMode::Playback, "x", None).unwrap();
        let mic = registry.acquire(&backend, FAST, InputMode::Microphone, "", None).unwrap();
        let other_mic = registry.acquire(&backend, FAST, InputMode::Microphone, "x", None).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(!Arc::ptr_eq(&mic, &other_mic));
        assert_eq!(registry.len(), 3);
        registry.release(&a);
        assert!(!b.is_stopped());
        registry.release(&b);
        assert!(b.is_stopped());
        assert_eq!(registry.len(), 2);
        registry.release(&mic);
        registry.release(&other_mic);
        assert_eq!(registry.len(), 0);
    }

    /// Proves that off Windows, acquiring fails cleanly, nothing is registered, and no microphones are listed.
    #[cfg(not(windows))]
    #[test]
    fn capture_is_windows_only() {
        assert_eq!(acquire(InputMode::Playback, "", None).err().unwrap(), UNSUPPORTED);
        assert_eq!(REGISTRY.len(), 0);
        assert!(list_microphones().is_empty());
    }
}
