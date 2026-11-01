# System tray icon for running windowless on Windows (pystray + Pillow).
#
# Menu (left or right click):
#   Open White Rabbit... / Brightness...
#   Stop animation / Start animation   - the icon turns grey while stopped
#   Identify panels / Next app / Freeze app switching
#   Music mode                          - checkbox: the audio visualizer on both panels (Ctrl+Alt+Shift+M)
#   Libre Hardware Monitor               - "Find Libre Hardware Monitor..." (pick the exe) when it isn't found
#   Open Framework Control              - only when it is installed
#   Open config folder / Open log / Reload config
#   Turn off when lid closes            - checkbox (also when the screen turns off or the PC sleeps)
#   Start with Windows                  - checkbox
#   Quit

# Built In Dependencies
import logging
import os
import sys
import uuid

# Internal Dependencies
from led_mon import platform_utils, external_apps, startup, app_settings

log = logging.getLogger(__name__)

DISPLAY_NAME = platform_utils.DISPLAY_NAME
LIT_COLOR = (255, 176, 32, 255)
DIM_COLOR = (90, 90, 90, 255)


def make_icon_image(size=64, lit=True):
    """Draw the tray / exe icon: two LED-matrix panels of lit dots, grey when stopped."""
    from PIL import Image, ImageDraw
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    unit = size / 16
    for panel_x in (2, 9):
        draw.rounded_rectangle(
            [panel_x * unit, 1 * unit, (panel_x + 5) * unit, 15 * unit],
            radius=unit, fill=(40, 40, 40, 255),
        )
        for col in range(2):
            for row in range(6):
                on = lit and (row + col + panel_x) % 3 != 0
                color = LIT_COLOR if on else DIM_COLOR
                cx = (panel_x + 1.5 + col * 2) * unit
                cy = (2.5 + row * 2) * unit
                r = 0.7 * unit
                draw.ellipse([cx - r, cy - r, cx + r, cy + r], fill=color)
    return img


class AppFinder:
    """Remembers where Libre Hardware Monitor and Framework Control are; refreshed each time the menu opens."""

    def __init__(self, lhm_path=""):
        self.lhm_path = lhm_path
        self.lhm = None
        self.framework_control = None

    def refresh(self):
        try:
            # Read again each time: the Find Libre Hardware Monitor window saves the path it was shown
            configured = app_settings.load_settings().get("libre_hardware_monitor_path") or self.lhm_path
            self.lhm = external_apps.find_libre_hardware_monitor(configured)
        except Exception as e:
            log.error(f"Error looking for Libre Hardware Monitor: {e}")
            self.lhm = None
        try:
            self.framework_control = external_apps.find_framework_control()
        except Exception as e:
            log.error(f"Error looking for Framework Control: {e}")
            self.framework_control = None


def lhm_label(finder):
    return "Libre Hardware Monitor" if finder.lhm is not None else "Find Libre Hardware Monitor..."


def animation_label(paused):
    return "Start animation" if paused else "Stop animation"


def tooltip(paused):
    return DISPLAY_NAME + (" (stopped)" if paused else "")


def startup_checked(item):
    try:
        return startup.is_enabled()
    except Exception as e:
        log.error(f"Could not read the startup setting: {e}")
        return False


def build_menu(pystray, actions, finder, is_paused, is_frozen, edit_layout_available, startup_supported=None,
               is_turn_off_with_lid=None, is_music_mode=None):
    """The tray menu. actions maps a name to the function called when that item is chosen."""
    startup_supported = startup.supported() if startup_supported is None else startup_supported
    item = pystray.MenuItem
    return pystray.Menu(
        item("Open White Rabbit...", actions["edit_layout"], default=True, visible=edit_layout_available),
        item("Brightness...", actions["brightness"]),
        pystray.Menu.SEPARATOR,
        item(lambda i: animation_label(is_paused()), actions["toggle_animation"]),
        item("Identify panels", actions["identify"]),
        item("Next app", actions["next_app"]),
        item("Freeze app switching", actions["toggle_freeze"], checked=lambda i: is_frozen()),
        item("Music mode", actions.get("toggle_music"),
             checked=lambda i: bool(is_music_mode and is_music_mode()),
             visible=is_music_mode is not None and "toggle_music" in actions),
        pystray.Menu.SEPARATOR,
        item(lambda i: lhm_label(finder), actions["open_lhm"]),
        item("Open Framework Control", actions["open_fc"], visible=lambda i: finder.framework_control is not None),
        pystray.Menu.SEPARATOR,
        item("Open config folder", actions["open_folder"]),
        item("Open log", actions["open_log"]),
        item("Reload config", actions["reload_config"]),
        pystray.Menu.SEPARATOR,
        item("Turn off when lid closes", actions.get("toggle_lid"),
             checked=lambda i: bool(is_turn_off_with_lid and is_turn_off_with_lid()),
             visible=is_turn_off_with_lid is not None and "toggle_lid" in actions),
        item("Start with Windows", actions["toggle_startup"], checked=startup_checked, visible=startup_supported),
        pystray.Menu.SEPARATOR,
        item("Quit", actions["quit"]),
    )


