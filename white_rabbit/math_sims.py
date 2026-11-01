# Math graphs and simulations for the LED panels, used by the Math tab.
#
# Each animation is a generator that yields frames forever: numpy arrays of brightness values 0-255,
# 34 rows tall and 9 columns wide per panel (18 wide for both panels side by side).
#
# The ideas (graphs of functions, Game of Life and other Life-like rules, the Biham-Middleton-Levine
# traffic model, the Hardy-Pomeau-Pazzis lattice gas, one-dimensional automata, and seeding the
# simulations from a graph) come from mariobx's Framework-LED-Matrix project
# (https://github.com/mariobx/Framework-LED-Matrix). This file is written from scratch from the
# textbook definitions; no code was copied.

# Built In Dependencies
import ast
import math

# External Dependencies
import numpy as np

HEIGHT = 34
PANEL_WIDTH = 9

OFF = 0
DIM = 45
MID = 120
FULL = 255


# Formulas ------------------------------------------------------------------------------------------

def _safe_log(x):
    with np.errstate(all="ignore"):
        return np.where(x > 0, np.log(np.where(x > 0, x, 1)), np.nan)


def _safe_sqrt(x):
    with np.errstate(all="ignore"):
        return np.where(x >= 0, np.sqrt(np.where(x >= 0, x, 0)), np.nan)


def _safe_sinc(x):
    with np.errstate(all="ignore"):
        return np.where(x == 0, 1.0, np.sin(x) / np.where(x == 0, 1, x))


# What a formula may use. Everything else is refused.
FORMULA_FUNCTIONS = {
    "sin": np.sin, "cos": np.cos, "tan": np.tan,
    "asin": np.arcsin, "acos": np.arccos, "atan": np.arctan,
    "sinh": np.sinh, "cosh": np.cosh, "tanh": np.tanh,
    "exp": np.exp, "log": _safe_log, "ln": _safe_log, "log10": lambda x: _safe_log(x) / math.log(10),
    "sqrt": _safe_sqrt, "abs": np.abs, "floor": np.floor, "ceil": np.ceil, "round": np.round,
    "sign": np.sign, "sinc": _safe_sinc,
    "min": np.minimum, "max": np.maximum,
}
TWO_ARGUMENT_FUNCTIONS = {"min", "max"}
FORMULA_NAMES = {"pi": math.pi, "e": math.e}
FORMULA_VARIABLES = ("x", "t")
_OPERATORS = (ast.Add, ast.Sub, ast.Mult, ast.Div, ast.Pow, ast.Mod, ast.FloorDiv, ast.USub, ast.UAdd)


class FormulaError(ValueError):
    pass


class _FloatConstants(ast.NodeTransformer):
    """Numbers become floats, so something like 9**9**9 overflows at once instead of running forever."""

    def visit_Constant(self, node):
        return ast.copy_location(ast.Constant(float(node.value)), node)


def compile_formula(text):
    """Turn text like 'sin(x) * x^2' into a function of numpy arrays f(x, t). Raises FormulaError."""
    source = (text or "").strip().replace("^", "**")
    if not source:
        raise FormulaError("Type a formula, for example sin(x).")
    if len(source) > 200:
        raise FormulaError("That formula is too long.")
    try:
        tree = ast.parse(source, mode="eval")
    except SyntaxError:
        raise FormulaError("That formula has a typo in it.")
    for node in ast.walk(tree):
        if isinstance(node, (ast.Expression, ast.Load)) or isinstance(node, _OPERATORS):
            continue
        if isinstance(node, ast.BinOp) or isinstance(node, ast.UnaryOp):
            continue
        if isinstance(node, ast.Constant):
            if isinstance(node.value, bool) or not isinstance(node.value, (int, float)):
                raise FormulaError("Only numbers can be used in a formula.")
            continue
        if isinstance(node, ast.Name):
            if node.id not in FORMULA_FUNCTIONS and node.id not in FORMULA_NAMES and node.id not in FORMULA_VARIABLES:
                raise FormulaError(f"I don't know '{node.id}'. Use x, t, pi, e and functions like sin.")
            continue
        if isinstance(node, ast.Call):
            if not isinstance(node.func, ast.Name) or node.func.id not in FORMULA_FUNCTIONS or node.keywords:
                raise FormulaError("Only functions like sin(x) or sqrt(x) can be called.")
            wanted = 2 if node.func.id in TWO_ARGUMENT_FUNCTIONS else 1
            if len(node.args) != wanted or any(isinstance(a, ast.Starred) for a in node.args):
                raise FormulaError(f"{node.func.id}() takes {wanted} number{'s' if wanted > 1 else ''}, "
                                   f"like {node.func.id}({'x, 1' if wanted > 1 else 'x'}).")
            continue
        raise FormulaError("Only numbers, x, t and maths can be used in a formula.")
    tree = ast.fix_missing_locations(_FloatConstants().visit(tree))
    code = compile(tree, "<formula>", "eval")
    namespace = {"__builtins__": {}, **FORMULA_FUNCTIONS, **FORMULA_NAMES}

    def f(x, t=0.0):
        with np.errstate(all="ignore"):
            try:
                y = eval(code, namespace, {"x": x, "t": t})
            except (OverflowError, ZeroDivisionError, ValueError, FloatingPointError):
                return np.full_like(x, np.nan, dtype=float)
        y = np.asarray(y, dtype=float)
        return np.broadcast_to(y, np.shape(x)).astype(float)
    # Check it once now so mistakes like sin(x, x) show up straight away
    try:
        f(np.linspace(-1, 1, 5))
    except Exception as e:
        raise FormulaError(f"That formula doesn't work: {e}")
    return f


