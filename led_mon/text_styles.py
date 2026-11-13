# The Text app: your own words on a whole panel, with live values filled in.
#
# Type any text, with these filled in each time the panel is drawn:
#   {time}     the clock, such as 10:47
#   {battery}  the battery level, such as 87%  (--% without a battery)
#   {cpu}      overall CPU use, such as 12%
#
# Three styles:
#   ticker    upright letters, one under the other, scrolling up the panel
#   sideways  letters turned on their side, so the panel reads like a long, thin screen
#             (tilt your head to the right); also scrolls up the panel
#   stacked   two letters per row, each word under the last with a dotted line between words.
#             Still when it fits on the panel; otherwise it slides down and back up.
#
# Letters use a small 3 x 5 font made for this app.

# Built In Dependencies
import re
import time
import datetime

# External Dependencies
import numpy as np

WIDTH, HEIGHT = 9, 34

STYLES = ("ticker", "sideways", "stacked")
STYLE_LABELS = {
    "ticker": "Ticker (scrolls up)",
    "sideways": "Sideways (read with your head tilted right)",
    "stacked": "Stacked (two letters per row)",
}
DEFAULT_TEXT = "{time} {battery} {cpu}"

# Rows per second the ticker and sideways styles move
SCROLL_SPEED = 6.0
# Empty rows between the end of the text and its next pass
LOOP_GAP = 10
# Stacked text that doesn't fit: seconds to wait at each end, and rows per second while moving
STACK_PAUSE = 2.5
STACK_SPEED = 4.0
# Brightness of the small % under a stacked number and of the dotted line between stacked words
DIM = 0.55

_FONT_ROWS = {
    "A": ("010", "101", "111", "101", "101"),
    "B": ("110", "101", "110", "101", "110"),
    "C": ("011", "100", "100", "100", "011"),
    "D": ("110", "101", "101", "101", "110"),
    "E": ("111", "100", "110", "100", "111"),
    "F": ("111", "100", "110", "100", "100"),
    "G": ("011", "100", "101", "101", "011"),
    "H": ("101", "101", "111", "101", "101"),
    "I": ("111", "010", "010", "010", "111"),
    "J": ("001", "001", "001", "101", "010"),
    "K": ("101", "101", "110", "101", "101"),
    "L": ("100", "100", "100", "100", "111"),
    "M": ("101", "111", "111", "101", "101"),
    "N": ("110", "101", "101", "101", "101"),
    "O": ("010", "101", "101", "101", "010"),
    "P": ("110", "101", "110", "100", "100"),
    "Q": ("010", "101", "101", "110", "011"),
    "R": ("110", "101", "110", "101", "101"),
    "S": ("011", "100", "010", "001", "110"),
    "T": ("111", "010", "010", "010", "010"),
    "U": ("101", "101", "101", "101", "111"),
    "V": ("101", "101", "101", "101", "010"),
    "W": ("101", "101", "111", "111", "101"),
    "X": ("101", "101", "010", "101", "101"),
    "Y": ("101", "101", "010", "010", "010"),
    "Z": ("111", "001", "010", "100", "111"),
    "0": ("111", "101", "101", "101", "111"),
    "1": ("010", "110", "010", "010", "111"),
    "2": ("111", "001", "111", "100", "111"),
    "3": ("111", "001", "011", "001", "111"),
    "4": ("101", "101", "111", "001", "001"),
    "5": ("111", "100", "111", "001", "111"),
    "6": ("111", "100", "111", "101", "111"),
    "7": ("111", "001", "010", "010", "010"),
    "8": ("111", "101", "111", "101", "111"),
    "9": ("111", "101", "111", "001", "111"),
    "%": ("101", "001", "010", "100", "101"),
    ":": ("000", "010", "000", "010", "000"),
    ".": ("000", "000", "000", "000", "010"),
    ",": ("000", "000", "000", "010", "100"),
    "!": ("010", "010", "010", "000", "010"),
    "?": ("110", "001", "010", "000", "010"),
    "-": ("000", "000", "111", "000", "000"),
    "+": ("000", "010", "111", "010", "000"),
    "=": ("000", "111", "000", "111", "000"),
    "/": ("001", "001", "010", "100", "100"),
    "'": ("010", "010", "000", "000", "000"),
    '"': ("101", "101", "000", "000", "000"),
    "(": ("001", "010", "010", "010", "001"),
    ")": ("100", "010", "010", "010", "100"),
    "#": ("101", "111", "101", "111", "101"),
    "*": ("000", "101", "010", "101", "000"),
    "<": ("001", "010", "100", "010", "001"),
    ">": ("100", "010", "001", "010", "100"),
    "_": ("000", "000", "000", "000", "111"),
    "°": ("010", "101", "010", "000", "000"),
}

