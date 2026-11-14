# Golden frames: what the Python monitor draws for fixed, made-up inputs.
#
# The Rust rewrite of the monitor and the music visualizer has to draw exactly the same pictures. This
# script draws every monitor app and every music look from fixed inputs (no real sensors, sound or
# clock) and saves the panels in tests/golden/. tests/test_golden_frames.py checks that the Python
# code still draws the same, and the Rust tests compare their own pictures with the same files.
#
#   python scripts/golden_frames.py          write tests/golden/*.json again (after a deliberate change)
#   python scripts/golden_frames.py --check  only compare, and list the frames that differ
#
# File format (JSON): {"frames": {"<case name>": ["<row 0>", ..., "<row 33>"]}}. Each row is one of the
# panel's 34 rows, top first, as 9 two-digit hex values, left column first. Monitor apps store the LED
# values (0-255). Music looks store their brightness 0.0-1.0 as round(value * 255).

# Built In Dependencies
import os
import sys
import json
import argparse

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GOLDEN_DIR = os.path.join(ROOT, "tests", "golden")
MONITOR_FILE = os.path.join(GOLDEN_DIR, "monitor_apps.json")
MUSIC_FILE = os.path.join(GOLDEN_DIR, "music_looks.json")

if ROOT not in sys.path:
    sys.path.insert(0, ROOT)

# External Dependencies
import numpy as np

FOREGROUND = 160   # rendering.DEFAULT_FOREGROUND
BACKGROUND = 35    # rendering.DEFAULT_BACKGROUND
WIDTH, HEIGHT = 9, 34


def import_drawing():
    """led_mon.drawing without loading the plugins (those need the audio and weather libraries)."""
    saved = sys.argv
    sys.argv = [saved[0], "--disable-plugins"]
    try:
        from led_mon import drawing
    finally:
        sys.argv = saved
    return drawing


def encode(grid):
    """A 9 x 34 grid of 0-255 values as 34 row strings."""
    grid = np.asarray(grid)
    assert grid.shape == (WIDTH, HEIGHT), grid.shape
    values = np.clip(np.rint(grid), 0, 255).astype(int)
    return ["".join("%02x" % values[x, y] for x in range(WIDTH)) for y in range(HEIGHT)]


def decode(rows):
    return np.array([[int(row[2 * x:2 * x + 2], 16) for row in rows] for x in range(WIDTH)])


class FrozenClock:
    """Stands in for the time module inside led_mon.drawing, so the battery's flash and pulse are fixed."""
    def __init__(self, now):
        self.now = now

    def time(self):
        return self.now

    def monotonic(self):
        return self.now


# Monitor apps ---------------------------------------------------------------------------------------

CPU_8 = [0.0, 0.12, 0.25, 0.37, 0.5, 0.63, 0.88, 1.0]
CPU_16 = [i / 15 for i in range(16)]
CPU_24 = [((i * 7) % 24) / 23 for i in range(24)]
HISTORY = [0.9, 0.75, None, 0.6, 0.45, 0.3, 0.15, 0.05, 0.0, 1.0, 0.5, 0.33, 0.66, 0.2, 0.8, 0.1]
FPS_HISTORY = [144.0, 140.5, 0.0, 120.0, 61.0, 59.9, 30.0, 75.0, 90.0]


