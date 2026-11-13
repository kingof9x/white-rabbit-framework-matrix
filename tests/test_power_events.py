# Turning the panels off when the lid closes, the screen turns off or the PC goes to sleep
import queue

import pytest

from led_mon import power_events as pe


def test_lid_screen_and_sleep_each_turn_the_panels_off():
    state = pe.PowerState()
    assert not state.should_sleep
    assert state.handle(pe.GUID_LIDSWITCH_STATE_CHANGE, 0) is True  # lid closed
    assert state.should_sleep
    assert state.handle(pe.GUID_CONSOLE_DISPLAY_STATE, 0) is False  # screen off too: still off
    assert state.handle(pe.GUID_LIDSWITCH_STATE_CHANGE, 1) is False  # lid open, screen still off
    assert state.should_sleep
    assert state.handle(pe.GUID_CONSOLE_DISPLAY_STATE, 1) is True  # screen on: panels back on
    assert not state.should_sleep
    assert state.handle(pe.PBT_APMSUSPEND) is True
    assert state.handle(pe.PBT_APMRESUMEAUTOMATIC) is True
    assert not state.should_sleep


def test_dimmed_screen_keeps_the_panels_on():
    state = pe.PowerState()
    assert state.handle(pe.GUID_CONSOLE_DISPLAY_STATE, 2) is False
    assert not state.should_sleep


def test_watcher_only_reports_changes():
    calls = []
    watcher = pe.PowerWatcher(calls.append)
    watcher.handle(pe.GUID_LIDSWITCH_STATE_CHANGE, 1)  # Windows reports the current (open) state first
    watcher.handle(pe.GUID_LIDSWITCH_STATE_CHANGE, 0)
    watcher.handle(pe.GUID_CONSOLE_DISPLAY_STATE, 0)
    watcher.handle(pe.GUID_LIDSWITCH_STATE_CHANGE, 1)
    watcher.handle(pe.GUID_CONSOLE_DISPLAY_STATE, 1)
    assert [state is watcher.state for state in calls] == [True, True]
    assert len(calls) == 2  # off, then back on
    assert not watcher.state.should_sleep


def test_watcher_does_nothing_off_windows(monkeypatch):
    monkeypatch.setattr(pe, "IS_WINDOWS", False)
    assert pe.PowerWatcher(lambda s: None).start() is False


def test_sleep_item_sends_the_sleep_command(monkeypatch):
    pytest.importorskip("numpy")
    try:
        from led_mon import drawing
    except Exception as e:
        pytest.skip(f"full dependencies not installed: {e}")
    import numpy as np

    class FakePort:
        lock = None
        def __init__(self):
            self.written = []
        def write(self, data):
            self.written.append(bytes(data))
            return len(data)
        def close(self):
            pass

    port = FakePort()
    monkeypatch.setattr(drawing, "init_device", lambda *a, **k: port)
    q = queue.Queue()
    thread = drawing.DrawingThread("COM9", q)
    q.put(drawing.SLEEP_PANEL)
    q.put((np.zeros((9, 34), dtype=int), None))
    q.put(None)
    thread.run()
    assert port.written[0] == bytes([0x32, 0xAC, 0x03, 0x01])
    # The next frame is drawn as usual (and wakes the panel); a dark frame is just the commit
    assert port.written[1][:3] == bytes([0x32, 0xAC, 0x08])


def test_lid_setting_saved_and_respected(tmp_path, monkeypatch):
    pytest.importorskip("numpy")
    pytest.importorskip("psutil")
    try:
        from led_mon import led_system_monitor as monitor
    except Exception as e:
        pytest.skip(f"full dependencies not installed: {e}")
    from led_mon import app_settings
    path = str(tmp_path / "settings.json")
    monkeypatch.setattr(app_settings, "settings_path", lambda: path)
    monitor.power_sleep.clear()
    assert monitor.is_turn_off_with_lid() is True  # on by default
    lid_closed = pe.PowerState()
    lid_closed.handle(pe.GUID_LIDSWITCH_STATE_CHANGE, 0)
    monitor.on_power_change(lid_closed)
    assert monitor.power_sleep.is_set()
    monitor.set_turn_off_with_lid(False)
    assert not monitor.power_sleep.is_set()
    assert app_settings.load_settings()["turn_off_with_lid"] is False
    monitor.on_power_change(lid_closed)
    assert not monitor.power_sleep.is_set()
    monitor.on_power_change(pe.PowerState())
    monitor.set_turn_off_with_lid(True)


