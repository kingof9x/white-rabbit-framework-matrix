# Golden frames for the effects over the panels (led_mon/effects.py): ripples, drains, the text line, slides.
#
# The Rust rewrite (rust/wr-core/src/effects.rs) has to draw exactly the same frames. The effects run on
# the clock, so each scenario here is a script of steps at fixed times ("plug left-back in at 0.05 s, draw
# the left panel at 0.1 s, ..."), played with a stand-in clock. The scripts, the input pictures and what
# came out are all saved in tests/golden/effects.json; the Rust test plays the same scripts from the file
# and compares every pixel, and tests/test_golden_effects.py checks that Python still draws the same.
#
#   python scripts/golden_effects.py          write tests/golden/effects.json again (after a deliberate change)
#   python scripts/golden_effects.py --check  only compare, and list what differs
#
# File format (JSON), on top of golden_frames.py's {"description", "frames": {name: [34 rows]}}:
#   "inputs":    {name: [34 rows]}, the pictures the scenarios draw, in the same row format
#   "scenarios": {name: [step, ...]}, each step a dict with "op" and its arguments (see play()). Times ("t")
#                are written as strings, repr() of the float: some JSON readers (serde_json by default) can
#                read a float one bit off, and that is enough to change a frame
#   "values":    {name: number or null}, what interval() said at the "interval" steps
#   "settings":  [[raw, normalized], ...], normalize() on odd settings
#   "positions": [[text, side, spot, label], ...], parse_position() and position_label()
# Every "frame" step saves the frame it drew as "<scenario>-<step number>-<side>".

# Built In Dependencies
import os
import sys
import json
import argparse

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EFFECTS_FILE = os.path.join(ROOT, "tests", "golden", "effects.json")

if ROOT not in sys.path:
    sys.path.insert(0, ROOT)

# External Dependencies
import numpy as np

from led_mon import effects  # noqa: E402

WIDTH, HEIGHT = 9, 34


def encode(grid):
    """A 9 x 34 grid of 0-255 values as 34 row strings."""
    grid = np.asarray(grid)
    assert grid.shape == (WIDTH, HEIGHT), grid.shape
    values = np.clip(np.rint(grid), 0, 255).astype(int)
    return ["".join("%02x" % values[x, y] for x in range(WIDTH)) for y in range(HEIGHT)]


def decode(rows):
    return np.array([[int(row[2 * x:2 * x + 2], 16) for row in rows] for x in range(WIDTH)])


class Clock:
    """Stands in for the time module inside led_mon.effects."""
    def __init__(self):
        self.now = 0.0

    def time(self):
        return self.now

    def monotonic(self):
        return self.now


# Input pictures ----------------------------------------------------------------------------------------

def inputs():
    pictures = {}
    g = np.zeros((WIDTH, HEIGHT), dtype=int)
    for x in range(WIDTH):
        for y in range(HEIGHT):
            if (x + 2 * y) % 5:
                g[x, y] = (x * 29 + y * 13) % 256
    pictures["pattern"] = g

    # Something like a monitor app: a border and bars
    g = np.zeros((WIDTH, HEIGHT), dtype=int)
    g[0, :] = g[-1, :] = 35
    g[:, 0] = g[:, 16] = g[:, 33] = 35
    for x in range(1, 8):
        g[x, 1 + x:16] = 160
        g[x, 17:17 + 2 * x] = 160
    pictures["app"] = g

    # Another app, to slide to
    g = np.zeros((WIDTH, HEIGHT), dtype=int)
    g[2:7, 3:30] = 90
    g[4, :] = 200
    g[:, 10] = 255
    pictures["second"] = g

    # A few lit pixels
    g = np.zeros((WIDTH, HEIGHT), dtype=int)
    for i, (x, y) in enumerate(((1, 2), (4, 4), (7, 9), (0, 20), (8, 33), (3, 27), (5, 17))):
        g[x, y] = 60 + 25 * i
    pictures["sparse"] = g

    pictures["full"] = np.full((WIDTH, HEIGHT), 255, dtype=int)
    return pictures


