//! Does the monitor's drawing path leak? Runs the steady-state work of the monitor's main loop many
//! thousands of times (pick each panel's apps from the bundled layout, draw them, put the effects over
//! them, send the frame through a drawing thread's code to a made-up serial port) and checks that the
//! memory in use at the end is no larger than after a warm-up.
//!
//! Memory is counted by wrapping the system allocator (bytes allocated minus bytes freed). This is
//! the program's own heap only: it can't see the Windows-only parts (sensors, tray, sound capture),
//! which this test can't run. It is its own test binary, with one test, so nothing else allocates
//! while it measures.

use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::io;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wr_core::effects::EffectsSettings;
use wr_core::layout::{App, Layout, Rotation};
use wr_core::queue::Item;
use wr_core::render::{brightness_values, render_panel_on, Readings};
use wr_core::Grid;
use wr_panel::hub::{Port, PortSystem};
use wr_panel::overlay::EffectsOverlay;
use wr_panel::panel::{Panel, Panels};
use wr_panel::ports::{Pins, PortInfo};
use wr_panel::protocol;

/// The counting allocator: forwards to the system allocator and keeps a running total in `LIVE`.
struct Counting;
/// Bytes allocated and not yet freed by the whole test binary (signed, so a realloc that shrinks can
/// subtract). Counts requested sizes, not the allocator's own overhead.
static LIVE: AtomicIsize = AtomicIsize::new(0);

// SAFETY: passes every call straight to the system allocator, only counting the sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: AllocLayout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: AllocLayout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: AllocLayout, new_size: usize) -> *mut u8 {
        // Counted as the change in size, whether the block grew or shrank.
        LIVE.fetch_add(new_size as isize - layout.size() as isize, Ordering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

/// Makes every allocation in this test binary go through `Counting`.
#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// A panel that takes every write and forgets it.
struct Sink;
impl Port for Sink {
    fn write_all(&mut self, _: &[u8]) -> io::Result<()> {
        Ok(())
    }
    fn read(&mut self, size: usize, _: Duration) -> io::Result<Vec<u8>> {
        Ok(vec![0; size])
    }
    fn clear_input(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Two panels that are always plugged in at the usual slots and never fail.
struct TwoPanels;
impl PortSystem for TwoPanels {
    fn list(&self) -> Vec<PortInfo> {
        [("COM4", "1-4.2:x.0"), ("COM5", "1-3.3:x.0")]
            .iter()
            .map(|(d, l)| PortInfo {
                device: d.to_string(),
                vid: Some(protocol::LED_MATRIX_VID),
                pid: Some(protocol::LED_MATRIX_PID),
                location: Some(l.to_string()),
                description: String::new(),
            })
            .collect()
    }
    fn open(&self, _: &str) -> io::Result<Box<dyn Port>> {
        Ok(Box::new(Sink))
    }
}

/// Readings that change every pass, like a busy laptop.
struct Changing(u64);
impl Readings for Changing {
    /// Eight cores whose load changes every pass.
    fn cpu_cores(&mut self) -> Vec<f64> {
        (0..8).map(|i| ((self.0 + i) % 100) as f64 / 100.0).collect()
    }
    /// 30 history points, all known.
    fn cpu_history(&mut self) -> Vec<Option<f64>> {
        (0..30).map(|i| Some(((self.0 + i) % 100) as f64 / 100.0)).collect()
    }
    fn memory(&mut self) -> f64 {
        (self.0 % 100) as f64 / 100.0
    }
    fn battery(&mut self) -> Option<(f64, bool)> {
        Some(((self.0 % 100) as f64 / 100.0, self.0 % 200 < 100))
    }
    /// A four-digit time and a blinking colon, changing every pass.
    fn clock(&mut self, _: &App) -> (String, bool) {
        (format!("{:04}", self.0 % 2400), self.0.is_multiple_of(2))
    }
    /// Simulated seconds: 0.1 per pass.
    fn now(&mut self) -> f64 {
        self.0 as f64 * 0.1
    }
    fn disk(&mut self) -> (f64, f64) {
        ((self.0 % 7) as f64 / 7.0, (self.0 % 5) as f64 / 5.0)
    }
    fn net(&mut self) -> (f64, f64) {
        ((self.0 % 11) as f64 / 11.0, (self.0 % 13) as f64 / 13.0)
    }
}

/// Proves that the main loop's drawing path, run for tens of thousands of passes, keeps a flat memory
/// use after its warm-up (less than 16 kB of growth).
#[test]
fn the_drawing_path_does_not_grow() {
    let layout = Layout::parse(include_str!("../../../led_mon/config.yaml")).expect("bundled layout");
    let mut rotation = Rotation::new(layout);
    let panels = Panels::new(Arc::new(TwoPanels), Arc::new(Mutex::new(Pins::default())));
    let overlay = EffectsOverlay::new(EffectsSettings::default());
    panels.set_overlay(overlay.clone());
    let mut drawing = [Panel::new(0, panels.clone()), Panel::new(1, panels.clone())];
    let mut readings = Changing(0);

    // One pass of the main loop, at simulated time `t` seconds (10 passes a second, as the monitor)
    // Note: the effects overlay keeps real time (its own `Instant`), not this simulated time, so the
    // ripples and messages started below may still be playing when the next ones start.
    let mut pass = |i: u64| {
        let t = i as f64 * 0.1;
        readings.0 = i;
        let changed = rotation.step(t, false);
        let (fg, bg) = brightness_values((i % 100) as f64 / 100.0);
        if i.is_multiple_of(50) {
            overlay.effects().say("Battery 20%");
        }
        if i.is_multiple_of(70) {
            overlay.plug(Some("left-back"), i.is_multiple_of(140));
        }
        for (side, panel) in drawing.iter_mut().enumerate() {
            let apps = rotation.panel(side == 1);
            let mut grid = Grid::new();
            readings.set_panel(side == 1);
            let animate = render_panel_on(&mut grid, &apps, &mut readings, fg, bg, ["left", "right"][side]);
            // The scroll setting is only sent when that side's app changed, as the monitor does.
            let scroll = [changed.left, changed.right][side].then_some(animate);
            // Called directly, without the drawing threads, so the whole pass runs on this one thread.
            panel.handle(Item::Frame(grid, scroll));
        }
    };

    // Warm up: every app in the layout has been shown, caches and history buffers are full
    let warm_up = 5_000;
    for i in 0..warm_up {
        pass(i);
    }
    // Measured with no other thread running, so the number is only this test's allocations.
    let after_warm_up = LIVE.load(Ordering::Relaxed);
    // 25,000 passes (about 40 minutes of the monitor's main loop, in simulated time) in CI;
    // WR_MEMORY_PASSES=500000 runs about 14 hours' worth (7 minutes on a debug build)
    let passes: u64 = std::env::var("WR_MEMORY_PASSES").ok().and_then(|s| s.parse().ok()).unwrap_or(25_000);
    for i in warm_up..warm_up + passes {
        pass(i);
    }
    let at_end = LIVE.load(Ordering::Relaxed);
    let growth = at_end - after_warm_up;
    eprintln!("memory: {after_warm_up} bytes in use after the warm-up, {at_end} after {passes} more passes");
    // Allow a little noise (a hash map resizing once), not steady growth: a leak of even 1 byte a
    // pass shows as 25 kB or more here
    assert!(growth < 16 * 1024, "the drawing path grew by {growth} bytes over {passes} passes");
}
