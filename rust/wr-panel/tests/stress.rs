//! Stress test of the panel layer with made-up serial ports: everything that can happen at once on a
//! real laptop, much faster than it happens there.
//!
//! - both drawing threads take frames as fast as the main loop can hand them over;
//! - the effects overlay plays plug ripples and text-line messages over them;
//! - a "window" thread borrows panels (hold), writes to them (serial, serial_many), reads replies and
//!   gives them back (release), and sometimes "crashes" (its panels are given back as orphans);
//! - a "USB" thread unplugs panels, plugs them back in at other COM ports and USB positions, swaps
//!   the sides, and makes writes fail as a panel does when the keyboard is moved.
//!
//! It checks that nothing deadlocks (every thread ends within a deadline), that no drawing-thread
//! frame reaches a panel while the window holds it, that the window's own writes always go through
//! while it holds a panel, and that both sides find their panels again at the end.
//!
//! Runs for 2 seconds in CI. `WR_STRESS_SECONDS=60 cargo test -p wr-panel --test stress` runs longer.

use std::collections::HashSet;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use wr_core::effects::EffectsSettings;
use wr_core::queue::{FrameQueue, Item};
use wr_core::Grid;
use wr_panel::hub::{Port, PortSystem};
use wr_panel::overlay::EffectsOverlay;
use wr_panel::panel::{Panel, Panels};
use wr_panel::ports::{Pins, PortInfo};
use wr_panel::protocol;

/// A tiny random number generator (xorshift), so the test needs no extra crate and a seed repeats a run.
struct Rng(u64);
impl Rng {
    /// The next pseudo-random number (Marsaglia's xorshift64: three shift-and-xor steps). The seed must
    /// not be 0, or every number is 0.
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// A number in 0..n (slightly uneven for large n, which doesn't matter here). `n` must not be 0.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// The first byte the "window" writes after the magic: the drawing threads never send it.
const WINDOW_MARK: u8 = 0xEE;

/// The made-up hardware, shared by the fake ports, the fake system and the "USB" and "window" threads.
#[derive(Default)]
struct World {
    /// The ports plugged in now: (COM name, USB position)
    plugged: Mutex<Vec<(String, String)>>,
    /// Ports whose next writes fail (the panel was pulled out mid-frame)
    failing: Mutex<HashSet<String>>,
    /// Ports the window has finished holding (hold() returned) and not yet started releasing
    confirmed_held: Mutex<HashSet<String>>,
    /// Problems seen by the fake ports
    violations: Mutex<Vec<String>>,
    /// Writes that came from the drawing threads (frames, scroll, sleep, LED current).
    monitor_writes: AtomicU64,
    /// Writes that came from the "window" thread (marked with `WINDOW_MARK`).
    window_writes: AtomicU64,
}

/// One opened fake port. It checks the world on every write, so unplugging a panel in `World` breaks
/// ports that are already open, as on real hardware.
struct FakePort {
    name: String,
    world: Arc<World>,
}

impl Port for FakePort {
    /// Fails if the port is unplugged or set to fail; otherwise counts the write and records a violation
    /// if it is a drawing-thread frame (StageCol or FlushCols) on a port the window has confirmed it holds.
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        let w = &self.world;
        let key = self.name.to_lowercase();
        // An open handle to a port that has been unplugged fails, as on real hardware.
        let unplugged = !w.plugged.lock().unwrap().iter().any(|(n, _)| n.eq_ignore_ascii_case(&self.name));
        if unplugged || w.failing.lock().unwrap().contains(&key) {
            return Err(io::Error::other("the device is gone"));
        }
        let from_window = data.get(2) == Some(&WINDOW_MARK);
        if from_window {
            w.window_writes.fetch_add(1, Ordering::Relaxed);
        } else {
            w.monitor_writes.fetch_add(1, Ordering::Relaxed);
            // Only frame commands count as violations. The LED current is also a drawing-thread write, but
            // the hub may legally reset it (in `hold`) while the panel is being held.
            let is_frame = matches!(data.get(2), Some(&protocol::STAGE_COL) | Some(&protocol::FLUSH_COLS));
            if is_frame && w.confirmed_held.lock().unwrap().contains(&key) {
                w.violations.lock().unwrap().push(format!("a drawing-thread frame reached {} while held", self.name));
            }
        }
        // Let other threads in between writes, as a real USB transfer would. (Not a sleep: on Windows
        // even a 20 µs sleep lasts a whole timer tick, about 15 ms, which starved the test on CI.)
        thread::yield_now();
        Ok(())
    }

