# The "Battery (sand)" app: the battery level as a pile of sand in a half panel.
# Charging rains grains in from the top until the pile matches the battery level;
# on battery the pile drains away grain by grain from the bottom as the level drops.

# Built In Dependencies
import time
import random

# External Dependencies
import numpy as np

# Grain steps per frame (the monitor draws about 10 frames a second)
SPEEDS = {"slow": 1, "normal": 2, "fast": 3}
SPEED_LABELS = {"slow": "Slow", "normal": "Normal", "fast": "Fast"}
LOW = 0.07


class SandPile:
    """Grains in a box of `rows` x 7. cells[row][col] is True for a grain; row 0 is the top."""

    def __init__(self, rows, cols=7, seed=None):
        self.rows = rows
        self.cols = cols
        self.cells = np.zeros((rows, cols), dtype=bool)
        self.rng = random.Random(seed)
        self.started = False

    def count(self):
        return int(self.cells.sum())

    def settle_to(self, target):
        """Fill instantly to target grains, packed from the bottom (used on first draw)."""
        self.cells[:, :] = False
        for i in range(min(target, self.rows * self.cols)):
            r = self.rows - 1 - i // self.cols
            c = i % self.cols
            self.cells[r, c] = True

    def step(self):
        """One fall step: each grain drops straight down, or slides diagonally off a pile."""
        moved = False
        for r in range(self.rows - 2, -1, -1):
            cols = list(range(self.cols))
            self.rng.shuffle(cols)
            for c in cols:
                if not self.cells[r, c]:
                    continue
                if not self.cells[r + 1, c]:
                    self.cells[r, c], self.cells[r + 1, c] = False, True
                    moved = True
                    continue
                sides = [d for d in (-1, 1) if 0 <= c + d < self.cols and not self.cells[r + 1, c + d]
                         and not self.cells[r, c + d]]
                if sides:
                    d = self.rng.choice(sides)
                    self.cells[r, c], self.cells[r + 1, c + d] = False, True
                    moved = True
        return moved

    def add_grain(self):
        free = [c for c in range(self.cols) if not self.cells[0, c]]
        if free:
            self.cells[0, self.rng.choice(free)] = True

    def remove_grain(self):
        """Let one grain out through the floor, from the bottom row."""
        bottom = [c for c in range(self.cols) if self.cells[self.rows - 1, c]]
        if bottom:
            self.cells[self.rows - 1, self.rng.choice(bottom)] = False

    def update(self, target, steps=2):
        target = max(0, min(target, self.rows * self.cols))
        if not self.started:
            self.settle_to(target)
            self.started = True
            return
        for _ in range(steps):
            n = self.count()
            if n < target:
                self.add_grain()
            elif n > target:
                self.remove_grain()
            self.step()


_piles = {}


def pile_for(key, rows, seed=None):
    pile = _piles.get(key)
    if pile is None or pile.rows != rows:
        pile = _piles[key] = SandPile(rows, seed=seed)
    return pile


def draw_sand_battery(grid, battery, fill_value, y, key=None, speed="normal", now=None):
    """Draw into the half panel starting at row y (0 top half, 16 bottom half), inside the border."""
    top = y + 1
    rows = 15 if y == 0 else 16
    pile = pile_for((key, y), rows)
    if battery is None:
        return
    ratio, plugged = battery
    target = int(round(rows * 7 * max(0.0, min(1.0, ratio))))
    pile.update(target, SPEEDS.get(speed, 2))
    now = time.time() if now is None else now
    if ratio <= LOW and not plugged and now % 2 < 1:
        return  # Flash when nearly empty, like the plain battery app
    # Each grain is a little brighter or dimmer, so the pile reads as sand
    shade = np.where((np.add.outer(np.arange(rows), np.arange(7)) % 3) == 0, 0.8, 1.0)
    block = (pile.cells * shade * fill_value).astype(int)
    grid[1:8, top:top + rows] = np.maximum(grid[1:8, top:top + rows], block.T)
    if plugged and ratio < 0.995:
        # While charging, a dimmer grain keeps falling into the pile
        col, row = drip_position(pile, now)
        if row is not None:
            grid[1 + col, top + row] = max(grid[1 + col, top + row], int(fill_value * 0.6))


def drip_position(pile, now, rate=12.0):
    """(column, row) of the falling charging grain, or (col, None) when there is no room for it."""
    cycle = int(now * rate)
    col = (cycle // pile.rows * 3) % pile.cols
    filled = np.nonzero(pile.cells[:, col])[0]
    surface = int(filled[0]) if len(filled) else pile.rows
    if surface == 0:
        return col, None
    return col, (cycle % pile.rows) % surface
