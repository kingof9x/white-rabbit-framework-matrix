# Live preview frames for the layout editor, and sending a frame to the panels.

# Built In Dependencies
import os
import re
import sys
import math
import time
import logging
import importlib.util

# Internal Dependencies
from led_mon import platform_utils, control, fps, app_settings
from led_mon.rendering import make_base_app_functions, render_panel, DEFAULT_FOREGROUND, DEFAULT_BACKGROUND
from led_mon.monitors import CPUMonitor, MemoryMonitor, BatteryMonitor, DiskMonitor, NetworkMonitor
from led_mon.equalizer_files import looks

# External Dependencies
import numpy as np

log = logging.getLogger(__name__)

BLANK = {"name": "none", "display": False}


def plugin_app_functions():
    """App functions from the plugins (already loaded by led_mon.drawing, or loaded here)."""
    functions = {}
    if re.search(r"--disable-plugins|-dp", str(sys.argv)):
        return functions
    folder = platform_utils.plugins_dir()
    for file in sorted(os.listdir(folder)):
        if not file.endswith("_plugin.py"):
            continue
        module_name = re.sub("_plugin.py", "", file)
        module = sys.modules.get(module_name)
        if module is None or not hasattr(module, "app_funcs"):
            spec = importlib.util.spec_from_file_location(module_name, os.path.join(folder, file))
            module = importlib.util.module_from_spec(spec)
            sys.modules[module_name] = module
            spec.loader.exec_module(module)
        for obj in module.app_funcs:
            functions[obj["name"]] = obj["fn"]
    return functions


_preview_visualizers = {}


def fake_equalizer(arg, grid, foreground_value, idx, **kwargs):
    """The real audio visualizer draws on the panel itself, so the preview plays made-up music through the chosen look."""
    side = kwargs.get("side", "left")
    settings = looks.normalize(kwargs)
    key = (side, tuple(sorted((k, str(v)) for k, v in settings.items())))
    now = time.monotonic()
    entry = _preview_visualizers.get(key)
    if entry is None:
        if len(_preview_visualizers) > 16:
            _preview_visualizers.clear()
        entry = _preview_visualizers[key] = [looks.Visualizer(settings, side, seed=0), now - 0.03]
    visualizer, last = entry
    entry[1] = now
    frame = visualizer.frame(looks.demo_samples(now), min(0.5, max(0.001, now - last)))
    grid[:, :] = looks.to_leds(frame, foreground_value)


def fake_now_playing(arg, grid, foreground_value, idx, side="left", **kwargs):
    """What is playing, or a made-up song when nothing is, so the preview shows what the app looks like."""
    from led_mon import now_playing
    options = {k: kwargs[k] for k in ("title", "visualizer", "drift", "when_idle") if k in kwargs}
    now_playing.draw_preview(grid, foreground_value, side, **options)


class PreviewRenderer:
    """Draws a page of the layout the same way the monitor does, using this computer's live values."""

    def __init__(self):
        # Its own trace session, so the preview doesn't stop the running monitor's FPS capture
        fps.SESSION_NAME = "led_mon_fps_preview"
        self.app_functions = make_base_app_functions(
            CPUMonitor(), MemoryMonitor(), BatteryMonitor(), DiskMonitor(), NetworkMonitor())
        try:
            self.app_functions.update(plugin_app_functions())
        except Exception as e:
            log.error(f"Could not load plugins for the preview: {e}")
        self.app_functions["equalizer"] = fake_equalizer
        self.app_functions["equalizer_dispose"] = lambda **kwargs: None
        self.app_functions["now-playing"] = fake_now_playing

    def plugin_names(self):
        return list(self.app_functions.keys())

    def render(self, page, side, entries=None):
        """Return the 9 x 34 grid for one page. entries maps 'full'/'top'/'bottom' to finalized app entries."""
        grid = np.zeros((9, 34), dtype=int)
        if page is None:
            return grid
        entries = entries or {}
        if page.full is not None:
            app = dict(entries.get("full") or page.full)
            app["scope"] = "panel"
            panel_apps = [app]
        else:
            panel_apps = [entries.get("top") or page.top or BLANK, entries.get("bottom") or page.bottom or BLANK]
        render_panel(grid, panel_apps, self.app_functions, DEFAULT_FOREGROUND, DEFAULT_BACKGROUND, side)
        return np.clip(grid, 0, 255)