# Each letter as a 5-row x 3-column array of 0/1
FONT = {ch: np.array([[c == "1" for c in row] for row in rows], dtype=float) for ch, rows in _FONT_ROWS.items()}
# The small % that sits under a stacked number
SMALL_PERCENT = np.array([[0, 0, 1], [0, 1, 0], [1, 0, 0]], dtype=float)
# Letters with no shape in the font draw as this
UNKNOWN = FONT["?"]


def glyph(ch):
    return FONT.get(ch.upper(), UNKNOWN)


# Live values ----------------------------------------------------------------------------------------

def time_text(now=None, fmt_24_hour=False):
    now = now or datetime.datetime.now()
    if fmt_24_hour:
        return now.strftime("%H:%M")
    return f"{now.hour % 12 or 12}:{now.minute:02d}"


def percent_text(ratio):
    if ratio is None:
        return "--%"
    return f"{int(round(max(0.0, min(1.0, ratio)) * 100))}%"


TOKEN = re.compile(r"\{(time|battery|cpu)\}", re.IGNORECASE)


def fill_tokens(text, values):
    """text with {time}, {battery} and {cpu} replaced by values (a dict of name -> string).
    Anything else in braces is left as typed."""
    return TOKEN.sub(lambda m: values.get(m.group(1).lower(), m.group(0)), text or "")


# Building the picture -------------------------------------------------------------------------------
# Strips are 9 columns wide and as many rows long as the text needs (indexed [x, y] like the panel grid).

def ticker_strip(text, fill_value):
    """Upright letters, one under the other in the middle of the panel."""
    pieces = []
    for ch in text:
        if ch.isspace():
            pieces.append(np.zeros((WIDTH, 3)))
            continue
        piece = np.zeros((WIDTH, 6))
        piece[3:6, 0:5] = glyph(ch).T * fill_value
        pieces.append(piece)
    return np.concatenate(pieces, axis=1) if pieces else np.zeros((WIDTH, 0))


def sideways_strip(text, fill_value):
    """Letters turned a quarter turn clockwise, so the text reads down the panel with your head tilted right."""
    pieces = []
    for ch in text:
        if ch.isspace():
            pieces.append(np.zeros((WIDTH, 3)))
            continue
        # rot90 with k=-1 turns the 5 x 3 letter clockwise into 3 rows x 5 columns
        turned = np.rot90(glyph(ch), k=-1)
        piece = np.zeros((WIDTH, 4))
        piece[2:7, 0:3] = turned.T * fill_value
        pieces.append(piece)
    return np.concatenate(pieces, axis=1) if pieces else np.zeros((WIDTH, 0))


def stacked_lines(text):
    """Each word as rows of up to two letters. A colon starts a new row (10:47 is 10 over 47), and a %
    left on its own row becomes a small % under the number. Returns a list of words, each a list of rows."""
    words = []
    for word in text.split():
        rows = []
        for part in word.split(":"):
            for i in range(0, len(part), 2):
                rows.append(part[i:i + 2])
        if rows:
            words.append(rows)
    return words