# Shell_NotifyIcon codes (shellapi.h)
NIM_ADD = 0
NIM_MODIFY = 1
NIM_DELETE = 2
NIF_GUID = 0x20

# Fixed namespace for the tray icon's GUID; changing it makes Windows forget the user's "always show" choice
TRAY_GUID_NAMESPACE = uuid.UUID("3f1c7a52-9e4b-4d8a-b6f0-5a2e8c1d7b94")


def tray_icon_guid(exe_path):
    """The GUID Windows remembers the tray icon by: the same every run of one exe, different for another copy.

    Windows ties a tray icon GUID to the exe's location and refuses it from anywhere else, so the
    installed app and a portable exe in Downloads each get their own.
    """
    path = os.path.normcase(os.path.abspath(exe_path)).lower()
    return uuid.uuid5(TRAY_GUID_NAMESPACE, path)


class StableTrayIdMixin:
    """Give the tray icon a fixed GUID so Windows lists it under Settings > Personalization > Taskbar >
    Other system tray icons, and keeps it there (with its on/off choice) from one run to the next.

    pystray identifies the icon only by its window handle, which is new every run. Subclasses provide
    _send(code, flags, **fields) -> bool and set _tray_guid (a ctypes GUID, or None to leave pystray's way).
    """
    _tray_guid = None

    def _message(self, code, flags, **kwargs):
        if self._tray_guid is not None:
            guid_fields = dict(kwargs, guidItem=self._tray_guid)
            if code == NIM_ADD:
                # An icon left behind by a crashed run still holds the GUID and would block adding it again
                self._send(NIM_DELETE, NIF_GUID, guidItem=self._tray_guid)
            if self._send(code, flags | NIF_GUID, **guid_fields):
                return
            if code != NIM_ADD:
                return
            # Never leave the user without an icon: fall back to pystray's way for the rest of this run
            log.warning("Windows refused the tray icon's GUID; showing it without one")
            self._tray_guid = None
        self._send(code, flags, **kwargs)


def _win32_guid(win32, value):
    guid = win32.NOTIFYICONDATAW.GUID()
    import ctypes
    ctypes.memmove(ctypes.byref(guid), value.bytes_le, 16)
    return guid


def _icon_class(pystray, finder):
    """On Windows, open the menu on left click too, look for the apps again before showing it, and give the
    icon a fixed GUID so Windows remembers it."""
    if not platform_utils.IS_WINDOWS:
        return pystray.Icon
    import ctypes
    from pystray._util import win32

    tray_guid = None
    if platform_utils.is_frozen():
        # Only the exe: when run from source the program is python.exe, shared with other scripts
        try:
            tray_guid = _win32_guid(win32, tray_icon_guid(sys.executable))
        except Exception as e:
            log.warning(f"No tray icon GUID: {e}")

    class ClickMenuIcon(StableTrayIdMixin, pystray.Icon):
        _tray_guid = tray_guid

        def _send(self, code, flags, **kwargs):
            return bool(win32.Shell_NotifyIcon(code, win32.NOTIFYICONDATAW(
                cbSize=ctypes.sizeof(win32.NOTIFYICONDATAW),
                hWnd=self._hwnd,
                uFlags=flags,
                **kwargs)))

        def _on_notify(self, wparam, lparam):
            if lparam in (win32.WM_LBUTTONUP, win32.WM_RBUTTONUP):
                finder.refresh()
                self._update_menu()  # visibility of the app items is decided when the menu is built
                lparam = win32.WM_RBUTTONUP
            return super()._on_notify(wparam, lparam)

    return ClickMenuIcon


