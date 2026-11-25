# How White Rabbit works

This page is for people reading or reviewing the code. It explains how the
pieces fit together, where each job lives, and how the code is tested. The
[README](../README.md) describes what the app does for someone using it.

Review findings, the stress-test results and the memory measurements are in
[REVIEW.md](../REVIEW.md).

## In one paragraph

White Rabbit is a Windows app for the Framework Laptop 16's LED Matrix panels
and its RGB keyboard and macropad. It is two Rust programs. `WhiteRabbit.exe`
(the **monitor**) runs all the time in the system tray: it owns the LED panels,
draws the system monitor onto them about ten times a second, and drives the
keyboard while the window is closed. `WhiteRabbitWindow.exe` (the **window**) is
the settings window. It only runs while it is open, and it never opens a panel
itself: it asks the monitor over a private local channel. Nothing is written in
Python any more; the Python code still in the repository is the older version,
kept as the reference the Rust code is tested against.

## The two programs

```
              ┌──────────────────────────── WhiteRabbit.exe (wr-monitor) ─────────────────────────────┐
              │                                                                                        │
 Windows ───► │  sensors / counters / thermal / weather / media / fps  (slow ones on their own threads) │
 readings     │                       │                                                                │
              │                       ▼                                                                │
              │  main loop (monitor.rs, 10x a second): layout → pick apps → draw 9x34 frames           │
              │                       │                                                                │
              │                       ▼  one "latest frame" queue per panel                            │
              │  drawing thread per panel ── effects overlay ── wr-panel Hub ──► COM port ──► panel     │
              │                                                    ▲                                    │
              │  tray window (ui.rs): tray menu, hotkeys, lid / sleep / screen messages                 │
              │  keyboard job (keyboard.rs): app-drawn effects + audio show while the window is closed  │
              │  control channel (control/): named pipe, one JSON request per connection ◄──────┐      │
              └──────────────────────────────────────────────────────────────────────────────────│──────┘
                                                                                                 │
              ┌──────────────────────────── WhiteRabbitWindow.exe (wr-window) ───────────────────┴──────┐
              │  Slint UI (ui/*.slint) + page logic (src/pages, src/keyboard)                           │
              │  previews drawn with the same wr-core / wr-music code the monitor uses                  │
              │  borrows a panel ("hold"), sends it raw commands ("serial"), gives it back ("release")  │
              │  holds the keyboard while open (its own wr-keyboard engine), gives it back on close     │
              └──────────────────────────────────────────────────────────────────────────────────────────┘
```

### How they start

- A plain start (Start menu, double-click) runs the monitor and opens the
  window. If the monitor is already running, the second copy only opens the
  window and exits (`wr-monitor/src/launch.rs`, `app.rs`).
