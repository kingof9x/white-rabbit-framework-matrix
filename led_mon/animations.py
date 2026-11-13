# Animations for the panels, and the old-TV switch-off / switch-on effect.
#
# The animations (fire, plasma, DNA helix, snowfall, chevrons, bouncing ball) are a whole-panel app
# called "animation" in the layout (args: look). Each look is an object whose frame(dt) returns the next
# picture as a 9 x 34 array of brightness 0.0-1.0 (column, row), moving on by dt seconds, so it runs at
# the same speed however often the monitor draws.
#
# tv_off_frames / tv_on_frames give the frames of an old TV turning off (the picture squeezes into a
# bright line, then a dot that fades) and back on (the reverse), played when the panels turn off or on.
#
# The ideas come from Deo's Framework16 LED Matrix Manager (MIT); the code here is our own.

# Built In Dependencies
import math

# External Dependencies
import numpy as np

WIDTH, HEIGHT = 9, 34


def spread_heat(heat, rng, k, cooling=0.14):
    """One step of a classic fire: each cell takes the average of the cells below it, minus a little.

    heat is WIDTH x (HEIGHT + 2): the bottom two rows are the fuel, already filled in by the caller.
    k scales the cooling for longer steps. Shared by the Fire animation and the Bonfire music look."""
    left = np.vstack([heat[:1], heat[:-1]])
    right = np.vstack([heat[1:], heat[-1:]])
    cool = (rng.random((WIDTH, HEIGHT)) ** 3 * cooling + 0.008) * k
    heat[:, :HEIGHT] = (heat[:, 1:HEIGHT + 1] * 2 + left[:, 1:HEIGHT + 1] + right[:, 1:HEIGHT + 1]
                        + heat[:, 2:HEIGHT + 2]) / 5.0 - cool
    np.clip(heat, 0.0, 1.0, out=heat)
    return heat


class _Look:
    """Steps a look at a steady 25 frames a second, whatever dt the caller gives."""
    STEP = 0.04

    def __init__(self, seed=None):
        self.rng = np.random.default_rng(seed)
        self.t = 0.0
        self._owed = 0.0
        self.grid = np.zeros((WIDTH, HEIGHT))

    def frame(self, dt=STEP):
        # A long pause (the window was hidden, the PC slept) doesn't fast-forward more than a second
        self._owed += min(max(dt, 0.0), 1.0)
        while self._owed >= self.STEP - 1e-9:
            self._owed -= self.STEP
            self.t += self.STEP
            self.step()
        return self.draw()

    def step(self):
        pass

    def draw(self):
        return self.grid


LOOK_STEP = _Look.STEP


class Fire(_Look):
    """Flames rising from the bottom of the panel, about a third of the way up."""

    def __init__(self, seed=None):
        super().__init__(seed)
        self.heat = np.zeros((WIDTH, HEIGHT + 2))

    def step(self):
        # Fuel along the bottom: most columns burning, a few dying down for a moment, so tongues of flame rise
        fuel = np.where(self.rng.random(WIDTH) < 0.7, 1.0, 0.2)
        self.heat[:, -2:] = fuel[:, None]
        spread_heat(self.heat, self.rng, 1.0, cooling=0.2)

    def draw(self):
        return np.clip(self.heat[:, :HEIGHT] * 1.5, 0.0, 1.0) ** 1.8


class Plasma(_Look):
    """Soft blobs of light that drift and merge, from a few sine waves added together."""

    def __init__(self, seed=None):
        super().__init__(seed)
        self.x, self.y = np.meshgrid(np.arange(WIDTH, dtype=float), np.arange(HEIGHT, dtype=float), indexing="ij")

    def draw(self):
        t, x, y = self.t, self.x, self.y
        v = (np.sin(x / 2.6 + t * 1.1)
             + np.sin(y / 4.5 - t * 0.9)
             + np.sin((x + y) / 6.0 + t * 0.7)
             + np.sin(np.hypot(x - 4 - 3 * math.sin(t * 0.5), y - 17 - 12 * math.cos(t * 0.37)) / 3.2 - t * 1.3))
        return ((v / 4.0 + 1.0) / 2.0) ** 1.3


