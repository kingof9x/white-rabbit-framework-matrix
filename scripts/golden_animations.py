# Golden frames for the panel animations, the old-TV effect and the sand battery.
#
# The Rust rewrite (rust/wr-core/src/animations.rs and sand.rs) has to draw exactly the same pictures
# as led_mon/animations.py and led_mon/sand.py. This script runs them with fixed seeds and a fixed
# clock and saves what they draw in tests/golden/animations.json. tests/test_golden_animations.py
# checks that the Python code still draws the same, and rust/wr-core/tests/golden_animations.rs
# checks the Rust code against the same file.
#
#   python scripts/golden_animations.py          write tests/golden/animations.json again
#   python scripts/golden_animations.py --check  only compare, and list the frames that differ
#
# File format (JSON), as in scripts/golden_frames.py: {"frames": {"<case name>": ["<row 0>", ...,
# "<row 33>"]}}, each row 9 two-digit hex LED values (0-255), left column first. The looks are saved
# as to_leds(frame, 255). Some look frames are also saved exactly, under "exact": the same 34 rows,
# each of 9 sixteen-digit hex numbers, the bits of the float64 brightness.
#
# numpy works out x ** y with its own AVX-512 code on computers that have it, which can differ from
# the C library's pow() in the last binary digit. Rust uses the C library's pow(), so this script
# turns numpy's AVX-512 code off (before numpy is loaded) to get the same numbers on every computer.

# Built In Dependencies
import os
import sys
import json
import struct
import argparse

os.environ["NPY_DISABLE_CPU_FEATURES"] = " ".join((
    os.environ.get("NPY_DISABLE_CPU_FEATURES", ""), "AVX512F AVX512CD AVX512_SKX AVX512_CLX AVX512_CNL",
    "AVX512_ICL AVX512_SPR X86_V4")).strip()

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN_FILE = os.path.join(ROOT, "tests", "golden", "animations.json")

if ROOT not in sys.path:
    sys.path.insert(0, ROOT)

# External Dependencies
import numpy as np

from led_mon import animations, sand

WIDTH, HEIGHT = 9, 34
FOREGROUND = 160

# How far each look frame moves on, in seconds: steady frames, short and long gaps, a pause of nothing,
# and pauses longer than the one second a look is allowed to catch up
LOOK_DTS = [0.04, 0.04, 0.04, 0.02, 0.02, 0.1, 0.0, 0.5, 1.0, 3.0, 1.0, 0.07, 1.0, 0.04, 1.0]
LOOK_SEED = 7
EXACT_FRAMES = (4, 9, 14)
# Looks that use random numbers, run again with another seed at a steady 10 frames a second
RANDOM_LOOKS = ("fire", "snow", "ball")
SECOND_SEED = 2024


def encode(grid):
    """A 9 x 34 grid of 0-255 values as 34 row strings."""
    grid = np.asarray(grid)
    assert grid.shape == (WIDTH, HEIGHT), grid.shape
    values = np.clip(np.rint(grid), 0, 255).astype(int)
    return ["".join("%02x" % values[x, y] for x in range(WIDTH)) for y in range(HEIGHT)]


def decode(rows):
    return np.array([[int(row[2 * x:2 * x + 2], 16) for row in rows] for x in range(WIDTH)])


def encode_exact(frame):
    """A 9 x 34 float frame as 34 row strings of the float64 bits, 16 hex digits a value."""
    frame = np.asarray(frame, dtype=float)
    assert frame.shape == (WIDTH, HEIGHT), frame.shape
    return ["".join("%016x" % struct.unpack("<Q", struct.pack("<d", frame[x, y]))[0] for x in range(WIDTH))
            for y in range(HEIGHT)]


def decode_exact(rows):
    return np.array([[struct.unpack("<d", struct.pack("<Q", int(row[16 * x:16 * x + 16], 16)))[0]
                      for row in rows] for x in range(WIDTH)])


# Python's own check of the exact frames allows a difference in the last few bits: numpy uses its own
# sin, cos and power on some computers (Windows, AVX-512), which can differ from the C library's there.
# The LED frames must still match exactly, and the Rust test compares the exact frames bit for bit.
EXACT_TOLERANCE = 1e-12


