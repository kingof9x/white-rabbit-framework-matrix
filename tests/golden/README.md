# Golden frames

What the monitor apps and the music visualizer looks draw for fixed, made-up inputs: no real sensors,
sound or clock. They are the reference for the Rust rewrite of the monitor: the Rust code has to draw
the same pictures.

- `monitor_apps.json`: LED values (0-255) for every monitor app in the top and bottom half, the
  battery at fixed times (it flashes and pulses), clock and weather characters, temperatures, the app
  names and the borders.
- `music_looks.json`: all 22 looks, left and right panel, on one panel or across both, with default and
  changed options, every 6th of 48 frames of `looks.demo_samples` music plus a frame after silence.
  Brightness 0.0-1.0 is stored as round(value * 255) and may differ by 1.

Each frame is 34 strings, one per row from the top, of 9 two-digit hex values from the left.

Made by `python scripts/golden_frames.py`. `tests/test_golden_frames.py` checks that the Python code
still draws the same; after a deliberate change to how something draws, run the script again and
commit the new files.