class DnaHelix(_Look):
    """Two strands twisting around each other, joined by rungs; the strand in front is brighter."""
    TURN = 17.0      # rows for one full twist
    SPEED = 1.6      # twist per second, in radians
    CENTRE = (WIDTH - 1) / 2
    RADIUS = 3.6

    def draw(self):
        grid = np.zeros((WIDTH, HEIGHT))
        for y in range(HEIGHT):
            angle = y * 2 * math.pi / self.TURN + self.t * self.SPEED
            for strand in (0, 1):
                a = angle + strand * math.pi
                x = self.CENTRE + self.RADIUS * math.sin(a)
                depth = 0.5 + 0.5 * math.cos(a)        # 1 in front, 0 behind
                xi = int(round(x))
                grid[xi, y] = max(grid[xi, y], 0.3 + 0.7 * depth)
            if y % 3 == 0:
                # A rung between the strands, dimmer than either
                xa = self.CENTRE + self.RADIUS * math.sin(angle)
                xb = self.CENTRE - self.RADIUS * math.sin(angle)
                lo, hi = sorted((int(round(xa)), int(round(xb))))
                for xi in range(lo + 1, hi):
                    grid[xi, y] = max(grid[xi, y], 0.28)
        return grid


class Snowfall(_Look):
    """Snowflakes drifting down and settling in a thin layer at the bottom, which slowly melts."""
    FLAKES = 16

    def __init__(self, seed=None):
        super().__init__(seed)
        self.flakes = [self._new_flake(top=False) for _ in range(self.FLAKES)]
        self.ground = np.zeros(WIDTH)   # how much snow has settled in each column, in rows

    def _new_flake(self, top=True):
        r = self.rng
        return [float(r.uniform(0, WIDTH - 1)),
                -float(r.uniform(0, 6)) if top else float(r.uniform(0, HEIGHT - 2)),
                float(r.uniform(3.5, 7.5)),         # rows per second
                float(r.uniform(0, 2 * math.pi)),   # sway phase
                float(r.uniform(0.6, 1.0))]         # brightness

    def step(self):
        for i, f in enumerate(self.flakes):
            f[1] += f[2] * self.STEP
            f[3] += self.STEP * 1.7
            x = int(round(f[0] + 0.7 * math.sin(f[3])))
            x = min(WIDTH - 1, max(0, x))
            floor = HEIGHT - 1 - int(self.ground[x])
            if f[1] >= floor:
                self.ground[x] = min(2.0, self.ground[x] + 0.35)
                self.flakes[i] = self._new_flake()
        self.ground = np.maximum(0.0, self.ground - 0.004)

    def draw(self):
        grid = np.zeros((WIDTH, HEIGHT))
        for x in range(WIDTH):
            depth = self.ground[x]
            for row in range(int(math.ceil(depth))):
                grid[x, HEIGHT - 1 - row] = 0.35 * min(1.0, depth - row)
        for f in self.flakes:
            x = min(WIDTH - 1, max(0, int(round(f[0] + 0.7 * math.sin(f[3])))))
            y = int(f[1])
            if 0 <= y < HEIGHT:
                grid[x, y] = max(grid[x, y], f[4])
        return grid


class Chevrons(_Look):
    """Arrow shapes (V) marching down the panel."""
    GAP = 8          # rows from one chevron to the next
    SPEED = 6.0      # rows per second

    def draw(self):
        grid = np.zeros((WIDTH, HEIGHT))
        shift = self.t * self.SPEED
        centre = (WIDTH - 1) / 2
        for x in range(WIDTH):
            arm = abs(x - centre)                       # 0 at the point, 4 at the edges
            for y in range(HEIGHT):
                # Position along the repeating pattern, with the arms raised above the point
                p = (y + arm - shift) % self.GAP
                if p < 2.0:
                    grid[x, y] = 1.0
                elif p < 3.0:
                    grid[x, y] = 0.25                   # a faint edge behind each chevron
        return grid


class BouncingBall(_Look):
    """A ball thrown around the panel, falling under gravity and bouncing off the edges, with a short tail."""
    GRAVITY = 60.0   # rows per second per second
    TAIL = 5

    def __init__(self, seed=None):
        super().__init__(seed)
        self.x, self.y = 0.0, 0.0
        self.vx, self.vy = 6.0, 0.0
        self.trail = []

    def step(self):
        dt = self.STEP
        self.vy += self.GRAVITY * dt
        self.x += self.vx * dt
        self.y += self.vy * dt
        if self.x < 0 or self.x > WIDTH - 1:
            self.x = min(WIDTH - 1.0, max(0.0, self.x))
            self.vx = -self.vx
        if self.y > HEIGHT - 1:
            self.y = HEIGHT - 1.0
            self.vy = -abs(self.vy) * 0.82
            if abs(self.vy) < 22:
                # Running out of bounce: throw it up again
                self.vy = -float(self.rng.uniform(42, 50))
                self.vx = math.copysign(float(self.rng.uniform(4, 9)), self.vx)
        if self.y < 0:
            self.y = 0.0
            self.vy = abs(self.vy)
        self.trail.append((int(round(self.x)), int(round(self.y))))
        del self.trail[:-self.TAIL]

    def draw(self):
        grid = np.zeros((WIDTH, HEIGHT))
        for i, (x, y) in enumerate(self.trail):
            grid[x, y] = max(grid[x, y], 0.12 + 0.88 * ((i + 1) / len(self.trail)) ** 2)
        return grid


