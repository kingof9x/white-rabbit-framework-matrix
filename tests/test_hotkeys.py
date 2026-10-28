# Windows keyboard shortcuts through RegisterHotKey (no keyboard hook)
import sys
import threading

import pytest

from led_mon import platform_utils

windows_only = pytest.mark.skipif(not platform_utils.IS_WINDOWS, reason="Windows hotkey API")


@windows_only
def test_hotkey_message_calls_its_callback():
    import ctypes
    from led_mon import hotkeys
    pressed = threading.Event()
    thread = hotkeys.start({"n": pressed.set})
    try:
        assert thread.registered == ["N"]
        # What Windows posts when the shortcut is pressed
        ctypes.windll.user32.PostThreadMessageW(thread._thread_id, hotkeys.WM_HOTKEY, 1, 0)
        assert pressed.wait(5)
    finally:
        thread.stop()
        thread.join(5)
    assert not thread.is_alive()


@windows_only
def test_monitor_does_not_load_pynput_on_windows():
    pytest.importorskip("numpy")
    try:
        from led_mon import led_system_monitor
    except Exception as e:
        pytest.skip(f"full dependencies not installed: {e}")
    assert not led_system_monitor.PYNPUT_AVAILABLE
    assert not any(name == "pynput" or name.startswith("pynput.") for name in sys.modules)


def test_shortcuts_are_on_until_turned_off(tmp_path, monkeypatch):
    from led_mon import app_settings, shortcuts
    settings_file = tmp_path / "settings.json"
    monkeypatch.setattr(app_settings, "settings_path", lambda: str(settings_file))
    assert shortcuts.reload() == {"i": True, "n": True, "f": True, "u": True}
    assert shortcuts.save({"i": True, "n": False, "f": True, "u": False})
    assert not shortcuts.is_enabled("n") and shortcuts.is_enabled("i")
    # Other settings stay, and the monitor reads the same choice back
    assert app_settings.load_settings()["brightness_level"] == 70
    assert shortcuts.reload() == {"i": True, "n": False, "f": True, "u": False}
    settings_file.write_text('{"shortcuts": "broken"}')
    assert shortcuts.reload() == {"i": True, "n": True, "f": True, "u": True}


def test_every_shortcut_has_keys_text():
    from led_mon import shortcuts
    for letter in shortcuts.LETTERS:
        assert shortcuts.keys_text(letter).endswith("+" + letter.upper())
