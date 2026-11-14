# Effects drawn over whatever the panels show: a ripple when something is plugged in, the picture draining
# out through the port when it is unplugged, sliding between apps, and a line of scrolling text along the
# top of both panels. Ideas from pixeled (github.com/slowpaste/pixeled, WTFPL); this is our own code.
#
# The drawing threads pass every frame through manager.apply(side, grid) just before it goes to the panel.
# While an effect is playing, they also redraw the last frame on their own about 30 times a second
# (manager.interval(side)), so the effect moves smoothly even though the monitor draws 10 frames a second.
#
# Grids here are the monitor's: 9 x 34 (x, y), values 0-255.

# Built In Dependencies
import math
import time
import logging
import threading
from collections import deque

# External Dependencies
import numpy as np

log = logging.getLogger(__name__)

WIDTH, HEIGHT = 9, 34
SIDES = ("left", "right")
SPOTS = ("back", "middle", "front")
FRAME_SECONDS = 1 / 30

DEFAULT_SETTINGS = {
    # Ripple when something is plugged in, drain when it is unplugged
    "plug_animation": True,
    "plug_charger": True,
    "plug_headphones": True,
    "plug_usb": True,
    # Panel row each port is level with, counted from the top (the hinge end)
    "port_rows": {"back": 5, "middle": 16, "front": 27},
    # Ports the user corrected on the Effects tab: {USB location key or "charger": "left-back", ...}
    "learned_ports": {},
    # Text line along the top of both panels
    "text_line": True,
    "text_song": True,
    "text_charger": True,
    "text_battery": True,
    "text_speed": 5,
    # Slide the next app in from the bottom when the panel changes app, instead of cutting to it
    "slide": True,
    # Dim the panels with the LED chip's current instead of darker values (needs newer firmware)
    "full_shades": False,
}


def normalize(raw):
    """The effects settings with every key filled in and values in range."""
    s = dict(DEFAULT_SETTINGS)
    if isinstance(raw, dict):
        s.update({k: v for k, v in raw.items() if k in DEFAULT_SETTINGS and v is not None})
    for key in ("plug_animation", "plug_charger", "plug_headphones", "plug_usb", "text_line", "text_song",
                "text_charger", "text_battery", "slide", "full_shades"):
        s[key] = bool(s[key])
    try:
        s["text_speed"] = max(1, min(10, int(s["text_speed"])))
    except (TypeError, ValueError):
        s["text_speed"] = DEFAULT_SETTINGS["text_speed"]
    rows = dict(DEFAULT_SETTINGS["port_rows"])
    if isinstance(s.get("port_rows"), dict):
        for spot in SPOTS:
            try:
                rows[spot] = max(0, min(HEIGHT - 1, int(s["port_rows"].get(spot, rows[spot]))))
            except (TypeError, ValueError):
                pass
    s["port_rows"] = rows
    learned = s.get("learned_ports")
    s["learned_ports"] = {str(k): v for k, v in learned.items() if parse_position(v)[0]} \
        if isinstance(learned, dict) else {}
    return s


def parse_position(text):
    """'left-back' -> ('left', 'back'). Unknown parts are None."""
    side = spot = None
    if isinstance(text, str):
        for part in text.lower().replace("_", "-").replace(" ", "-").split("-"):
            if part in SIDES:
                side = part
            elif part in SPOTS:
                spot = part
    return side, spot


def position_text(side, spot):
    """('left', 'back') -> 'left-back'; None when the side is unknown."""
    if side not in SIDES:
        return None
    return f"{side}-{spot}" if spot in SPOTS else side


def position_label(position):
    side, spot = parse_position(position)
    if side is None:
        return "Unknown port"
    return f"{side.capitalize()} {spot}" if spot else f"{side.capitalize()} side"


def laplacian(f):
    p = np.pad(f, 1, mode="edge")      # edges reflect
    return p[:-2, 1:-1] + p[2:, 1:-1] + p[1:-1, :-2] + p[1:-1, 2:] - 4 * f


