# The Text app: your own words with {time}, {battery} and {cpu}, as a ticker, sideways or stacked
import sys
import datetime
import importlib

import pytest

np = pytest.importorskip("numpy")

from led_mon import text_styles as ts
from led_mon.editor import catalog


def lit(grid):
    return int(np.count_nonzero(grid))


def test_tokens_are_filled_in_and_other_braces_left_alone():
    values = {"time": "10:47", "battery": "87%", "cpu": "12%"}
    assert ts.fill_tokens("{time} {battery} {cpu}", values) == "10:47 87% 12%"
    assert ts.fill_tokens("Hi {TIME} {other}", values) == "Hi 10:47 {other}"
    assert ts.fill_tokens("", values) == ""
    assert ts.fill_tokens(None, values) == ""


def test_time_and_percent_text():
    when = datetime.datetime(2026, 11, 13, 22, 5)
    assert ts.time_text(when) == "10:05"
    assert ts.time_text(when, fmt_24_hour=True) == "22:05"
    assert ts.time_text(datetime.datetime(2026, 1, 1, 0, 30)) == "12:30"
    assert ts.percent_text(0.874) == "87%"
    assert ts.percent_text(1.5) == "100%"
    assert ts.percent_text(None) == "--%"


def test_every_letter_has_the_right_shape_and_unknown_letters_draw():
    for ch, g in ts.FONT.items():
        assert g.shape == (5, 3), ch
    assert ts.glyph("a") is ts.FONT["A"]
    assert ts.glyph("~") is ts.UNKNOWN


def test_ticker_scrolls_up_over_time():
    a = np.zeros((9, 34))
    b = np.zeros((9, 34))
    ts.draw_text(a, "HELLO", "ticker", 160, 0.0)
    ts.draw_text(b, "HELLO", "ticker", 160, 3.0)
    assert lit(a) > 0 and lit(b) > 0
    c = np.zeros((9, 34))
    ts.draw_text(c, "HELLO", "ticker", 160, 3.0 + 1 / ts.SCROLL_SPEED)
    # One row further on: the picture moved up by one row
    assert np.array_equal(c[:, :-1], b[:, 1:])
    # Upright letters stay in the middle columns
    assert not b[:3].any() and not b[6:].any()


def test_sideways_letters_read_with_head_tilted_right():
    strip = ts.sideways_strip("L", 1)
    # Turn the panel picture a quarter turn anticlockwise (what you see with your head tilted right)
    seen = np.rot90(strip.T, 1)
    seen = seen[seen.any(axis=1)][:, seen.any(axis=0)]
    rows = ["".join("#" if v else "." for v in r) for r in seen]
    assert rows == ["#..", "#..", "#..", "#..", "###"]


def test_stacked_default_text_fits_and_stays_still():
    text = "10:47 87% 12%"
    a = np.zeros((9, 34))
    b = np.zeros((9, 34))
    ts.draw_text(a, text, "stacked", 160, 0.0)
    ts.draw_text(b, text, "stacked", 160, 50.0)
    assert np.array_equal(a, b)
    assert ts.stacked_strip(text, 160).shape[1] <= 34
    # The dotted lines between words and the small % are dimmer
    assert set(np.unique(a)) == {0, 160, int(round(160 * ts.DIM))}


def test_stacked_rows():
    assert ts.stacked_lines("10:47 87% hello") == [["10", "47"], ["87", "%"], ["he", "ll", "o"]]


def test_long_stacked_text_slides_down_and_back():
    text = "ONE TWO THREE FOUR"
    overflow = ts.stacked_strip(text, 1).shape[1] - 34
    assert overflow > 0
    assert ts.bounce_offset(overflow, 0) == 0
    travel = overflow / ts.STACK_SPEED
    assert ts.bounce_offset(overflow, ts.STACK_PAUSE + travel + 0.1) == overflow
    cycle = 2 * (ts.STACK_PAUSE + travel)
    assert ts.bounce_offset(overflow, cycle) == 0
    for t in np.linspace(0, cycle, 80):
        assert 0 <= ts.bounce_offset(overflow, t) <= overflow
    g = np.zeros((9, 34))
    ts.draw_text(g, text, "stacked", 160, ts.STACK_PAUSE + travel / 2)
    assert lit(g) > 0


def test_blank_text_draws_nothing():
    for style in ts.STYLES:
        g = np.ones((9, 34))
        ts.draw_text(g, "   ", style, 160, 5.0)
        assert lit(g) == 0


def test_app_reads_values_and_survives_missing_ones():
    def broken():
        raise RuntimeError("no sensor")
    app = ts.TextApp(lambda: 0.5, lambda: None, clock=lambda: 0.0,
                     now=lambda: datetime.datetime(2026, 1, 1, 9, 5))
    assert app.values() == {"time": "9:05", "battery": "--%", "cpu": "50%"}
    app = ts.TextApp(broken, lambda: (0.87, True), clock=lambda: 0.0,
                     now=lambda: datetime.datetime(2026, 1, 1, 9, 5))
    assert app.values(fmt_24_hour=True) == {"time": "09:05", "battery": "87%", "cpu": "--%"}
    g = np.zeros((9, 34))
    app.draw(g, 160, text="{battery}", style="stacked")
    assert lit(g) > 0
    # An unknown style falls back to the ticker instead of failing
    app.draw(g, 160, text="{battery}", style="nope")


def test_layout_editor_offers_the_text_app():
    info = catalog.get("text")
    assert info.label == "Text" and info.size == catalog.FULL
    entry = catalog.new_app("text", "left")
    assert entry["args"] == {"text": ts.DEFAULT_TEXT, "style": "ticker", "fmt_24_hour": False}
    style = next(s for s in info.settings if s.key == "style")
    assert style.options == ts.STYLES and len(style.labels) == len(ts.STYLES)


def test_monitor_draws_the_text_app(monkeypatch):
    pytest.importorskip("psutil")
    pytest.importorskip("serial")
    from led_mon.platform_utils import IS_WINDOWS
    if IS_WINDOWS:
        # led_mon.monitors needs it on Windows; the quick CI test job doesn't install it, the build job does
        pytest.importorskip("wmi")
    monkeypatch.setattr(sys, "argv", ["led_mon", "--disable-plugins"])
    drawing = importlib.import_module("led_mon.drawing")
    from led_mon.rendering import make_base_app_functions, render_panel
    from led_mon.monitors import CPUMonitor, MemoryMonitor, BatteryMonitor, DiskMonitor, NetworkMonitor
    functions = make_base_app_functions(CPUMonitor(), MemoryMonitor(), BatteryMonitor(), DiskMonitor(), NetworkMonitor())
    for style in ts.STYLES:
        grid = np.zeros((9, 34), dtype=int)
        app = {"name": "text", "scope": "panel", "args": {"text": "8:88 {cpu}", "style": style}}
        # Stacked is still; the scrolling ones have letters somewhere within a pass
        render_panel(grid, [app], functions, 160, 35)
        assert grid.max() <= 160
    # Alt+I shows TEXT on the panel
    assert drawing.id_patterns["text"].shape == (9, 34)