# Scenarios ---------------------------------------------------------------------------------------------

def frames(steps, sides, start, count, every, picture="app", new=True, keep=True):
    """Draw these sides count times, every `every` seconds from start."""
    for k in range(count):
        for side in sides:
            steps.append({"op": "frame", "side": side, "t": start + k * every, "picture": picture, "new": new,
                          "keep": keep})


def interval(steps, side, t):
    steps.append({"op": "interval", "side": side, "t": t})


def scenarios():
    s = {}
    both = ["left", "right"]

    # A ripple from the left panel's back port, on one panel, to the end
    st = [{"op": "present", "side": "left"}]
    frames(st, ["left"], 0.0, 1, 0.0)
    st.append({"op": "plug", "position": "left-back", "in": True, "t": 0.05})
    interval(st, "left", 0.05)
    frames(st, ["left"], 0.05, 25, 1 / 30)
    frames(st, ["left"], 0.9, 40, 0.07, keep=False)
    frames(st, ["left"], 3.7, 1, 0.0)
    interval(st, "left", 3.7)
    s["ripple-left-back"] = st

    # From the right panel's front port, running across both panels; some frames are redraws
    st = [{"op": "present", "side": "left"}, {"op": "present", "side": "right"}]
    st.append({"op": "plug", "position": "right-front", "in": True, "t": 0.0})
    frames(st, both, 0.0, 12, 1 / 30)
    frames(st, both, 0.4, 8, 1 / 25, picture="pattern", new=False)
    interval(st, "left", 0.7)
    s["ripple-right-front-two"] = st

    # A port we can't place: from between the panels. Then a second ripple on top
    st = [{"op": "present", "side": "left"}, {"op": "present", "side": "right"}]
    st.append({"op": "plug", "position": None, "in": True, "t": 0.0})
    frames(st, both, 0.0, 8, 1 / 30, picture="pattern")
    st.append({"op": "plug", "position": "left-middle", "in": True, "t": 0.27})
    frames(st, both, 0.27, 8, 1 / 30, picture="pattern")
    s["ripple-unknown-two"] = st

    # No panel said it is there: the left one is assumed. The right side passes through
    st = [{"op": "plug", "position": "right-middle", "in": True, "t": 0.0}]
    frames(st, ["left", "right"], 0.0, 10, 1 / 30)
    interval(st, "right", 0.34)
    s["ripple-no-panels"] = st

    # Unplugged from the right panel's middle port: drain, empty, rise
    st = [{"op": "present", "side": "right"}]
    frames(st, ["right"], 0.0, 1, 0.0)
    st.append({"op": "plug", "position": "right-middle", "in": False, "t": 0.1})
    interval(st, "right", 0.1)
    frames(st, ["right"], 0.1, 75, 1 / 30)
    interval(st, "right", 2.6)
    s["drain-right-middle"] = st

    # Unplugged from the left (the picture is flipped), plugged back in while it drains
    st = [{"op": "present", "side": "left"}, {"op": "present", "side": "right"}]
    frames(st, both, 0.0, 1, 0.0, picture="pattern")
    st.append({"op": "plug", "position": "left-back", "in": False, "t": 0.1})
    frames(st, both, 0.1, 10, 1 / 20, picture="pattern")
    st.append({"op": "plug", "position": "left_back", "in": True, "t": 0.6})
    frames(st, both, 0.6, 14, 1 / 20, picture="pattern")
    s["drain-left-replug-two"] = st

    # Few pixels, a port with no spot (the middle row), all the way through empty and rising
    st = [{"op": "present", "side": "left"}]
    frames(st, ["left"], 0.0, 1, 0.0, picture="sparse")
    st.append({"op": "plug", "position": "left", "in": False, "t": 0.0})
    frames(st, ["left"], 0.0, 80, 1 / 30, picture="sparse")
    s["drain-sparse-rise"] = st

    # Every pixel lit, slower redraws (dt is capped at 0.1)
    st = [{"op": "present", "side": "right"}]
    frames(st, ["right"], 0.0, 1, 0.0, picture="full")
    st.append({"op": "plug", "position": "Right Front", "in": False, "t": 0.0})
    frames(st, ["right"], 0.0, 14, 0.13, picture="full")
    s["drain-full"] = st

    # Plugged in again once the panel is empty: it rises at once, with a ripple
    st = [{"op": "present", "side": "right"}]
    frames(st, ["right"], 0.0, 1, 0.0, picture="sparse")
    st.append({"op": "plug", "position": "right-back", "in": False, "t": 0.0})
    frames(st, ["right"], 0.0, 30, 0.07, picture="sparse", keep=False)
    frames(st, ["right"], 2.1, 1, 0.0, picture="sparse")
    st.append({"op": "plug", "position": "right-back", "in": True, "t": 2.12})
    frames(st, ["right"], 2.12, 10, 1 / 30, picture="sparse")
    s["drain-empty-replug"] = st

    # Unplugged before the panel drew anything: nothing to drain
    st = [{"op": "present", "side": "left"}]
    st.append({"op": "plug", "position": "left-front", "in": False, "t": 0.0})
    interval(st, "left", 0.0)
    frames(st, ["left"], 0.0, 2, 0.05)
    s["drain-no-frame"] = st

    # The text line across both panels at a few speeds
    for speed in (1, 5, 10):
        st = [{"op": "configure", "settings": {"text_speed": speed}},
              {"op": "present", "side": "left"}, {"op": "present", "side": "right"},
              {"op": "say", "text": "Now playing: X"}]
        frames(st, both, 0.0, 1, 0.0)
        frames(st, both, 0.05, 34, 0.11 if speed > 1 else 0.37)
        interval(st, "right", 0.05 + 34 * (0.11 if speed > 1 else 0.37))
        s[f"text-speed-{speed}-two"] = st

    # The default speed before configure() (18 columns a second), one panel, odd characters, a queue
    st = [{"op": "present", "side": "right"}, {"op": "say", "text": "  abc ~ ß 100%  \n ok?"},
          {"op": "say", "text": "Charger: 65W"}]
    frames(st, ["right"], 0.0, 30, 0.1)
    frames(st, ["left"], 1.0, 2, 0.1)
    s["text-default-queue"] = st

    # More than 8 messages (the oldest go) and one over 120 letters (cut)
    st = [{"op": "configure", "settings": {"text_speed": 10}}, {"op": "present", "side": "left"}]
    for i in range(10):
        st.append({"op": "say", "text": f"M{i}"})
    st.append({"op": "say", "text": "".join(chr(ord("A") + i % 26) for i in range(130))})
    frames(st, ["left"], 0.0, 40, 0.1, keep=False)
    frames(st, ["left"], 4.0, 50, 0.31)
    interval(st, "left", 20.0)
    s["text-queue-overflow"] = st

    # Turning the text line off drops what is showing, and later messages
    st = [{"op": "present", "side": "left"}, {"op": "say", "text": "Battery 40%"}]
    frames(st, ["left"], 0.0, 4, 0.1)
    st.append({"op": "configure", "settings": {"text_line": False}})
    st.append({"op": "say", "text": "ignored"})
    frames(st, ["left"], 0.4, 2, 0.1)
    interval(st, "left", 0.6)
    s["text-off"] = st

    # Sliding to another app
    st = [{"op": "present", "side": "left"}]
    frames(st, ["left"], 0.0, 1, 0.0)
    st.append({"op": "slide", "side": "left", "t": 0.1})
    interval(st, "left", 0.1)
    frames(st, ["left"], 0.1, 18, 1 / 30, picture="second")
    interval(st, "left", 0.75)
    s["slide-one"] = st

    # A slide, the text line and a ripple at once, on both panels
    st = [{"op": "present", "side": "left"}, {"op": "present", "side": "right"}, {"op": "say", "text": "Hi"}]
    frames(st, both, 0.0, 1, 0.0)
    st.append({"op": "slide", "side": "right", "t": 0.05})
    st.append({"op": "plug", "position": "right-back", "in": True, "t": 0.1})
    frames(st, ["left"], 0.05, 1, 0.0)
    frames(st, ["right"], 0.05, 1, 0.0, picture="second")
    frames(st, both, 0.1, 20, 1 / 30, picture="second")
    s["combo-slide-text-ripple"] = st

    # A drain under the text line; a slide asked for while draining is ignored
    st = [{"op": "present", "side": "left"}, {"op": "present", "side": "right"}]
    frames(st, both, 0.0, 1, 0.0)
    st.append({"op": "plug", "position": "right-middle", "in": False, "t": 0.0})
    st.append({"op": "say", "text": "Unplugged"})
    st.append({"op": "slide", "side": "right", "t": 0.1})
    frames(st, both, 0.0, 30, 1 / 30)
    s["combo-drain-text"] = st

    # Effects turned off in the settings
    st = [{"op": "configure", "settings": {"plug_animation": False, "slide": False, "text_line": False}},
          {"op": "present", "side": "left"}]
    frames(st, ["left"], 0.0, 1, 0.0)
    st.append({"op": "plug", "position": "left-back", "in": True, "t": 0.0})
    st.append({"op": "plug", "position": "left-back", "in": False, "t": 0.0})
    st.append({"op": "slide", "side": "left", "t": 0.0})
    st.append({"op": "say", "text": "nothing"})
    interval(st, "left", 0.0)
    frames(st, ["left"], 0.05, 2, 0.05, picture="second")
    s["settings-off"] = st

    # Port rows moved in the settings, and the panel's picture replaced mid-effect
    st = [{"op": "configure", "settings": {"port_rows": {"back": 0, "middle": 33, "front": "12"}}},
          {"op": "present", "side": "right"}]
    frames(st, ["right"], 0.0, 1, 0.0)
    st.append({"op": "plug", "position": "right-middle", "in": False, "t": 0.0})
    frames(st, ["right"], 0.0, 6, 1 / 30)
    st.append({"op": "forget", "side": "right"})
    interval(st, "right", 0.2)
    frames(st, ["right"], 0.2, 1, 0.0, picture="second")
    st.append({"op": "plug", "position": "right-front", "in": True, "t": 0.25})
    frames(st, ["right"], 0.25, 6, 1 / 30, picture="second")
    st.append({"op": "present", "side": "right", "present": False})
    frames(st, ["right"], 0.5, 2, 1 / 30, picture="second")
    s["port-rows-forget"] = st
    for steps in s.values():
        for step in steps:
            if "t" in step:
                step["t"] = repr(float(step["t"]))
    return s