class Ripple:
    """The panel as a pool something was dropped into at (jack_x, jack_y), in pixels.

    A damped wave on a grid three times finer than the panel. Crests push the picture away from the
    jack and troughs pull it back, so the picture wobbles out and in and ends exactly as it was.
    Arrays here are (rows, columns).
    """
    S = 3
    STEP = 1 / 240
    SPEED = 120.0
    DAMP = 5.0
    SPRING = 6.25
    FREQ = 3.4
    DRIVE = 5000.0
    PUSH = 10.0
    PUSH_MAX = 12.0
    MAX_SECONDS = 3.0

    def __init__(self, jack_x, jack_y, width=WIDTH, height=HEIGHT):
        s = self.S
        self.width, self.height = width, height
        ys, xs = np.mgrid[0:height * s, 0:width * s].astype(float)
        jx, jy = jack_x * s - 0.5, jack_y * s - 0.5
        self.splash = np.exp(-((xs - jx) ** 2 + (ys - jy) ** 2) / (2 * 5.0 ** 2))
        py, px = np.mgrid[0:height, 0:width].astype(float)
        dx, dy = px + 0.5 - jack_x, py + 0.5 - jack_y
        r = np.hypot(dx, dy)
        fade = np.minimum(r * s / 5.0, 1.0) / np.maximum(r, 1e-9)
        self.ox, self.oy = dx * fade, dy * fade
        self.py, self.px = py, px
        self.h = np.zeros_like(xs)
        self.v = np.zeros_like(xs)
        self.t = 0.0
        self.age = 0.0
        self.carry = 0.0
        self.active = True

    def advance(self, dt):
        if not self.active:
            return
        dt = min(max(dt, 0.0), 0.1)
        self.age += dt
        self.carry += dt
        while self.carry >= self.STEP:
            self.carry -= self.STEP
            a = self.SPEED ** 2 * laplacian(self.h) - self.DAMP * self.v - self.SPRING * self.h
            if self.t is not None:
                env = (1 - math.exp(-self.t / 0.04)) * math.exp(-self.t / 0.2)
                a += self.DRIVE * env * math.sin(2 * math.pi * self.FREQ * self.t) * self.splash
                self.t += self.STEP
                if self.t > 1 / self.FREQ:
                    self.t = None
            self.v += self.STEP * a
            self.h += self.STEP * self.v
        quiet = np.abs(self.h).max() < 0.05 and np.abs(self.v).max() < 6
        if (self.t is None and quiet) or self.age > self.MAX_SECONDS:
            self.active = False

    def apply(self, pixels):
        """pixels: (rows, columns). Returns the picture as seen through the water."""
        if not self.active:
            return pixels
        s = self.S
        hp = self.h.reshape(self.height, s, self.width, s).mean(axis=(1, 3))
        push = self.PUSH_MAX * np.tanh(self.PUSH * hp / self.PUSH_MAX) / s

        def src(along, moved, size):
            p = np.abs(along - moved)
            p = np.where(p > size - 1, 2 * (size - 1) - p, p)
            return np.clip(np.rint(p), 0, size - 1).astype(int)

        x = src(self.px, push * self.ox, self.width)
        y = src(self.py, push * self.oy, self.height)
        return pixels[y, x]


