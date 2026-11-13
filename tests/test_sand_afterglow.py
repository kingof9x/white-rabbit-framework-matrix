# The Battery (sand) app and the CPU app's afterglow.
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

from led_mon import sand


def _rendering():
    if sys.platform == "win32":
        pytest.importorskip("wmi")  # The monitors read the screen brightness through WMI on Windows
    from led_mon import rendering
    return rendering
from led_mon.editor import catalog


def test_pile_starts_at_the_battery_level():
    pile = sand.SandPile(15, seed=1)
    pile.update(50)
    assert pile.count() == 50
    # Packed from the bottom: the bottom row is full
    assert pile.cells[-1].all()


def test_charging_rains_grains_in_until_the_level():
    pile = sand.SandPile(15, seed=1)
    pile.update(10)
    for _ in range(200):
        pile.update(40, steps=2)
    assert pile.count() == 40
    # Settled: nothing floats above an empty cell
    for r in range(pile.rows - 1):
        assert not (pile.cells[r] & ~pile.cells[r + 1]).any()


def test_discharging_drains_grains_out():
    pile = sand.SandPile(16, seed=2)
    pile.update(80)
    for _ in range(100):
        pile.update(30)
    assert pile.count() == 30


def test_draw_fills_only_inside_the_half():
    sand._piles.clear()
    grid = np.zeros((9, 34), dtype=int)
    sand.draw_sand_battery(grid, (0.5, False), 100, 16, key="left", now=0.5)
    assert grid[1:8, 17:33].any()
    assert not grid[:, :17].any() and not grid[0].any() and not grid[8].any()
    # About half the 7 x 16 cells
    assert abs((grid > 0).sum() - 56) <= 1


def test_no_battery_draws_nothing():
    grid = np.zeros((9, 34), dtype=int)
    sand.draw_sand_battery(grid, None, 100, 0, key="x")
    assert not grid.any()


def test_low_battery_flashes():
    sand._piles.clear()
    on = np.zeros((9, 34), dtype=int)
    off = np.zeros((9, 34), dtype=int)
    sand.draw_sand_battery(on, (0.05, False), 100, 0, key="a", now=10.5)
    sand.draw_sand_battery(off, (0.05, False), 100, 0, key="b", now=11.5)
    assert not on.any() and off.any()


def test_sand_app_is_offered_with_a_speed_setting():
    info = catalog.get("battery-sand")
    assert info.size == catalog.HALF
    assert [s.key for s in info.settings] == ["sand_speed"]
    assert catalog.new_app("battery-sand", "left")["args"]["sand_speed"] == "normal"


def test_afterglow_fades_after_a_spike():
    rendering = _rendering()
    state = {}
    assert rendering.glow_values(state, [1.0, 0.2], 0.0) == [1.0, 0.2]
    glow = rendering.glow_values(state, [0.0, 0.2], 1.0)
    assert 0.4 < glow[0] < 0.7 and glow[1] == 0.2
    glow = rendering.glow_values(state, [0.0, 0.2], 2.0)
    assert glow[0] < 0.15


class _Fixed:
    def __init__(self, v):
        self.v = v

    def get(self):
        return self.v


def test_cpu_draws_a_dim_trail_under_the_live_load():
    rendering = _rendering()
    cpu = _Fixed([1.0] * 8)
    funcs = rendering.make_base_app_functions(cpu, _Fixed(0.5), _Fixed((0.5, False)), _Fixed((0, 0)), _Fixed((0, 0)))
    grid = np.zeros((9, 34), dtype=int)
    funcs["cpu"]("cpu", grid, 200, 0)
    cpu.v = [0.0] * 8
    grid = np.zeros((9, 34), dtype=int)
    funcs["cpu"]("cpu", grid, 200, 0)
    lit = set(grid[grid > 0].tolist())
    assert lit and max(lit) < 200  # Only the dim trail is left
    plain = np.zeros((9, 34), dtype=int)
    funcs["cpu"]("cpu", plain, 200, 0, afterglow=False)
    assert not plain[1:8, 1:16].any()


def test_cpu_afterglow_setting_defaults_on():
    assert catalog.new_app("cpu", "left")["args"]["afterglow"] is True