def _names_in(formula):
    try:
        return {n.id for n in ast.walk(ast.parse(formula.replace("^", "**"), mode="eval")) if isinstance(n, ast.Name)}
    except SyntaxError:
        return set()


# (name, formula, x from, x to, y range or None to fit the graph)
GRAPHS = [
    ("Sine", "sin(x)", -2 * math.pi, 2 * math.pi, (-1.2, 1.2)),
    ("Cosine", "cos(x)", -2 * math.pi, 2 * math.pi, (-1.2, 1.2)),
    ("Tangent", "tan(x)", -math.pi, math.pi, (-4, 4)),
    ("Parabola", "x^2", -3, 3, None),
    ("Cubic", "x^3 - 3*x", -2.5, 2.5, None),
    ("Exponential", "exp(x)", -3, 2, None),
    ("Logarithm", "log(x)", 0.05, 6, None),
    ("Square root", "sqrt(x)", 0, 9, None),
    ("Tanh", "tanh(x)", -4, 4, (-1.2, 1.2)),
    ("Sinc", "sinc(x)", -12, 12, (-0.4, 1.1)),
    ("Bell curve", "exp(-x^2)", -3, 3, (-0.1, 1.1)),
    ("Absolute value", "abs(x)", -3, 3, None),
    ("Damped wave", "exp(-x/4) * sin(2*x)", 0, 12, (-1, 1)),
    ("Beats", "sin(5*x) + sin(5.6*x)", 0, 12, (-2.1, 2.1)),
    ("1/x", "1/x", -4, 4, (-4, 4)),
    ("Arcsin", "asin(x)", -1, 1, None),
    ("Steps", "floor(x)", -4, 4, None),
    ("Sawtooth", "x - floor(x)", -3, 3, (-0.2, 1.2)),
    ("Moving wave", "sin(x - t)", -2 * math.pi, 2 * math.pi, (-1.2, 1.2)),
    ("Breathing wave", "sin(t) * sin(x)", -2 * math.pi, 2 * math.pi, (-1.2, 1.2)),
]
GRAPH_NAMES = [g[0] for g in GRAPHS]
# Moving graphs need t, so they aren't good starting pictures for the simulations
STILL_GRAPHS = [g for g in GRAPHS if "t" not in _names_in(g[1])]


def fit_range(y):
    """A y range that shows most of the graph, ignoring the odd point that shoots off."""
    finite = y[np.isfinite(y)]
    if finite.size == 0:
        return (-1.0, 1.0)
    low, high = np.percentile(finite, [2, 98])
    # Show the whole graph unless a few points shoot far off (like 1/x near 0)
    if finite.max() - finite.min() <= 3 * (high - low):
        low, high = finite.min(), finite.max()
    if high - low < 1e-9:
        low, high = low - 1, high + 1
    pad = (high - low) * 0.08
    return (float(low - pad), float(high + pad))


