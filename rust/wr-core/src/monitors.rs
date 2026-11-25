//! Smoothing the readings the way `led_mon/monitors.py` does, so the bars move the same.
//!
//! The raw readings (per-CPU load, GPU load, ...) are taken from Windows by `wr-monitor/src/sensors.rs`
//! on the monitor's main loop, and by the window's Monitor page preview
//! (`wr-window/src/pages/monitor/readings.rs`). Both pass them through these types so the panel and the
//! preview show the same thing. Nothing here touches the operating system: callers hand in the numbers
//! and the current time, which also makes the behaviour easy to test.
//!
//! Two kinds of smoothing: `CpuSmoother` averages each core's load over the last ten readings so a
//! CPU bar eases in and out instead of flickering, and `History` keeps one sample per second for the
//! scrolling graphs.

/// Logical CPUs grouped per physical core. Windows numbers a core's threads next to each other.
///
/// `logical` is the number of logical CPUs (threads) and `physical` the number of cores. With 16 and 8
/// the result is `[[0, 1], [2, 3], ...]`. A `physical` of 0 (unknown) is treated as one thread per
/// core, and a `logical` of 0 as 1, so the result is never empty. If the threads do not divide evenly
/// the last group is shorter.
pub fn core_groups(logical: usize, physical: usize) -> Vec<Vec<usize>> {
    let logical = logical.max(1);
    let physical = if physical == 0 { logical } else { physical };
    let per_core = (logical / physical).max(1);
    (0..logical).step_by(per_core).map(|i| (i..(i + per_core).min(logical)).collect()).collect()
}

/// The cpu app's values: each core's load averaged over the last 10 readings (so it eases in).
///
/// One output value per core (per group from `core_groups`), each 0.0-1.0.
pub struct CpuSmoother {
    /// Which logical CPUs belong to each core (from `core_groups`).
    groups: Vec<Vec<usize>>,
    /// Per core, the last `size` loads as fractions 0.0-1.0, oldest first.
    history: Vec<Vec<f64>>,
    /// How many readings are averaged (10, as in Python).
    size: usize,
}

impl CpuSmoother {
    /// A smoother for these core groups, with no history yet.
    pub fn new(groups: Vec<Vec<usize>>) -> Self {
        let history = vec![Vec::new(); groups.len()];
        CpuSmoother { groups, history, size: 10 }
    }

    /// `percent` is each logical CPU's load in percent (0-100), as psutil.cpu_percent(percpu=True).
    ///
    /// Records one reading and returns each core's smoothed load (0.0-1.0). A core's load is taken as
    /// "number of threads x busiest thread", capped at 100%, which is how the Python app estimated it.
    /// A missing CPU index counts as 0. The sum is always divided by 10, even before ten readings have
    /// arrived, so at start-up the bars grow from zero over the first second instead of jumping.
    pub fn add(&mut self, percent: &[f64]) -> Vec<f64> {
        for (i, group) in self.groups.iter().enumerate() {
            let max = group.iter().map(|&j| percent.get(j).copied().unwrap_or(0.0)).fold(0.0, f64::max);
            // e.g. two threads with the busiest at 30% -> 60% for the core
            let usage = (group.len() as f64 * max).min(100.0);
            self.history[i].push(usage / 100.0);
            if self.history[i].len() > self.size {
                // Dropping the oldest of ten is cheap; a ring buffer would not be worth it here.
                self.history[i].remove(0);
            }
        }
        self.history.iter().map(|h| (h.iter().sum::<f64>() / self.size as f64).clamp(0.0, 1.0)).collect()
    }
}

/// The last 16 readings for a scrolling graph, newest first, one per second.
///
/// Used for the CPU / GPU graphs. A sample is `None` when the reading was not available (for example no
/// GPU counter), which the graph draws as a gap.
pub struct History {
    /// Up to `size` samples, newest at index 0, each clamped to 0.0-1.0.
    pub samples: Vec<Option<f64>>,
    /// The `now` of the last sample taken, in seconds; `None` until the first one.
    last: Option<f64>,
    /// Seconds between samples (1.0).
    interval: f64,
    /// How many samples are kept (16).
    size: usize,
}

impl History {
    /// An empty history: the first `get` always takes a sample.
    pub fn new() -> Self {
        History { samples: Vec::new(), last: None, interval: 1.0, size: 16 }
    }

    /// Takes a reading from `read` when a second has passed since the last one.
    ///
    /// `now` is any steadily increasing clock in seconds (the callers use the time since start). `read` is
    /// only called when a sample is due, so an expensive reading is not taken ten times a second. Returns
    /// a copy of the samples, newest first.
    pub fn get(&mut self, now: f64, read: impl FnOnce() -> Option<f64>) -> Vec<Option<f64>> {
        if self.last.is_none_or(|t| now - t >= self.interval) {
            self.last = Some(now);
            let value = read().map(|v| v.clamp(0.0, 1.0));
            self.samples.insert(0, value);
            self.samples.truncate(self.size);
        }
        self.samples.clone()
    }
}

impl Default for History {
    /// Same as `History::new()`.
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves that neighbouring logical CPUs are grouped per core, and one-thread cores stay single.
    #[test]
    fn groups_threads_by_core() {
        assert_eq!(
            core_groups(16, 8),
            vec![vec![0, 1], vec![2, 3], vec![4, 5], vec![6, 7], vec![8, 9], vec![10, 11], vec![12, 13], vec![14, 15]]
        );
        assert_eq!(core_groups(4, 4), vec![vec![0], vec![1], vec![2], vec![3]]);
    }

    /// Proves the first reading is divided by ten (eases in) and a steady load settles at full value,
    /// including the 100% cap when two threads x 80% would be 160%.
    #[test]
    fn cpu_eases_in_over_ten_readings() {
        let mut cpu = CpuSmoother::new(core_groups(4, 2));
        assert_eq!(cpu.add(&[30.0, 10.0, 80.0, 0.0]), vec![0.06, 0.1]);
        for _ in 0..20 {
            cpu.add(&[30.0, 10.0, 80.0, 0.0]);
        }
        let v = cpu.add(&[30.0, 10.0, 80.0, 0.0]);
        assert!((v[0] - 0.6).abs() < 1e-9 && v[1] == 1.0, "{v:?}");
    }

    /// Proves that a reading is only taken once a second has passed, newest first, clamped to 1.0.
    #[test]
    fn history_once_a_second() {
        let mut h = History::new();
        assert_eq!(h.get(0.0, || Some(0.5)), vec![Some(0.5)]);
        assert_eq!(h.get(0.5, || Some(0.9)), vec![Some(0.5)]);
        assert_eq!(h.get(1.0, || Some(2.0)), vec![Some(1.0), Some(0.5)]);
    }
}
