# Code review notes

For anyone reviewing White Rabbit's code. This file covers a full review of the Rust app done in
November 2026: what was checked, what was fixed, what was stress tested, the open points, and the
memory comparison with the old Python version. How the app fits together (processes, crates, the
control channel, files on disk, network use) is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## What the app is, in one paragraph

Two Windows programs written in Rust. `WhiteRabbit.exe` (crate `wr-monitor`) runs all the time in the
tray: it draws system readings, clocks, animations and an audio visualizer on the Framework 16's two
LED Matrix modules over USB serial, and drives the RGB keyboard and macropad while the window is
closed. `WhiteRabbitWindow.exe` (crate `wr-window`, built with Slint) is the settings window; it
talks to the monitor over a named pipe and borrows panels from it. The shared crates are `wr-core`
(drawing, layout, settings; no I/O, no threads), `wr-panel` (serial protocol, finding panels,
drawing threads), `wr-music` (sound capture and the visualizer) and `wr-keyboard` (the keyboard's
HID protocol and lighting engine). The Python code in `led_mon/` and `white_rabbit/` is the earlier
version, kept as the Linux version and as the reference the Rust drawing is checked against pixel
for pixel (`rust/wr-core/tests/golden*.rs`).

About 47,000 lines of Rust and 4,500 lines of Slint markup.

## How the review was done

- Read the code that runs all the time with an eye on threads, locks, error paths and input from
  outside: the panel layer (`wr-panel`), the monitor's main loop, start-up, tray and control channel
  (`wr-monitor`), the window's link to the monitor and its panel handling (`wr-window`), and sound
  capture (`wr-music/src/capture*`). The drawing code was covered by the fuzz tests below rather than
  line by line; its output is already checked against Python's golden frames.
- Listed every `unsafe` block (106): all now carry a `SAFETY:` comment saying why they are sound.
- Listed every `unwrap`/`expect` outside tests in the always-running crates: the remaining ones are
  on locks or on invariants (a value that was just inserted, a constant table).
- Listed every float-to-`Duration` conversion, because `Duration::from_secs_f64` panics on a
  negative, NaN or too-large value. One reachable case was found and fixed (see below).