def draw_graph(f, x_from, x_to, y_range=None, width=PANEL_WIDTH, t=0.0, axes=True, samples_per_row=16):
    """A picture of y = f(x). x runs down the panel (one row per step), y runs across it."""
    frame = np.zeros((HEIGHT, width), dtype=np.uint8)
    xs = np.linspace(x_from, x_to, HEIGHT * samples_per_row + 1)
    ys = f(xs, t)
    low, high = y_range if y_range else fit_range(ys)
    span = high - low

    def column(y):
        return int(round((y - low) / span * (width - 1)))

    if axes:
        if low <= 0 <= high:
            frame[:, column(0)] = DIM
        if x_from <= 0 <= x_to:
            frame[int(round((0 - x_from) / (x_to - x_from) * (HEIGHT - 1))), :] = DIM

    for row in range(HEIGHT):
        part = slice(row * samples_per_row, (row + 1) * samples_per_row + 1)
        segment = []
        for y in list(ys[part]) + [None]:
            usable = y is not None and math.isfinite(y) and low <= y <= high
            # A jump of more than half the height is a break (like tan), not a steep line
            jump = usable and segment and abs(y - segment[-1]) > span / 2
            if not usable or jump:
                if segment:
                    a, b = column(min(segment)), column(max(segment))
                    frame[row, a:b + 1] = FULL
                segment = [y] if usable else []
            else:
                segment.append(y)
    return frame


def graph_by_name(name):
    for graph in GRAPHS:
        if graph[0] == name:
            return graph
    raise KeyError(name)


def graph_frames(formula, x_from, x_to, y_range=None, width=PANEL_WIDTH, axes=True, speed=0.15):
    """One graph. If the formula uses t, it moves."""
    f = compile_formula(formula)
    moving = "t" in _names_in(formula)
    t = 0.0
    while True:
        yield draw_graph(f, x_from, x_to, y_range, width, t, axes)
        if moving:
            t += speed


def random_graph_picture(rng, width=PANEL_WIDTH, axes=False):
    name, formula, x_from, x_to, y_range = STILL_GRAPHS[rng.integers(len(STILL_GRAPHS))]
    return name, draw_graph(compile_formula(formula), x_from, x_to, y_range, width, axes=axes)


def random_graphs_frames(rng, width=PANEL_WIDTH, frames_each=20, axes=True, on_change=None):
    """A different graph every few seconds."""
    last = None
    while True:
        choices = [g for g in GRAPHS if g[0] != last]
        name, formula, x_from, x_to, y_range = choices[rng.integers(len(choices))]
        last = name
        if on_change:
            on_change(f"{name}: y = {formula}")
        frames = graph_frames(formula, x_from, x_to, y_range, width, axes)
        for _ in range(frames_each):
            yield next(frames)


# Starting pictures for the simulations -------------------------------------------------------------

# Well-known Game of Life patterns as (row, column) cells
LIFE_PATTERNS = {
    "Glider": [(0, 1), (1, 2), (2, 0), (2, 1), (2, 2)],
    "R-pentomino": [(0, 1), (0, 2), (1, 0), (1, 1), (2, 1)],
    "Acorn": [(0, 1), (1, 3), (2, 0), (2, 1), (2, 4), (2, 5), (2, 6)],
    "Diehard": [(0, 6), (1, 0), (1, 1), (2, 1), (2, 5), (2, 6), (2, 7)],
    "Lightweight spaceship": [(0, 1), (0, 4), (1, 0), (2, 0), (2, 4), (3, 0), (3, 1), (3, 2), (3, 3)],
    "Pentadecathlon": [(r, 1) for r in range(10) if r not in (2, 7)] + [(2, 0), (2, 2), (7, 0), (7, 2)],
    "Pulsar quarter": [(0, 2), (0, 3), (0, 4), (2, 0), (3, 0), (4, 0), (2, 5), (3, 5), (4, 5), (5, 2), (5, 3), (5, 4)],
}
STARTS = ["Random", "Math graph"] + list(LIFE_PATTERNS)


def place_pattern(cells, width):
    grid = np.zeros((HEIGHT, width), dtype=bool)
    rows = max(r for r, _ in cells) + 1
    cols = max(c for _, c in cells) + 1
    top, left = (HEIGHT - rows) // 2, max(0, (width - cols) // 2)
    for r, c in cells:
        if 0 <= left + c < width:
            grid[top + r, left + c] = True
    return grid


def starting_grid(start, rng, width, density=0.35, on_change=None):
    """A True/False picture to start a simulation from."""
    if start == "Random":
        return rng.random((HEIGHT, width)) < density
    if start == "Math graph":
        name, picture = random_graph_picture(rng, width)
        if on_change:
            on_change(f"Started from the {name.lower()} graph")
        return picture > 0
    return place_pattern(LIFE_PATTERNS[start], width)