class Drain:
    """The picture as particles, sucked out through a hole in the right-hand wall at row jack_y.
    For a port on the left, the caller flips the picture. Arrays here are (rows, columns)."""
    SUB = 2
    GIVE_UP = 1.6

    def __init__(self, pixels, jack_y, opening=2.0, seed=11, width=WIDTH, height=HEIGHT):
        self.width, self.height = width, height
        self.jack = (float(width), float(jack_y))
        self.opening = opening
        rng = np.random.default_rng(seed)
        n = self.SUB * self.SUB
        rows, cols = np.nonzero(pixels > 0)
        light = np.repeat(pixels[rows, cols].astype(float) / n, n)
        x = np.repeat(cols + 0.5, n) + rng.uniform(-0.2, 0.2, len(light))
        y = np.repeat(rows + 0.5, n) + rng.uniform(-0.2, 0.2, len(light))
        lag = rng.uniform(0.04, 0.12, len(light))
        swirl = rng.uniform(-1, 1, len(light))
        self.p = np.column_stack((x, y, np.zeros_like(x), np.zeros_like(x), light, lag, swirl))
        self.trail = np.zeros((height, width))
        self.time = 0.0
        self.glow = 0.0
        self.drained = False

    def advance(self, dt):
        dt = min(max(dt, 0.0), 0.1)
        p = self.p
        self.time += dt
        if not len(p):
            return
        jx, jy = self.jack
        dx, dy = jx - p[:, 0], jy - p[:, 1]
        r = np.hypot(dx, dy) + 1e-6
        drive = (1 - math.exp(-self.time / 0.08)) * (1 + 3 * self.time)
        speed = drive * (25 + 160 / (r + 2))
        tx, ty = -dy / r, dx / r            # turning around the hole: eddies
        sw = p[:, 6] * 18 * np.sin(self.time * 9 + p[:, 1] * 0.7) * np.minimum(r / 6, 1)
        fx = dx / r * speed + tx * sw
        fy = dy / r * speed + ty * sw
        k = 1 - np.exp(-dt / p[:, 5])
        p[:, 2] += (fx - p[:, 2]) * k
        p[:, 3] += (fy - p[:, 3]) * k
        p[:, 0] = np.clip(p[:, 0] + p[:, 2] * dt, 0, self.width - 1e-6)
        p[:, 1] = np.clip(p[:, 1] + p[:, 3] * dt, 0, self.height - 1e-6)
        beside = np.maximum(np.abs(p[:, 1] - jy) - self.opening / 2, 0)
        out = np.hypot(p[:, 0] - jx, beside) < 0.9
        if self.time > self.GIVE_UP:
            out[:] = True
        self.glow += p[out, 4].sum() * 0.02
        self.p = p[~out]

    def pixels(self, dt):
        f = np.zeros(self.height * self.width)
        p = self.p
        if len(p):
            x = np.clip(p[:, 0] - 0.5, 0, self.width - 1.001)
            y = np.clip(p[:, 1] - 0.5, 0, self.height - 1.001)
            x0, y0 = x.astype(int), y.astype(int)
            fx, fy = x - x0, y - y0
            i = y0 * self.width + x0
            for at, share in ((i, (1 - fx) * (1 - fy)), (i + 1, fx * (1 - fy)),
                              (i + self.width, (1 - fx) * fy), (i + self.width + 1, fx * fy)):
                f += np.bincount(at, p[:, 4] * share, f.size)
        f = f.reshape(self.height, self.width)
        self.trail = np.maximum(f, self.trail * math.exp(-max(dt, 0.0) / 0.06))
        self.glow *= math.exp(-max(dt, 0.0) / 0.12)
        out = self.trail.copy()
        jy = int(min(max(self.jack[1], 0), self.height - 1))
        for yy in range(max(0, jy - 1), min(self.height, jy + 2)):
            out[yy, -1] += self.glow * (1.0 if yy == jy else 0.5)
        if not len(p) and out.max() < 2:
            self.drained = True
        return np.clip(out, 0, 255)


# 3 x 5 letters for the text line
FONT = {
    "A": "010101111101101", "B": "110101110101110", "C": "011100100100011", "D": "110101101101110",
    "E": "111100110100111", "F": "111100110100100", "G": "011100101101011", "H": "101101111101101",
    "I": "111010010010111", "J": "001001001101010", "K": "101101110101101", "L": "100100100100111",
    "M": "101111111101101", "N": "110101101101101", "O": "010101101101010", "P": "110101110100100",
    "Q": "010101101110011", "R": "110101110101101", "S": "011100010001110", "T": "111010010010010",
    "U": "101101101101111", "V": "101101101101010", "W": "101101111111101", "X": "101101010101101",
    "Y": "101101010010010", "Z": "111001010100111",
    "0": "111101101101111", "1": "010110010010111", "2": "111001111100111", "3": "111001011001111",
    "4": "101101111001001", "5": "111100111001111", "6": "111100111101111", "7": "111001010010010",
    "8": "111101111101111", "9": "111101111001111",
    " ": "000000000000000", ".": "000000000000010", ",": "000000000010100", ":": "000010000010000",
    "!": "010010010000010", "?": "111001011000010", "-": "000000111000000", "+": "000010111010000",
    "%": "101001010100101", "'": "010010000000000", "\"": "101101000000000", "/": "001001010100100",
    "(": "001010010010001", ")": "100010010010100", "&": "010101010101011", "#": "101111101111101",
    "*": "000101010101000", "=": "000111000111000", "_": "000000000000111", "<": "001010100010001",
    ">": "100010001010100", "@": "010101111100011", "$": "011110010011110",
}
FONT_W, FONT_H = 3, 5
TEXT_ROWS = 6       # 5 rows of letters and a dark row under them


def text_columns(text):
    """The text as a (FONT_H, n) array of 0/1, one dark column between letters."""
    cols = []
    for ch in text.upper():
        glyph = FONT.get(ch, FONT["?"])
        g = np.array([int(c) for c in glyph]).reshape(FONT_H, FONT_W)
        cols.append(g)
        cols.append(np.zeros((FONT_H, 1), dtype=int))
    if not cols:
        return np.zeros((FONT_H, 0), dtype=int)
    return np.hstack(cols[:-1])


