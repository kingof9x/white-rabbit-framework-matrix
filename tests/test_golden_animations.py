# The golden frames in tests/golden/animations.json match what the panel animations, the old-TV
# effect and the sand battery draw today.
#
# The Rust rewrite is checked against the same file, so a change to how they draw has to be
# deliberate: run `python scripts/golden_animations.py` to save the new pictures, and the Rust side
# has to draw the same.
import os
import sys
import json
import subprocess

import pytest

pytest.importorskip("numpy")

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT = os.path.join(ROOT, "scripts", "golden_animations.py")
GOLDEN = os.path.join(ROOT, "tests", "golden", "animations.json")


def test_animations_draw_the_golden_frames():
    # In its own Python: the script turns off numpy's AVX-512 maths before numpy is loaded, so the
    # exact (float64) frames come out the same on every computer
    result = subprocess.run([sys.executable, SCRIPT, "--check"], cwd=ROOT, capture_output=True, text=True)
    assert result.returncode == 0, result.stdout + result.stderr


def test_every_look_is_covered():
    from led_mon import animations
    with open(GOLDEN) as f:
        names = json.load(f)["frames"]
    for key in animations.LOOK_KEYS:
        assert f"look-{key}-14" in names, key
    for prefix in ("sand-", "tv-off-", "tv-on-", "to-leds-"):
        assert any(n.startswith(prefix) for n in names), prefix