# Game of Life and other Life-like rules ------------------------------------------------------------

# (name, birth counts, survive counts)
LIFE_RULES = [
    ("Conway's Game of Life (B3/S23)", "3", "23"),
    ("HighLife (B36/S23)", "36", "23"),
    ("Seeds (B2/S)", "2", ""),
    ("Day & Night (B3678/S34678)", "3678", "34678"),
    ("Maze (B3/S12345)", "3", "12345"),
    ("Replicator (B1357/S1357)", "1357", "1357"),
    ("Life without death (B3/S012345678)", "3", "012345678"),
    ("Diamoeba (B35678/S5678)", "35678", "5678"),
    ("2x2 (B36/S125)", "36", "125"),
]
LIFE_RULE_NAMES = [r[0] for r in LIFE_RULES]


def parse_counts(text):
    """'3,6' or '36' -> {3, 6}. Raises ValueError for anything that isn't neighbour counts 0-8."""
    counts = set()
    for ch in (text or "").replace(",", "").replace(" ", ""):
        if not ch.isdigit() or int(ch) > 8:
            raise ValueError("Use the numbers 0 to 8, like 3 or 23.")
        counts.add(int(ch))
    return counts


def neighbours(grid):
    """How many of the 8 cells around each cell are alive. The edges wrap around."""
    g = grid.astype(np.uint8)
    total = np.zeros_like(g)
    for dr in (-1, 0, 1):
        for dc in (-1, 0, 1):
            if dr or dc:
                total += np.roll(np.roll(g, dr, axis=0), dc, axis=1)
    return total


def life_step(grid, birth, survive):
    n = neighbours(grid)
    born = ~grid & np.isin(n, list(birth))
    stays = grid & np.isin(n, list(survive))
    return born | stays


def life_frames(rng, width=PANEL_WIDTH, birth=(3,), survive=(2, 3), start="Random", density=0.35,
                pause_frames=12, max_generations=600, on_change=None):
    """Runs a Life-like rule. When it dies out, settles, or loops, it pauses a moment and starts again."""
    birth, survive = set(birth), set(survive)
    while True:
        grid = starting_grid(start, rng, width, density, on_change)
        seen = {}
        for generation in range(max_generations):
            yield np.where(grid, FULL, OFF).astype(np.uint8)
            key = grid.tobytes()
            if not grid.any() or key in seen:
                break
            seen[key] = generation
            grid = life_step(grid, birth, survive)
        for _ in range(pause_frames):
            yield np.where(grid, FULL, OFF).astype(np.uint8)


# Biham-Middleton-Levine traffic ---------------------------------------------------------------------

BML_EMPTY, BML_ACROSS, BML_DOWN = 0, 1, 2


def bml_grid(rng, width, density):
    r = rng.random((HEIGHT, width))
    grid = np.zeros((HEIGHT, width), dtype=np.uint8)
    grid[r < density / 2] = BML_ACROSS
    grid[(r >= density / 2) & (r < density)] = BML_DOWN
    return grid


def bml_half_step(grid, kind):
    """Every car of one kind moves one cell (across or down, wrapping) if that cell is free."""
    axis = 1 if kind == BML_ACROSS else 0
    cars = grid == kind
    ahead_free = np.roll(grid, -1, axis=axis) == BML_EMPTY
    moving = cars & ahead_free
    new = grid.copy()
    new[moving] = BML_EMPTY
    new[np.roll(moving, 1, axis=axis)] = kind
    return new, int(moving.sum())


def bml_frames(rng, width=PANEL_WIDTH, density=0.35, jam_pause=15, max_steps=2000):
    """Cars going across (bright) and cars going down (dim) take turns. A full jam starts it over."""
    while True:
        grid = bml_grid(rng, width, density)
        for step in range(max_steps):
            yield np.choose(grid, [OFF, FULL, MID]).astype(np.uint8)
            grid, moved_a = bml_half_step(grid, BML_ACROSS if step % 2 == 0 else BML_DOWN)
            if step % 2 == 0:
                first_moved = moved_a
            elif first_moved == 0 and moved_a == 0:
                break
        for _ in range(jam_pause):
            yield np.choose(grid, [OFF, FULL, MID]).astype(np.uint8)


# Hardy-Pomeau-Pazzis lattice gas -------------------------------------------------------------------

N, E, S, W = 1, 2, 4, 8