def text_width(text):
    return text_columns(text).shape[1]


class TextLine:
    """Messages scrolling right to left along the top rows of both panels, one after another."""

    def __init__(self):
        self.queue = deque(maxlen=8)
        self.current = None      # (columns, started at)
        self.speed = 18.0        # columns a second
        self.lock = threading.Lock()

    def say(self, text):
        text = " ".join(str(text).split())
        if text:
            with self.lock:
                self.queue.append(text[:120])

    def set_speed(self, setting):
        self.speed = 6.0 + 3.0 * max(1, min(10, int(setting)))

    def strip(self, width, now):
        """The text line as (TEXT_ROWS, width) of 0/1 across all panels, or None when it is idle."""
        with self.lock:
            if self.current is None:
                if not self.queue:
                    return None
                self.current = (text_columns(self.queue.popleft()), now)
            cols, start = self.current
            x = width - (now - start) * self.speed
            if x + cols.shape[1] < 0:
                # Gone off the left edge: the next message, if any
                self.current = None
                if not self.queue:
                    return None
                self.current = (text_columns(self.queue.popleft()), now)
                cols, start = self.current
                x = float(width)
        out = np.zeros((TEXT_ROWS, width), dtype=int)
        xi = int(round(x))
        for c in range(cols.shape[1]):
            if 0 <= xi + c < width:
                out[:FONT_H, xi + c] = cols[:, c]
        return out

    def active(self):
        with self.lock:
            return self.current is not None or bool(self.queue)


class SidePlayer:
    """The effects on one panel."""
    SLIDE_SECONDS = 0.45
    EMPTY_SECONDS = 0.2
    RISE_SECONDS = 0.25

    def __init__(self):
        self.ripples = []        # [(Ripple, flipped)]
        self.drain = None        # (Drain, flipped)
        self.phase = None        # None, 'drain', 'empty', 'rise'
        self.phase_since = 0.0
        self.slide = None        # (old frame (rows, cols), started at)
        self.last_base = None    # last frame from the monitor, (rows, cols) floats
        self.last_time = None

    def active(self):
        return bool(self.ripples) or self.phase is not None or self.slide is not None