def same_exact(now, rows):
    return np.allclose(np.asarray(now, dtype=float), decode_exact(rows), rtol=0, atol=EXACT_TOLERANCE)


# The inputs, made the same way in the Rust test ------------------------------------------------------

def tv_picture():
    """A picture for the TV effect: bright and dim values, a few below zero, and dark rows."""
    g = np.array([[((x * 37 + y * 11) % 230) - 10 for y in range(HEIGHT)] for x in range(WIDTH)])
    g[:, 30:] = 0
    return g


def tv_dot():
    g = np.zeros((WIDTH, HEIGHT), dtype=int)
    g[3, 5] = 90
    return g


def leds_frame():
    """Brightness for to_leds: below zero, above one, around the 0.02 cut-off and at exact halves."""
    f = np.array([[((x * 34 + y) * 0.0137) % 1.3 - 0.1 for y in range(HEIGHT)] for x in range(WIDTH)])
    f[0, :6] = [0.02, 0.0200001, 0.019999, 2.5 / 160, 4.5 / 160, 5.5 / 255]
    f[1, :4] = [0.5 / 160, 1.0, 1.5 / 160, 255.5 / 255]
    return f


def sand_base():
    """What is already on the panel under the sand in one scenario, to check the sand keeps the brighter."""
    g = np.zeros((WIDTH, HEIGHT), dtype=int)
    for x in range(WIDTH):
        for y in range(HEIGHT):
            if (x + y) % 5 == 0:
                g[x, y] = 200 if y % 2 else 60
    return g


# (name, y, seed, speed, key, frames: [(battery, now)], draw on sand_base)
def _run(first, then, n=16, start=1000.0, step=0.1):
    return [(first, start)] + [(then, start + i * step) for i in range(1, n)]


SAND_SCENARIOS = [
    ("charge-top", 0, 1, "normal", "left", _run((0.2, True), (0.6, True)), False),
    ("drain-bottom", 16, 2, "normal", "left", _run((0.8, False), (0.3, False)), False),
    ("fast-charge-bottom", 16, 3, "fast", "right", _run((0.0, True), (1.0, True)), False),
    ("slow-drain-top", 0, 4, "slow", "right", _run((1.0, False), (0.5, False)), False),
    ("low-flash-top", 0, 5, "normal", "left", _run((0.05, False), (0.05, False), start=1000.3, step=0.25), False),
    ("low-charging-bottom", 16, 6, "normal", "left", _run((0.05, True), (0.07, True), step=0.37), False),
    ("nearly-full-top", 0, 8, "normal", "left",
     [((0.999, True), 1000.0 + i * 0.1) for i in range(4)] + [((0.99, True), 1000.4 + i * 0.1) for i in range(8)],
     False),
    ("no-battery-then-half-bottom", 16, 9, "normal", "left",
     [(None, 1000.0), (None, 1000.1)] + _run((0.5, False), (0.45, False), n=10, start=1000.2), False),
    ("unknown-speed-top", 0, 10, "warp", "left", _run((0.3, True), (0.9, True)), False),
    ("on-a-picture-bottom", 16, 11, "fast", "left", _run((0.4, True), (0.1, False)), True),
]


# The cases ----------------------------------------------------------------------------------------

def look_cases():
    frames, exact = {}, {}
    for key in animations.LOOK_KEYS + ("unknown",):
        look = animations.make_look(key, LOOK_SEED)
        for i, dt in enumerate(LOOK_DTS):
            frame = look.frame(dt)
            name = f"look-{key}-{i:02d}"
            frames[name] = animations.to_leds(frame, 255)
            if i in EXACT_FRAMES and key != "unknown":
                exact[name] = np.array(frame, dtype=float)
    for key in RANDOM_LOOKS:
        look = animations.make_look(key, SECOND_SEED)
        for i in range(15):
            frame = look.frame(0.1)
            frames[f"look-{key}-seed{SECOND_SEED}-{i:02d}"] = animations.to_leds(frame, 255)
        exact[f"look-{key}-seed{SECOND_SEED}-14"] = np.array(frame, dtype=float)
    # spread_heat on its own, as the Bonfire music look calls it (default cooling)
    heat = np.random.default_rng(5).random((WIDTH, HEIGHT + 2))
    animations.spread_heat(heat, np.random.default_rng(9), 1.7)
    exact["spread-heat-k1.7-top"] = heat[:, :HEIGHT].copy()
    exact["spread-heat-k1.7-bottom"] = heat[:, 2:].copy()
    return frames, exact


