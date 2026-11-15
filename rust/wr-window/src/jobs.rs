//! Things the window keeps sending to the panels on a thread of their own: the clock, the countdown, the
//! random equalizer, a game, a Math picture. Starting one on a panel stops the one before it there, and
//! giving the panel back to the monitor stops it too (`inputmodule/gui/gui_threading.py`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

type Flags = Mutex<HashMap<String, Arc<AtomicBool>>>;

fn running() -> &'static Flags {
    static RUNNING: OnceLock<Flags> = OnceLock::new();
    RUNNING.get_or_init(Flags::default)
}

/// A job's stop flag. `stopped()` turns true once something else takes over the panels.
#[derive(Clone)]
pub struct Job(Arc<AtomicBool>);

impl Job {
    pub fn stopped(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn stop(&self) {
        self.0.store(true, Ordering::SeqCst)
    }
}

/// Starts a job on these panels, stopping what ran on them before.
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
pub fn stop_panel_jobs(ports: &[String]) {
    let mut map = running().lock().unwrap();
    for p in ports {
        if let Some(old) = map.remove(p) {
            old.store(true, Ordering::SeqCst);
        }
    }
}

pub fn stop_all() {
    let mut map = running().lock().unwrap();
    for (_, flag) in map.drain() {
        flag.store(true, Ordering::SeqCst);
    }
}

/// Sleeps in short steps; false if the job was stopped meanwhile.
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

#[cfg(test)]
mod tests {
    use super::*;

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
