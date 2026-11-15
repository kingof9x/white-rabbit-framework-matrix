//! A reading taken on its own thread, so the main loop never waits for WMI, a web service or a tool.
//! The thread starts on the first `get` and only reads while someone asked in the last 10 seconds.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Stop reading when nothing asked for this long.
const IDLE: Duration = Duration::from_secs(10);

/// Made on the reading thread and kept there, so it needn't be Send (WMI and PDH handles aren't).
type Reader<T> = Box<dyn FnMut() -> T>;
/// The reader can change the time between readings through the handle it is given.
pub type Interval = Arc<Mutex<Duration>>;
type MakeReader<T> = Box<dyn FnOnce(Interval) -> Reader<T> + Send>;

pub struct Background<T> {
    value: Arc<Mutex<T>>,
    wanted: Arc<Mutex<Instant>>,
    make: Option<(String, MakeReader<T>)>,
    interval: Interval,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl<T: Clone + Send + 'static> Background<T> {
    /// `make` runs on the new thread and returns the function that takes one reading; that way
    /// things that must live on one thread (COM, WMI) are made there.
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
    pub fn get(&mut self) -> T {
        *lock(&self.wanted) = Instant::now();
        if let Some((name, make)) = self.make.take() {
            let (value, wanted, interval) = (self.value.clone(), self.wanted.clone(), self.interval.clone());
            let started = std::thread::Builder::new().name(name.clone()).spawn(move || {
                let mut read = make(interval.clone());
                loop {
                    if lock(&wanted).elapsed() < IDLE {
                        let v = read();
                        *lock(&value) = v;
                    }
                    let wait = *lock(&interval);
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
#[derive(Default)]
pub struct OnceLog {
    last: Option<String>,
}

impl OnceLog {
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

    pub fn ok(&mut self, what: &str) {
        if self.last.take().is_some() {
            wr_core::info!("{what} works again");
        }
    }
}
