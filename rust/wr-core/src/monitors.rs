//! Smoothing the readings the way `led_mon/monitors.py` does, so the bars move the same.

/// Logical CPUs grouped per physical core. Windows numbers a core's threads next to each other.
pub fn core_groups(logical: usize, physical: usize) -> Vec<Vec<usize>> {
    let logical = logical.max(1);
    let physical = if physical == 0 { logical } else { physical };
    let per_core = (logical / physical).max(1);
    (0..logical).step_by(per_core).map(|i| (i..(i + per_core).min(logical)).collect()).collect()
}

/// The cpu app's values: each core's load averaged over the last 10 readings (so it eases in).
pub struct CpuSmoother {
    groups: Vec<Vec<usize>>,
    history: Vec<Vec<f64>>,
    size: usize,
}

impl CpuSmoother {
    pub fn new(groups: Vec<Vec<usize>>) -> Self {
        let history = vec![Vec::new(); groups.len()];
        CpuSmoother { groups, history, size: 10 }
    }

    /// `percent` is each logical CPU's load in percent (0-100), as psutil.cpu_percent(percpu=True).
    pub fn add(&mut self, percent: &[f64]) -> Vec<f64> {
        for (i, group) in self.groups.iter().enumerate() {
            let max = group.iter().map(|&j| percent.get(j).copied().unwrap_or(0.0)).fold(0.0, f64::max);
            let usage = (group.len() as f64 * max).min(100.0);
            self.history[i].push(usage / 100.0);
            if self.history[i].len() > self.size {
                self.history[i].remove(0);
            }
        }
        self.history.iter().map(|h| (h.iter().sum::<f64>() / self.size as f64).clamp(0.0, 1.0)).collect()
    }
}

/// The last 16 readings for a scrolling graph, newest first, one per second.
pub struct History {
    pub samples: Vec<Option<f64>>,
    last: Option<f64>,
    interval: f64,
    size: usize,
}

impl History {
    pub fn new() -> Self {
        History { samples: Vec::new(), last: None, interval: 1.0, size: 16 }
    }

    /// Takes a reading from `read` when a second has passed since the last one.
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
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_threads_by_core() {
        assert_eq!(
            core_groups(16, 8),
            vec![vec![0, 1], vec![2, 3], vec![4, 5], vec![6, 7], vec![8, 9], vec![10, 11], vec![12, 13], vec![14, 15]]
        );
        assert_eq!(core_groups(4, 4), vec![vec![0], vec![1], vec![2], vec![3]]);
    }

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

    #[test]
    fn history_once_a_second() {
        let mut h = History::new();
        assert_eq!(h.get(0.0, || Some(0.5)), vec![Some(0.5)]);
        assert_eq!(h.get(0.5, || Some(0.9)), vec![Some(0.5)]);
        assert_eq!(h.get(1.0, || Some(2.0)), vec![Some(1.0), Some(0.5)]);
    }
}
