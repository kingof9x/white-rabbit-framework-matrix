//! The queue between the main loop and a panel's drawing thread: a port of `LatestFrameQueue` in
//! `led_mon/drawing.py`. A new frame replaces the one still waiting, so the panel never shows the past;
//! a change to the panel's own scrolling, a sleep or a stop keeps its place.
//!
//! Who uses it: `wr-monitor`'s main loop calls `put` about ten times a second for each panel, and that
//! panel's drawing thread (`wr-panel/src/panel.rs`, `Panel::run`) calls `get` in a loop and sends what
//! it gets to the panel. The two sides share one `FrameQueue` through an `Arc`.
//!
//! How it works: a `VecDeque` behind a `Mutex`, plus a `Condvar` that wakes the drawing thread when
//! something arrives. Sending a frame over the serial port can be slower than drawing it, so without
//! the "replace the waiting frame" rule the queue would grow and the panel would lag further and
//! further behind. Items that are not plain frames (sleep, stop, a scrolling change) must not be lost,
//! so those are appended instead of replacing anything.

use crate::Grid;
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// One thing for a panel's drawing thread to do.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // frames are what the queue carries; no point boxing them
pub enum Item {
    /// A frame, and Some(on/off) to turn the panel's own scrolling on or off.
    Frame(Grid, Option<bool>),
    /// Send the panel to sleep (it wakes on the next command).
    Sleep,
    /// Stop the drawing thread.
    Stop,
}

/// The newest-frame-wins queue itself. Safe to share between threads (`Arc<FrameQueue>`).
#[derive(Default)]
pub struct FrameQueue {
    /// Waiting items, oldest at the front. Usually holds zero or one frame.
    items: Mutex<VecDeque<Item>>,
    /// Signalled whenever an item is added, to wake a `get` that is waiting.
    ready: Condvar,
}

impl FrameQueue {
    /// An empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an item for the drawing thread and wakes it.
    ///
    /// If the last waiting item is a frame and the new item is a frame that does not change the panel's
    /// scrolling (its flag is `None` or the same as the waiting one's), the waiting frame is overwritten
    /// in place. The waiting frame's scrolling flag is kept, so a pending "turn scrolling on" is not lost
    /// when a plain frame replaces it. Anything else is appended at the back.
    ///
    /// Panics only if the lock is poisoned (another thread panicked while holding it).
    pub fn put(&self, item: Item) {
        let mut items = self.items.lock().unwrap();
        if let (Item::Frame(grid, animate), Some(Item::Frame(_, last_animate))) = (&item, items.back()) {
            if animate.is_none() || animate == last_animate {
                let keep = *last_animate;
                *items.back_mut().unwrap() = Item::Frame(grid.clone(), keep);
                self.ready.notify_one();
                return;
            }
        }
        items.push_back(item);
        self.ready.notify_one();
    }

    /// The next item, waiting up to `timeout` for one.
    ///
    /// Returns `None` if nothing arrived within `timeout` (the drawing thread uses that to do its own
    /// periodic work). `wait_timeout_while` also copes with spurious wake-ups: it keeps waiting while the
    /// queue is empty. Panics only if the lock is poisoned.
    pub fn get(&self, timeout: Duration) -> Option<Item> {
        let items = self.items.lock().unwrap();
        let (mut items, _) = self.ready.wait_timeout_while(items, timeout, |i| i.is_empty()).unwrap();
        items.pop_front()
    }

    /// How many items are waiting (takes the lock briefly).
    pub fn len(&self) -> usize {
        self.items.lock().unwrap().len()
    }

    /// True when nothing is waiting.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: a frame with LED (0, 0) set to `v`, so frames can be told apart.
    fn frame(v: i32, animate: Option<bool>) -> Item {
        let mut g = Grid::new();
        g.set(0, 0, v);
        Item::Frame(g, animate)
    }

    /// Proves that plain frames overwrite the one still waiting, so only the newest one is delivered.
    #[test]
    fn newest_frame_replaces_the_waiting_one() {
        let q = FrameQueue::new();
        q.put(frame(1, Some(false)));
        q.put(frame(2, None));
        q.put(frame(3, Some(false)));
        assert_eq!(q.len(), 1);
        assert_eq!(q.get(Duration::ZERO), Some(frame(3, Some(false))));
    }

    /// Proves that a scrolling change, a sleep and a stop are never overwritten and come out in order.
    #[test]
    fn animation_changes_sleep_and_stop_keep_their_place() {
        let q = FrameQueue::new();
        q.put(frame(1, Some(false)));
        q.put(frame(2, Some(true)));
        q.put(Item::Sleep);
        q.put(frame(3, None));
        q.put(Item::Stop);
        assert_eq!(q.len(), 5);
        assert_eq!(q.get(Duration::ZERO), Some(frame(1, Some(false))));
        assert_eq!(q.get(Duration::ZERO), Some(frame(2, Some(true))));
        assert_eq!(q.get(Duration::ZERO), Some(Item::Sleep));
        assert_eq!(q.get(Duration::ZERO), Some(frame(3, None)));
        assert_eq!(q.get(Duration::ZERO), Some(Item::Stop));
        assert_eq!(q.get(Duration::from_millis(1)), None);
    }
}
