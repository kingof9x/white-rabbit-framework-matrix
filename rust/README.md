# White Rabbit in Rust

The whole Windows app, in Rust: the always-running monitor that owns the LED panels, the RGB keyboard
(while the window is closed) and the tray icon, and the settings window. The monitor uses about 6 MB of
memory, against about 66 MB for the old Python version (see [REVIEW.md](../REVIEW.md)). The Python version
in `led_mon/` and `white_rabbit/` is not shipped: it is the reference the Rust drawing code is checked
against, and still runs on Linux in a console.

How the pieces fit together (the two programs, the control channel, sharing the panels and the keyboard,
files on disk, testing) is in [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md).

- `wr-core`: everything that isn't Windows-specific. The apps' drawing, the layout file and which app
  shows when, settings and the log file. `tests/golden.rs` draws every case in
  `scripts/golden_frames.py` and compares it with the pictures Python saved in `tests/golden/`;
  every pixel must match.
- `wr-panel`: talking to the panels. The bytes of each command, finding which serial port is the left
  or right panel, and each panel's drawing thread.
- `wr-music`: the music visualizer.
- `wr-keyboard`: the Framework 16 RGB keyboard and macropad (`f16custom` firmware): the raw HID protocol,
  the key layouts, the built-in and app-drawn effects, the lighting engine, the keyboard picture, and a
  pretend keyboard for tests and `--demo`.
- `wr-monitor`: the Windows program, `WhiteRabbit.exe` in the installed app. It opens the window,
  `WhiteRabbitWindow.exe` next to it (`wr-window`), on a plain start, from the tray's
  "Open White Rabbit..." and when the window's options (`--window`, `--diagnose`, ...) are given;
  `--background` (Start with Windows) runs the monitor only. Finds the panels, draws the
  layout's apps, has a tray menu, turns the panels off with the lid, has the
  Ctrl+Alt+Shift+I / N / F / U / M shortcuts, and drives the keyboard's app-drawn effects and audio show
  while the window is closed. It answers the window on the control channel (a named pipe with the
  wire format of Python's `multiprocessing.connection`, kept from when the window was Python), so the
  window can change settings, show previews and drive the panels through it.

- `wr-window`: the settings window, `WhiteRabbitWindow.exe`, built with [Slint](https://slint.dev)
  (its pages are in `ui/` and `src/pages/`). It talks to the monitor on the control channel and, for
  previews, games and drawings, borrows the panels from it. `--shoot OUT.png PAGE` draws a page into
  a picture without Windows (for screenshots and checks on Linux); `WHITE_RABBIT_FAKE_PANELS=COM4,COM5`
  and `WHITE_RABBIT_FAKE_KEYBOARD=1` pretend the hardware is there.

```
cd rust
cargo test                                        # more than 320 tests; drawing checked against the golden frames
cargo build --release -p wr-monitor -p wr-window  # WhiteRabbit.exe and WhiteRabbitWindow.exe
```

The icon and file details of both exes come from `res/WhiteRabbit.ico` and `windows_resources.rs`
(used by each crate's `build.rs`). The Windows build (`.github/workflows/windows.yml`) puts the two
exes, the firmware and PresentMon together and makes the installer and the portable zip.

On Linux the window and `wr-keyboard` need `libudev-dev`, `libfontconfig1-dev` and `libxkbcommon-dev`.

The pixel patterns come from `led_mon/patterns.py`: after changing one, run
`python scripts/export_patterns.py` to update `wr-core/src/patterns_data.rs`.
