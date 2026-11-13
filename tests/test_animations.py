# The panel animations (fire, plasma, DNA helix, snowfall, chevrons, bouncing ball) and the old-TV effect
import pytest
import numpy as np

from led_mon import animations as an


@pytest.mark.parametrize("key", an.LOOK_KEYS)
def test_every_look_draws_moving_pictures(key):
    look = an.make_look(key, seed=1)
    frames = [look.frame(0.04).copy() for _ in range(60)]
    for frame in frames:
        assert frame.shape == (9, 34)
        assert frame.min() >= 0.0 and frame.max() <= 1.0
    lit = [int((f > 0.02).sum()) for f in frames]
    assert max(lit) > 0, "nothing lit"
    # It moves: not every frame is the same
    assert any(not np.array_equal(frames[0], f) for f in frames[1:])


def test_looks_run_at_the_same_speed_however_often_they_are_drawn():
    slow = an.make_look("plasma", seed=0)
    fast = an.make_look("plasma", seed=0)
    for _ in range(10):
        slow.frame(0.1)            # the monitor's usual 10 frames a second
    for _ in range(25):
        fast.frame(0.04)           # 25 frames a second
    assert np.allclose(slow.frame(0), fast.frame(0))


def test_a_long_pause_does_not_fast_forward_more_than_a_second():
    look = an.make_look("ball", seed=0)
    look.frame(3600)
    assert look.t == pytest.approx(1.0)


def test_unknown_look_falls_back_to_fire():
    assert isinstance(an.make_look("nonsense"), an.Fire)


def test_fire_stays_low_and_bright_at_the_bottom():
    fire = an.make_look("fire", seed=3)
    for _ in range(100):
        frame = fire.frame(0.04)
    assert frame[:, -3:].mean() > 0.6     # the bottom burns
    assert frame[:, :8].max() < 0.2       # the top of the panel stays (nearly) dark


def test_bouncing_ball_stays_on_the_panel():
    ball = an.make_look("ball", seed=5)
    for _ in range(1000):
        frame = ball.frame(0.04)
        assert (frame > 0.9).sum() == 1   # one bright head, the rest is the tail


def test_to_leds_keeps_faint_light_visible():
    leds = an.to_leds(np.array([[0.0, 0.01, 0.03, 1.0]] * 9), 100)
    assert leds.tolist()[0] == [0, 0, 3, 100]
    assert an.to_leds(np.full((9, 34), 0.03), 10).min() == 1


def test_tv_off_squeezes_to_a_line_then_a_dot_then_dark():
    picture = np.full((9, 34), 80)
    frames = an.tv_off_frames(picture, 160)
    assert all(f.shape == (9, 34) for f in frames)
    assert not frames[-1].any()
    # Rows lit go down while the picture squeezes
    rows = [int(f.any(axis=0).sum()) for f in frames]
    assert rows[0] < 34 and rows == sorted(rows, reverse=True)
    # A single row, then a single LED in the middle of the panel
    line = next(f for f in frames if f.any(axis=0).sum() == 1)
    assert line[:, 17].all()
    dot = [f for f in frames if (f > 0).sum() == 1]
    assert dot and dot[0][4, 17] > 0
    assert max(f.max() for f in frames) <= 255


def test_tv_off_with_nothing_known_starts_at_the_line():
    frames = an.tv_off_frames(None, 120)
    assert frames[0][:, 17].all() and frames[0].any(axis=0).sum() == 1
    assert not frames[-1].any()


def test_tv_on_is_the_reverse_and_ends_on_the_picture():
    picture = np.zeros((9, 34), dtype=int)
    picture[2:7, 5:30] = 100
    frames = an.tv_on_frames(picture, 160)
    assert (frames[0] > 0).sum() == 1
    assert np.array_equal(frames[-1], picture)
    rows = [int(f.any(axis=0).sum()) for f in frames]
    assert rows == sorted(rows)


def test_bonfire_still_uses_the_same_fire_step():
    # The Bonfire music look and the Fire animation share spread_heat; the golden frames check the pictures
    from led_mon.equalizer_files import looks
    assert looks.spread_heat is an.spread_heat


def test_animation_app_draws_each_panel_on_its_own():
    from led_mon.rendering import make_base_app_functions, render_panel
    from unittest import mock
    fns = make_base_app_functions(*(mock.Mock() for _ in range(5)))
    app = {"name": "animation", "scope": "panel", "args": {"look": "snow"}}
    left = np.zeros((9, 34), dtype=int)
    right = np.zeros((9, 34), dtype=int)
    for _ in range(5):
        render_panel(left, [app], fns, 160, 35, "left")
        render_panel(right, [app], fns, 160, 35, "right")
    assert left.any() and right.any()
    assert not np.array_equal(left, right)    # different snowflakes on each panel
    assert left.max() <= 160


def test_layout_editor_offers_the_animation_app():
    from led_mon.editor import catalog
    info = catalog.get("animation")
    assert info.size == catalog.FULL
    look = info.settings[0]
    assert look.key == "look" and set(look.options) == set(an.LOOK_KEYS)
    assert len(look.labels) == len(look.options)


def test_identify_shows_a_name_for_the_animation_app():
    import sys
    saved = sys.argv
    sys.argv = [saved[0], "--disable-plugins"]
    try:
        from led_mon import drawing
    finally:
        sys.argv = saved
    grid = np.zeros((9, 34), dtype=int)
    drawing.draw_id(grid, "animation", 100, {})
    assert grid.any()


def test_tv_effect_setting_and_tray_checkbox(monkeypatch, tmp_path):
    try:
        from led_mon import led_system_monitor as monitor
    except Exception as e:
        pytest.skip(f"full dependencies not installed: {e}")
    from led_mon import app_settings
    path = str(tmp_path / "settings.json")
    monkeypatch.setattr(app_settings, "settings_path", lambda: path)
    assert monitor.is_tv_effect() is True    # on by default
    monitor.set_tv_effect(False)
    assert monitor.is_tv_effect() is False
    assert app_settings.load_settings()["tv_effect"] is False


def test_play_tv_effect_sends_every_frame_to_every_panel(monkeypatch):
    try:
        from led_mon import led_system_monitor as monitor
    except Exception as e:
        pytest.skip(f"full dependencies not installed: {e}")
    monkeypatch.setattr(monitor.time, "sleep", lambda s: None)
    import queue
    left, right = queue.Queue(), queue.Queue()
    off = an.tv_off_frames(np.full((9, 34), 50), 100)
    monitor.play_tv_effect([(left, off), (right, off[:3])])
    assert left.qsize() == len(off) and right.qsize() == 3
    assert all(item[1] is False for item in list(left.queue))


def test_tray_tv_checkbox(monkeypatch):
    pytest.importorskip("PIL")
    from tests.test_tray_and_brightness import load_pystray
    pystray = load_pystray(monkeypatch)
    from led_mon import tray
    state = {"on": True}
    noop = lambda icon, item: None
    actions = {name: noop for name in (
        "edit_layout", "brightness", "toggle_animation", "identify", "next_app", "toggle_freeze", "open_lhm",
        "open_fc", "open_folder", "open_log", "reload_config", "toggle_startup", "quit")}
    actions["toggle_tv"] = lambda icon, item: state.update(on=not state["on"])
    menu = tray.build_menu(pystray, actions, tray.AppFinder(), lambda: False, lambda: False, True,
                           startup_supported=True, is_tv_effect=lambda: state["on"])
    item = [i for i in menu.items if str(i.text) == "Old-TV effect when turning off and on"][0]
    assert item.visible and item.checked is True
    item(None)
    assert item.checked is False