    /// Always answers at once with zeros (at most 32 bytes), like White Rabbit firmware.
    fn read(&mut self, size: usize, _timeout: Duration) -> io::Result<Vec<u8>> {
        Ok(vec![0; size.min(protocol::ANSWER_LEN)])
    }

    fn clear_input(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Lists the plugged ports as LED panels and opens only those.
struct FakeSystem(Arc<World>);

impl PortSystem for FakeSystem {
    /// Every plugged port, as an LED panel at its USB position.
    fn list(&self) -> Vec<PortInfo> {
        self.0
            .plugged
            .lock()
            .unwrap()
            .iter()
            .map(|(name, location)| PortInfo {
                device: name.clone(),
                vid: Some(protocol::LED_MATRIX_VID),
                pid: Some(protocol::LED_MATRIX_PID),
                location: Some(format!("{location}:x.0")),
                description: String::new(),
            })
            .collect()
    }

    /// Opens a plugged port; an unplugged one gives "not found".
    fn open(&self, name: &str) -> io::Result<Box<dyn Port>> {
        if !self.0.plugged.lock().unwrap().iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        Ok(Box::new(FakePort { name: name.to_string(), world: self.0.clone() }))
    }
}

/// How long to run, from `WR_STRESS_SECONDS` (default 2 s).
fn seconds() -> f64 {
    std::env::var("WR_STRESS_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(2.0)
}

/// Proves that the panel layer has no deadlock, never lets a drawing-thread frame reach a held panel,
/// and finds both panels again, while every thread that touches it runs at full speed at once.
#[test]
fn everything_at_once() {
    let world = Arc::new(World::default());
    *world.plugged.lock().unwrap() = vec![("COM4".into(), "1-4.2".into()), ("COM5".into(), "1-3.3".into())];
    let panels = Panels::new(Arc::new(FakeSystem(world.clone())), Arc::new(Mutex::new(Pins::default())));
    let overlay = EffectsOverlay::new(EffectsSettings::default());
    panels.set_overlay(overlay.clone());
    // Dimming by LED current on, so the current commands and the dimming query are exercised too.
    panels.options.full_shades.store(true, Ordering::Relaxed);
    panels.options.foreground.store(120, Ordering::Relaxed);

    let queues = [Arc::new(FrameQueue::new()), Arc::new(FrameQueue::new())];
    let stop = Arc::new(AtomicBool::new(false));
    let deadline = Duration::from_secs_f64(seconds());
    let mut threads: Vec<(&str, thread::JoinHandle<()>)> = Vec::new();

    // (They are kept at the front of `threads`: they are joined last, after the panels are back.)
    // The two drawing threads, as the monitor starts them
    for (side, queue) in queues.iter().enumerate() {
        let (panel, queue) = (Panel::new(side, panels.clone()), queue.clone());
        threads.push(("drawing", thread::spawn(move || panel.run(queue))));
    }

    // The main loop: frames as fast as possible, scrolling changes and sleeps now and then
    let frames = Arc::new(AtomicUsize::new(0));
    {
        let (queues, stop, frames, overlay) = (queues.clone(), stop.clone(), frames.clone(), overlay.clone());
        threads.push((
            "main loop",
            thread::spawn(move || {
                // Fixed seeds, so a failing run can be repeated (thread timing still varies from run to run).
                let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
                while !stop.load(Ordering::Relaxed) {
                    for q in &queues {
                        let mut g = Grid::new();
                        for _ in 0..rng.below(40) {
                            g.set(rng.below(9) as usize, rng.below(34) as usize, rng.below(300) as i32);
                        }
                        let animate = match rng.below(50) {
                            0 => Some(true),
                            1 => Some(false),
                            _ => None,
                        };
                        // Mostly random frames; now and then a change of the firmware scroll or a sleep.
                        q.put(if rng.below(500) == 0 { Item::Sleep } else { Item::Frame(g, animate) });
                    }
                    match rng.below(200) {
                        0 => {
                            overlay.plug(Some(["left-back", "right-front", "left-front"][rng.below(3) as usize]), true)
                        }
                        1 => overlay.effects().say("Stress test"),
                        2 => overlay.start_slide(rng.below(2) as usize),
                        _ => {}
                    }
                    frames.fetch_add(1, Ordering::Relaxed);
                    thread::sleep(Duration::from_micros(200));
                }
            }),
        ));
    }

    // The window: borrow, write, read, give back; sometimes "crash" without giving back
    {
        let (panels, stop, world) = (panels.clone(), stop.clone(), world.clone());
        threads.push((
            "window",
            thread::spawn(move || {
                let mut rng = Rng(0xD1B5_4A32_D192_ED03);
                let mark = protocol::command(WINDOW_MARK, &[1, 2, 3]);
                while !stop.load(Ordering::Relaxed) {
                    let names: Vec<String> = panels.port_names().into_iter().flatten().collect();
                    if names.is_empty() {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    // One of three made-up process ids, so the orphan check below has others that are still "alive".
                    let pid = 1000 + rng.below(3) as u32;
                    panels.hub().hold(&names, Some(pid));
                    // Only after `hold` has returned is a drawing-thread frame a violation: during `hold` a frame
                    // already being written may still finish, which is allowed.
                    world.confirmed_held.lock().unwrap().extend(names.iter().map(|n| n.to_lowercase()));
                    for _ in 0..rng.below(20) {
                        let name = &names[rng.below(names.len() as u64) as usize];
                        // Errors are expected: the USB thread pulls panels out at any time
                        let _ = panels.hub().serial(
                            name,
                            rng.below(2) == 0,
                            std::slice::from_ref(&mark),
                            rng.below(2) as usize * 32,
                            Some(Duration::from_millis(5)),
                        );
                        let many: Vec<(String, Vec<Vec<u8>>)> =
                            names.iter().map(|n| (n.clone(), vec![mark.clone(), mark.clone()])).collect();
                        let _ = panels.hub().serial_many(&many);
                    }
                    // No longer checked from here, before the release starts, for the same reason.
                    for n in &names {
                        world.confirmed_held.lock().unwrap().remove(&n.to_lowercase());
                    }
                    if rng.below(5) == 0 {
                        // The window "crashed": the monitor's orphan check gives its panels back
                        panels.hub().release_dead_holders(&|p| p != pid);
                    } else {
                        panels.hub().release(Some(&names));
                    }
                    thread::sleep(Duration::from_micros(rng.below(2000)));
                }
                panels.hub().release(None);
            }),
        ));
    }

    // USB: unplug and replug at new COM ports and positions, fail writes, swap the sides
    {
        let (panels, stop, world) = (panels.clone(), stop.clone(), world.clone());
        threads.push((
            "usb",
            thread::spawn(move || {
                let mut rng = Rng(0x2545_F491_4F6C_DD1D);
                let mut next_com = 6;
                while !stop.load(Ordering::Relaxed) {
                    match rng.below(6) {
                        0 => {
                            // Pull one panel out
                            let mut p = world.plugged.lock().unwrap();
                            if !p.is_empty() {
                                let i = rng.below(p.len() as u64) as usize;
                                p.remove(i);
                            }
                        }
                        1 => {
                            // Plug a panel in, at a new COM port, in a free slot
                            let mut p = world.plugged.lock().unwrap();
                            for slot in ["1-4.2", "1-3.3"] {
                                if p.len() < 2 && !p.iter().any(|(_, l)| l == slot) {
                                    p.push((format!("COM{next_com}"), slot.into()));
                                    next_com += 1;
                                    break;
                                }
                            }
                        }
                        // Make the next writes to one panel fail, as when the keyboard is lifted mid-frame.
                        2 => {
                            let p = world.plugged.lock().unwrap();
                            if let Some((n, _)) = p.get(rng.below(2) as usize) {
                                world.failing.lock().unwrap().insert(n.to_lowercase());
                            }
                        }
                        3 => world.failing.lock().unwrap().clear(),
                        4 => panels.swap(),
                        _ => {
                            panels.align();
                        }
                    }
                    thread::sleep(Duration::from_millis(rng.below(30)));
                }
                // Leave both panels plugged in and working
                world.failing.lock().unwrap().clear();
                let mut p = world.plugged.lock().unwrap();
                p.clear();
                p.push((format!("COM{next_com}"), "1-4.2".into()));
                p.push((format!("COM{}", next_com + 1), "1-3.3".into()));
            }),
        ));
    }

    thread::sleep(deadline);
    stop.store(true, Ordering::Relaxed);

    // The helpers end within a few seconds, or something is stuck
    let (done_tx, done_rx) = mpsc::channel();
    // The first two threads are the drawing threads; the rest (main loop, window, USB) must end by
    // themselves now that `stop` is set. They are joined on another thread so that a stuck one shows
    // up as a timeout here instead of hanging the test run.
    let helpers: Vec<_> = threads.drain(2..).collect();
    let drawing: Vec<_> = threads;
    {
        let done_tx = done_tx.clone();
        thread::spawn(move || {
            for (name, t) in helpers {
                t.join().unwrap_or_else(|_| panic!("the {name} thread panicked"));
            }
            done_tx.send(()).unwrap();
        });
    }
    done_rx.recv_timeout(Duration::from_secs(20)).expect("a helper thread is stuck (deadlock?)");

    // With the panels back, a few frames make both sides find a panel again
    let start = Instant::now();
    while panels.port_names().iter().any(Option::is_none) && start.elapsed() < Duration::from_secs(30) {
        for q in &queues {
            q.put(Item::Frame(Grid::new(), None));
        }
        thread::sleep(Duration::from_millis(50));
    }
    let names = panels.port_names();
    assert!(names.iter().all(Option::is_some), "both sides found a panel again: {names:?}");

    for q in &queues {
        q.put(Item::Stop);
    }
    // Same for the drawing threads: Stop was queued, so they must end within the deadline.
    thread::spawn(move || {
        for (name, t) in drawing {
            t.join().unwrap_or_else(|_| panic!("the {name} thread panicked"));
        }
        done_tx.send(()).unwrap();
    });
    done_rx.recv_timeout(Duration::from_secs(20)).expect("a drawing thread is stuck (deadlock?)");

    let violations = world.violations.lock().unwrap();
    assert!(violations.is_empty(), "{} problems, first: {:?}", violations.len(), violations.first());
    let (m, w) = (world.monitor_writes.load(Ordering::Relaxed), world.window_writes.load(Ordering::Relaxed));
    eprintln!(
        "stress: {} main-loop passes, {m} drawing-thread writes, {w} window writes in {:.1} s",
        frames.load(Ordering::Relaxed),
        deadline.as_secs_f64()
    );
    // Only a check that both sides did real work; how much depends on the machine's speed
    assert!(m > 20 && w > 20, "both sides got through: {m} drawing writes, {w} window writes");
}
