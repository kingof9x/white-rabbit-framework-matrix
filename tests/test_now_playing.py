# The Now playing app: cover, scrolling song name and small visualizer, across both panels.
import sys
import importlib.util

import pytest

np = pytest.importorskip("numpy")
pytest.importorskip("psutil")

# Without the weather library the plugins can't load, so load led_mon.drawing without them
if importlib.util.find_spec("requests") is None:
    _argv, sys.argv = sys.argv, [sys.argv[0], "--disable-plugins"]
    try:
        from led_mon import drawing  # noqa: E402,F401
    finally:
        sys.argv = _argv

from led_mon import now_playing as np_mod
from led_mon.editor import catalog


def make_watcher(media):
    w = np_mod.Watcher(reader=lambda: media)
    w.poll()
    return w


def test_cover_from_image_is_18_by_18_and_stretched():
    from PIL import Image
    img = Image.new("RGB", (300, 300), (40, 40, 40))
    for x in range(150, 300):
        for y in range(0, 300, 7):
            img.putpixel((x, y), (200, 200, 200))
    cover = np_mod.cover_from_image(img)
    assert cover.shape == (18, 18)
    assert cover.min() == 0.0 and cover.max() == 1.0


def test_each_panel_shows_its_half_of_the_cover():
    cover = np.zeros((18, 18))
    cover[:, 9:] = 1.0  # Only the right half is lit
    media = np_mod.Media(title="A", artist="B", playing=True, cover=cover)
    canvas = np_mod.scene(media, cover, 0.0, title=False, visualizer=False, drift=False)
    assert not canvas[:, :9].any()
    assert canvas[np_mod.COVER_TOP:np_mod.COVER_TOP + 18, 9:].all()


def test_title_scrolls():
    a = np_mod.title_strip("Hello - World", 0.0)
    b = np_mod.title_strip("Hello - World", 1.0)
    assert a.shape == (5, 18) and not np.array_equal(a, b)


def test_bars_mirror_around_the_middle():
    media = np_mod.Media(title="", playing=True)
    levels = np.arange(9, dtype=float)
    canvas = np_mod.scene(media, None, 0.0, title=False, visualizer=True, drift=False, levels=levels)
    assert np.array_equal(canvas[:, :9], canvas[:, 9:][:, ::-1])
    assert canvas[33, 0] > 0 and canvas[33, 8] == 0  # Bass in the middle, treble outside


def test_song_change_is_announced_once():
    said = []
    media = np_mod.Media(title="Song", artist="Band", playing=True)
    w = np_mod.Watcher(reader=lambda: media, on_song=said.append)
    w.poll()
    w.poll()
    assert said == ["Song - Band"]
    media.title = "Next"
    w.poll()
    assert said == ["Song - Band", "Next - Band"]


def test_skip_only_when_nothing_is_playing(monkeypatch):
    monkeypatch.setattr(np_mod, "watcher", make_watcher(np_mod.Media(title="x", playing=False)))
    np_mod.watcher.want = lambda now=None: None
    assert np_mod.should_skip({"when_idle": "skip"})
    assert not np_mod.should_skip({"when_idle": "last"})
    monkeypatch.setattr(np_mod, "watcher", make_watcher(np_mod.Media(title="x", playing=True)))
    np_mod.watcher.want = lambda now=None: None
    assert not np_mod.should_skip({"when_idle": "skip"})


def test_idle_clock_or_last_cover(monkeypatch):
    cover = np.ones((18, 18))
    w = make_watcher(np_mod.Media(title="x", playing=True, cover=cover))
    w.reader = lambda: np_mod.Media(title="x", playing=False)
    w.poll()
    w.want = lambda now=None: None
    monkeypatch.setattr(np_mod, "watcher", w)
    last = np.zeros((9, 34), dtype=int)
    np_mod.draw_now_playing(last, 100, "left", title=False, visualizer=False, drift=False, when_idle="last", now=0)
    assert last[:, np_mod.COVER_TOP:np_mod.COVER_TOP + 18].all()
    clock = np.zeros((9, 34), dtype=int)
    np_mod.draw_now_playing(clock, 100, "left", when_idle="clock", now=0)
    assert clock.any() and not np.array_equal(clock, last)


def test_catalog_entry_gets_its_panel_side():
    info = catalog.get("now-playing")
    assert info.size == catalog.FULL
    entry = catalog.new_app("now-playing", "right")
    assert entry["args"]["side"] == "right" and entry["args"]["when_idle"] == "skip"


def test_preview_draws_a_made_up_song():
    grid = np.zeros((9, 34), dtype=int)
    np_mod.draw_preview(grid, 100, "left", now=1.0)
    assert grid.any()