SETTINGS_CASES = [
    {}, None, [], "x",
    {"text_speed": 99, "port_rows": {"back": -4, "middle": "20", "front": None},
     "learned_ports": {"k": "nowhere", "j": "Left back", "n": 5, "c": "right"}, "slide": 0, "full_shades": "yes",
     "bogus": 1},
    {"text_speed": "7"}, {"text_speed": 3.9}, {"text_speed": "x"}, {"text_speed": True}, {"text_speed": [1]},
    {"text_speed": " 8 "}, {"text_speed": "1_0"}, {"text_speed": -3}, {"text_speed": "+4"}, {"text_speed": "4.0"},
    {"port_rows": 5}, {"port_rows": {"back": 2.7, "middle": 1e9, "front": "abc"}},
    {"text_line": None, "plug_usb": "", "plug_charger": [], "text_song": {}, "text_battery": 0.0},
    {"learned_ports": ["left"]}, {"learned_ports": None},
]

POSITION_CASES = ["left-back", "Right Middle", "right", None, "middle", "left_front", "RIGHT-front-left",
                  "front back", "", "nowhere", 5]


def play(steps, pictures):
    """Plays one scenario; returns ({frame name: grid}, {value name: interval})."""
    clock = Clock()
    saved_time = effects.time
    effects.time = clock
    try:
        m = effects.Effects()
        drawn, values = {}, {}
        for i, step in enumerate(steps):
            op = step["op"]
            clock.now = float(step.get("t", clock.now))
            if op == "configure":
                m.configure(step["settings"])
            elif op == "present":
                m.panel_present(step["side"], step.get("present", True))
            elif op == "plug":
                m.plug(step["position"], step["in"])
            elif op == "say":
                m.say(step["text"])
            elif op == "slide":
                m.start_slide(step["side"])
            elif op == "forget":
                m.forget(step["side"])
            elif op == "interval":
                values[f"{i:03d}-{step['side']}"] = m.interval(step["side"])
            elif op == "frame":
                out = m.apply(step["side"], pictures[step["picture"]].copy(), new_frame=step["new"])
                if step["keep"]:
                    drawn[f"{i:03d}-{step['side']}"] = np.asarray(out)
            else:
                raise ValueError(op)
        return drawn, values
    finally:
        effects.time = saved_time