def monitor_cases():
    """{case name: 9 x 34 grid of LED values} for every monitor app's drawing, top and bottom half."""
    drawing = import_drawing()
    from led_mon import patterns
    cases = {}

    def case(name, draw):
        grid = np.zeros((WIDTH, HEIGHT), dtype=int)
        draw(grid)
        cases[name] = grid

    halves = (("top", 0), ("bottom", 16))
    for half, y in halves:
        for label, values in (("8", CPU_8), ("16", CPU_16), ("24", CPU_24)):
            def cpu(g, values=values, y=y):
                drawing.draw_app("cpu", g, values, FOREGROUND, y)
                drawing.draw_app_border("cpu", g, BACKGROUND, y)
            case(f"cpu-{label}-cores-{half}", cpu)
        for app in ("cpu-graph", "gpu-graph"):
            def graph(g, app=app, y=y):
                drawing.draw_app(app, g, HISTORY, FOREGROUND, y)
                drawing.draw_app_border(app, g, BACKGROUND, y)
            case(f"{app}-{half}", graph)
        for ratio in (0.0, 0.07, 0.33, 0.5, 0.99, 1.0):
            def mem(g, ratio=ratio, y=y):
                drawing.draw_app("mem", g, ratio, FOREGROUND, y)
                drawing.draw_app_border("mem", g, BACKGROUND, y)
            case(f"mem-{ratio}-{half}", mem)
        for app in ("disk", "net"):
            for a, b in ((0.0, 0.0), (0.1, 0.9), (0.5, 0.25), (1.0, 0.66)):
                def bars(g, app=app, a=a, b=b, y=y):
                    drawing.draw_app(app, g, a, FOREGROUND, bar_x_offset=1, y=y)
                    drawing.draw_app(app, g, b, FOREGROUND, bar_x_offset=5, y=y)
                    drawing.draw_app_border(app, g, BACKGROUND, y)
                case(f"{app}-{a}-{b}-{half}", bars)
        for disk, net in ((((0.0, 0.0)), (0.0, 0.0)), ((0.2, 0.8), (1.0, 0.4)), ((0.55, 0.05), (0.3, 0.95))):
            def disk_net(g, disk=disk, net=net, y=y):
                drawing.draw_app("disk-net", g, disk, net, FOREGROUND, y)
                drawing.draw_app_border("disk-net", g, BACKGROUND, y)
            case(f"disk-net-{disk[0]}-{disk[1]}-{net[0]}-{net[1]}-{half}", disk_net)
        for fps, available in ((0, True), (7, True), (60, True), (144, True), (199.6, True), (240, True),
                               (999, True), (1500, True), (60, False)):
            def fps_num(g, fps=fps, available=available, y=y):
                drawing.draw_app("fps-num", g, fps, available, FOREGROUND, y)
            case(f"fps-num-{fps}-{'ok' if available else 'unavailable'}-{half}", fps_num)

    # The battery sits under the memory bars: y=3 on the top half, y=19 on the bottom half
    for half, y in (("top", 3), ("bottom", 19)):
        for ratio, plugged in ((1.0, False), (0.66, False), (0.33, True), (0.05, False), (0.05, True)):
            for now in (0.0, 1.0, 2.5, 4.7):
                def battery(g, ratio=ratio, plugged=plugged, y=y, now=now):
                    real = drawing.time
                    drawing.time = FrozenClock(now)
                    try:
                        drawing.draw_app("mem", g, 0.4, FOREGROUND, y - 3)
                        drawing.draw_app("bat", g, ratio, plugged, FOREGROUND, y)
                        drawing.draw_app_border("mem", g, BACKGROUND, y - 3)
                    finally:
                        drawing.time = real
                state = "plugged" if plugged else "battery"
                case(f"mem-bat-{ratio}-{state}-t{now}-{half}", battery)

    # The whole-panel FPS counter: number on top, histogram below
    for fps, history, available in ((144, FPS_HISTORY, True), (61, FPS_HISTORY[:4], True), (0, [], True),
                                    (480, [480, 470, 300], True), (0, FPS_HISTORY, False)):
        def fps_panel(g, fps=fps, history=history, available=available):
            drawing.draw_app("fps", g, fps, history, available, FOREGROUND)
        case(f"fps-{fps}-{len(history)}-{'ok' if available else 'unavailable'}", fps_panel)

    # Pictures from the snapshot files
    for panel, file in (("left", "zigzag.json"), ("left", "the-way-left.json"), ("right", "the-way-right.json")):
        if os.path.exists(os.path.join(ROOT, "led_mon", "snapshot_files", panel, file)):
            case(f"snap-{panel}-{file[:-5]}", lambda g, panel=panel, file=file: drawing.draw_app(
                "snap", g, FOREGROUND, path="snapshot_files", panel=panel, file=file))

    # Clock and weather characters (the time and weather plugins draw these lists)
    for name, chars in (("time-1234", ["1", "2", "horiz_colon", "3", "4"]),
                        ("time-0905", ["0", "9", "horiz_colon", "0", "5"]),
                        ("weather-21c-clouds", ["2", "1", "degC", "clouds"]),
                        ("weather-minus-4f-snow", ["-", "4", "degF", "snow"]),
                        ("weather-wind-chill", ["wc", " ", "1", "8", "degC"]),
                        ("weather-wind", ["wd", "1", "2", "km", "wind-sw"]),
                        ("weather-unknown", ["?", "?"])):
        case(name, lambda g, chars=chars: drawing.draw_chars_list(g, chars, FOREGROUND, 1))
    for icon in sorted(patterns.icons):
        if icon != "pm_indicator":
            case(f"weather-icon-{icon}", lambda g, icon=icon: drawing.draw_chars_list(g, ["2", "0", "degC", icon], FOREGROUND, 1))

    # Temperatures: three numbers, the middle one dimmer
    from led_mon.plugins.temp_fan_plugin import draw_temp_numbers
    for half, y in halves:
        for values in ((45, 62.4, 38), (100, 7, None), (199, 0, 150.6), (None, None, None)):
            label = "-".join("none" if v is None else str(v) for v in values)
            case(f"temp-{label}-{half}", lambda g, values=values, y=y: draw_temp_numbers(g, values, FOREGROUND, y))

    # Fans: two bars, split down the middle
    for half, y in halves:
        for a, b in ((0.0, 0.0), (0.25, 1.0), (0.5, 0.1)):
            def fans(g, a=a, b=b, y=y):
                drawing.draw_bar(g, a, FOREGROUND, bar_x_offset=1, y=y)
                drawing.draw_bar(g, b, FOREGROUND, bar_x_offset=5, y=y)
                drawing.draw_2_x_1_horiz_grid(g, BACKGROUND, y)
            case(f"fan-{a}-{b}-{half}", fans)

    # Weather readings as the weather app draws them, with the forecast's day and hour bars
    from led_mon.plugins import time_weather_plugin as tw
    readings = {
        "mild": tw.Weather("Current", 21.4, 19.5, 12.5, "km", 225.0, "degC", "Clouds"),
        "cold": tw.Weather("Current", -4.5, -11.6, 30.2, "mi", 350.0, "degF", "snow"),
        "hot": tw.Weather("Forecast", 101.0, 99.5, 3.0, "km", 92.0, "degK", "clear"),
    }
    for label, weather in readings.items():
        for measure in ("temp_condition", "wind_chill", "wind", "bogus"):
            for half, y in halves:
                def weather_case(g, weather=weather, measure=measure, y=y):
                    try:
                        drawing.draw_chars_list(g, tw.get_weather_values(weather, measure), FOREGROUND, y)
                    except ValueError:
                        # Python stops at the first character below the panel (the monitor logs it); the
                        # characters already drawn stay
                        pass
                case(f"weather-{label}-{measure}-{half}", weather_case)
    for day, hour in ((1, 12), (3, 0), (5, 23), (9, -1)):
        def forecast(g, day=day, hour=hour):
            drawing.draw_chars_list(g, tw.get_weather_values(readings["mild"], "temp_condition"), FOREGROUND, 0)
            tw.draw_fc_period_indicator(g, FOREGROUND, day, hour)
        case(f"weather-forecast-d{day}-h{hour}", forecast)

    # Now playing: the made-up cover, song name and bars, each panel's half
    from led_mon import now_playing as np_app
    levels = [7.4, 6.5, 5.5, 4.2, 3.5, 2.5, 1.0, 0.4, -1.0]
    songs = {
        "full": np_app.Media(title="Preview song", artist="White Rabbit", playing=True),
        "title": np_app.Media(title="ok?", playing=True),
        "none": np_app.Media(playing=True),
    }
    options = {
        "all": dict(title=True, visualizer=True, drift=True),
        "plain": dict(title=False, visualizer=False, drift=False),
    }
    for song, media in songs.items():
        for opt, kwargs in options.items():
            for now in (0.0, 2.3, 7.9, 61.25):
                for side in ("left", "right"):
                    def now_playing_case(g, media=media, kwargs=kwargs, now=now, side=side):
                        canvas = np_app.scene(media, np_app.demo_cover(), now, levels=np.array(levels), **kwargs)
                        half = canvas[:, :WIDTH] if side == "left" else canvas[:, WIDTH:]
                        g[:, :] = np.maximum(g, np.rint(half.T * FOREGROUND).astype(int))
                    case(f"now-playing-{song}-{opt}-t{now}-{side}", now_playing_case)

    # The app names shown while choosing apps (Ctrl+Alt+Shift+I): half-panel names in pairs,
    # whole-panel names on their own. A fixed list, because loading the plugins adds names to the table.
    # (fps-num's name is 16 rows tall and doesn't fit the 15 rows draw_ids gives it, so it's left out.)
    from led_mon.plugins import temp_fan_plugin
    id_table = dict(patterns.id_patterns)
    id_table.update(temp_fan_plugin.id_patterns)
    halves_ids = ["cpu", "cpu-graph", "disk", "disk-net", "fan", "gpu-graph", "mem-bat", "net", "temp"]
    whole_ids = ["fps", "snap"]
    real = dict(patterns.id_patterns)
    patterns.id_patterns.update(id_table)
    try:
        for i, top in enumerate(halves_ids):
            bottom = halves_ids[(i + 3) % len(halves_ids)]
            case(f"ids-{top}-{bottom}", lambda g, top=top, bottom=bottom: drawing.draw_ids(g, top, bottom, FOREGROUND))
        for name in whole_ids:
            case(f"id-{name}", lambda g, name=name: drawing.draw_id(g, name, FOREGROUND, {}))
    finally:
        patterns.id_patterns.clear()
        patterns.id_patterns.update(real)

    # Borders on their own, and the whole-panel outline (the cpu border follows the last core count drawn)
    drawing.cpu_core_count = 8
    for app in ("cpu", "cpu-graph", "disk-net", "disk", "mem"):
        for half, y in halves:
            case(f"border-{app}-{half}", lambda g, app=app, y=y: drawing.draw_app_border(app, g, BACKGROUND, y))
    case("border-outline", lambda g: drawing.draw_outline_border(g, BACKGROUND))
    return cases


