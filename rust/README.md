# White Rabbit in Rust

The whole Windows app, in Rust: the always-running monitor that owns the LED panels and the tray icon,
and the settings window. It uses far less memory than the Python version (about 8 MB for the monitor,
against 66 MB). The Python version in `led_mon/` and `white_rabbit/` stays: it is what runs on Linux,
and it is the reference the Rust code is checked against.

- `wr-core`: everything that isn't Windows-specific. The apps' drawing, the layout file and which app
  shows when, settings and the log file. `tests/golden.rs` draws every case in
  `scripts/golden_frames.py` and compares it with the pictures Python saved in `tests/golden/`;
  every pixel must match.
- `wr-panel`: talking to the panels. The bytes of each command, finding which serial port is the left
  or right panel, and each panel's drawing thread.
- `wr-music`: the music visualizer.
- `wr-monitor`: the Windows program, `WhiteRabbit.exe` in the installed app. It opens the window,
  `WhiteRabbitWindow.exe` next to it (`wr-window`), on a plain start, from the tray's
  "Open White Rabbit..." and when the window's options (`--window`, `--diagnose`, ...) are given;
  `--background` (Start with Windows) runs the monitor only. Finds the panels, draws the
  layout's apps, has a tray menu, turns the panels off with the lid, and has the
  Ctrl+Alt+Shift+I / N / F / U shortcuts. It uses the same settings, layout file and single-instance lock
  as the Python app, so only one of them runs at a time. It answers the White Rabbit window on the same
  control channel as the Python monitor (`led_mon/control.py`), so the window can change settings,
  show previews and drive the panels through it.

- `wr-window`: the settings window, `WhiteRabbitWindow.exe`, built with [Slint](https://slint.dev)
  (its pages are in `ui/` and `src/pages/`). It talks to the monitor on the control channel and, for
  previews, games and drawings, borrows the panels from it. `--shoot OUT.png PAGE` draws a page into
  a picture without Windows (for screenshots and checks on Linux).

```
cd rust
cargo test                                        # drawing checked against the golden frames
cargo build --release -p wr-monitor -p wr-window  # WhiteRabbit.exe and WhiteRabbitWindow.exe
```

The icon and file details of both exes come from `res/WhiteRabbit.ico` and `windows_resources.rs`
(used by each crate's `build.rs`). The Windows build (`.github/workflows/windows.yml`) puts the two
exes, the firmware and PresentMon together and makes the installer and the portable zip.

The pixel patterns come from `led_mon/patterns.py`: after changing one, run
`python scripts/export_patterns.py` to update `wr-core/src/patterns_data.rs`.
