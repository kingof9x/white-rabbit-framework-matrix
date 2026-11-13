# One picture over both panels (side by side, keyboard gap, mirror) and panel names.
import numpy as np

from led_mon import app_settings
from led_mon.panel_names import name_grid
from white_rabbit import span, panels


class Port:
    def __init__(self, device, location):
        self.device, self.location = device, location
        self.vid, self.pid = 0x32AC, 0x0020
        self.serial_number = ""


def picture(width):
    """Each column holds its own number, so it's easy to see which columns land where."""
    return np.tile(np.arange(width), (34, 1))


def test_defaults_and_bad_values():
    assert span.normalize(None) == {"mode": span.SIDE_BY_SIDE, "gap_columns": span.DEFAULT_GAP}
    assert span.normalize({"mode": "sideways", "gap_columns": "lots"})["mode"] == span.SIDE_BY_SIDE
    assert span.normalize({"gap_columns": 9999})["gap_columns"] == span.MAX_GAP
    assert span.normalize({"gap_columns": -3})["gap_columns"] == span.MIN_GAP


def test_side_by_side_is_unchanged():
    layout = {"mode": span.SIDE_BY_SIDE}
    assert span.picture_width(2, layout) == 18
    p = picture(18)
    assert np.array_equal(span.to_panels(p, 2, layout), p)


def test_keyboard_gap_leaves_out_the_hidden_columns():
    layout = {"mode": span.KEYBOARD_GAP, "gap_columns": 30}
    width = span.picture_width(2, layout)
    assert width == 48
    shown = span.to_panels(picture(width), 2, layout)
    assert shown.shape == (34, 18)
    assert list(shown[0, :9]) == list(range(9))          # left panel: the first 9 columns
    assert list(shown[0, 9:]) == list(range(39, 48))     # right panel: the last 9, after the gap


def test_mirror_flips_the_right_panel_onto_the_left():
    layout = {"mode": span.MIRROR}
    assert span.picture_width(2, layout) == 9
    shown = span.to_panels(picture(9), 2, layout)
    assert list(shown[0, 9:]) == list(range(9))
    assert list(shown[0, :9]) == list(range(8, -1, -1))


def test_one_panel_ignores_the_layout():
    for mode, _ in span.MODES:
        layout = {"mode": mode, "gap_columns": 30}
        assert span.picture_width(1, layout) == 9
        p = picture(9)
        assert np.array_equal(span.to_panels(p, 1, layout), p)


def test_span_setting_is_saved(tmp_path):
    path = str(tmp_path / "settings.json")
    assert span.load(path)["mode"] == span.SIDE_BY_SIDE
    span.save({"mode": span.KEYBOARD_GAP, "gap_columns": 27}, path)
    assert span.load(path) == {"mode": span.KEYBOARD_GAP, "gap_columns": 27}
    # Other settings are kept
    assert app_settings.load_settings(path)["turn_off_with_lid"] is True


def test_panel_names_are_cleaned_and_saved(tmp_path):
    path = str(tmp_path / "settings.json")
    assert app_settings.load_panel_names(path) == {}
    app_settings.save_panel_names({"left": "  Desk   side ", "right": "   ", "middle": "x"}, path)
    assert app_settings.load_panel_names(path) == {"left": "Desk side"}


def test_named_panels_in_the_window(monkeypatch):
    monkeypatch.setattr(app_settings, "load_panel_names", lambda path=None: {"right": "Desk"})
    found = panels.find_panels({}, [Port("COM7", "1-3.3:1.0"), Port("COM4", "1-3.2:1.0")])
    assert found[0].label == "Left panel (COM4)" and found[0].called == "left panel"
    assert found[1].label == "Desk (right, COM7)" and found[1].called == "right panel (Desk)"


def test_name_grid_stacks_the_first_five_letters():
    grid = name_grid("desk side", 200)
    assert grid.shape == (9, 34)
    rows = set(np.nonzero(grid.any(axis=0))[0])
    # "DESK " fills the panel: 5 letters, 6 rows each with a blank row between (the 5th is a space)
    assert rows <= set(range(0, 27)) and not rows & {6, 13, 20}
    # A shorter name sits in the middle: "AB" is 13 rows tall, starting at row 10
    assert set(np.nonzero(name_grid("ab", 1).any(axis=0))[0]) <= set(range(10, 23))
    assert not grid[:2].any() and not grid[7:].any()
    assert set(np.unique(grid)) == {0, 200}
    assert not name_grid("", 200).any()


def test_gap_test_dot_crosses_and_comes_back():
    frames = span.gap_test_frames(48)
    xs = [int(np.argmax(next(frames)[16])) for _ in range(100)]
    assert max(xs) >= 44 and min(xs[50:]) <= 1
