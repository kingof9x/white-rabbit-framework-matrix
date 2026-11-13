# How one picture is spread over both LED panels (the Math tab), saved as "span" in settings.json:
#
#   side_by_side  the panels touch: the picture is 18 columns wide, 9 on each panel (as before)
#   keyboard_gap  the picture is wider and the columns hidden behind the keyboard are left out,
#                 so something moving from one panel to the other lines up across the keyboard
#   mirror        the picture is one panel wide; the right panel shows it and the left panel
#                 shows it mirrored
#
# The keyboard gap is measured in LED columns ("gap_columns"). Nobody has measured it on a real
# Framework 16 yet, so it can be changed in the Math tab; 30 is a guess.

# Internal Dependencies
from led_mon import app_settings

PANEL_WIDTH = 9
SIDE_BY_SIDE, KEYBOARD_GAP, MIRROR = "side_by_side", "keyboard_gap", "mirror"
MODES = [
    (SIDE_BY_SIDE, "Side by side"),
    (KEYBOARD_GAP, "Side by side, skip the keyboard"),
    (MIRROR, "Mirror (left copies right)"),
]
DEFAULT_GAP = 30
MIN_GAP, MAX_GAP = 0, 120


def normalize(span):
    """The span settings with every key filled in and in range."""
    span = span if isinstance(span, dict) else {}
    mode = span.get("mode")
    if mode not in [key for key, _ in MODES]:
        mode = SIDE_BY_SIDE
    try:
        gap = int(span.get("gap_columns", DEFAULT_GAP))
    except (TypeError, ValueError):
        gap = DEFAULT_GAP
    return {"mode": mode, "gap_columns": max(MIN_GAP, min(MAX_GAP, gap))}


def load(path=None):
    return normalize(app_settings.load_settings(path).get("span"))


def save(span, path=None):
    settings = app_settings.load_settings(path)
    settings["span"] = normalize(span)
    return app_settings.save_settings(settings, path)


def picture_width(panel_count, span):
    """How wide a picture to make for this many panels (one panel is always 9 columns)."""
    span = normalize(span)
    if panel_count < 2:
        return PANEL_WIDTH * max(1, panel_count)
    if span["mode"] == MIRROR:
        return PANEL_WIDTH
    if span["mode"] == KEYBOARD_GAP:
        return 2 * PANEL_WIDTH + span["gap_columns"]
    return PANEL_WIDTH * panel_count


def to_panels(picture, panel_count, span):
    """The columns the panels show (9 per panel, left to right) for a picture made picture_width() wide."""
    import numpy as np
    span = normalize(span)
    if panel_count != 2:
        return picture
    if span["mode"] == MIRROR:
        right = picture[:, :PANEL_WIDTH]
        return np.concatenate([right[:, ::-1], right], axis=1)
    if span["mode"] == KEYBOARD_GAP:
        return np.concatenate([picture[:, :PANEL_WIDTH], picture[:, -PANEL_WIDTH:]], axis=1)
    return picture


def gap_test_frames(width):
    """A dot going back and forth across the whole picture, on a dim line, to set the keyboard gap:
    with the right gap it leaves one panel and arrives at the other after the right pause."""
    import numpy as np
    x, step = 0, 1
    while True:
        frame = np.zeros((34, width), dtype=np.uint8)
        frame[16:18, :] = 40
        frame[15:19, max(0, x - 1):x + 2] = 255
        yield frame
        if not 0 < x + step < width - 1:
            step = -step
        x += step