def test_tray_lid_checkbox(monkeypatch):
    pytest.importorskip("PIL")
    from tests.test_tray_and_brightness import load_pystray
    pystray = load_pystray(monkeypatch)
    from led_mon import tray
    state = {"on": True}
    noop = lambda icon, item: None
    actions = {name: noop for name in (
        "edit_layout", "brightness", "toggle_animation", "identify", "next_app", "toggle_freeze", "open_lhm",
        "open_fc", "open_folder", "open_log", "reload_config", "toggle_startup", "quit")}
    actions["toggle_lid"] = lambda icon, item: state.update(on=not state["on"])
    menu = tray.build_menu(pystray, actions, tray.AppFinder(), lambda: False, lambda: False, True,
                           startup_supported=True, is_turn_off_with_lid=lambda: state["on"])
    item = [i for i in menu.items if str(i.text) == "Turn off when lid closes"][0]
    assert item.visible and item.checked is True
    item(None)
    assert item.checked is False


@pytest.mark.skipif(not pe.IS_WINDOWS, reason="Windows power messages")
def test_windows_listener_starts_and_stops():
    import time
    # Twice, as when the monitor starts again in the same process (another test may have one running too)
    for _ in range(2):
        watcher = pe.PowerWatcher(lambda s: None)
        assert watcher.start() is True
        deadline = time.monotonic() + 5
        while watcher._hwnd is None and time.monotonic() < deadline:
            time.sleep(0.05)
        assert watcher._hwnd, "the hidden power window was not created"
        assert watcher._thread.is_alive()
        watcher.stop()
        watcher._thread.join(timeout=5)
        assert not watcher._thread.is_alive()


# The laptop's own screen not in use: docked with the lid shut, or Win+P "Second screen only"
EXTERNAL_HDMI = 5


def test_builtin_screen_unused_only_when_an_external_screen_is_used_instead():
    builtin = pe.OUTPUT_DISPLAYPORT_EMBEDDED
    # Laptop screen only, or laptop screen plus a monitor (extend / duplicate): in use
    assert pe.builtin_screen_unused([builtin], [builtin, EXTERNAL_HDMI]) is False
    assert pe.builtin_screen_unused([builtin, EXTERNAL_HDMI], [builtin, EXTERNAL_HDMI]) is False
    # "Second screen only", or docked with the lid shut: not in use
    assert pe.builtin_screen_unused([EXTERNAL_HDMI], [builtin, EXTERNAL_HDMI]) is True
    assert pe.builtin_screen_unused([EXTERNAL_HDMI], [pe.OUTPUT_INTERNAL, EXTERNAL_HDMI]) is True
    # A desktop PC with no built-in screen: never
    assert pe.builtin_screen_unused([EXTERNAL_HDMI], [EXTERNAL_HDMI]) is False
    # No screen at all (lid shut, nothing plugged in) or Windows couldn't say: leave it to the lid setting
    assert pe.builtin_screen_unused([], [builtin]) is False
    assert pe.builtin_screen_unused(None, None) is False


def test_screen_unused_has_its_own_setting():
    state = pe.PowerState()
    assert state.handle(pe.BUILTIN_SCREEN_UNUSED, True) is True
    assert not state.should_sleep  # not part of the lid setting
    assert state.wants_sleep(with_lid=True, when_screen_unused=True)
    assert state.wants_sleep(with_lid=False, when_screen_unused=True)
    assert not state.wants_sleep(with_lid=True, when_screen_unused=False)
    assert state.handle(pe.BUILTIN_SCREEN_UNUSED, True) is False  # no change, no call
    assert state.handle(pe.BUILTIN_SCREEN_UNUSED, False) is True
    assert not state.wants_sleep()
    assert "laptop screen" not in state.describe()


