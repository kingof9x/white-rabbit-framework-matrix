# The Draw tab: drawing files, PNG/GIF import, the Picture app playing drawings, and the tab itself.

import json

import numpy as np
import pytest

from led_mon import drawings


@pytest.fixture
def folder(tmp_path, monkeypatch):
    monkeypatch.setattr(drawings, "folder", lambda: str(tmp_path / "drawings"))
    return tmp_path / "drawings"


def frame_with(value, row=0, col=0):
    f = drawings.blank_frame()
    f[row, col] = value
    return f


def test_save_load_round_trip(folder):
    d = drawings.Drawing([frame_with(255), frame_with(40, 33, 8)], 250)
    assert drawings.save("Rabbit", d) == "Rabbit"
    assert drawings.list_names() == ["Rabbit"]
    back = drawings.load("Rabbit")
    assert back.delay_ms == 250
    assert len(back.frames) == 2
    assert back.frames[1][33, 8] == 40
    data = json.loads((folder / "Rabbit.json").read_text())
    assert data["version"] == 1 and len(data["frames"][0]) == 34 and len(data["frames"][0][0]) == 9


def test_names_are_safe_file_names(folder):
    assert drawings.safe_name(' a/b:c*?"<>|. ') == "abc"
    with pytest.raises(ValueError):
        drawings.save("  ", drawings.Drawing())
    drawings.save("my: pic", drawings.Drawing())
    assert drawings.list_names() == ["my pic"]
    drawings.delete("my pic")
    assert drawings.list_names() == []


def test_missing_folder_lists_nothing(folder):
    assert drawings.list_names() == []


def test_wrong_frame_size_is_refused():
    with pytest.raises(ValueError):
        drawings.Drawing.from_json({"frames": [[[0] * 8] * 34]})


def test_delay_is_kept_in_range():
    assert drawings.Drawing(delay_ms=1).delay_ms == drawings.MIN_DELAY_MS
    assert drawings.Drawing(delay_ms=10 ** 9).delay_ms == drawings.MAX_DELAY_MS
    assert drawings.Drawing(delay_ms="x").delay_ms == drawings.DEFAULT_DELAY_MS


def test_frame_at_loops():
    d = drawings.Drawing([frame_with(1), frame_with(2), frame_with(3)], 100)
    assert [d.frame_at(t)[0, 0] for t in (0, 0.15, 0.25, 0.31)] == [1, 2, 3, 1]


def test_import_png_fits_and_brightens(tmp_path):
    from PIL import Image
    image = Image.new("RGBA", (90, 340), (0, 0, 0, 0))
    image.paste((100, 100, 100, 255), (0, 0, 90, 170))  # top half mid-grey, bottom see-through
    path = tmp_path / "half.png"
    image.save(path)
    d = drawings.import_image(str(path))
    assert len(d.frames) == 1
    f = d.frames[0]
    assert f.shape == (34, 9)
    # The brightest part becomes full brightness; see-through becomes dark
    assert f[:16].min() == 255
    assert f[18:].max() == 0
    inverted = drawings.import_image(str(path), invert=True).frames[0]
    assert inverted[30, 4] == 255 and inverted[5, 4] == 255 - 100


def test_import_wide_picture_whole_or_fill(tmp_path):
    from PIL import Image
    path = tmp_path / "wide.png"
    Image.new("L", (340, 90), 200).save(path)
    whole = drawings.import_image(str(path), drawings.FIT_WHOLE).frames[0]
    # Squashed into a short band in the middle, dark above and below
    assert whole[0].max() == 0 and whole[33].max() == 0 and whole[17].min() == 255
    fill = drawings.import_image(str(path), drawings.FIT_FILL).frames[0]
    assert fill.min() == 255
    stretch = drawings.import_image(str(path), drawings.FIT_STRETCH).frames[0]
    assert stretch.min() == 255


def test_import_gif_keeps_frames_and_speed(tmp_path):
    from PIL import Image
    frames = []
    for i in range(5):
        im = Image.new("L", (9, 34), 0)
        im.paste(255, (0, i * 6, 9, i * 6 + 4))
        frames.append(im)
    path = tmp_path / "anim.gif"
    frames[0].save(path, save_all=True, append_images=frames[1:], duration=80, loop=0)
    d = drawings.import_image(str(path))
    assert len(d.frames) == 5
    assert d.delay_ms == 80
    assert d.frames[2][13, 4] == 255 and d.frames[2][1, 4] == 0


def test_player_reloads_changed_files(folder):
    now = [0.0]
    player = drawings.Player(clock=lambda: now[0])
    assert player.frame("nope") is None
    drawings.save("a", drawings.Drawing([frame_with(10), frame_with(20)], 100))
    first = player.frame("a")
    assert first.shape == (9, 34) and first[0, 0] == 10
    now[0] = 0.1
    assert player.frame("a")[0, 0] == 20
    import os
    import time
    drawings.save("a", drawings.Drawing([frame_with(99)], 100))
    later = time.time() + 5
    os.utime(drawings.path_for("a"), (later, later))
    assert player.frame("a")[0, 0] == 99