def run_tray(identify, next_app, set_freeze, is_frozen, reload_config, config_dir, log_file, quit,
             edit_layout=None, set_paused=None, is_paused=None, show_brightness=None, lhm_path="",
             set_turn_off_with_lid=None, is_turn_off_with_lid=None, set_music_mode=None, is_music_mode=None):
    """Show the tray icon and block until Quit is chosen."""
    import pystray

    platform_utils.enable_dpi_awareness()
    finder = AppFinder(lhm_path)
    finder.refresh()

    def notify(icon, message):
        if not message:
            return
        log.warning(message)
        try:
            icon.notify(message, DISPLAY_NAME)
        except Exception:
            pass

    def open_folder(icon, item):
        try:
            platform_utils.open_path(config_dir)
        except Exception as e:
            log.error(f"Could not open {config_dir}: {e}")

    def open_log(icon, item):
        try:
            platform_utils.open_path(log_file)
        except Exception as e:
            log.error(f"Could not open {log_file}: {e}")

    def open_editor(icon, item):
        try:
            edit_layout()
        except Exception as e:
            log.error(f"Could not open the layout editor: {e}")

    def open_brightness(icon, item):
        try:
            show_brightness()
        except Exception as e:
            notify(icon, f"Could not open the brightness slider: {e}")

    def toggle_animation(icon, item):
        paused = not is_paused()
        set_paused(paused)
        icon.icon = make_icon_image(lit=not paused)
        icon.title = tooltip(paused)

    def toggle_freeze(icon, item):
        set_freeze(not is_frozen())

    def open_lhm(icon, item):
        finder.refresh()
        if finder.lhm is None:
            # Not found in the usual places: let the user show where it is. The file picker runs in
            # its own process, since a dialog on the tray's thread freezes (it saves the path and opens it)
            try:
                platform_utils.launch_self("--find-lhm")
            except Exception as e:
                notify(icon, f"Could not open the file picker: {e}")
            return
        notify(icon, external_apps.open_target(finder.lhm, "Libre Hardware Monitor"))

    def open_fc(icon, item):
        finder.refresh()
        notify(icon, external_apps.open_target(finder.framework_control, "Framework Control"))

    def toggle_startup(icon, item):
        try:
            enable = not startup.is_enabled()
            startup.set_enabled(enable)
            log.info("Start with Windows " + ("on" if enable else "off"))
        except Exception as e:
            notify(icon, f"Could not change the startup setting: {e}")
        icon.update_menu()

    def toggle_lid(icon, item):
        try:
            set_turn_off_with_lid(not is_turn_off_with_lid())
        except Exception as e:
            notify(icon, f"Could not change the lid setting: {e}")
        icon.update_menu()

    def toggle_music(icon, item):
        try:
            set_music_mode(not is_music_mode())
        except Exception as e:
            notify(icon, f"Could not change music mode: {e}")
        icon.update_menu()

    def on_quit(icon, item):
        log.info("Quit requested from tray")
        quit()
        icon.stop()

    actions = {
        "edit_layout": open_editor,
        "brightness": open_brightness,
        "toggle_animation": toggle_animation,
        "identify": lambda icon, item: identify(),
        "next_app": lambda icon, item: next_app(),
        "toggle_freeze": toggle_freeze,
        "open_lhm": open_lhm,
        "open_fc": open_fc,
        "open_folder": open_folder,
        "open_log": open_log,
        "reload_config": lambda icon, item: reload_config(),
        "toggle_startup": toggle_startup,
        "quit": on_quit,
    }
    if set_turn_off_with_lid is not None and is_turn_off_with_lid is not None:
        actions["toggle_lid"] = toggle_lid
    else:
        is_turn_off_with_lid = None
    if set_music_mode is not None and is_music_mode is not None:
        actions["toggle_music"] = toggle_music
    else:
        is_music_mode = None
    menu = build_menu(pystray, actions, finder, is_paused, is_frozen, edit_layout is not None,
                      is_turn_off_with_lid=is_turn_off_with_lid, is_music_mode=is_music_mode)
    icon_class = _icon_class(pystray, finder)
    icon = icon_class(platform_utils.APP_NAME, make_icon_image(lit=not is_paused()), tooltip(is_paused()), menu)
    icon.run()
