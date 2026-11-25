//! A reading taken on its own thread, so the main loop never waits for WMI, a web service or a tool.
//! The thread starts on the first `get` and only reads while someone asked in the last 10 seconds.
//!
//! Used by `sensors.rs` for every slow source (temperatures, GPU, weather, media, FPS ...). The main
//! loop calls `get` ten times a second and always gets the latest stored value at once, even if the
//! reading thread is in the middle of a slow WMI query. The reading thread writes the value, the
//! main loop reads it; the only shared things are three small mutexes, each locked just long enough to
//! copy a value. Once started, a reading thread lives until the process exits; when nobody has asked
//! for 10 s it keeps sleeping without reading, so an app that is not on screen costs almost nothing.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Stop reading when nothing asked for this long.
const IDLE: Duration = Duration::from_secs(10);

/// Made on the reading thread and kept there, so it needn't be Send (WMI and PDH handles aren't).
///
/// Called once per interval; it returns the new value (a failed read should return a sensible
/// "nothing" value such as `None`, since there is no error channel).
type Reader<T> = Box<dyn FnMut() -> T>;
/// The reader can change the time between readings through the handle it is given.
pub type Interval = Arc<Mutex<Duration>>;
/// Builds the reader on the new thread. Only this builder has to be `Send`, not the reader it returns.
type MakeReader<T> = Box<dyn FnOnce(Interval) -> Reader<T> + Send>;

/// One slow reading kept up to date on a background thread (see the file header).
pub struct Background<T> {
    /// The latest reading (or the initial value until the first read finishes).
    value: Arc<Mutex<T>>,
    /// When `get` was last called; the thread reads only while this is less than `IDLE` ago.
    wanted: Arc<Mutex<Instant>>,
    /// The thread's name and the reader builder, until the first `get` takes them to start the thread.
    make: Option<(String, MakeReader<T>)>,
    /// Time between readings; the reader may change it (e.g. read less often while failing).
    interval: Interval,
}

/// Locks a mutex even if another thread panicked while holding it (a "poisoned" mutex): the values
/// here are plain data that stay valid, so carrying on is safe and better than crashing.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// `T: Clone` because each `get` hands out a copy; `Send + 'static` because the value crosses threads.
impl<T: Clone + Send + 'static> Background<T> {
    /// `make` runs on the new thread and returns the function that takes one reading; that way
    /// things that must live on one thread (COM, WMI) are made there.
    ///
    /// `name` names the thread (shows up in debuggers and panic logs), `interval` is the starting time
    /// between readings and `initial` is what `get` returns until the first reading arrives.
    /// No thread starts here.
    pub fn new(
        name: &str,
        interval: Duration,
        initial: T,
        make: impl FnOnce(Interval) -> Reader<T> + Send + 'static,
    ) -> Background<T> {
        Background {
            value: Arc::new(Mutex::new(initial)),
            wanted: Arc::new(Mutex::new(Instant::now())),
            make: Some((name.to_string(), Box::new(make))),
            interval: Arc::new(Mutex::new(interval)),
        }
    }

    /// The last reading. Starts the thread the first time.
    ///
    /// Never blocks on the reading itself, only on a brief lock. If the thread can't be started the
    /// failure is logged and `get` keeps returning the initial value.
    pub fn get(&mut self) -> T {
        *lock(&self.wanted) = Instant::now();
        // `take` leaves `None` behind, so the thread is started at most once.
        if let Some((name, make)) = self.make.take() {
            let (value, wanted, interval) = (self.value.clone(), self.wanted.clone(), self.interval.clone());
            let started = std::thread::Builder::new().name(name.clone()).spawn(move || {
                let mut read = make(interval.clone());
                loop {
                    // Read only while the value is wanted. The lock on `wanted` is a temporary, so it
                    // is released before the (possibly slow) `read()` call.
                    if lock(&wanted).elapsed() < IDLE {
                        let v = read();
                        *lock(&value) = v;
                    }
                    let wait = *lock(&interval);
                    // At least 100 ms between readings, whatever the reader asks for, so a bad interval
                    // can't turn this into a busy loop.
                    std::thread::sleep(wait.max(Duration::from_millis(100)));
                }
            });
            if let Err(e) = started {
                wr_core::warn!("Can't start the {name} thread: {e}");
            }
        }
        lock(&self.value).clone()
    }
}

/// Logs a failure once, until it works again or fails differently.
///
/// Sources such as WMI or a web service can fail every second for hours; this keeps the log to one
/// line per distinct problem.
#[derive(Default)]
pub struct OnceLog {
    /// The message logged last; `None` while things work.
    last: Option<String>,
}

impl OnceLog {
    /// Reports a failure: logged as a warning only if it differs from the last one.
    pub fn fail(&mut self, message: String) {
        if self.last.as_ref() != Some(&message) {
            wr_core::warn!("{message}");
            self.last = Some(message);
        }
    }

    /// The failure logged last, until it works again.
    pub fn error(&self) -> Option<&str> {
        self.last.as_deref()
    }

    /// Reports a success: if there was a failure before, logs "`what` works again" and forgets it.
    pub fn ok(&mut self, what: &str) {
        if self.last.take().is_some() {
            wr_core::info!("{what} works again");
        }
    }
}