# Music looks ----------------------------------------------------------------------------------------

MUSIC_FRAMES = 48       # frames played per case
MUSIC_SAVE_EVERY = 6    # every 6th one is saved
MUSIC_DT = 0.05         # seconds between frames, as in music mode (20 frames a second)
OPTION_SETS = {
    "plain": {},
    "options": {"peak-dots": True, "shaded": True, "flip": True, "fall-speed": "fast", "sensitivity": 8},
}


def music_cases():
    """{case name: 9 x 34 grid of brightness 0.0-1.0} for every look, both panels, alone and spanned."""
    from led_mon.equalizer_files import looks
    cases = {}
    for style in looks.STYLE_KEYS:
        for span in (False, True):
            for option_name, extra in OPTION_SETS.items():
                for side in ("left", "right"):
                    options = dict(extra, style=style)
                    options["span-panels"] = span
                    visualizer = looks.Visualizer(options, side, seed=1)
                    t = 1.0
                    for frame in range(1, MUSIC_FRAMES + 1):
                        grid = visualizer.frame(looks.demo_samples(t), MUSIC_DT)
                        t += MUSIC_DT
                        if frame % MUSIC_SAVE_EVERY == 0:
                            name = f"{style}-{'span' if span else 'single'}-{option_name}-{side}-f{frame:02d}"
                            cases[name] = np.asarray(grid, dtype=float) * 255.0
                    # Then silence, which most looks fade out of (and some pulse in)
                    silence = np.zeros((looks.FFT_SIZE, 2))
                    for _ in range(30):
                        grid = visualizer.frame(silence, MUSIC_DT)
                    name = f"{style}-{'span' if span else 'single'}-{option_name}-{side}-silence"
                    cases[name] = np.asarray(grid, dtype=float) * 255.0
    # The brightness the panels get from a look, at three brightness settings
    ramp = np.array([[x / 8 * y / 33 for y in range(HEIGHT)] for x in range(WIDTH)])
    ramp[0, :3] = 0.015
    for foreground in (1, 60, 255):
        cases[f"to-leds-ramp-{foreground}"] = looks.to_leds(ramp, foreground)
    return cases