# (setting value, name shown in the window, what it shows, class)
LOOKS = [
    ("fire", "Fire", "Flames flickering up from the bottom of the panel.", Fire),
    ("plasma", "Plasma", "Soft blobs of light drifting and merging.", Plasma),
    ("dna", "DNA helix", "Two strands twisting around each other.", DnaHelix),
    ("snow", "Snowfall", "Snowflakes drifting down and settling at the bottom.", Snowfall),
    ("chevrons", "Chevrons", "Arrow shapes marching down the panel.", Chevrons),
    ("ball", "Bouncing ball", "A ball bouncing around the panel with a short tail.", BouncingBall),
]
LOOK_KEYS = tuple(k for k, _, _, _ in LOOKS)
LOOK_LABELS = {k: label for k, label, _, _ in LOOKS}
_CLASSES = {k: cls for k, _, _, cls in LOOKS}


def make_look(key, seed=None):
    """A new animation by its setting value (unknown values give Fire)."""
    return _CLASSES.get(key, Fire)(seed)


def to_leds(frame, foreground_value):
    """Brightness 0.0-1.0 to LED values, keeping faint LEDs just visible."""
    values = np.rint(np.asarray(frame, dtype=float) * max(1, int(foreground_value))).astype(int)
    values = np.where(np.asarray(frame) > 0.02, np.maximum(values, 1), 0)
    return np.clip(values, 0, 255)


# Old-TV switch off and on -------------------------------------------------------------------------

TV_FRAME_SECONDS = 0.04
_SQUEEZE = (0.72, 0.48, 0.3, 0.16, 0.07)     # picture height, as a share of the panel, while squeezing
_SHRINK = (4, 3, 2, 1, 0)                      # half-width of the bright line, in LEDs
_FADE = (1.0, 0.7, 0.45, 0.25, 0.1)            # the last dot fading out


def _squeezed(grid, share):
    """The picture squashed towards the middle row, as an old TV does when it turns off."""
    grid = np.asarray(grid, dtype=float)
    out = np.zeros((WIDTH, HEIGHT))
    centre = (HEIGHT - 1) / 2
    for y in range(HEIGHT):
        src = centre + (y - centre) / share
        if 0 <= src <= HEIGHT - 1:
            out[:, y] = grid[:, int(round(src))]
    # Squeezing the same light into fewer rows makes it brighter
    return out * min(2.0, 1.0 / math.sqrt(share))


def _line(brightness, half_width, row=HEIGHT // 2):
    out = np.zeros((WIDTH, HEIGHT))
    centre = WIDTH // 2
    out[centre - half_width:centre + half_width + 1, row] = brightness
    return out


def tv_off_frames(grid, foreground_value):
    """The frames of an old TV turning off, from the picture now on the panel (9 x 34 LED values).

    The picture squeezes into a bright line across the middle, the line shrinks to a dot, and the dot
    fades. Ends with a dark frame. If nothing is known about the picture, it starts at the line."""
    bright = max(1, min(255, int(foreground_value)))
    frames = []
    if grid is not None and np.asarray(grid).any():
        for share in _SQUEEZE:
            frames.append(np.clip(np.rint(_squeezed(grid, share)), 0, 255).astype(int))
    for half_width in _SHRINK:
        frames.append(_line(bright, half_width).astype(int))
    for fade in _FADE[1:]:
        frames.append(_line(max(1, int(bright * fade)), 0).astype(int))
    frames.append(np.zeros((WIDTH, HEIGHT), dtype=int))
    return frames


def tv_on_frames(grid, foreground_value):
    """The frames of an old TV turning on into the given picture: a dot, a line, then the picture opening
    out from the middle. The last frame is the picture itself."""
    bright = max(1, min(255, int(foreground_value)))
    frames = [_line(max(1, int(bright * fade)), 0).astype(int) for fade in reversed(_FADE[1:3])]
    frames += [_line(bright, half_width).astype(int) for half_width in reversed(_SHRINK[:-1])]
    if grid is not None and np.asarray(grid).any():
        frames += [np.clip(np.rint(_squeezed(grid, share)), 0, 255).astype(int) for share in reversed(_SQUEEZE)]
    frames.append(np.clip(np.asarray(grid if grid is not None else np.zeros((WIDTH, HEIGHT))), 0, 255).astype(int))
    return frames
