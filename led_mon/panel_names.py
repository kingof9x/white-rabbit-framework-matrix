# Draws a panel's name (settings.json "panel_names", set in the White Rabbit window) for Identify panels.

# External Dependencies
import numpy as np


def name_grid(name, fill_value, letters=5):
    """A 9 x 34 grid with the first few letters of a panel's name stacked top to bottom (5 x 6 font)."""
    from inputmodule import font
    grid = np.zeros((9, 34), dtype=int)
    text = str(name or "").upper()[:letters]
    if not text:
        return grid
    top = (34 - (len(text) * 7 - 1)) // 2
    for i, letter in enumerate(text):
        pixels = font.convert_font(letter)
        for y in range(6):
            for x in range(5):
                if pixels[x + y * 5]:
                    grid[2 + x, top + i * 7 + y] = fill_value
    return grid
