# White Rabbit monitor in Rust

The Rust rewrite of White Rabbit's monitor: the always-running part that owns the LED panels and the tray
icon. It aims to use far less memory and CPU than the Python monitor (about 66 MB). The Python monitor in
`led_mon/` stays: it is what runs on Linux, and it is the reference the Rust code is checked against.

- `wr-core`: everything that isn't Windows-specific. The apps' drawing, the layout file and which app
  shows when, settings and the log file. `tests/golden.rs` draws every case in
  `scripts/golden_frames.py` and compares it with the pictures Python saved in `tests/golden/`;
  every pixel must match.
- `wr-panel`: talking to the panels. The bytes of each command, finding which serial port is the left
  or right panel, and each panel's drawing thread.
- `wr-music`: the music visualizer (being written).
- `wr-monitor`: the Windows program, `WhiteRabbit-rust-test.exe` for now. Finds the panels, draws the
  layout's apps, has a tray menu, turns the panels off with the lid, and has the
  Ctrl+Alt+Shift+I / N / F / U shortcuts. It uses the same settings, layout file and single-instance lock
  as the Python app, so only one of them runs at a time.

So far it draws the cpu, cpu-graph, mem-bat and time apps; other apps stay dark until they are ported.

```
cd rust
cargo test                       # drawing checked against the golden frames
cargo build --release -p wr-monitor
```

The pixel patterns come from `led_mon/patterns.py`: after changing one, run
`python scripts/export_patterns.py` to update `wr-core/src/patterns_data.rs`.
