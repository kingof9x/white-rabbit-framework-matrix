# The monitor's keyboard shortcuts and which of them are turned on.
# Saved as "shortcuts": {letter: true/false} in settings.json; a shortcut that isn't listed there is on.

# Internal Dependencies
from led_mon import app_settings, platform_utils

# (letter, name, what it does)
SHORTCUTS = [
    ("i", "Show app names", "Shows the name of each app on the panels for a few seconds."),
    ("n", "Next app", "Skips to the next app on both panels."),
    ("f", "Freeze app switching", "Keeps the current apps on the panels instead of rotating."),
    ("u", "Unfreeze app switching", "Goes back to rotating through the apps."),
]
LETTERS = [letter for letter, _, _ in SHORTCUTS]

# Windows needs all three modifiers so the shortcuts don't take Alt+F (File menus) from other programs
MODIFIERS_TEXT = "Ctrl+Alt+Shift" if platform_utils.IS_WINDOWS else "Alt"

_enabled = None


def keys_text(letter):
    return f"{MODIFIERS_TEXT}+{letter.upper()}"


def load(settings=None):
    """{letter: on/off} for every shortcut, from settings.json."""
    saved = (settings or app_settings.load_settings()).get("shortcuts")
    saved = saved if isinstance(saved, dict) else {}
    return {letter: bool(saved.get(letter, True)) for letter in LETTERS}


def save(enabled, path=None):
    settings = app_settings.load_settings(path)
    settings["shortcuts"] = {letter: bool(enabled.get(letter, True)) for letter in LETTERS}
    reload(settings)
    return app_settings.save_settings(settings, path)


def reload(settings=None):
    """Read the on/off choices again (the monitor does this when the window changes them)."""
    global _enabled
    _enabled = load(settings)
    return dict(_enabled)


def is_enabled(letter):
    if _enabled is None:
        reload()
    return _enabled.get(letter, True)