def led_cases():
    frame = leds_frame()
    return {f"to-leds-fg{fg}": animations.to_leds(frame, fg) for fg in (160, 255, 1, 0, -5, 300)}


def tv_cases():
    cases = {}

    def add(name, frames):
        for i, f in enumerate(frames):
            cases[f"{name}-{i:02d}"] = f

    picture = tv_picture()
    add("tv-off-picture-fg160", animations.tv_off_frames(picture, FOREGROUND))
    add("tv-on-picture-fg160", animations.tv_on_frames(picture, FOREGROUND))
    add("tv-off-picture-fg300", animations.tv_off_frames(picture, 300))
    add("tv-on-picture-fg0", animations.tv_on_frames(picture, 0))
    add("tv-off-dot-fg160", animations.tv_off_frames(tv_dot(), FOREGROUND))
    add("tv-on-dot-fg160", animations.tv_on_frames(tv_dot(), FOREGROUND))
    add("tv-off-none-fg160", animations.tv_off_frames(None, FOREGROUND))
    add("tv-on-none-fg160", animations.tv_on_frames(None, FOREGROUND))
    add("tv-off-dark-fg160", animations.tv_off_frames(np.zeros((WIDTH, HEIGHT), dtype=int), FOREGROUND))
    add("tv-on-dark-fg160", animations.tv_on_frames(np.zeros((WIDTH, HEIGHT), dtype=int), FOREGROUND))
    return cases


def sand_cases():
    cases = {}
    for name, y, seed, speed, key, steps, on_base in SAND_SCENARIOS:
        sand._piles.clear()
        sand.pile_for((key, y), 15 if y == 0 else 16, seed=seed)
        for i, (battery, now) in enumerate(steps):
            grid = sand_base() if on_base else np.zeros((WIDTH, HEIGHT), dtype=int)
            sand.draw_sand_battery(grid, battery, FOREGROUND, y, key=key, speed=speed, now=now)
            cases[f"sand-{name}-{i:02d}"] = grid
    sand._piles.clear()
    return cases


def all_cases():
    frames, exact = look_cases()
    frames.update(led_cases())
    frames.update(tv_cases())
    frames.update(sand_cases())
    return frames, exact


# Saving and checking --------------------------------------------------------------------------------

def save(frames, exact):
    data = {"description": "LED values (0-255) the panel animations (as to_leds(frame, 255)), the old-TV "
                           "effect and the sand battery draw with fixed seeds and a fixed clock, and some "
                           "look frames exactly (float64 bits). Made by scripts/golden_animations.py.",
            "frames": {name: encode(g) for name, g in frames.items()},
            "exact": {name: encode_exact(f) for name, f in exact.items()}}
    with open(GOLDEN_FILE, "w", newline="\n") as f:
        json.dump(data, f, indent=0, sort_keys=True)
        f.write("\n")


def differences(frames, exact):
    """Names of frames that differ from the saved ones, are missing or are no longer drawn."""
    with open(GOLDEN_FILE) as f:
        saved = json.load(f)
    problems = []
    same = {"frames": lambda now, rows: encode(now) == rows, "exact": same_exact}
    for section, now in (("frames", frames), ("exact", exact)):
        old = saved[section]
        problems += [f"missing from animations.json: {n}" for n in sorted(set(now) - set(old))]
        problems += [f"no longer drawn: {n}" for n in sorted(set(old) - set(now))]
        problems += [f"differs: {n}" for n in sorted(set(now) & set(old)) if not same[section](now[n], old[n])]
    return problems


def main():
    parser = argparse.ArgumentParser(description="Write or check tests/golden/animations.json.")
    parser.add_argument("--check", action="store_true", help="only compare with the saved frames")
    args = parser.parse_args()
    frames, exact = all_cases()
    if args.check:
        problems = differences(frames, exact)
        print("\n".join(problems) if problems else "All animation golden frames match.")
        return 1 if problems else 0
    save(frames, exact)
    print(f"Saved {len(frames)} frames and {len(exact)} exact frames in {GOLDEN_FILE}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
