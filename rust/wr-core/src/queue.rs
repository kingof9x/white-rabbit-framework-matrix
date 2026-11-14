//! The queue between the main loop and a panel's drawing thread: a port of `LatestFrameQueue` in
//! `led_mon/drawing.py`. A new frame replaces the one still waiting, so the panel never shows the past;
//! a change to the panel's own scrolling, a sleep or a stop keeps its place.

use crate::Grid;
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

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

#[derive(Default)]
pub struct FrameQueue {
    items: Mutex<VecDeque<Item>>,
    ready: Condvar,
}

impl FrameQueue {
    pub fn new() -> Self {
        Self::default()
    }

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
    pub fn get(&self, timeout: Duration) -> Option<Item> {
        let items = self.items.lock().unwrap();
        let (mut items, _) = self.ready.wait_timeout_while(items, timeout, |i| i.is_empty()).unwrap();
        items.pop_front()
    }

    pub fn len(&self) -> usize {
        self.items.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(v: i32, animate: Option<bool>) -> Item {
        let mut g = Grid::new();
        g.set(0, 0, v);
        Item::Frame(g, animate)
    }

    #[test]
    fn newest_frame_replaces_the_waiting_one() {
        let q = FrameQueue::new();
        q.put(frame(1, Some(false)));
        q.put(frame(2, None));
        q.put(frame(3, Some(false)));
        assert_eq!(q.len(), 1);
        assert_eq!(q.get(Duration::ZERO), Some(frame(3, Some(false))));
    }

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