# Saving and checking --------------------------------------------------------------------------------

def save(path, cases, description):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    data = {"description": description, "frames": {name: encode(grid) for name, grid in cases.items()}}
    with open(path, "w", newline="\n") as f:
        json.dump(data, f, indent=0, sort_keys=True)
        f.write("\n")


def load(path):
    with open(path) as f:
        return {name: decode(rows) for name, rows in json.load(f)["frames"].items()}


def differences(path, cases, tolerance=0):
    """Names of frames that differ from the saved ones by more than tolerance, missing or extra."""
    saved = load(path)
    problems = [f"missing from {os.path.basename(path)}: {n}" for n in sorted(set(cases) - set(saved))]
    problems += [f"no longer drawn: {n}" for n in sorted(set(saved) - set(cases))]
    for name in sorted(set(cases) & set(saved)):
        now = decode(encode(cases[name]))
        if np.abs(now - saved[name]).max() > tolerance:
            problems.append(f"differs: {name}")
    return problems


def main():
    parser = argparse.ArgumentParser(description="Write or check the golden frames in tests/golden/.")
    parser.add_argument("--check", action="store_true", help="only compare with the saved frames")
    args = parser.parse_args()
    monitor, music = monitor_cases(), music_cases()
    if args.check:
        problems = differences(MONITOR_FILE, monitor) + differences(MUSIC_FILE, music, tolerance=1)
        print("\n".join(problems) if problems else "All golden frames match.")
        return 1 if problems else 0
    save(MONITOR_FILE, monitor, "LED values (0-255) the monitor apps draw for fixed inputs. "
                                "Made by scripts/golden_frames.py.")
    save(MUSIC_FILE, music, "Music look brightness (0.0-1.0, stored as round(value * 255)) for fixed made-up "
                            "music, and to_leds() values. Made by scripts/golden_frames.py.")
    print(f"Saved {len(monitor)} monitor frames and {len(music)} music frames in {GOLDEN_DIR}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
