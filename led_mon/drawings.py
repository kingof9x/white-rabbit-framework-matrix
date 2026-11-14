# Drawings made on the window's Draw tab: one panel's worth of greyscale LEDs (9 wide, 34 tall),
# one or more frames, saved as small JSON files in the user's settings folder (drawings/).
#
# The Picture app plays them on a panel, and the Draw tab imports PNG and GIF files into them.
#
# File format (version 1):
#   {"version": 1, "delay_ms": 100, "frames": [[[0-255 x 9] x 34], ...]}
# Each frame is 34 rows of 9 brightness values, top row first.

# Built In Dependencies
import os
import re
import json
import logging

# External Dependencies
import numpy as np

# Internal Dependencies
from led_mon import platform_utils

log = logging.getLogger(__name__)

WIDTH = 9
HEIGHT = 34
VERSION = 1
FOLDER = "drawings"
EXTENSION = ".json"
DEFAULT_DELAY_MS = 100
MIN_DELAY_MS = 20
MAX_DELAY_MS = 10000
MAX_FRAMES = 200
# How a picture is fitted into 9 x 34 when it is imported
FIT_WHOLE = "whole"      # all of the picture, with dark bars where the shape differs
FIT_FILL = "fill"        # fill the panel, cutting off what doesn't fit
FIT_STRETCH = "stretch"  # squash the picture to 9 x 34
FITS = (FIT_WHOLE, FIT_FILL, FIT_STRETCH)
FIT_LABELS = {FIT_WHOLE: "Show the whole picture", FIT_FILL: "Fill the panel (cut the edges)",
              FIT_STRETCH: "Stretch to fit"}
# The Picture app's file setting starts with this for a drawing, so built-in pictures keep working
PREFIX = "drawing:"


def folder():
    return os.path.join(platform_utils.user_config_dir(), FOLDER)


def blank_frame():
    return np.zeros((HEIGHT, WIDTH), dtype=np.uint8)


class Drawing:
    """Frames of 34 x 9 brightness values (numpy uint8), and how long each frame shows."""

    def __init__(self, frames=None, delay_ms=DEFAULT_DELAY_MS):
        self.frames = [np.array(f, dtype=np.uint8) for f in frames] if frames else [blank_frame()]
        self.delay_ms = clamp_delay(delay_ms)

    def to_json(self):
        return {"version": VERSION, "delay_ms": self.delay_ms,
                "frames": [f.tolist() for f in self.frames]}

    @classmethod
    def from_json(cls, data):
        frames = []
        for f in data.get("frames", [])[:MAX_FRAMES]:
            a = np.clip(np.array(f, dtype=float), 0, 255).astype(np.uint8)
            if a.shape != (HEIGHT, WIDTH):
                raise ValueError(f"a frame is {a.shape[1] if a.ndim == 2 else '?'} x {a.shape[0]}, not 9 x 34")
            frames.append(a)
        return cls(frames, data.get("delay_ms", DEFAULT_DELAY_MS))

    def frame_at(self, seconds):
        """The frame showing after this many seconds of playing (loops)."""
        index = int(seconds * 1000 // self.delay_ms) % len(self.frames)
        return self.frames[index]


def clamp_delay(ms):
    try:
        ms = int(ms)
    except (TypeError, ValueError):
        ms = DEFAULT_DELAY_MS
    return max(MIN_DELAY_MS, min(MAX_DELAY_MS, ms))


def safe_name(name):
    """A drawing name usable as a file name on Windows."""
    name = re.sub(r'[<>:"/\\|?*\x00-\x1f]', "", name or "").strip().rstrip(".")
    return name[:60]


def path_for(name):
    return os.path.join(folder(), safe_name(name) + EXTENSION)


def list_names():
    """The saved drawings' names, sorted."""
    try:
        files = os.listdir(folder())
    except OSError:
        return []
    return sorted((f[:-len(EXTENSION)] for f in files if f.endswith(EXTENSION)), key=str.lower)


def save(name, drawing):
    name = safe_name(name)
    if not name:
        raise ValueError("Type a name for the drawing first")
    os.makedirs(folder(), exist_ok=True)
    path = path_for(name)
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(drawing.to_json(), f, separators=(",", ":"))
    os.replace(tmp, path)
    return name


def load(name):
    with open(path_for(name)) as f:
        return Drawing.from_json(json.load(f))


def delete(name):
    os.remove(path_for(name))


def _fit(image, fit):
    """Resize a greyscale PIL image to 9 x 34 (averaging areas: no halos, and pixel art stays sharp)."""
    from PIL import Image
    w, h = image.size
    if fit == FIT_STRETCH or w == 0 or h == 0:
        return image.resize((WIDTH, HEIGHT), Image.BOX)
    scale = (min if fit == FIT_WHOLE else max)(WIDTH / w, HEIGHT / h)
    nw, nh = max(1, round(w * scale)), max(1, round(h * scale))
    resized = image.resize((nw, nh), Image.BOX)
    canvas = Image.new("L", (WIDTH, HEIGHT), 0)
    canvas.paste(resized, ((WIDTH - nw) // 2, (HEIGHT - nh) // 2))
    return canvas


def import_image(path, fit=FIT_WHOLE, invert=False):
    """A PNG, GIF (all its frames), JPEG or BMP turned into a Drawing.

    See-through parts become dark. The brightest part of the picture becomes full brightness.
    """
    from PIL import Image, ImageSequence
    frames = []
    delays = []
    with Image.open(path) as image:
        for frame in ImageSequence.Iterator(image):
            if len(frames) >= MAX_FRAMES:
                break
            delays.append(frame.info.get("duration") or DEFAULT_DELAY_MS)
            rgba = frame.convert("RGBA")
            background = Image.new("RGBA", rgba.size, (0, 0, 0, 255))
            grey = Image.alpha_composite(background, rgba).convert("L")
            a = np.asarray(_fit(grey, fit), dtype=float)
            if invert:
                a = 255 - a
            frames.append(a)
    top = max((f.max() for f in frames), default=0)
    if top > 0:
        frames = [f * (255.0 / top) for f in frames]
    frames = [np.clip(np.round(f), 0, 255).astype(np.uint8) for f in frames]
    delay = int(np.median(delays)) if delays else DEFAULT_DELAY_MS
    return Drawing(frames, delay)


# The Picture app
def setting_value(name):
    return PREFIX + name


def name_from_setting(value):
    if isinstance(value, str) and value.startswith(PREFIX):
        return value[len(PREFIX):]
    return None


class Player:
    """Plays saved drawings for the monitor, reloading a file when it changes."""

    def __init__(self, clock=None):
        import time
        self.clock = clock or time.monotonic
        self.started = self.clock()
        self.cache = {}  # name: (modified time, Drawing or None)

    def get(self, name):
        try:
            mtime = os.path.getmtime(path_for(name))
        except OSError:
            return None
        cached = self.cache.get(name)
        if cached is None or cached[0] != mtime:
            try:
                drawing = load(name)
            except Exception as e:
                log.warning(f"Could not read the drawing {name}: {e}")
                drawing = None
            self.cache[name] = (mtime, drawing)
        return self.cache[name][1]

    def frame(self, name):
        """The frame to show now as a 9 x 34 array (x, y) of 0-255, or None if the drawing is missing."""
        drawing = self.get(name)
        if drawing is None:
            return None
        return drawing.frame_at(self.clock() - self.started).T