def test_picture_app_draws_a_drawing_at_the_panel_brightness(folder):
    from led_mon import drawing
    f = frame_with(255, 2, 3)
    f[4, 5] = 128
    drawings.save("pic", drawings.Drawing([f]))
    grid = np.zeros((9, 34), dtype=int)
    drawing.draw_snapshot(grid, 100, path="snapshot_files", panel="left", file=drawings.setting_value("pic"))
    assert grid[3, 2] == 100
    assert grid[5, 4] == 50
    assert grid.sum() == 150
    # A missing drawing leaves the panel dark instead of failing
    grid[:] = 0
    drawing.draw_snapshot(grid, 100, path="snapshot_files", panel="left", file=drawings.setting_value("gone"))
    assert not grid.any()


def test_picture_app_still_shows_built_in_patterns():
    from led_mon import drawing
    from led_mon.editor import catalog
    name = catalog.snapshot_files("left")[0]
    grid = np.zeros((9, 34), dtype=int)
    drawing.draw_snapshot(grid, 7, path="snapshot_files", panel="left", file=name)
    assert grid.max() == 7


def test_picture_choices_list_drawings_first(folder):
    from led_mon.editor import catalog
    drawings.save("Rabbit", drawings.Drawing())
    options = catalog.snapshot_options("left")
    assert options[0] == ("drawing:Rabbit", "My drawing: Rabbit")
    assert all(label.startswith("Built-in: ") for _, label in options[1:])
    assert catalog.new_app("snap", "left")["args"]["file"] == "drawing:Rabbit"


def test_flood_fill_stays_inside_the_area():
    from white_rabbit.draw_tab import flood_fill
    f = drawings.blank_frame()
    f[10, :] = 255  # a wall across the panel
    out = flood_fill(f, 0, 0, 80)
    assert (out[:10] == 80).all()
    assert (out[10] == 255).all()
    assert (out[11:] == 0).all()
    assert f[0, 0] == 0  # the original is unchanged


class FakeLender:
    def is_borrowed(self, panel):
        return False


class FakeWindow:
    def __init__(self, notebook):
        self.notebook = notebook
        self.lender = FakeLender()

    def selected_panels(self):
        return []


def test_draw_tab_draws_saves_and_opens(folder, tmp_path):
    tk = pytest.importorskip("tkinter")
    try:
        root = tk.Tk()
    except tk.TclError:
        pytest.skip("no desktop to open a window on")
    from tkinter import ttk
    from white_rabbit import draw_tab
    try:
        notebook = ttk.Notebook(root)
        notebook.pack()
        tab = draw_tab.DrawTab(notebook, FakeWindow(notebook))
        notebook.add(tab, text="Draw")
        root.update()

        class Click:
            def __init__(self, row, col):
                self.x = 6 + col * draw_tab.LED + 3
                self.y = 6 + row * draw_tab.LED + 3

        # A stroke from the top-left down three rows fills the LEDs in between
        tab.shade.set(200)
        tab.press(Click(0, 0), erase=False)
        tab.drag(Click(3, 0), erase=False)
        tab.release()
        assert list(tab.current()[:4, 0]) == [200] * 4
        # The right button rubs out
        tab.press(Click(1, 0), erase=True)
        tab.release()
        assert tab.current()[1, 0] == 0
        tab.undo()
        assert tab.current()[1, 0] == 200
        tab.redo()
        assert tab.current()[1, 0] == 0

        tab.duplicate()
        assert len(tab.drawing.frames) == 2 and tab.index == 1
        tab.invert_frame()
        assert tab.current()[1, 0] == 255 and tab.drawing.frames[0][1, 0] == 0
        tab.move_frame(-1)
        assert tab.index == 0 and tab.current()[1, 0] == 255

        tab.name_entry.insert(0, "Test")
        tab.save()
        assert drawings.list_names() == ["Test"]
        assert len(drawings.load("Test").frames) == 2

        tab.new_drawing()
        assert len(tab.drawing.frames) == 1 and not tab.current().any()
        tab.saved_list.selection_set(0)
        tab.open_selected()
        assert tab.name == "Test" and len(tab.drawing.frames) == 2

        from PIL import Image
        path = tmp_path / "white.png"
        Image.new("L", (9, 34), 255).save(path)
        tab.import_from(str(path))
        assert len(tab.drawing.frames) == 1 and tab.current().min() == 255
        tab.undo()
        assert len(tab.drawing.frames) == 2
    finally:
        root.destroy()