def stacked_strip(text, fill_value):
    pieces = []
    dim = int(round(fill_value * DIM))
    for w, rows in enumerate(stacked_lines(text)):
        if w > 0:
            # The row above is already empty (the gap under the last line), so: dots, then one empty row
            divider = np.zeros((WIDTH, 2))
            divider[1:8:2, 0] = dim
            pieces.append(divider)
        for r, row in enumerate(rows):
            if row == "%" and r > 0:
                # Right under the number, dimmer, so the word stays short enough to fit
                piece = np.zeros((WIDTH, 3))
                piece[3:6, 0:3] = SMALL_PERCENT.T * dim
                pieces[-1] = pieces[-1][:, :-1]  # no gap above it
                pieces.append(piece)
                pieces.append(np.zeros((WIDTH, 1)))
                continue
            piece = np.zeros((WIDTH, 6))
            if len(row) == 1:
                piece[3:6, 0:5] = glyph(row).T * fill_value
            else:
                piece[1:4, 0:5] = glyph(row[0]).T * fill_value
                piece[5:8, 0:5] = glyph(row[1]).T * fill_value
            pieces.append(piece)
    if not pieces:
        return np.zeros((WIDTH, 0))
    strip = np.concatenate(pieces, axis=1)
    # Drop the empty row after the last line
    return strip[:, :-1] if strip.shape[1] and not strip[:, -1].any() else strip


def scroll_window(strip, offset):
    """34 rows of strip, starting offset rows in, the text repeating with LOOP_GAP empty rows between passes."""
    loop = np.concatenate((strip, np.zeros((WIDTH, LOOP_GAP))), axis=1)
    length = loop.shape[1]
    rows = (np.arange(HEIGHT) + int(offset)) % length
    return loop[:, rows]


def bounce_offset(overflow, seconds):
    """For stacked text taller than the panel: wait at the top, slide down, wait, slide back up."""
    travel = overflow / STACK_SPEED
    cycle = 2 * (STACK_PAUSE + travel)
    t = seconds % cycle
    if t < STACK_PAUSE:
        return 0
    t -= STACK_PAUSE
    if t < travel:
        return int(t * STACK_SPEED)
    t -= travel
    if t < STACK_PAUSE:
        return overflow
    t -= STACK_PAUSE
    return overflow - int(t * STACK_SPEED)


def draw_text(grid, text, style, fill_value, seconds):
    """Draw text (live values already filled in) on the whole panel. seconds is a steadily rising clock
    that moves the scrolling styles."""
    grid[:, :] = 0
    if not text or not text.strip():
        return
    if style == "stacked":
        strip = stacked_strip(text, fill_value)
        overflow = strip.shape[1] - HEIGHT
        if overflow <= 0:
            grid[:, :strip.shape[1]] = strip
        else:
            start = bounce_offset(overflow, seconds)
            grid[:, :] = strip[:, start:start + HEIGHT]
        return
    strip = sideways_strip(text, fill_value) if style == "sideways" else ticker_strip(text, fill_value)
    if strip.shape[1] == 0:
        return
    grid[:, :] = scroll_window(strip, int(seconds * SCROLL_SPEED))


class TextApp:
    """The Text app's live values. cpu_ratio and battery are functions that read the computer."""

    def __init__(self, cpu_ratio, battery, clock=time.monotonic, now=datetime.datetime.now):
        self.cpu_ratio = cpu_ratio
        self.battery = battery
        self.clock = clock
        self.now = now

    def values(self, fmt_24_hour=False):
        try:
            cpu = self.cpu_ratio()
        except Exception:
            cpu = None
        try:
            bat = self.battery()
            bat = bat[0] if bat else None
        except Exception:
            bat = None
        return {
            "time": time_text(self.now(), fmt_24_hour),
            "battery": percent_text(bat),
            "cpu": percent_text(cpu),
        }

    def draw(self, grid, foreground_value, text=DEFAULT_TEXT, style="ticker", fmt_24_hour=False, **kwargs):
        filled = fill_tokens(text, self.values(fmt_24_hour))
        draw_text(grid, filled, style if style in STYLES else "ticker", foreground_value, self.clock())
