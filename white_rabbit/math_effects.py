# More maths for the Math tab: Langton's Ant, Collatz, primes, Lorenz, double pendulum, sorting, mazes,
# Fourier epicycles, random-walk frost, digits of pi and e, and Lissajous curves.
#
# Like math_sims.py, each one is a generator that yields frames forever: numpy arrays of brightness
# 0-255, 34 rows tall and `width` columns wide (9 for one panel, 18 for both side by side).
# Everything here is written from the textbook definitions.

# Built In Dependencies
import math

# External Dependencies
import numpy as np

HEIGHT = 34


def _bytes(img):
    return (np.clip(img, 0.0, 1.0) * 255).astype(np.uint8)


def _splat(img, x, y, v=1.0):
    """Add a soft dot at a point between LEDs, shared by the 4 LEDs around it."""
    h, w = img.shape
    x0, y0 = int(math.floor(x)), int(math.floor(y))
    fx, fy = x - x0, y - y0
    for dx, dy, share in ((0, 0, (1 - fx) * (1 - fy)), (1, 0, fx * (1 - fy)),
                          (0, 1, (1 - fx) * fy), (1, 1, fx * fy)):
        xx, yy = x0 + dx, y0 + dy
        if 0 <= xx < w and 0 <= yy < h:
            img[yy, xx] = min(1.0, img[yy, xx] + v * share * 1.6)


def _line(img, x0, y0, x1, y1, v):
    n = int(max(abs(x1 - x0), abs(y1 - y0)) * 2) + 1
    for i in range(n + 1):
        t = i / n
        _splat(img, x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, v / 2)


# Langton's Ant -------------------------------------------------------------------------------------

def langton_frames(rng, width, steps_each=330):
    """An ant turns right on a dark square and left on a lit one, flipping it as it leaves.

    The edges wrap around. It starts over now and then, speeding up as it goes."""
    turns = [(-1, 0), (0, 1), (1, 0), (0, -1)]
    while True:
        grid = np.zeros((HEIGHT, width), np.uint8)
        y, x, d = HEIGHT // 2, width // 2, int(rng.integers(4))
        for f in range(steps_each):
            for _ in range(1 if f < 60 else 3 if f < 140 else 12):
                d = (d + 1) % 4 if grid[y, x] == 0 else (d - 1) % 4
                grid[y, x] ^= 1
                y, x = (y + turns[d][0]) % HEIGHT, (x + turns[d][1]) % width
            img = grid * 0.45
            img[y, x] = 1.0
            yield _bytes(img)


# Collatz -------------------------------------------------------------------------------------------