def results():
    """Everything the file holds, worked out again."""
    pictures = inputs()
    plays = scenarios()
    frames_out, values_out = {}, {}
    for name, steps in plays.items():
        drawn, values = play(steps, pictures)
        frames_out.update({f"{name}-{k}": g for k, g in drawn.items()})
        values_out.update({f"{name}-{k}": v for k, v in values.items()})
    settings = [[raw, effects.normalize(raw)] for raw in SETTINGS_CASES]
    positions = [[t, *effects.parse_position(t), effects.position_label(t)] for t in POSITION_CASES]
    return {
        "description": "The effects over the panels (led_mon/effects.py) played from fixed scripts at fixed "
                       "times: LED values (0-255). Made by scripts/golden_effects.py.",
        "inputs": {name: encode(g) for name, g in pictures.items()},
        "scenarios": plays,
        "frames": {name: encode(g) for name, g in frames_out.items()},
        "values": values_out,
        "settings": settings,
        "positions": positions,
    }


def save(data, path=EFFECTS_FILE):
    with open(path, "w", newline="\n") as f:
        json.dump(data, f, indent=0, sort_keys=True)
        f.write("\n")


def load(path=EFFECTS_FILE):
    with open(path) as f:
        return json.load(f)


def differences(data, saved, tolerance=0):
    """What differs between two results(): frames by more than tolerance, anything else at all."""
    problems = []
    for key in ("inputs", "frames"):
        for n in sorted(set(data[key]) - set(saved[key])):
            problems.append(f"{key}: missing from the file: {n}")
        for n in sorted(set(saved[key]) - set(data[key])):
            problems.append(f"{key}: no longer drawn: {n}")
        for n in sorted(set(data[key]) & set(saved[key])):
            if np.abs(decode(data[key][n]) - decode(saved[key][n])).max() > tolerance:
                problems.append(f"{key}: differs: {n}")
    for key in ("scenarios", "values", "settings", "positions"):
        if json.loads(json.dumps(data[key])) != saved[key]:
            problems.append(f"{key} differ")
    return problems


def main():
    parser = argparse.ArgumentParser(description="Write or check tests/golden/effects.json.")
    parser.add_argument("--check", action="store_true", help="only compare with the saved file")
    args = parser.parse_args()
    data = results()
    if args.check:
        problems = differences(data, load())
        print("\n".join(problems) if problems else "All effects frames match.")
        return 1 if problems else 0
    save(data)
    print(f"Saved {len(data['frames'])} effects frames in {EFFECTS_FILE}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
