//! Disk and network rates as a share of the highest rate seen: `DiskMonitor` and `NetworkMonitor`
//! in `led_mon/monitors.py`.

/// Two byte counters that only go up (read and written, or sent and received), turned into two
/// rates from 0.0 to 1.0. The rate is taken over the last 20 readings, and 1.0 is the highest
/// rate seen so far.
pub struct RateMonitor {
    a: Vec<f64>,
    b: Vec<f64>,
    times: Vec<f64>,
    highest_a: f64,
    highest_b: f64,
    size: usize,
}

impl RateMonitor {
    pub fn new() -> Self {
        // Python starts its lists with a 0 at time 0, so the first rate is tiny; keep that.
        RateMonitor { a: vec![0.0], b: vec![0.0], times: vec![0.0], highest_a: 0.00001, highest_b: 0.00001, size: 20 }
    }

    /// Adds a reading of both counters at `now` (seconds since 1970, like Python's time.time()).
    pub fn add(&mut self, a: u64, b: u64, now: f64) -> (f64, f64) {
        self.a.push(a as f64);
        self.b.push(b as f64);
        self.times.push(now);
        if self.a.len() > self.size {
            let cut = self.a.len() - self.size;
            self.a.drain(..cut);
            self.b.drain(..cut);
            self.times.drain(..cut);
        }
        let time_diff = self.times[self.times.len() - 1] - self.times[0];
        if time_diff <= 0.0 {
            return (0.0, 0.0);
        }
        let rate_a = (self.a[self.a.len() - 1] - self.a[0]) / time_diff;
        let rate_b = (self.b[self.b.len() - 1] - self.b[0]) / time_diff;
        self.highest_a = self.highest_a.max(rate_a);
        self.highest_b = self.highest_b.max(rate_b);
        ((rate_a / self.highest_a).min(1.0), (rate_b / self.highest_b).min(1.0))
    }
}

impl Default for RateMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_follow_the_highest_seen() {
        let mut m = RateMonitor::new();
        // 100 bytes a second for a while (the start at time 0 drops out of the window)
        for i in 1..25 {
            assert_eq!(m.add(i * 100, i * 50, 1000.0 + i as f64), (1.0, 1.0));
        }
        // Then slower: the rate over the window drops below the highest
        let (a, b) = m.add(2410, 1205, 1025.0);
        assert!(a < 1.0 && a > 0.5 && b < 1.0, "{a} {b}");
    }

    #[test]
    fn keeps_twenty_readings() {
        let mut m = RateMonitor::new();
        for i in 0..30 {
            m.add(i * 10, i * 10, 1000.0 + i as f64);
        }
        assert_eq!(m.a.len(), 20);
    }
}