def hpp_step(grid):
    """Particles moving N/E/S/W. Two meeting head on leave at right angles. Walls bounce them back."""
    grid = grid.copy()
    ns = grid == (N | S)
    ew = grid == (E | W)
    grid[ns] = E | W
    grid[ew] = N | S
    new = np.zeros_like(grid)
    north, east, south, west = (grid & N) > 0, (grid & E) > 0, (grid & S) > 0, (grid & W) > 0
    # Moving north = towards row 0
    new[:-1][north[1:]] |= N
    new[0][north[0]] |= S
    new[1:][south[:-1]] |= S
    new[-1][south[-1]] |= N
    new[:, 1:][east[:, :-1]] |= E
    new[:, -1][east[:, -1]] |= W
    new[:, :-1][west[:, 1:]] |= W
    new[:, 0][west[:, 0]] |= E
    return new


HPP_STARTS = ["Random", "Gas in a corner", "Math graph"]
_HPP_BRIGHTNESS = np.array([OFF, 90, 160, 215, FULL], dtype=np.uint8)
_BITS = np.array([bin(i).count("1") for i in range(16)])


def hpp_grid(rng, width, start, density, on_change=None):
    if start == "Gas in a corner":
        where = np.zeros((HEIGHT, width), dtype=bool)
        where[:HEIGHT // 3, :max(3, width // 2)] = True
        density = max(density, 0.7)
    elif start == "Math graph":
        where = starting_grid("Math graph", rng, width, on_change=on_change)
        density = 1.0
    else:
        where = np.ones((HEIGHT, width), dtype=bool)
    grid = np.zeros((HEIGHT, width), dtype=np.uint8)
    for bit in (N, E, S, W):
        grid |= np.where(where & (rng.random((HEIGHT, width)) < density / 2), bit, 0).astype(np.uint8)
    if start == "Math graph":
        # Make sure every lit cell of the graph has at least one particle
        empty = where & (grid == 0)
        grid[empty] = rng.choice([N, E, S, W], size=int(empty.sum()))
    return grid


def hpp_frames(rng, width=PANEL_WIDTH, start="Random", density=0.3, steps_each=300, on_change=None):
    while True:
        grid = hpp_grid(rng, width, start, density, on_change)
        for _ in range(steps_each):
            yield _HPP_BRIGHTNESS[_BITS[grid]]
            grid = hpp_step(grid)


# One-dimensional automata, drawn one generation per row -------------------------------------------

AUTOMATA_KINDS = ["Elementary (rule 0-255)", "Totalistic, 3 colours (code 0-2186)"]
AUTOMATA_STARTS = ["Single cell", "Random"]


def elementary_step(row, rule):
    left, right = np.roll(row, 1), np.roll(row, -1)
    index = (left << 2) | (row << 1) | right
    return ((rule >> index) & 1).astype(np.uint8)


def totalistic_step(row, code):
    total = np.roll(row, 1) + row + np.roll(row, -1)
    digits = np.array([(code // 3 ** i) % 3 for i in range(7)], dtype=np.uint8)
    return digits[total]


def automaton_frames(rng, width=PANEL_WIDTH, kind=AUTOMATA_KINDS[0], number=30, start="Single cell",
                     rows_before_restart=400):
    """Each new generation appears at the bottom and the older ones scroll up.

    Once the rows start repeating, it runs one more screenful and starts again (with a new random row
    if it started from a random one)."""
    totalistic = kind == AUTOMATA_KINDS[1]
    colours = 3 if totalistic else 2
    shades = np.array([OFF, MID, FULL] if totalistic else [OFF, FULL], dtype=np.uint8)
    while True:
        if start == "Random":
            row = rng.integers(0, colours, width).astype(np.uint8)
        else:
            row = np.zeros(width, dtype=np.uint8)
            row[width // 2] = 1
        history = np.zeros((HEIGHT, width), dtype=np.uint8)
        seen = set()
        left = rows_before_restart
        while left > 0:
            history = np.roll(history, -1, axis=0)
            history[-1] = row
            yield shades[history]
            key = row.tobytes()
            if key in seen:
                left = min(left, HEIGHT)
            seen.add(key)
            left -= 1
            row = totalistic_step(row, number) if totalistic else elementary_step(row, number)


# Random grey ---------------------------------------------------------------------------------------

def noise_frames(rng, width=PANEL_WIDTH):
    while True:
        yield rng.integers(0, 256, (HEIGHT, width), dtype=np.uint16).astype(np.uint8)
