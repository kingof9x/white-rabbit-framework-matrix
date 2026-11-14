# The golden frames in tests/golden/effects.json match what the effects over the panels draw today.
#
# The Rust effects (rust/wr-core/src/effects.rs) are checked against the same file, so a change to how a
# ripple, drain, slide or the text line draws has to be deliberate: run `python scripts/golden_effects.py`
# to save the new frames, and the Rust side has to draw the same.
import os
import sys

import pytest

pytest.importorskip("numpy")

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "scripts"))
import golden_effects  # noqa: E402


def test_effects_draw_the_golden_frames():
    # A pixel may differ by 1 out of 255 on another computer: numpy's exp and hypot can differ in the
    # last bit between CPUs, and a value can land on the other side of a .5
    assert golden_effects.differences(golden_effects.results(), golden_effects.load(), tolerance=1) == []


def test_every_scenario_keeps_frames():
    data = golden_effects.load()
    for name in data["scenarios"]:
        assert any(n.startswith(name + "-") for n in data["frames"]), name


def test_encoding_round_trips():
    np = pytest.importorskip("numpy")
    grid = np.arange(9 * 34).reshape(9, 34) % 256
    assert (golden_effects.decode(golden_effects.encode(grid)) == grid).all()
