# The Math tab's graphs and simulations (white_rabbit/math_sims.py).
import numpy as np
import pytest

from white_rabbit import math_sims as sims


def take(frames, n):
    for _ in range(n - 1):
        next(frames)
    return next(frames)


@pytest.mark.parametrize("formula", [g[1] for g in sims.GRAPHS] + ["sin(x) * x", "max(x, 0)", "2^x", "-x"])
def test_formulas_compile_and_run(formula):
    f = sims.compile_formula(formula)
    assert f(np.linspace(-3, 3, 50), 1.0).shape == (50,)


@pytest.mark.parametrize("formula", ['__import__("os")', "x.real", "open(1)", "lambda: 1", "sin(x, x)",
                                     "max(x)", "(1).__class__", "'a'", "y + 1", "", "sin(x"])
def test_unsafe_or_broken_formulas_are_refused(formula):
    with pytest.raises(sims.FormulaError):
        sims.compile_formula(formula)


def test_huge_numbers_dont_hang():
    f = sims.compile_formula("9^9^9")
    assert np.isnan(f(np.array([1.0]))).all()


def test_sine_graph_has_one_lit_run_per_row():
    frame = sims.draw_graph(sims.compile_formula("sin(x)"), -6.28, 6.28, (-1.2, 1.2), width=9, axes=False)
    assert frame.shape == (34, 9) and frame.dtype == np.uint8
    for row in frame:
        lit = np.nonzero(row)[0]
        assert lit.size and lit.max() - lit.min() + 1 == lit.size


def test_tan_breaks_instead_of_drawing_across():
    frame = sims.draw_graph(sims.compile_formula("tan(x)"), -3.14, 3.14, (-4, 4), width=9, axes=False)
    assert not (frame == sims.FULL).all(axis=1).any()


def test_axes_are_dim():
    frame = sims.draw_graph(sims.compile_formula("x * 0 + 5"), -1, 1, (-6, 6), width=9)
    assert (frame == sims.DIM).any()


def test_blinker_blinks():
    grid = np.zeros((34, 9), dtype=bool)
    grid[10, 3:6] = True
    once = sims.life_step(grid, {3}, {2, 3})
    assert once[9:12, 4].all() and once.sum() == 3
    assert (sims.life_step(once, {3}, {2, 3}) == grid).all()


def test_life_restarts_when_settled():
    rng = np.random.default_rng(0)
    frames = sims.life_frames(rng, 9, start="Glider", max_generations=5, pause_frames=2)
    pictures = [next(frames) for _ in range(8)]
    assert (pictures[0] == pictures[7]).all()  # 5 generations, 2 paused, then the glider again


def test_parse_counts():
    assert sims.parse_counts("3,6") == {3, 6}
    assert sims.parse_counts("") == set()
    with pytest.raises(ValueError):
        sims.parse_counts("9")


def test_bml_keeps_every_car():
    rng = np.random.default_rng(1)
    grid = sims.bml_grid(rng, 18, 0.4)
    counts = np.bincount(grid.ravel(), minlength=3)
    for step in range(50):
        grid, _ = sims.bml_half_step(grid, sims.BML_ACROSS if step % 2 == 0 else sims.BML_DOWN)
    assert (np.bincount(grid.ravel(), minlength=3) == counts).all()


def test_hpp_keeps_every_particle():
    rng = np.random.default_rng(2)
    grid = sims.hpp_grid(rng, 9, "Random", 0.5)
    total = sims._BITS[grid].sum()
    for _ in range(100):
        grid = sims.hpp_step(grid)
    assert sims._BITS[grid].sum() == total


def test_hpp_head_on_collision_turns():
    grid = np.zeros((34, 9), dtype=np.uint8)
    grid[10, 4] = sims.E | sims.W
    after = sims.hpp_step(grid)
    assert after[9, 4] == sims.N and after[11, 4] == sims.S


def test_rule_90_makes_a_triangle():
    row = np.zeros(9, dtype=np.uint8)
    row[4] = 1
    row = sims.elementary_step(row, 90)
    assert list(np.nonzero(row)[0]) == [3, 5]


def test_totalistic_code_digits():
    row = np.array([0, 1, 0], dtype=np.uint8)
    # code 777 in base 3 is 1001210: sums 0,1,2 -> 0,1,2
    assert list(sims.totalistic_step(row, 777)) == [1, 1, 1]


@pytest.mark.parametrize("width", [9, 18])
def test_every_animation_makes_panel_sized_frames(width):
    rng = np.random.default_rng(3)
    animations = [
        sims.graph_frames("sin(x - t)", -3, 3, None, width),
        sims.random_graphs_frames(rng, width, frames_each=3),
        sims.life_frames(rng, width, start="Math graph"),
        sims.bml_frames(rng, width),
        sims.hpp_frames(rng, width, start="Gas in a corner"),
        sims.hpp_frames(rng, width, start="Math graph"),
        sims.automaton_frames(rng, width, sims.AUTOMATA_KINDS[1], 777, "Random"),
        sims.noise_frames(rng, width),
    ]
    for frames in animations:
        frame = take(frames, 40)
        assert frame.shape == (34, width) and frame.dtype == np.uint8


def test_send_frame_stages_9_columns_then_shows_them(monkeypatch):
    from inputmodule import inputmodule as im
    from white_rabbit import math_tab
    from white_rabbit.panels import Panel

    written = []

    class Recorder:
        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

        def write(self, data):
            written.append(bytes(data))

    monkeypatch.setattr(im, "serial_opener", lambda port, timeout=None: Recorder())
    frame = np.arange(34 * 9, dtype=np.uint16).reshape(34, 9).astype(np.uint8)
    math_tab.send_frame(Panel("left", "COM9"), frame)
    assert len(written) == 10
    for x, command in enumerate(written[:9]):
        assert command[:4] == bytes([0x32, 0xAC, 0x07, x]) and list(command[4:]) == list(frame[:, x])
    assert written[9] == bytes([0x32, 0xAC, 0x08, 0x00])