- Wrote stress, memory and fuzz tests (below) and ran them, short in CI and long by hand.
- Added explanatory comments: each crate root now describes its threads and data flow (the
  monitor's thread table in `wr-monitor/src/main.rs`, the frame path and lock order in
  `wr-panel/src/lib.rs`, the window's threading model in `wr-window/src/main.rs`), plus the panel
  wire protocol, the control commands and the main loop's steps. Old comments that still called the
  window "the Python app" were corrected.

One thing a reviewer should know up front: release builds use `panic = "abort"` (smaller exe), so a
panic on any thread ends the whole program after the panic hook writes it to the log. That is why
the review paid most attention to panics on bad input in the code that runs all the time.

## Fixed in this review

Each fix is its own commit, with a test.

| What | Effect before the fix | Where |
|------|------------------------|-------|
| The log only shrank at start-up | A monitor running for weeks could grow its log without limit. Now a log past 1 MB is renamed `<name>.old` and a new one starts, checked on every write. | `wr-core/src/log.rs` |
| Dimming recorded while the window held a panel | With "Keep every shade when the panels are dim" on, the drawing threads recorded an LED current as sent although the write was dropped. After a game or preview the panel was left at full current with full-range pixels: brighter than the brightness setting until it changed. | `wr-panel/src/dimming.rs`, `hub.rs` |
| `hold` order | The panel is now marked held before its current is reset, so a frame drawn in between can't lower it again; and the open-ports lock is no longer held during a serial write. | `wr-panel/src/hub.rs` |
| Pinned USB position matched by prefix | A side pinned to `1-3.3` also matched `1-3.30` or a device behind a hub at `1-3.3.1`. | `wr-panel/src/ports.rs` |
| Huge read timeout ended the monitor | The `serial` control command converted the client's timeout to a `Duration` before capping it; `1e300` panicked, which with `panic = abort` ends the monitor. Capped first now. (Only a local client that has the per-user key can send it.) | `wr-monitor/src/control/bridge.rs`, `wr-window/src/panels.rs` |
| Time app trusted the clock reading's length | Indexed the digits `d[0]..d[3]`; a reading of another length would panic the main loop. Today's reading is always four digits, so this is hardening found by the fuzz test. | `wr-core/src/render.rs` |
| settings.json parsed every second | The main loop re-read and parsed it once a second all day. Now it checks size and change time and parses only on a change. Same behaviour. | `wr-monitor/src/monitor.rs` |

## Open points (not changed)

None of these is known to cause a problem in normal use. They are listed so a reviewer doesn't have
to find them again.

1. **The Windows control pipe serves one client at a time with no read timeout.** A client that
   connects and then stalls without sending would block the control channel (the Unix socket used
   in tests has a 10-second timeout). Only the same Windows user can open the pipe (its ACL names the
   user's SID, remote clients are rejected) and must also know the key in `control.key`, and the
   window sends its request at once, so this needs a misbehaving local program. An overlapped read
   with a timeout would close it.
2. **The `serial` command opens whatever port name it is given**, not only LED panels. Same
   protection as above (same user, key). Restricting it to the ports `discover` finds would narrow it.
3. **`panic = "abort"`** (see above): any panic ends the program. The fuzz tests cover the inputs the
   always-running code takes; a panic elsewhere would still end the monitor and is only logged.
   Windows' Run key restarts it at the next login, not before.
4. **Background reader threads** (sensors, GPU, weather, media) are started on first use and live
   until the program ends; they stop reading after 10 seconds without being asked, but the thread
   stays. Fine for a tray app; worth knowing when reading thread counts.
5. **The window's `Link::status` holds its lock during a request** (up to 1.5 s when the monitor
   doesn't answer), so two window threads asking at once wait for each other.
6. **The window duplicates small pieces of the monitor**: `link.rs` has its own copy of the pipe
   address logic and includes the monitor's `control/wire.rs` with `#[path]`. A shared crate would
   remove the duplication.
7. **The Python reference code** (`led_mon/shared_state.py`) still matches pinned USB positions by
   prefix; it runs only on Linux and was left as it is.
8. **Each log line opens and closes the file.** Cheap at the monitor's rate of a few lines an hour,
   and it lets the user open or delete the log at any time.

## Stress testing

What was run on Linux (the code is the same; only the Windows device layer is swapped for test
doubles):

| Test | What it does | Result |
|------|--------------|--------|
| `wr-panel/tests/stress.rs` | Both drawing threads, the effects overlay, a "window" thread borrowing, writing to and releasing panels (and sometimes "crashing"), and a "USB" thread unplugging, replugging at new COM ports, failing writes and swapping sides, all at once. Fails on a deadlock, on a monitor frame reaching a held panel, or if the panels aren't found again. | 2 s in CI. Run for 120 s: 394,948 main-loop passes, 3,716 drawing writes and 19,573 window writes while panels came and went; no deadlock, no violation, both panels found again. |
| `wr-panel/tests/memory.rs` | The main loop's drawing path (rotation, drawing every app of the bundled layout, effects, sending through the drawing-thread code) with memory counted by a wrapping allocator. | 25,000 passes in CI. Run for 500,000 passes (about 14 hours of the real loop in simulated time): **0 bytes of growth** (553,423 bytes in use after warm-up and at the end). |
| `wr-core/tests/fuzz.rs` | Random and broken layout files must parse and then draw for 12 passes without panicking, with odd readings (empty lists, out-of-range values); random settings.json, effects settings, weather replies, Framework Control replies and framework_tool / GPU counter text must not panic. | 1,500 + 3,000 cases in CI. Run with 100,000 cases each: passed. Found the time-app issue above. |
| `wr-monitor` control fuzz | Garbage handshakes (must be refused without a reply), 20,000 random request bodies, broken frame lengths, and 50,000 random requests to every real command. | Passed. A direct test found the timeout issue above. |
| Existing suites | 338 Rust tests in all (unit, golden frames against Python, end-to-end main loop to panel bytes, the real Python client against the control channel, the keyboard's Fn-lock emulator). | All pass on Linux; CI runs them on Linux and Windows with `clippy -D warnings`. |

To run the long versions:

```
cd rust
WR_STRESS_SECONDS=120 cargo test -p wr-panel --test stress -- --nocapture
WR_MEMORY_PASSES=500000 cargo test -p wr-panel --test memory -- --nocapture
WR_FUZZ_CASES=100000 cargo test -p wr-core --test fuzz
```

The Python test suite (`tests/`) runs in the Windows build workflow; it was not re-run for this
review.

### What could not be tested without Windows and the hardware

- Real serial I/O to the LED panels, real unplugging, sleep/resume and the lid (the logic is tested
  with test doubles; the Win32 calls themselves are not).
- WASAPI sound capture, WMI/PDH sensors, the tray icon, hotkeys, power notifications, Windows
  Location and the named pipe's ACL.
- The keyboard and macropad over real HID (a simulated device is tested).
- Memory and CPU of the real exes over days of use (see below for what was measured).

## Memory: Rust compared with the old Python version

| Version | Process | Memory |
|---------|---------|--------|
| Python (PyInstaller), 2026-11-08 | monitor | 65.6 MB |
| | PyInstaller launcher process | 0.6 MB |
| First Rust monitor, 2026-11-15 | `WhiteRabbit.exe` | 8 MB |
| Current Rust monitor, 2026-11-25 | `WhiteRabbit.exe` | about 6 MB |

So the always-running part went from about 66 MB to about 6 MB, roughly **11 times less**.

How these were measured: by Callum on his Framework 16, in Windows Task Manager, Details tab, the
"Memory" column (Windows' private working set: memory only this process uses, as Task Manager
counts it), with the settings window closed. The Python figure was the same with the audio
visualizer on and off; its CPU use was 0.2 to 1.4 %.

Caveats, stated plainly:
- One machine, one reading of each version, not an average over time.
- The settings window (`WhiteRabbitWindow.exe`) is a separate process that only runs while it is
  open, and is not in these numbers.
- Private working set leaves out memory shared with other processes (system DLLs), so the total
  footprint of either version is larger than these figures; it is the same measure for both, so the
  comparison holds.
- The long memory test above shows the Rust monitor's drawing path doesn't grow over time; it does
  not measure the Windows-only parts (sensors, sound capture), which only a long run on Windows can.
