# The golden frames in tests/golden/ match what the monitor apps and music looks draw today.
#
# The Rust rewrite of the monitor is checked against the same files, so a change to how an app or a
# look draws has to be deliberate: run `python scripts/golden_frames.py` to save the new pictures,
# and the Rust side has to draw the same.
import os
import sys

import pytest

pytest.importorskip("numpy")
pytest.importorskip("psutil")

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "scripts"))
import golden_frames  # noqa: E402


def test_monitor_apps_draw_the_golden_frames():
    assert golden_frames.differences(golden_frames.MONITOR_FILE, golden_frames.monitor_cases()) == []


def test_music_looks_draw_the_golden_frames():
    # Brightness may differ by 1 out of 255: the FFT's last digits can differ between computers
    assert golden_frames.differences(golden_frames.MUSIC_FILE, golden_frames.music_cases(), tolerance=1) == []


def test_every_music_look_is_covered():
    from led_mon.equalizer_files import looks
    names = golden_frames.load(golden_frames.MUSIC_FILE)
    for style in looks.STYLE_KEYS:
        assert any(n.startswith(style + "-single-") for n in names), style
        assert any(n.startswith(style + "-span-") for n in names), style


def test_encoding_round_trips():
    np = pytest.importorskip("numpy")
    grid = np.arange(9 * 34).reshape(9, 34) % 256
    assert (golden_frames.decode(golden_frames.encode(grid)) == grid).all()
