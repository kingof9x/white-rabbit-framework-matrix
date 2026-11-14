# Writes the monitor's pixel patterns (digits, symbols, weather icons, app names, the CPU spirals and the
# battery's lightning bolt) from led_mon/patterns.py and the plugins into rust/wr-core/src/patterns_data.rs,
# so the Rust monitor draws with exactly the same pictures. Run it again after changing a pattern:
#
#   python scripts/export_patterns.py
#
# tests/test_export_patterns.py fails when the Rust file is out of date.

# Built In Dependencies
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_FILE = os.path.join(ROOT, "rust", "wr-core", "src", "patterns_data.rs")
if ROOT not in sys.path:
    sys.path.insert(0, ROOT)

# External Dependencies
import numpy as np


def load_tables():
    """{table name: {key: 2-D 0/1 array}}, with the plugins' app names added to id_patterns."""
    saved = sys.argv
    sys.argv = [saved[0], "--disable-plugins"]
    try:
        from led_mon import drawing  # its lookup_table is the spirals turned the way the app draws them
        from led_mon import patterns as p
    finally:
        sys.argv = saved
    from led_mon.plugins import temp_fan_plugin
    ids = dict(p.id_patterns)
    ids.update(temp_fan_plugin.id_patterns)
    try:
        # The clock, weather and audio visualizer plugins need the weather and audio libraries
        from led_mon.plugins import time_weather_plugin, equalizer_plugin
        ids.update(time_weather_plugin.id_patterns)
        ids.update(equalizer_plugin.id_patterns)
    except Exception as e:  # pragma: no cover - only without those libraries
        raise SystemExit(f"Install requirements.txt first: {e}")
    return {
        "NUMERALS": p.numerals,
        "SYMBOLS": p.symbols,
        "ICONS": p.icons,
        "DIGITS_3X5": p.digits_3_x_5,
        "LETTERS_SMALL": p.letters_small,
        "LETTERS_5X6": p.letters_5_x_6,
        "ID_PATTERNS": ids,
        "SPIRALS": {str(i): drawing.lookup_table[i] for i in range(drawing.lookup_table.shape[0])},
        "LIGHTNING_BOLT": {"top": p.lightning_bolt_top, "bottom": p.lightning_bolt_bot},
    }


def rust_source(tables):
    lines = [
        "// Made by scripts/export_patterns.py from led_mon/patterns.py and the plugins. Don't edit by hand.",
        "// Each pattern keeps numpy's shape: `rows` x `cols`, row by row, 1 = lit.",
        "",
        "use crate::patterns::Pattern;",
        "",
    ]
    for table, entries in tables.items():
        lines.append(f"pub static {table}: &[(&str, Pattern)] = &[")
        for key in sorted(entries):
            a = np.asarray(entries[key])
            if a.ndim != 2:
                raise ValueError(f"{table}[{key!r}] is not 2-D: {a.shape}")
            values = np.asarray(a, dtype=float)
            if not np.isin(values, (0.0, 1.0)).all():
                raise ValueError(f"{table}[{key!r}] has values other than 0 and 1")
            bits = "".join("1" if v else "0" for v in values.astype(bool).ravel())
            lines.append(f"    ({key!r:s}, Pattern {{ rows: {a.shape[0]}, cols: {a.shape[1]}, bits: \"{bits}\" }}),"
                         .replace("'", '"'))
        lines.append("];")
        lines.append("")
    return "\n".join(lines)


def main():
    source = rust_source(load_tables())
    os.makedirs(os.path.dirname(OUT_FILE), exist_ok=True)
    with open(OUT_FILE, "w", newline="\n") as f:
        f.write(source)
    print(f"Wrote {OUT_FILE}")


if __name__ == "__main__":
    main()
