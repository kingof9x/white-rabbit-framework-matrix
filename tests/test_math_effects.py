# The Math tab's "More maths" effects (white_rabbit/math_effects.py).
import numpy as np
import pytest

from white_rabbit import math_effects as fx


@pytest.mark.parametrize("width", [9, 18])
@pytest.mark.parametrize("name,make", [(e[0], e[1]) for e in fx.EFFECTS])
def test_every_effect_makes_panel_sized_frames(name, make, width):
    frames = make(np.random.default_rng(1), width)
    lit = 0
    for _ in range(700):  # long enough to go past the first restart of most of them
        frame = next(frames)
        assert frame.shape == (34, width) and frame.dtype == np.uint8, name
        lit += int(frame.any())
    assert lit > 600, name


def test_digits_of_pi_and_e():
    assert "".join(map(str, fx.pi_digits(15))) == "314159265358979"
    assert "".join(map(str, fx.e_digits(15))) == "271828182845904"


def test_collatz_27_takes_111_steps():
    seq = fx.collatz_sequence(27)
    assert len(seq) == 112 and max(seq) == 9232 and seq[-1] == 1


@pytest.mark.parametrize("name,sort", fx.SORTS)
def test_sorts_put_bars_in_order(name, sort):
    a = list(np.random.default_rng(3).permutation(18))
    for _ in sort(a):
        pass
    assert a == sorted(a), name


def test_ulam_spiral_visits_each_spot_once():
    pos = fx.ulam_positions(18, 400)[1:]
    assert len(set(pos)) == len(pos) == 400


def test_primes_table():
    assert list(np.nonzero(fx.is_prime_table(30))[0]) == [2, 3, 5, 7, 11, 13, 17, 19, 23, 29]


def test_maze_solve_lights_a_path_from_corner_to_corner():
    frames = fx.maze_frames(np.random.default_rng(5), 18)
    seen_full = False
    for _ in range(600):
        frame = next(frames)
        if frame[1, 1] == 255 and frame[31, 15] == 255:
            seen_full = True
            break
    assert seen_full