def monitor_status():
    """The running monitor's status ({'ports': [...]}), or None if it isn't running."""
    reply = control.request({"cmd": "status"}, timeout=1.5)
    return reply if reply and reply.get("ok") else None


def sensor_source_status(reload=False):
    """What the running monitor reads temperatures and fans from (see sensor_sources.SensorSource.status),
    or None if it isn't running. reload: read the choice from settings.json again first."""
    reply = control.request({"cmd": "sensor_source", "reload": bool(reload)}, timeout=3.0)
    return reply.get("sensor_source") if reply and reply.get("ok") else None


def brightness():
    """The brightness choice ({'follow_screen': bool, 'level': 0-100}): the running monitor's, or the saved one."""
    status = monitor_status()
    if status and status.get("brightness"):
        return status["brightness"]
    return app_settings.Brightness().get()


def set_brightness(follow_screen, level):
    """Change the panel brightness. The running monitor applies and saves it; otherwise it is saved for next start.

    Returns True if the running monitor took it.
    """
    reply = control.request({"cmd": "brightness", "follow_screen": follow_screen, "level": level})
    if reply is not None and reply.get("ok"):
        return True
    app_settings.Brightness().set(follow_screen, level)
    return False


POWER_SETTINGS = ("turn_off_with_lid", "turn_off_when_screen_unused")


def power_settings():
    """The two "Turn off when..." choices ({'turn_off_with_lid': bool, 'turn_off_when_screen_unused': bool}):
    the running monitor's, or the saved ones."""
    reply = control.request({"cmd": "power"}, timeout=1.5)
    if reply and reply.get("ok") and all(key in reply for key in POWER_SETTINGS):
        return {key: bool(reply[key]) for key in POWER_SETTINGS}
    saved = app_settings.load_settings()
    return {key: bool(saved.get(key, True)) for key in POWER_SETTINGS}


def set_power_setting(key, enabled):
    """Change one "Turn off when..." choice. The running monitor applies and saves it straight away; otherwise
    it is saved for next start. Returns True if the running monitor took it."""
    if key not in POWER_SETTINGS:
        raise ValueError(f"unknown power setting {key}")
    reply = control.request({"cmd": "power", key: bool(enabled)})
    if reply is not None and reply.get("ok"):
        return True
    settings = app_settings.load_settings()
    settings[key] = bool(enabled)
    app_settings.save_settings(settings)
    return False


def send_frames(frames, seconds=30):
    """Show still frames ({'left': grid, 'right': grid}) on the panels.

    Goes through the running monitor when there is one (it owns the ports), otherwise opens the
    ports directly. Returns a message for the status bar.
    """
    payload = {side: np.asarray(grid).astype(int).tolist() for side, grid in frames.items()}
    reply = control.request({"cmd": "preview", "frames": payload, "seconds": seconds})
    if reply is not None:
        if not reply.get("ok"):
            raise RuntimeError(reply.get("error", "the monitor refused the preview"))
        return f"Showing the preview for {seconds} seconds (sent through the running monitor)."
    return _send_direct(frames)


def _send_direct(frames):
    from led_mon.shared_state import discover_led_devices
    from led_mon.drawing import init_device, draw_to_LEDs
    from led_mon.commands import do_animate
    devices = discover_led_devices() or []
    if not devices:
        raise RuntimeError("No LED Matrix panels were found. Check that no other program is using them.")
    sent = []
    for i, side in enumerate(("left", "right")):
        if side not in frames or i >= len(devices):
            continue
        location, port = devices[i]
        s = init_device(port, fatal=False)
        try:
            do_animate(s, False)
            draw_to_LEDs(s, np.asarray(frames[side]))
            sent.append(f"{side} ({port})")
        finally:
            s.close()
    if not sent:
        raise RuntimeError("That panel isn't connected.")
    return f"Sent the preview to the {' and '.join(sent)} panel. The monitor isn't running, so it stays until something else draws."


def apply_config():
    """Ask the running monitor to reload the config. Returns True if a monitor was running."""
    reply = control.request({"cmd": "reload"})
    return reply is not None and reply.get("ok", False)


def resume_monitor():
    reply = control.request({"cmd": "resume"})
    return reply is not None


def identify():
    reply = control.request({"cmd": "identify"})
    return reply is not None


def start_monitor(config_file=None):
    # --background: just the monitor and tray icon, no second window
    args = ["--background"] + (["--config-file", config_file] if config_file else [])
    platform_utils.launch_self(*args)