def collatz_sequence(n):
    seq = [n]
    while seq[-1] != 1:
        n = seq[-1]
        seq.append(n // 2 if n % 2 == 0 else 3 * n + 1)
    return seq


def collatz_frames(rng, width):
    """Halve even numbers, triple odd ones and add 1, until reaching 1. Each number is a bar."""
    starts = [27, 97, 871, 703, 6171]
    i = 0
    while True:
        seq = collatz_sequence(starts[i] if i < len(starts) else int(rng.integers(30, 3000)))
        i += 1
        top = max(seq)
        bars = []
        for n in seq + [1] * width:
            bars.append(max(1.0, HEIGHT * (n / top) ** 0.35))
            shown = bars[-width:]
            img = np.zeros((HEIGHT, width))
            for c, h in enumerate(shown):
                col = width - len(shown) + c
                full = int(h)
                img[HEIGHT - full:, col] = 0.55
                if full < HEIGHT:
                    img[HEIGHT - full - 1, col] = (h - full) * 0.55
            img[HEIGHT - int(shown[-1]):, width - 1] = 1.0
            yield _bytes(img)


# Primes --------------------------------------------------------------------------------------------

def ulam_positions(width, count):
    """Where each number 1..count lands on a square spiral starting in the middle of the picture."""
    x, y = width // 2 - (1 if width % 2 == 0 else 0), HEIGHT // 2
    pos = [None, (x, y)]
    turns = [(1, 0), (0, -1), (-1, 0), (0, 1)]
    step, d = 1, 0
    while len(pos) <= count:
        for _ in range(2):
            for _ in range(step):
                x, y = x + turns[d][0], y + turns[d][1]
                pos.append((x, y))
            d = (d + 1) % 4
        step += 1
    return pos[:count + 1]


def is_prime_table(n):
    table = np.ones(n + 1, bool)
    table[:2] = False
    for p in range(2, int(n ** 0.5) + 1):
        if table[p]:
            table[p * p::p] = False
    return table


def primes_frames(rng, width):
    """The sieve of Eratosthenes on every LED (numbered left to right, top to bottom), then the
    primes again on a spiral from the middle (the Ulam spiral)."""
    n = HEIGHT * width
    while True:
        state = np.ones(n + 1)
        state[:2] = 0

        def frame(current=None, crossing=()):
            img = state[1:].reshape(HEIGHT, width) * 0.5
            for k in crossing:
                img[(k - 1) // width, (k - 1) % width] = 0.18
            if current:
                img[(current - 1) // width, (current - 1) % width] = 1.0
            return _bytes(img)

        for _ in range(8):
            yield frame()
        for p in range(2, int(n ** 0.5) + 1):
            if not state[p]:
                continue
            multiples = list(range(p * p, n + 1, p))
            chunk = max(1, len(multiples) // 10)
            for i in range(0, len(multiples), chunk):
                part = multiples[i:i + chunk]
                yield frame(p, part)
                state[part] = 0
            yield frame(p)
        for _ in range(20):
            yield _bytes(state[1:].reshape(HEIGHT, width))

        count = (max(HEIGHT, width) + 2) ** 2
        prime = is_prime_table(count)
        img = np.zeros((HEIGHT, width))
        for k, (x, y) in enumerate(ulam_positions(width, count)[1:], start=1):
            if 0 <= x < width and 0 <= y < HEIGHT:
                img[y, x] = 1.0 if prime[k] else 0.3 if k == 1 else 0.0
            if k % 25 == 0:
                yield _bytes(img)
        for _ in range(40):
            yield _bytes(img)


# Lorenz attractor ----------------------------------------------------------------------------------

def lorenz_frames(rng, width, sigma=10.0, rho=28.0, beta=8 / 3):
    """A dot following the Lorenz weather equations, leaving a fading trail (the butterfly)."""
    x, y, z = 1.0 + rng.random(), 1.0, 20.0
    trail = np.zeros((HEIGHT, width))
    dt = 0.004
    while True:
        trail *= 0.965
        for _ in range(4):
            for _ in range(2):
                dx, dy, dz = sigma * (y - x), x * (rho - z) - y, x * y - beta * z
                x, y, z = x + dx * dt, y + dy * dt, z + dz * dt
            px = (x + 21) / 42 * (width - 1)
            py = (HEIGHT - 1) - (z - 2) / 50 * (HEIGHT - 1)
            _splat(trail, px, py, 0.3)
        img = trail * 0.75
        _splat(img, px, py, 1.0)
        yield _bytes(img)


# Double pendulum -----------------------------------------------------------------------------------

def pendulum_step(a1, a2, w1, w2, dt, g=9.8):
    """One small step of a double pendulum with equal arms and weights."""
    d = a1 - a2
    den = 3 - math.cos(2 * d)
    acc1 = (-3 * g * math.sin(a1) - g * math.sin(a1 - 2 * a2)
            - 2 * math.sin(d) * (w2 * w2 + w1 * w1 * math.cos(d))) / den
    acc2 = 2 * math.sin(d) * (2 * w1 * w1 + 2 * g * math.cos(a1) + w2 * w2 * math.cos(d)) / den
    w1, w2 = w1 + acc1 * dt, w2 + acc2 * dt
    return a1 + w1 * dt, a2 + w2 * dt, w1, w2


def pendulum_frames(rng, width):
    """A pendulum hanging from another pendulum. The end leaves a trail and never repeats."""
    a1, a2 = 2.6 + rng.random() * 0.2, 2.2,
    w1 = w2 = 0.0
    cx, cy = (width - 1) / 2, (HEIGHT - 1) / 2
    arm = min(4.2, (width - 1) / 4)
    trail = np.zeros((HEIGHT, width))
    while True:
        trail *= 0.92
        for _ in range(10):
            a1, a2, w1, w2 = pendulum_step(a1, a2, w1, w2, 0.006)
            x1, y1 = cx + arm * math.sin(a1), cy + arm * math.cos(a1)
            x2, y2 = x1 + arm * math.sin(a2), y1 + arm * math.cos(a2)
            _splat(trail, x2, y2, 0.12)
        img = trail * 0.8
        _line(img, cx, cy, x1, y1, 0.35)
        _line(img, x1, y1, x2, y2, 0.35)
        _splat(img, cx, cy, 0.4)
        _splat(img, x1, y1, 0.8)
        _splat(img, x2, y2, 1.0)
        yield _bytes(img)


# Sorting -------------------------------------------------------------------------------------------

def bubble_sort(a):
    n = len(a)
    for i in range(n):
        for j in range(n - 1 - i):
            yield (j, j + 1)
            if a[j] > a[j + 1]:
                a[j], a[j + 1] = a[j + 1], a[j]
                yield (j, j + 1)


def quick_sort(a, lo=0, hi=None):
    if hi is None:
        hi = len(a) - 1
    if lo >= hi:
        return
    pivot, i = a[hi], lo
    for j in range(lo, hi):
        yield (j, hi)
        if a[j] < pivot:
            a[i], a[j] = a[j], a[i]
            yield (i, j)
            i += 1
    a[i], a[hi] = a[hi], a[i]
    yield (i, hi)
    yield from quick_sort(a, lo, i - 1)
    yield from quick_sort(a, i + 1, hi)


def merge_sort(a, lo=0, hi=None):
    if hi is None:
        hi = len(a)
    if hi - lo < 2:
        return
    mid = (lo + hi) // 2
    yield from merge_sort(a, lo, mid)
    yield from merge_sort(a, mid, hi)
    left, right = a[lo:mid], a[mid:hi]
    i = j = 0
    for k in range(lo, hi):
        if j >= len(right) or (i < len(left) and left[i] <= right[j]):
            a[k] = left[i]
            i += 1
        else:
            a[k] = right[j]
            j += 1
        yield (k,)


SORTS = [("Bubble sort", bubble_sort), ("Quick sort", quick_sort), ("Merge sort", merge_sort)]


def sorting_frames(rng, width, on_change=None):
    """Bars of different heights put in order by bubble sort, then quick sort, then merge sort.
    The bars being compared or moved are bright."""
    def bars(a, bright=()):
        img = np.zeros((HEIGHT, width))
        for i, v in enumerate(a):
            img[HEIGHT - v:, i] = 1.0 if i in bright else 0.45
        return _bytes(img)

    while True:
        for name, sort in SORTS:
            if on_change:
                on_change(name)
            a = [int(v) for v in rng.permutation(np.linspace(2, HEIGHT, width).round().astype(int))]
            for _ in range(6):
                yield bars(a)
            for bright in sort(a):
                yield bars(a, bright)
            for i in range(width):
                yield bars(a, tuple(range(i + 1)))
            for _ in range(10):
                yield bars(a, tuple(range(width)))


# Maze ----------------------------------------------------------------------------------------------

def maze_frames(rng, width):
    """Carves a maze by wandering and backing up from dead ends, then floods it from the top-left
    corner until the flood reaches the bottom-right, and lights the shortest path."""
    cw, ch = (width - 1) // 2, (HEIGHT - 1) // 2
    steps = ((-1, 0), (1, 0), (0, -1), (0, 1))
    while True:
        wall = np.ones((HEIGHT, width))
        seen = np.zeros((ch, cw), bool)
        stack = [(0, 0)]
        seen[0, 0] = True
        wall[1, 1] = 0
        k = 0
        while stack:
            cy, cx = stack[-1]
            options = [(cy + dy, cx + dx) for dy, dx in steps
                       if 0 <= cy + dy < ch and 0 <= cx + dx < cw and not seen[cy + dy, cx + dx]]
            if options:
                ny, nx = options[int(rng.integers(len(options)))]
                seen[ny, nx] = True
                wall[cy + ny + 1, cx + nx + 1] = 0
                wall[2 * ny + 1, 2 * nx + 1] = 0
                stack.append((ny, nx))
            else:
                stack.pop()
            k += 1
            if k % 2 == 0:
                img = wall * 0.4
                if stack:
                    img[2 * stack[-1][0] + 1, 2 * stack[-1][1] + 1] = 1.0
                yield _bytes(img)

        start, goal = (1, 1), (2 * ch - 1, 2 * cw - 1)
        dist, came_from, edge = {start: 0}, {}, [start]
        while edge and goal not in dist:
            new_edge = []
            for y, x in edge:
                for dy, dx in steps:
                    p = (y + dy, x + dx)
                    if wall[p] == 0 and p not in dist:
                        dist[p] = dist[(y, x)] + 1
                        came_from[p] = (y, x)
                        new_edge.append(p)
            edge = new_edge
            img = wall * 0.4
            for p in dist:
                img[p] = 0.15
            for p in edge:
                img[p] = 0.7
            yield _bytes(img)
        path = [goal]
        while path[-1] != start:
            path.append(came_from[path[-1]])
        path.reverse()
        for i in range(0, len(path) + 1, 2):
            img = wall * 0.4
            for p in dist:
                img[p] = 0.12
            for p in path[:i + 1]:
                img[p] = 1.0
            yield _bytes(img)
        for _ in range(25):
            yield _bytes(img)


# Fourier epicycles ---------------------------------------------------------------------------------

# The outline of a sitting rabbit, drawn for a picture 18 wide and 34 tall.
RABBIT = [(7.5, 11), (6.2, 5), (6.3, 1.5), (7.6, 1.2), (8.5, 4), (8.8, 9.5), (9.6, 9.5), (10, 4),
          (11, 1.3), (12.2, 1.8), (12, 5.5), (11, 10.5), (12.6, 12.5), (13.5, 15), (12.5, 17.5),
          (13.6, 20), (15, 24), (15.4, 28), (16.6, 29.5), (16.2, 31.8), (14.5, 32.5), (9, 32.6),
          (5.5, 32.5), (4.2, 31.2), (5, 29.8), (6.3, 28.6), (6.6, 25), (7.6, 21), (6, 18.5),
          (4.6, 16.2), (4.8, 13.6)]


def _outline(points, n):
    """n points evenly spaced along a closed outline, as complex numbers x + iy."""
    pts = np.array(points + [points[0]], float)
    lengths = np.concatenate([[0], np.cumsum(np.hypot(*np.diff(pts, axis=0).T))])
    at = np.linspace(0, lengths[-1], n, endpoint=False)
    return np.interp(at, lengths, pts[:, 0]) + 1j * np.interp(at, lengths, pts[:, 1])


def epicycle_frames(rng, width, circles=24, steps=220):
    """Spinning circles on spinning circles, whose tip draws the White Rabbit."""
    samples = 256
    path = _outline([(x * width / 18, y) for x, y in RABBIT], samples)
    coeffs = np.fft.fft(path) / samples
    freqs = np.fft.fftfreq(samples, 1 / samples).astype(int)
    order = [i for i in np.argsort(-np.abs(coeffs)) if freqs[i] != 0][:circles]
    centre = coeffs[0]
    while True:
        drawn = []
        for s in range(steps + 40):
            img = np.zeros((HEIGHT, width))
            if s <= steps:
                t = s / steps
                pos = centre
                for i in order:
                    r = abs(coeffs[i])
                    if r > 0.6:
                        for a in np.linspace(0, 2 * math.pi, int(r * 8) + 6, endpoint=False):
                            _splat(img, pos.real + r * math.cos(a), pos.imag + r * math.sin(a), 0.06)
                    nxt = pos + coeffs[i] * np.exp(2j * math.pi * freqs[i] * t)
                    if r > 0.3:
                        _line(img, pos.real, pos.imag, nxt.real, nxt.imag, 0.25)
                    pos = nxt
                drawn.append(pos)
                _splat(img, pos.real, pos.imag, 1.0)
            for p in drawn:
                _splat(img, p.real, p.imag, 0.7)
            yield _bytes(img)


# Random walk frost (diffusion-limited aggregation) -------------------------------------------------

def frost_frames(rng, width, walkers=14):
    """Specks wander at random, drifting down, and freeze when they touch the frost, which grows up
    from the bottom edge in branches. Starts over when it nearly reaches the top."""
    while True:
        stuck = np.zeros((HEIGHT, width), bool)
        stuck[HEIGHT - 1, width // 2 - 1:width // 2 + 1] = True
        specks = [[int(rng.integers(width)), int(rng.integers(0, 6))] for _ in range(walkers)]
        glow = np.zeros((HEIGHT, width))
        frames = 0
        while not stuck[3].any() and frames < 900:
            for _ in range(3):
                for s in specks:
                    s[0] = (s[0] + int(rng.integers(-1, 2))) % width
                    drift = 1 if rng.random() < 0.15 else 0
                    s[1] = min(HEIGHT - 1, max(0, s[1] + int(rng.integers(-1, 2)) + drift))
                    x, y = s
                    if not stuck[y, x] and stuck[max(0, y - 1):y + 2, max(0, x - 1):x + 2].any():
                        stuck[y, x] = True
                        glow[y, x] = 1.0
                        s[0], s[1] = int(rng.integers(width)), 0
            glow = np.maximum(glow * 0.9, stuck * 0.5)
            img = glow.copy()
            for x, y in specks:
                if not stuck[y, x]:
                    img[y, x] = max(img[y, x], 0.25)
            frames += 1
            yield _bytes(img)
        for _ in range(30):
            yield _bytes(stuck * 0.5)


# Digits of pi and e --------------------------------------------------------------------------------

def pi_digits(count):
    """The first digits of pi (3, 1, 4, 1, 5...), by Gibbons' spigot."""
    q, r, t, k, m, x = 1, 0, 1, 1, 3, 3
    out = []
    while len(out) < count:
        if 4 * q + r - t < m * t:
            out.append(m)
            q, r, m = 10 * q, 10 * (r - m * t), (10 * (3 * q + r)) // t - 10 * m
        else:
            q, r, t, k, m, x = q * k, (2 * q + r) * x, t * x, k + 1, (q * (7 * k + 2) + r * x) // (t * x), x + 2
    return out


def e_digits(count):
    """The first digits of e (2, 7, 1, 8, 2, 8...), by the spigot method."""
    a = [1] * (count + 10)
    out = [2]
    for _ in range(count - 1):
        carry = 0
        for i in range(len(a) - 1, -1, -1):
            v = a[i] * 10 + carry
            a[i], carry = v % (i + 2), v // (i + 2)
        out.append(carry)
    return out


def digits_frames(rng, width, count=1000):
    """Digits stream down as bars, one digit per row: the bar is as many lights long as the digit.

    With both panels, pi is on the left (growing out from the keyboard) and e is on the right;
    with one panel it's just pi."""
    pi, e = pi_digits(count), e_digits(count)
    half = width // 2 if width >= 18 else 0
    rows = np.zeros((HEIGHT, width))
    i = 0
    while True:
        row = np.zeros(width)
        if half:
            row[half - pi[i % count]:half] = 0.6
            row[half:half + e[i % count]] = 0.6
        else:
            row[:pi[i % count]] = 0.6
        rows = np.roll(rows, 1, axis=0)
        rows[0] = row
        img = rows.copy()
        img[0][img[0] > 0] = 1.0
        yield _bytes(img)
        i += 1


# Lissajous curves ----------------------------------------------------------------------------------

LISSAJOUS_RATIOS = [(1, 2), (3, 2), (3, 4), (5, 4), (1, 3)]


def lissajous_frames(rng, width, frames_each=140):
    """A dot swinging side to side and up and down at different speeds, drawing a looping figure
    that slowly turns. Moves on to the next pair of speeds every so often."""
    trail = np.zeros((HEIGHT, width))
    cx, cy = (width - 1) / 2, (HEIGHT - 1) / 2
    rx, ry = cx - 0.3, cy - 0.3
    t = 0.0
    while True:
        for a, b in LISSAJOUS_RATIOS:
            for f in range(frames_each):
                trail *= 0.93
                shift = f * 0.012
                for _ in range(10):
                    t += 0.012
                    x, y = cx + rx * math.sin(a * t + shift), cy + ry * math.sin(b * t)
                    _splat(trail, x, y, 0.18)
                img = trail * 0.85
                _splat(img, x, y, 1.0)
                yield _bytes(img)


# name, frames(rng, width), suggested steps per second, what it is
EFFECTS = [
    ("Langton's Ant", langton_frames, 15,
     "An ant turns right on dark squares and left on lit ones, flipping each square it leaves."),
    ("Collatz sequences", collatz_frames, 10,
     "Halve even numbers, triple odd ones and add 1. The bars bounce until they reach 1."),
    ("Prime sieve and spiral", primes_frames, 15,
     "Crosses out the multiples of each prime, then lays the primes out on a spiral (the Ulam spiral)."),
    ("Lorenz attractor", lorenz_frames, 20,
     "A dot following the Lorenz weather equations, tracing the butterfly."),
    ("Double pendulum", pendulum_frames, 20,
     "A pendulum swinging from another pendulum. It never repeats."),
    ("Sorting", sorting_frames, 20,
     "Bubble sort, quick sort and merge sort putting bars in order."),
    ("Maze: build and solve", maze_frames, 15,
     "Carves a maze, then floods it from one corner and lights the shortest way through."),
    ("Fourier circles draw a rabbit", epicycle_frames, 15,
     "Spinning circles on spinning circles, whose tip draws the White Rabbit."),
    ("Random walk frost", frost_frames, 15,
     "Wandering specks freeze when they touch the frost, which grows up in branches."),
    ("Digits of pi and e", digits_frames, 8,
     "Each digit is a bar that many lights long. Pi on the left, e on the right."),
    ("Lissajous curves", lissajous_frames, 20,
     "A dot swinging two ways at once, drawing a looping figure that slowly turns."),
]
EFFECT_NAMES = [e[0] for e in EFFECTS]