def test_screen_unused_setting_saved_and_respected(tmp_path, monkeypatch):
    pytest.importorskip("numpy")
    pytest.importorskip("psutil")
    try:
        from led_mon import led_system_monitor as monitor
    except Exception as e:
        pytest.skip(f"full dependencies not installed: {e}")
    from led_mon import app_settings
    path = str(tmp_path / "settings.json")
    monkeypatch.setattr(app_settings, "settings_path", lambda: path)
    watcher = pe.PowerWatcher(monitor.on_power_change)
    monkeypatch.setattr(monitor, "power_watcher", watcher)
    monitor.power_sleep.clear()
    assert monitor.is_turn_off_when_screen_unused() is True  # on by default
    watcher.handle(pe.BUILTIN_SCREEN_UNUSED, True)
    assert monitor.power_sleep.is_set()
    # The lid checkbox alone doesn't change this one
    monitor.set_turn_off_with_lid(False)
    assert monitor.power_sleep.is_set()
    # Turning it off wakes the panels straight away, and back on turns them off again
    reply = monitor.handle_power_request({"turn_off_when_screen_unused": False})
    assert reply == {"turn_off_with_lid": False, "turn_off_when_screen_unused": False}
    assert not monitor.power_sleep.is_set()
    assert app_settings.load_settings()["turn_off_when_screen_unused"] is False
    monitor.set_turn_off_when_screen_unused(True)
    assert monitor.power_sleep.is_set()
    watcher.handle(pe.BUILTIN_SCREEN_UNUSED, False)  # laptop screen back in use
    assert not monitor.power_sleep.is_set()
    monitor.set_turn_off_with_lid(True)


def test_window_power_checkboxes_fall_back_to_settings_file(tmp_path, monkeypatch):
    try:
        from led_mon.editor import preview
    except Exception as e:
        pytest.skip(f"full dependencies not installed: {e}")
    from led_mon import app_settings
    path = str(tmp_path / "settings.json")
    monkeypatch.setattr(app_settings, "settings_path", lambda: path)
    monkeypatch.setattr(preview.control, "request", lambda *a, **k: None)  # monitor not running
    assert preview.power_settings() == {"turn_off_with_lid": True, "turn_off_when_screen_unused": True}
    assert preview.set_power_setting("turn_off_when_screen_unused", False) is False
    assert preview.power_settings()["turn_off_when_screen_unused"] is False
    with pytest.raises(ValueError):
        preview.set_power_setting("something_else", True)


def test_tray_screen_unused_checkbox(monkeypatch):
    pytest.importorskip("PIL")
    from tests.test_tray_and_brightness import load_pystray
    pystray = load_pystray(monkeypatch)
    from led_mon import tray
    state = {"on": True}
    noop = lambda icon, item: None
    actions = {name: noop for name in (
        "edit_layout", "brightness", "toggle_animation", "identify", "next_app", "toggle_freeze", "open_lhm",
        "open_fc", "open_folder", "open_log", "reload_config", "toggle_startup", "quit")}
    actions["toggle_screen_unused"] = lambda icon, item: state.update(on=not state["on"])
    menu = tray.build_menu(pystray, actions, tray.AppFinder(), lambda: False, lambda: False, True,
                           startup_supported=True, is_turn_off_when_screen_unused=lambda: state["on"])
    item = [i for i in menu.items if str(i.text) == tray.SCREEN_UNUSED_LABEL][0]
    assert item.visible and item.checked is True
    item(None)
    assert item.checked is False


@pytest.mark.skipif(not pe.IS_WINDOWS, reason="Windows display configuration")
def test_windows_display_query_runs():
    # A CI machine may have no screen at all; this checks the call itself works and gives a sensible answer
    found = pe.query_display_outputs()
    if found is not None:
        active, every = found
        assert len(every) >= len(active)
        assert pe.builtin_screen_unused(active, every) in (True, False)
    calls = []
    pe.check_builtin_screen(pe.PowerWatcher(calls.append))