class Effects:
    """Shared by the monitor's drawing threads and the visualizer's (one per process)."""

    def __init__(self):
        self.lock = threading.RLock()
        self.settings = normalize({})
        self.players = {side: SidePlayer() for side in SIDES}
        self.text = TextLine()
        self.panels = set()      # sides with a panel drawing now
        self.owner = {}          # side: the drawing thread that drew there last
        self.foreground = 160

    # Settings and panels
    def configure(self, settings):
        with self.lock:
            self.settings = normalize(settings)
            self.text.set_speed(self.settings["text_speed"])
            if not self.settings["text_line"]:
                with self.text.lock:
                    self.text.queue.clear()
                    self.text.current = None

    def panel_present(self, side, present=True):
        with self.lock:
            (self.panels.add if present else self.panels.discard)(side)

    def _panel_for(self, side):
        """Which panel shows a port on this side of the laptop, and whether its port is at the panel's
        left edge (so the picture is flipped for the drain)."""
        panels = [s for s in SIDES if s in self.panels] or ["left"]
        if side == "left":
            return panels[0], True
        if side == "right":
            return panels[-1], False
        return None, False

    # Events
    def plug(self, position, plugged_in=True):
        """Something went into (or came out of) the port at position ('left-back', 'right', None...)."""
        if not self.settings["plug_animation"]:
            return
        side, spot = parse_position(position)
        row = self.settings["port_rows"].get(spot, HEIGHT // 2) if spot else HEIGHT // 2
        with self.lock:
            panel, at_left = self._panel_for(side)
            if panel is None:
                # Unknown port: a ripple from the middle of every panel
                if plugged_in:
                    for p in (self.panels or {"left"}):
                        self.players[p].ripples.append((Ripple(WIDTH / 2, HEIGHT / 2), False))
                return
            player = self.players[panel]
            if plugged_in:
                if player.phase in ("drain", "empty"):
                    self._start_rise(player)
                # Computed on a flipped copy for a port on the left, so the ripple starts at the left edge
                player.ripples.append((Ripple(WIDTH, row + 0.5), at_left))
                player.ripples = player.ripples[-3:]
            else:
                base = player.last_base
                if base is None:
                    return
                frame = base[:, ::-1] if at_left else base
                player.ripples = []
                player.drain = (Drain(frame, row + 0.5), at_left)
                player.phase, player.phase_since = "drain", time.monotonic()

    def say(self, text):
        if self.settings["text_line"]:
            self.text.say(text)

    def start_slide(self, side):
        """The panel on this side is changing app: slide the new one in over the last frame."""
        if not self.settings["slide"]:
            return
        with self.lock:
            player = self.players.get(side)
            if player is not None and player.last_base is not None and player.phase is None:
                player.slide = (player.last_base.copy(), time.monotonic())

    def _start_rise(self, player):
        player.drain = None
        player.phase, player.phase_since = "rise", time.monotonic()

    # Drawing
    def active(self, side):
        with self.lock:
            player = self.players.get(side)
            return (player is not None and player.active()) or self.text.active()

    def interval(self, side):
        """How long a drawing thread may wait for a new frame before redrawing the effect itself;
        None while nothing is playing."""
        return FRAME_SECONDS if self.active(side) else None

    def forget(self, side):
        """The panel's picture was replaced by something else (a game, sleep): drop what is playing."""
        with self.lock:
            player = self.players.get(side)
            if player is not None:
                self.players[side] = SidePlayer()

    def apply(self, side, grid, new_frame=True):
        """grid: 9 x 34 (x, y) from the monitor or the visualizer. Returns the grid to send."""
        if side not in self.players:
            return grid
        now = time.monotonic()
        with self.lock:
            player = self.players[side]
            base = np.asarray(grid, dtype=float).T          # (rows, cols)
            if new_frame or player.last_base is None:
                player.last_base = base
            dt = 0.0 if player.last_time is None else min(now - player.last_time, 0.1)
            player.last_time = now
            if not player.active() and not self.text.active():
                return grid
            out = base.copy()

            if player.slide is not None:
                old, start = player.slide
                k = min((now - start) / player.SLIDE_SECONDS, 1.0)
                k = 1 - (1 - k) ** 3
                shift = int(round(HEIGHT * k))
                if shift >= HEIGHT:
                    player.slide = None
                else:
                    slid = np.zeros_like(out)
                    slid[:HEIGHT - shift] = old[shift:]
                    if shift:
                        slid[HEIGHT - shift:] = out[:shift]
                    out = slid

            if player.phase == "drain":
                drain, flipped = player.drain
                drain.advance(dt)
                picture = drain.pixels(dt)
                out = picture[:, ::-1] if flipped else picture
                if drain.drained:
                    player.drain = None
                    player.phase, player.phase_since = "empty", now
            elif player.phase == "empty":
                out = np.zeros_like(out)
                if now - player.phase_since >= player.EMPTY_SECONDS:
                    self._start_rise(player)
                    out = np.zeros_like(out)
            elif player.phase == "rise":
                k = min((now - player.phase_since) / player.RISE_SECONDS, 1.0)
                shift = int(round(HEIGHT * (1 - k)))
                risen = np.zeros_like(out)
                if HEIGHT - shift > 0:
                    risen[shift:] = out[:HEIGHT - shift]
                out = risen
                if k >= 1:
                    player.phase = None

            if player.phase in (None, "rise") and player.ripples:
                kept = []
                for ripple, flipped in player.ripples:
                    ripple.advance(dt)
                    if flipped:
                        out = ripple.apply(out[:, ::-1])[:, ::-1]
                    else:
                        out = ripple.apply(out)
                    if ripple.active:
                        kept.append((ripple, flipped))
                player.ripples = kept

            if self.settings["text_line"]:
                order = [s for s in SIDES if s in self.panels] or [side]
                if side in order:
                    strip = self.text.strip(WIDTH * len(order), now)
                    if strip is not None:
                        i = order.index(side)
                        part = strip[:, i * WIDTH:(i + 1) * WIDTH]
                        out[:TEXT_ROWS] = part * self.foreground

            return np.clip(np.rint(out), 0, 255).astype(int).T


manager = Effects()


def load_settings():
    """The effects settings from settings.json, with every key filled in."""
    from led_mon import app_settings
    return normalize(app_settings.load_settings().get("effects"))


def load():
    """Read the effects settings from settings.json into the shared manager."""
    manager.configure(load_settings())
    return manager.settings


def save(settings):
    from led_mon import app_settings
    current = app_settings.load_settings()
    current["effects"] = normalize(settings)
    return app_settings.save_settings(current)