- `WhiteRabbit.exe --background` is what Windows runs at sign-in (the
  `WhiteRabbit` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`).
  It runs the monitor only.
- Options meant for the window (`--window`, `--tab`, `--diagnose`,
  `--list-apps`, ...) are passed on to `WhiteRabbitWindow.exe`.
- One copy of each: the monitor and the window each hold a named mutex.

### The control channel

The window and the monitor talk over a named pipe,
`\\.\pipe\WhiteRabbit-<user name>` (`wr-monitor/src/control/`).

- Only the current user can open it, and only from this PC.
- Both sides prove they know a random key the monitor writes to
  `control.key` when it starts. The wire format and handshake are the ones
  Python's `multiprocessing.connection` uses, because the window used to be
  Python and the protocol was kept (`control/wire.rs`).
- One JSON request, one JSON answer (`{"ok": true, ...}` or
  `{"ok": false, "error": "..."}`) per connection. Requests are capped at 16 MB.
- Requests include reading and changing settings, previews, `hold` /
  `release` / `serial` / `serial_many` for the panels (`control/bridge.rs`),
  and `keyboard` with `hold: true/false` for the RGB keyboard.

### Sharing the panels

Windows lets only one program open a COM port. The monitor keeps both panels'
ports open in a shared **hub** (`wr-panel/src/hub.rs`); its drawing threads and
the window's requests write through the same handle and take turns. When the
window plays a game, a pattern or a preview on a panel it first *holds* that
panel: the monitor's frames for it are dropped until the window *releases* it.
If the window's process dies while holding a panel, the monitor notices and
takes the panel back.

Panels are found by USB location first (left or right slot), then by USB
vendor and product ID (`32AC:0020`), and are found again if they move or are
re-plugged (`wr-panel/src/ports.rs`, `panel.rs`).

### Drawing a frame

1. `wr-monitor/src/monitor.rs` runs ten times a second. It reads the layout
   (`config-local.yaml`): each quarter of the two panels has a list of apps that
   take turns (`wr-core/src/layout.rs`).
2. `wr-core/src/render.rs` and `wr-core/src/apps/` draw each app into a 9 x 34
   grid of brightness values. Readings (CPU, memory, battery, disks, network,
   GPU, temperatures, weather, now playing, FPS) come from `wr-monitor`; the
   slow ones are read on their own threads so the loop never waits
   (`background.rs`).
3. The frame goes into the panel's queue, which keeps only the newest frame
   (`wr-core/src/queue.rs`), so a slow panel never falls behind.
4. The panel's drawing thread passes the frame through the effects (plug
   animations, app transitions, the text line; `wr-core/src/effects.rs`,
   `plug_looks.rs`) and sends it to the panel (`wr-panel/src/protocol.rs`).

Audio reactivity and the Audio visualizer app run their own thread per panel
that draws every 30 ms (`wr-monitor/src/music.rs`, crate `wr-music`).

### The keyboard and macropad

`wr-keyboard` talks to the Framework 16 RGB keyboard and RGB macropad over raw
HID (32-byte reports on the VIA interface, usage page `0xFF60`), using the
`f16custom` firmware from
[framework-16-keyboard-customization](https://github.com/kingof9x/framework-16-keyboard-customization).
Most lighting effects run on the devices themselves. Effects drawn by the
app and the audio show need the PC to send frames: the window does that while
it is open, and the monitor's keyboard job (`wr-monitor/src/keyboard.rs`) takes
over when it closes. Key mapping changes are saved on the device (VIA dynamic
keymap), so they work on any computer.

## The crates

All Rust code is in [`rust/`](../rust), one Cargo workspace.

| Crate | What it is | Platform |
|---|---|---|
| `wr-core` | Drawing every monitor app, the effects and animations, the layout file, settings, the log. No Windows code. | any |
| `wr-panel` | The LED panel serial protocol, finding the panels, the shared port hub, dimming by LED current, the drawing threads. | any (Windows port list in `winports.rs`) |
| `wr-music` | The audio visualizer: 22 looks, the frequency analysis, sound capture (WASAPI loopback or a microphone). | any (capture is Windows) |
| `wr-keyboard` | RGB keyboard and macropad: HID protocol, layouts, the 44 built-in effects copied for the preview, app-drawn effects, the lighting engine, a pretend device for tests and `--demo`. | any (HID via hidapi) |
| `wr-monitor` | `WhiteRabbit.exe`: the main loop, Windows readings, tray, hotkeys, power events, plug events, control channel. | Windows |
| `wr-window` | `WhiteRabbitWindow.exe`: every page of the window, built with [Slint](https://slint.dev). `--shoot` draws a page to a PNG without a screen. | Windows (runs on Linux for screenshots and tests) |

Rough size: about 46,000 lines of Rust and 4,500 lines of Slint.

## Files on disk

| Where | What |
|---|---|
| `%APPDATA%\WhiteRabbit\settings.json` | Window and monitor settings (brightness, lid, shortcuts, music, plug animations, panel names, ...). Each program only changes the keys it owns. |
| `%APPDATA%\WhiteRabbit\config-local.yaml` | The panel layout (which apps, in which quarter, for how long). |
| `%APPDATA%\WhiteRabbit\keyboard.txt` | Keyboard and macropad settings. Copied once from the old Keyboard Control app's `settings.txt`, which is left alone. |
| `%APPDATA%\WhiteRabbit\drawings\*.json` | Drawings from the Draw page. |
| `%APPDATA%\WhiteRabbit\.env` | Optional `OPENWEATHER_API_KEY` for the weather app. |
| `%LOCALAPPDATA%\WhiteRabbit\WhiteRabbit-monitor.log`, `WhiteRabbit-window.log` | Logs of the monitor and the window. When one passes 1 MB it is renamed to `.log.old` (one old copy is kept) and a new one starts. |
| `control.key` (in the log folder) | The control channel's key, new on every monitor start. |

## Network use

White Rabbit only goes on the internet for these. Nothing else is sent, and
there is no telemetry:

- **Weather app** (only while it is in the layout, every 30 seconds): the
  weather from Open-Meteo, or OpenWeather if an API key is set. The location
  comes from Windows Location, a typed zip code (Open-Meteo's geocoding) or
  coordinates, or else the internet address (ipapi.co, ipinfo.io or ipwho.is).
- **Check for updates** (only when clicked): the GitHub Releases API for this
  repository, the firmware repository and the keyboard firmware repository.
- **Firmware and app downloads** (only when the person clicks to install).

Temperatures from Framework Control are read from its local service on
`127.0.0.1`, which never leaves the PC.

## How it is tested

- **Golden frames.** The Rust drawing code was ported from the Python version.
  The Python code saved what every app, effect, animation and music look draws
  for many fixed inputs (`scripts/golden_*.py` → `tests/golden/*.json`). The
  Rust tests (`wr-core/tests/golden*.rs`, `wr-music/tests/golden.rs`) draw the
  same cases and must match every pixel. To make that possible the Rust code
  reproduces numpy's and Python's random number generators and rounding
  exactly (`nprand.rs`, `pyrand.rs`, `pyround.rs`, `npmath.rs`).
- **Unit and end-to-end tests** in each crate (more than 320 Rust tests), including
  the whole drawing path with made-up panels (`wr-monitor/src/e2e_test.rs`), the
  control channel handshake, panel discovery, and a stress test that drives a
  copy of the keyboard firmware's key logic through thousands of random steps
  (`wr-keyboard/src/fn_lock_stress.rs`).
- **Python tests** (`tests/`, about 490) still run in CI for the reference
  version and the scripts.
- **CI** (`.github/workflows/`): `rust.yml` runs formatting, tests and clippy on
  Windows and Linux. `windows.yml` builds the exes, runs smoke tests on a real
  Windows runner (`--list-apps`, `--diagnose`, and the window opening every page
  and every theme), fails if any Python file would be shipped, and builds the
  installer and portable zip. Every pull request gets a test build.
- **Not covered by CI:** anything that needs the real hardware (the LED panels,
  the keyboard, the lid, plugging things in). Those are tested by hand on a
  Framework Laptop 16.

## Building

```powershell
cd rust
cargo test                                          # all tests, including the golden frames
cargo build --release -p wr-monitor -p wr-window    # WhiteRabbit.exe and WhiteRabbitWindow.exe
```

On Linux, the window and the keyboard crate need `libudev-dev` and the usual
Slint libraries (`libfontconfig1-dev`, `libxkbcommon-dev`). The installer
(`installer/white_rabbit.iss`, Inno Setup) and the bundled firmware and
PresentMon are put together by `windows.yml`; see there for the exact steps.

## Known limits

- The exes are not code-signed, so Windows SmartScreen warns on first run.
- Release builds use `panic = "abort"` (smaller exes): a panic on any thread is
  written to the log and then ends that program, rather than carrying on in a
  broken state.
- The monitor is Windows only. The Python version in `led_mon/` still runs on
  Linux in a console, but it is not maintained as a product.
- The small visualizer under the *Now playing* app's cover is not fed yet (the
  monitor doesn't capture sound for that app), so it stays dark.
- The FPS counter needs the account to be in *Performance Log Users*, and it
  counts frames presented to the screen, which can differ from a game's own
  counter.
- Temperatures and fan speeds depend on a third-party source (Framework
  Control, Libre Hardware Monitor or `framework_tool`).
- Screenshots in the docs are drawn by the window on Linux with pretend panels
  and a pretend keyboard, so the font differs from Windows and the panel
  previews are empty.

## Where it comes from

See *Where it comes from* in the [README](../README.md#where-it-comes-from) for
the projects White Rabbit is built on, and [LICENSES/](../LICENSES) for their
licences.
