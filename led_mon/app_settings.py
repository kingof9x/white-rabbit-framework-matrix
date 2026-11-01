# App settings that are changed from the tray or the layout editor rather than in config.yaml:
# the brightness level, where Libre Hardware Monitor is and music mode's look. Stored as settings.json next to the config.

# Built In Dependencies
import os
import json
import logging
import threading

# Internal Dependencies
from led_mon import platform_utils

log = logging.getLogger(__name__)

DEFAULT_SETTINGS = {
    # True: the panels follow the screen brightness (as before). False: they use brightness_level.
    "follow_screen_brightness": True,
    # 0-100, used when follow_screen_brightness is off
    "brightness_level": 70,
    # Full path to LibreHardwareMonitor.exe, or the folder it is in. Empty: look for it automatically.
    "libre_hardware_monitor_path": "",
    # Turn the panels off while the lid is closed, the screen is off or the PC is going to sleep
    "turn_off_with_lid": True,
    # Music mode's audio visualizer: the look and input last picked in the Audio tab (see
    # led_mon/equalizer_files/looks.py for the keys)
    "music": {},
    # Where the Temperatures and Fans apps read from on Windows: auto, framework_control,
    # libre_hardware_monitor or framework_tool (see sensor_sources.py)
    "sensor_source": "auto",
}


def settings_path():
    return os.path.join(platform_utils.user_config_dir(), "settings.json")


def load_settings(path=None):
    """Read settings.json, filling in defaults for anything missing or unreadable."""
    path = path or settings_path()
    settings = dict(DEFAULT_SETTINGS)
    try:
        with open(path, "r", encoding="utf-8") as f:
            loaded = json.load(f)
        if isinstance(loaded, dict):
            settings.update({k: v for k, v in loaded.items() if v is not None})
    except FileNotFoundError:
        pass
    except (OSError, ValueError) as e:
        log.warning(f"Could not read {path}, using defaults: {e}")
    return settings


def save_settings(settings, path=None):
    path = path or settings_path()
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(settings, f, indent=2)
        os.replace(tmp, path)
        return True
    except OSError as e:
        log.error(f"Could not write {path}: {e}")
        return False


def load_music(path=None):
    """Music mode's visualizer settings, with every key filled in."""
    from led_mon.equalizer_files import looks
    music = load_settings(path).get("music")
    return looks.normalize(music if isinstance(music, dict) else {})


def save_music(music, path=None):
    from led_mon.equalizer_files import looks
    settings = load_settings(path)
    settings["music"] = looks.normalize(music)
    return save_settings(settings, path)


def clamp_level(level):
    try:
        return max(0, min(100, int(round(float(level)))))
    except (TypeError, ValueError):
        return DEFAULT_SETTINGS["brightness_level"]


class Brightness:
    """The brightness choice shared by the main loop, the tray and the control channel."""

    def __init__(self, path=None):
        self.path = path
        self._lock = threading.Lock()
        settings = load_settings(path)
        self.follow_screen = bool(settings["follow_screen_brightness"])
        self.level = clamp_level(settings["brightness_level"])

    def get(self):
        with self._lock:
            return {"follow_screen": self.follow_screen, "level": self.level}

    def set(self, follow_screen=None, level=None, save=True):
        with self._lock:
            if follow_screen is not None:
                self.follow_screen = bool(follow_screen)
            if level is not None:
                self.level = clamp_level(level)
            state = {"follow_screen": self.follow_screen, "level": self.level}
        if save:
            self.save()
        return state

    def save(self):
        state = self.get()
        settings = load_settings(self.path)
        settings["follow_screen_brightness"] = state["follow_screen"]
        settings["brightness_level"] = state["level"]
        return save_settings(settings, self.path)

    def ratio(self, read_screen_brightness):
        """Brightness from 0.0 to 1.0 for the main loop: the screen's, or the chosen level."""
        with self._lock:
            follow, level = self.follow_screen, self.level
        if follow:
            return read_screen_brightness()
        return level / 100.0
