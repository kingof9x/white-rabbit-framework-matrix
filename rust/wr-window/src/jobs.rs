//! Things the window keeps sending to the panels on a thread of their own: the clock, the countdown, the
//! random equalizer, a game, a Math picture. Starting one on a panel stops the one before it there, and
//! giving the panel back to the monitor stops it too (`inputmodule/gui/gui_threading.py`).
//!
//! How it works: a job is just a shared stop flag (`Arc<AtomicBool>`). The code that starts a job calls
//! `start(ports)` to get a `Job`, spawns its own thread, and that thread checks `job.stopped()` (or
//! sleeps with `sleep`, which checks it every 10 ms) between frames and returns when it turns true. A
//! global map remembers, for each port, the flag of the job running there, so the next `start` on that
//! port, `stop_panel_jobs` (called by `Lender::give_back`) or `stop_all` (called when the window
//! closes) can raise it. Nothing is joined or killed: a stopped thread notices within one frame or
//! 10 ms and ends by itself.
//!
//! One job can cover several ports (a game across both panels). Raising the flag stops the whole job,
//! so taking over either of its panels stops it on both.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Port name -> stop flag of the job now running on that port.
type Flags = Mutex<HashMap<String, Arc<AtomicBool>>>;

/// The one global port -> flag map, made on first use. Locked only briefly, never while sleeping.
fn running() -> &'static Flags {
    static RUNNING: OnceLock<Flags> = OnceLock::new();
    RUNNING.get_or_init(Flags::default)
}

/// A job's stop flag. `stopped()` turns true once something else takes over the panels.
///
/// Cloneable: every clone shares the same flag.
#[derive(Clone)]
pub struct Job(Arc<AtomicBool>);

impl Job {
    /// True once this job has been told to stop (by a newer job on one of its panels, by giving the panels
    /// back, by closing the window, or by `stop`).
    pub fn stopped(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// Stops this job (its thread ends at its next check). Leaves the map alone: the next `start` replaces
    /// the entry anyway.
    pub fn stop(&self) {
        self.0.store(true, Ordering::SeqCst)
    }
}

/// Starts a job on these panels, stopping what ran on them before.
///
/// The old job on each port has its flag raised; if that old job also covered other ports it stops
/// there too. Returns the new job's handle for the caller's thread.
pub fn start(ports: &[String]) -> Job {
    let flag = Arc::new(AtomicBool::new(false));
    let mut map = running().lock().unwrap();
    for p in ports {
        if let Some(old) = map.insert(p.clone(), flag.clone()) {
            old.store(true, Ordering::SeqCst);
        }
    }
    Job(flag)
}

/// Stops whatever runs on these panels.
///
/// Removes their entries from the map and raises the flags (a job on other ports as well stops there
/// too). Ports with no job are ignored.
pub fn stop_panel_jobs(ports: &[String]) {
    let mut map = running().lock().unwrap();
    for p in ports {
        if let Some(old) = map.remove(p) {
            old.store(true, Ordering::SeqCst);
        }
    }
}

/// Stops every job; used when the window closes.
pub fn stop_all() {
    let mut map = running().lock().unwrap();
    for (_, flag) in map.drain() {
        flag.store(true, Ordering::SeqCst);
    }
}

/// Sleeps in short steps; false if the job was stopped meanwhile.
///
/// Steps of at most 10 ms, so a stop is noticed quickly without busy-waiting. `seconds` must not be
/// negative (`Duration::from_secs_f64` panics on that). Returns true if the full time passed with the
/// job still running, so callers write `if !sleep(&job, t) { return; }`.
pub fn sleep(job: &Job, seconds: f64) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_secs_f64(seconds);
    while std::time::Instant::now() < end {
        if job.stopped() {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10).min(end - std::time::Instant::now()));
    }
    !job.stopped()
}

/// Tests for the job stop flags.
#[cfg(test)]
mod tests {
    use super::*;

    /// Starting a job on a port stops the job already there (even one also on other ports), and
    /// `stop_panel_jobs` stops the new one.
    #[test]
    fn a_new_job_stops_the_old_one() {
        let a = start(&["T1".into(), "T2".into()]);
        let b = start(&["T2".into()]);
        assert!(a.stopped());
        assert!(!b.stopped());
        stop_panel_jobs(&["T2".into()]);
        assert!(b.stopped());
    }
}
