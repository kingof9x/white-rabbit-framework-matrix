//! Disk and network rates as a share of the highest rate seen: `DiskMonitor` and `NetworkMonitor`
//! in `led_mon/monitors.py`.
//!
//! Where this sits: the monitor's readings object (`wr-monitor`'s `sensors.rs`, and the preview in
//! `wr-window`) owns one `RateMonitor` for the disk and one for the network. Each time the disk, net
//! or disk-net app is drawn it reads the system's total byte counters (bytes read/written since boot,
//! or bytes sent/received) and calls `RateMonitor::add`, which turns them into two 0.0-1.0 bar
//! heights. Everything here is plain arithmetic; there is no I/O and no thread.
//!
//! The idea: a counter that only goes up is turned into a speed by comparing the newest reading with
//! the oldest one kept (up to 20 readings back), divided by the seconds between them. That averages
//! out spikes. The speed is then shown relative to the fastest speed seen since the program started,
//! so the bars adapt to the machine (a fast SSD and a slow hard disk both fill the bar at their peak).

/// Two byte counters that only go up (read and written, or sent and received), turned into two
/// rates from 0.0 to 1.0. The rate is taken over the last 20 readings, and 1.0 is the highest
/// rate seen so far.
/// Not shared between threads; the owner calls `add` from the one thread that draws.
pub struct RateMonitor {
    /// Readings of the first counter (bytes read, or bytes sent), oldest first, at most `size` of them.
    a: Vec<f64>,
    /// Readings of the second counter (bytes written, or bytes received), same length as `a`.
    b: Vec<f64>,
    /// When each reading was taken, in seconds since 1970; same length as `a`.
    times: Vec<f64>,
    /// The fastest rate of the first counter seen so far, in bytes a second. It never goes down, so one
    /// big burst makes later normal traffic look small until the program restarts (as in Python).
    highest_a: f64,
    /// The fastest rate of the second counter seen so far, in bytes a second.
    highest_b: f64,
    /// How many readings to keep (20). The monitor draws ten times a second, so this is about two seconds.
    size: usize,
}

impl RateMonitor {
    /// A monitor with no real readings yet. The highest rates start at a tiny positive number (0.00001
    /// bytes a second) rather than 0 so the division in `add` never divides by zero.
    pub fn new() -> Self {
        // Python starts its lists with a 0 at time 0, so the first rate is tiny; keep that.
        RateMonitor { a: vec![0.0], b: vec![0.0], times: vec![0.0], highest_a: 0.00001, highest_b: 0.00001, size: 20 }
    }

    /// Adds a reading of both counters at `now` (seconds since 1970, like Python's time.time()).
    /// Returns the two rates as fractions 0.0-1.0 of the highest seen (1.0 = as fast as ever). Returns
    /// (0.0, 0.0) when no time has passed between the oldest and newest reading (for example two
    /// readings in the same instant, or a clock that went backwards).
    pub fn add(&mut self, a: u64, b: u64, now: f64) -> (f64, f64) {
        self.a.push(a as f64);
        self.b.push(b as f64);
        self.times.push(now);
        // Drop the oldest readings so only the last `size` remain; the three lists stay in step.
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
        // Average speed over the whole window: bytes added between the oldest and newest
        // reading, divided by the seconds between them.
        let rate_a = (self.a[self.a.len() - 1] - self.a[0]) / time_diff;
        let rate_b = (self.b[self.b.len() - 1] - self.b[0]) / time_diff;
        // Remember a new record before dividing, so the current rate is at most 1.0 of it; the
        // `.min(1.0)` below is just a guard against rounding.
        self.highest_a = self.highest_a.max(rate_a);
        self.highest_b = self.highest_b.max(rate_b);
        ((rate_a / self.highest_a).min(1.0), (rate_b / self.highest_b).min(1.0))
    }
}

/// The same as `RateMonitor::new()` (needed so owners can derive `Default`, and asked for by clippy).
impl Default for RateMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A steady speed shows as a full bar; a slower speed afterwards shows as part of the bar, because
    /// the highest speed seen is remembered.
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

    /// After 30 readings only the newest 20 are kept.
    #[test]
    fn keeps_twenty_readings() {
        let mut m = RateMonitor::new();
        for i in 0..30 {
            m.add(i * 10, i * 10, 1000.0 + i as f64);
        }
        assert_eq!(m.a.len(), 20);
    }
}
