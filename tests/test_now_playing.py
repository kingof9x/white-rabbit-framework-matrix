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


def test_a_playing_app_wins_over_a_paused_current_one():
    paused, spotify, other = object(), object(), object()
    playing = {spotify}
    assert np_mod._pick_session([paused, spotify], paused, lambda s: s in playing) is spotify
    assert np_mod._pick_session([paused, spotify], spotify, lambda s: s in playing) is spotify
    assert np_mod._pick_session([paused, other], paused, lambda s: False) is paused
    assert np_mod._pick_session([], None, lambda s: False) is None


def test_failures_are_logged_once_and_described(caplog):
    def broken():
        raise OSError("no media controls")
    w = np_mod.Watcher(reader=broken)
    with caplog.at_level("INFO", logger="led_mon.now_playing"):
        w.poll()
        w.poll()
    warnings = [r for r in caplog.records if r.levelname == "WARNING"]
    assert len(warnings) == 1 and "no media controls" in warnings[0].getMessage()
    media, _ = w.current()
    assert not media.playing and "no media controls" in media.error
    assert "could not ask Windows" in np_mod.describe(broken)
    assert np_mod.describe(lambda: np_mod.Media()) == "Windows says nothing is playing"
    song = np_mod.Media(title="Song", artist="Band", playing=True, app="Spotify.exe")
    assert np_mod.describe(lambda: song) == "Spotify.exe is playing: Song - Band"


def test_what_is_playing_goes_in_the_log(caplog):
    song = np_mod.Media(title="Song", artist="Band", playing=True, app="Spotify.exe")
    w = np_mod.Watcher(reader=lambda: song)
    with caplog.at_level("INFO", logger="led_mon.now_playing"):
        w.poll()
        w.poll()
    lines = [r.getMessage() for r in caplog.records if "Spotify" in r.getMessage()]
    assert lines == ["Now playing: Spotify.exe is playing: Song - Band"]


def test_lone_now_playing_falls_back_to_the_monitor_apps(monkeypatch):
    pytest.importorskip("requests")  # The monitor loads the weather plugin
    from led_mon import led_system_monitor as mon
    media = {"m": np_mod.Media()}
    w = np_mod.Watcher(reader=lambda: media["m"])
    w.poll()
    monkeypatch.setattr(np_mod, "watcher", w)
    monkeypatch.setattr(w, "want", lambda now=None: None)
    lone = [{"name": "now-playing", "args": {"when_idle": "skip", "side": "left"}}]
    assert [a["name"] for a in mon.now_playing_fallback("left", lone)] == ["cpu", "mem-bat"]
    assert [a["name"] for a in mon.now_playing_fallback("right", lone)] == ["disk", "net"]
    # Set to keep the cover or show the clock: stays
    clock = [{"name": "now-playing", "args": {"when_idle": "clock"}}]
    assert mon.now_playing_fallback("left", clock) is clock
    # Something playing: Now playing shows
    media["m"] = np_mod.Media(title="Song", playing=True)
    w.poll()
    assert mon.now_playing_fallback("left", lone) is lone
    # Other apps are left alone
    two = [{"name": "cpu"}, {"name": "mem-bat"}]
    assert mon.now_playing_fallback("left", two) is two


def test_now_playing_page_comes_back_when_music_starts(monkeypatch):
    pytest.importorskip("requests")
    from led_mon import led_system_monitor as mon
    media = {"m": np_mod.Media()}
    w = np_mod.Watcher(reader=lambda: media["m"])
    monkeypatch.setattr(np_mod, "watcher", w)
    monkeypatch.setattr(w, "want", lambda now=None: None)
    quads = {"top-left": [{"name": "cpu"}, {"name": "now-playing", "args": {"when_idle": "skip"}}],
             "bottom-left": [{"name": "mem-bat"}, {"name": "blank"}]}
    assert mon.now_playing_page(quads["top-left"]) == 1
    assert mon.now_playing_page([{"name": "now-playing", "args": {"when_idle": "last"}}]) is None
    tracker = mon.NowPlayingTracker()
    assert not tracker.update(quads)          # nothing heard yet
    w.poll()
    assert not tracker.update(quads)          # nothing playing
    media["m"] = np_mod.Media(title="Song", playing=True)
    w.poll()
    assert tracker.update(quads)              # it started: jump to the page
    assert not tracker.update(quads)          # only once
    assert not tracker.update({"top-left": [{"name": "cpu"}]})
