//! Music mode and the audio visualizer app: a drawing thread per panel that listens to the sound and
//! hands a frame to the panel every 30 ms. A port of `run_equalizer` in
//! `led_mon/plugins/equalizer_plugin.py` and `Equalizer.run` in `led_mon/equalizer_files/visualize.py`.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use wr_core::queue::{FrameQueue, Item};
use wr_core::{error, info};
use wr_music::{capture, to_leds, MusicPanel, Settings};

/// How often a frame is drawn (Python's UPDATE_RATE: about 33 a second).
pub const FRAME_EVERY: Duration = Duration::from_millis(30);
/// After the sound can't be opened, wait this long before trying again.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Seconds of silence before a panel shows its look's calm idle version.
pub const IDLE_AFTER_SEC: f64 = 4.0;

/// What a panel's visualizer hears, for the window's Audio tab.
#[derive(Clone, Default)]
struct Hearing {
    device: Option<String>,
    problem: Option<String>,
    loudness: i64,
    silent: bool,
    input: String,
}

struct Running {
    settings: Settings,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    hearing: Arc<Mutex<Option<Hearing>>>,
}

impl Running {
    fn end(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The visualizers running now, one place per panel (0 left, 1 right).
pub struct Music {
    sides: [Option<Running>; 2],
    retry_after: [Option<Instant>; 2],
    /// The brightness of lit LEDs, set by the main loop every pass.
    pub foreground: Arc<AtomicI32>,
    /// While true the visualizers keep listening but leave the panels alone (the names are showing).
    pub held: Arc<AtomicBool>,
}

impl Default for Music {
    fn default() -> Self {
        Music {
            sides: [None, None],
            retry_after: [None, None],
            foreground: Arc::new(AtomicI32::new(255)),
            held: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Music {
    pub fn running(&self, side: usize) -> bool {
        self.sides[side].is_some()
    }

    /// Keeps a visualizer with these settings drawing on this panel: starts one, or starts it again
    /// when the settings changed.
    pub fn ensure(&mut self, side: usize, settings: &Settings, queue: &Arc<FrameQueue>) {
        if let Some(r) = &self.sides[side] {
            let ended = r.thread.as_ref().is_some_and(|t| t.is_finished());
            if r.settings == *settings && !ended {
                return;
            }
            if ended {
                // It couldn't open the sound: try again a little later
                self.retry_after[side] = Some(Instant::now() + RETRY_AFTER);
            }
            self.stop(side);
        }
        if self.retry_after[side].is_some_and(|t| Instant::now() < t) {
            return;
        }
        self.retry_after[side] = None;
        let stop = Arc::new(AtomicBool::new(false));
        let hearing = Arc::new(Mutex::new(None));
        let thread = {
            let (settings, stop, hearing) = (settings.clone(), stop.clone(), hearing.clone());
            let (queue, foreground, held) = (queue.clone(), self.foreground.clone(), self.held.clone());
            std::thread::Builder::new()
                .name(format!("music-{side}"))
                .spawn(move || draw(side, settings, queue, foreground, held, stop, hearing))
                .ok()
        };
        self.sides[side] = Some(Running { settings: settings.clone(), stop, thread, hearing });
    }

    pub fn stop(&mut self, side: usize) {
        if let Some(r) = self.sides[side].take() {
            r.end();
        }
    }

    pub fn stop_all(&mut self) {
        self.stop(0);
        self.stop(1);
    }

    /// {side: {device, problem, loudness, silent, input}} for the panels that hear something, as the
    /// Python monitor's `hearing()` returns it.
    pub fn hearing(&self) -> Value {
        let mut out = serde_json::Map::new();
        for (side, name) in [(0, "left"), (1, "right")] {
            let Some(r) = &self.sides[side] else { continue };
            if let Some(h) = r.hearing.lock().unwrap().clone() {
                out.insert(
                    name.into(),
                    json!({"device": h.device, "problem": h.problem, "loudness": h.loudness,
                           "silent": h.silent, "input": h.input}),
                );
            }
        }
        Value::Object(out)
    }
}

impl Drop for Music {
    fn drop(&mut self) {
        self.stop_all();
    }
}

fn draw(
    side: usize,
    settings: Settings,
    queue: Arc<FrameQueue>,
    foreground: Arc<AtomicI32>,
    held: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    hearing: Arc<Mutex<Option<Hearing>>>,
) {
    let input = settings.input_mode;
    let sound = match capture::acquire(input, &settings.input_device, None) {
        Ok(c) => c,
        Err(e) => {
            error!("Unable to open the audio visualizer's sound input: {e}");
            return;
        }
    };
    info!(
        "Audio visualizer ({}) on the {} panel, listening to {}",
        settings.style,
        ["left", "right"][side],
        input.as_str()
    );
    let mut panel = MusicPanel::new(&settings, side == 1, IDLE_AFTER_SEC);
    let mut last = Instant::now();
    let mut first = true;
    while !stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        let dt = started.duration_since(last).as_secs_f64();
        last = started;
        let frame = panel.frame(&sound.snapshot_f64(), dt);
        *hearing.lock().unwrap() = Some(Hearing {
            device: sound.device_name(),
            problem: sound.problem(),
            loudness: panel.loudness(),
            silent: panel.is_silent(),
            input: input.as_str().to_string(),
        });
        if !held.load(Ordering::SeqCst) {
            let grid = to_leds(&frame, foreground.load(Ordering::SeqCst));
            // The panel's own scrolling goes off once, with the first frame
            queue.put(Item::Frame(grid, if first { Some(false) } else { None }));
            first = false;
        }
        std::thread::sleep(FRAME_EVERY.saturating_sub(started.elapsed()));
    }
    capture::release(&sound);
}
