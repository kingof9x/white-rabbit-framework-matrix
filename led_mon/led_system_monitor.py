# Built In Dependencies
from threading import Thread
import threading
import time
import queue
import sys
import re
from argparse import ArgumentParser, ArgumentDefaultsHelpFormatter
import importlib.util
import sys
import os
from collections import defaultdict
import logging

# Internal Dependencies
from led_mon import platform_utils
from led_mon.platform_utils import IS_WINDOWS, IS_LINUX
# Load API keys before the plugins are imported (via drawing) and read them
platform_utils.load_env_file()
from led_mon.drawing import draw_outline_border, draw_ids, draw_id, DrawingThread, SLEEP_PANEL, LatestFrameQueue
from led_mon.monitors import CPUMonitor, MemoryMonitor, BatteryMonitor, DiskMonitor, NetworkMonitor, get_monitor_brightness
from led_mon import shared_state
from led_mon.shared_state import discover_led_devices
from led_mon.serial_ports import close_all as close_all_ports, release as release_held_ports
from led_mon.rendering import make_base_app_functions, build_app_kwargs, render_panel, SMOOTH_APPS
from led_mon import animations
from led_mon import control
from led_mon import fps
from led_mon import shortcuts
from led_mon import app_settings
from led_mon import effects
from led_mon import now_playing
from led_mon.panel_names import name_grid

# External Dependencies
import numpy as np
from yaml import safe_load
# evdev (direct keyboard device access) only exists on Linux; elsewhere only pynput is used
if IS_LINUX:
    import evdev
    from evdev import ecodes
else:
    evdev = None
    ecodes = None

# Optional cross-platform keyboard input; on some Linux Wayland setups this may be unavailable.
# Not used on Windows: pynput hooks every keystroke, which antivirus scanners flag as a keylogger,
# so Windows uses registered hotkeys instead (led_mon/hotkeys.py).
try:
    if IS_WINDOWS:
        raise ImportError("Windows uses registered hotkeys")
    from pynput.keyboard import Key, Listener
    PYNPUT_AVAILABLE = True
except Exception:
    Key = None
    Listener = None
    PYNPUT_AVAILABLE = False
from serial.tools import list_ports

log = logging.getLogger(__name__)

LOG_LEVELS = {
    "debug": logging.DEBUG,
    "info": logging.INFO,
    "warning": logging.WARNING,
    "error": logging.ERROR,
}

log_level = LOG_LEVELS[os.environ.get("LOG_LEVEL", "warning").lower()]
log.setLevel(log_level)

is_frozen = platform_utils.is_frozen

MODIFIER_KEYS = [('KEY_RIGHTALT', 100), ('KEY_LEFTALT', 56)]
# Key press to show widget IDs
KEY_I = ('KEY_I', 23)
# Keypress to force advance to next widget without waiting for time slice to expire
KEY_N = ('KEY_N', 49)
# Used to ensure that advance to next widget from Alt-N keypress occurs only once until key is released and perssed again
global freeze_app_switching
freeze_app_switching = False

# Set to stop the main loop (tray Quit); set reload_config_event to re-read the config file
shutdown_event = threading.Event()
reload_config_event = threading.Event()
# Show app IDs (as with Alt+I) until this monotonic time; used by the tray "Identify panels" item
id_display_until = 0.0
# Panels with a name (settings.json "panel_names", set in the White Rabbit window) show it first,
# until this time, then the app IDs
id_names_until = 0.0
id_names = {}
NAME_SECONDS = 2.0

def request_identify(seconds=3.0):
    global id_display_until, id_names_until, id_names
    now = time.monotonic()
    try:
        id_names = app_settings.load_panel_names()
    except Exception as e:
        log.warning(f"Could not read the panel names: {e}")
        id_names = {}
    id_names_until = now + NAME_SECONDS if id_names else now
    id_display_until = id_names_until + seconds

def identify_name(side):
    """The name to show on this side's panel right now while identifying, or None."""
    if time.monotonic() < id_names_until:
        return id_names.get(side) or None
    return None

# Still frames sent from the layout editor: {'left': grid, 'right': grid}, shown until preview_until
preview_frames = {}
preview_until = 0.0
preview_lock = threading.Lock()

def show_preview(frames, seconds):
    global preview_frames, preview_until
    with preview_lock:
        preview_frames = frames
        preview_until = time.monotonic() + seconds

def clear_preview():
    global preview_until
    with preview_lock:
        preview_until = 0.0

def active_preview():
    with preview_lock:
        if time.monotonic() < preview_until:
            return preview_frames
    return None

def request_next_app():
    global evdev_next_key_pressed
    # One-shot flag, cleared by the main loop once the apps have advanced
    evdev_next_key_pressed = True

def set_freeze(frozen):
    global freeze_app_switching
    freeze_app_switching = frozen

def is_frozen_switching():
    return freeze_app_switching

# More control channel commands, added by White Rabbit: {cmd: handler(message) -> reply dict}
extra_control_handlers = {}

# Set while the animation is stopped from the tray: the panels go dark until it is started again
display_paused = threading.Event()

def set_paused(paused):
    if paused:
        display_paused.set()
    else:
        display_paused.clear()
    log.info("Animation " + ("stopped" if paused else "started"))

def is_paused():
    return display_paused.is_set()

# Music mode (tray, Ctrl+Alt+Shift+M or the window's Audio tab): both panels show the audio visualizer,
# with the look last picked in the Audio tab, until it is turned off. Not remembered after a restart.
music_mode = threading.Event()
# Set when the Audio tab changes the look, so the visualizer starts again with it
music_restart = threading.Event()

def set_music_mode(on):
    if on:
        # Panels lent to the White Rabbit window ignore the monitor's drawing; music mode takes them back
        released = release_held_ports()
        if released:
            log.info(f"Music mode took back the panels lent to the window: {', '.join(released)}")
        music_mode.set()
    else:
        music_mode.clear()
    log.info("Music mode " + ("on" if on else "off"))

def is_music_mode():
    return music_mode.is_set()

def toggle_music_mode():
    set_music_mode(not is_music_mode())

def handle_music_request(message):
    """Control channel command 'music': action on | off | toggle | reload | status."""
    action = message.get('action', 'status')
    if action == 'on':
        set_music_mode(True)
    elif action == 'off':
        set_music_mode(False)
    elif action == 'toggle':
        toggle_music_mode()
    elif action == 'reload':
        music_restart.set()
    return {'music_mode': is_music_mode()}

# Set while the panels should be off (lid closed, screen off, going to sleep, or the laptop's own screen not in
# use, each as its setting allows): the panels sleep until then
power_sleep = threading.Event()
power_watcher = None

LID_SETTING = "turn_off_with_lid"
SCREEN_UNUSED_SETTING = "turn_off_when_screen_unused"

def _power_setting(key):
    return bool(app_settings.load_settings().get(key, app_settings.DEFAULT_SETTINGS.get(key, True)))

def apply_power_state(state=None):
    """Turn the panels off or back on from what Windows last said and the two settings."""
    state = state if state is not None else (power_watcher.state if power_watcher is not None else None)
    if state is not None and state.wants_sleep(_power_setting(LID_SETTING), _power_setting(SCREEN_UNUSED_SETTING)):
        power_sleep.set()
    else:
        power_sleep.clear()

def on_power_change(state):
    apply_power_state(state)

def _set_power_setting(key, enabled, label):
    settings = app_settings.load_settings()
    settings[key] = bool(enabled)
    app_settings.save_settings(settings)
    log.info(f"{label} " + ("on" if enabled else "off"))
    apply_power_state()

def set_turn_off_with_lid(enabled):
    """Tray checkbox: turn the panels off when the lid closes or the screen turns off."""
    _set_power_setting(LID_SETTING, enabled, "Turn off with lid")

def is_turn_off_with_lid():
    return _power_setting(LID_SETTING)

def set_turn_off_when_screen_unused(enabled):
    """Tray checkbox: turn the panels off when the laptop's own screen isn't in use (docked, "Second screen only")."""
    _set_power_setting(SCREEN_UNUSED_SETTING, enabled, "Turn off when laptop screen isn't used")

def is_turn_off_when_screen_unused():
    return _power_setting(SCREEN_UNUSED_SETTING)

def handle_power_request(message):
    """Control channel command 'power': change either setting (when given) and reply with both."""
    if LID_SETTING in message:
        set_turn_off_with_lid(message[LID_SETTING])
    if SCREEN_UNUSED_SETTING in message:
        set_turn_off_when_screen_unused(message[SCREEN_UNUSED_SETTING])
    return {LID_SETTING: is_turn_off_with_lid(), SCREEN_UNUSED_SETTING: is_turn_off_when_screen_unused()}

# Plugging things in and out: a ripple from the port, and a drain through it (effects.py, plug_events.py)
plug_watcher = None
# The last thing plugged in or out, for the Effects tab: {'kind', 'position', 'plugged_in', 'name', 'key', 'how', 'time'}
last_plug = {}

def on_plug(kind, position, plugged_in, info):
    settings = effects.manager.settings
    last_plug.clear()
    last_plug.update(dict(info or {}, kind=kind, position=position, plugged_in=plugged_in, time=time.time()))
    wanted = {"charger": settings["plug_charger"], "headphones": settings["plug_headphones"],
              "usb": settings["plug_usb"]}.get(kind, True)
    if wanted:
        effects.manager.plug(position, plugged_in)
    if kind == "charger" and settings["text_charger"]:
        effects.manager.say("Charging" if plugged_in else "On battery")

def on_song(song):
    """A new song started playing: show its name on the text line, if the Effects tab says so."""
    if effects.manager.settings["text_song"]:
        effects.manager.say(f"Now playing: {song}")

now_playing.watcher.on_song = on_song

# What each panel showed last, so the slide only plays when the apps really change
shown_apps = {}

def apps_key(panel_apps):
    """The apps a panel shows, comparable from one frame to the next."""
    return repr([(a.get('name'), a.get('args')) for a in panel_apps if a.get('display', True)])

def learned_ports():
    return effects.manager.settings.get("learned_ports", {})

def handle_effects_request(message):
    """The Effects tab: reload the settings, try an effect, show a message or correct a port."""
    action = message.get('action')
    if action == 'reload':
        effects.load()
    elif action == 'try':
        effects.manager.plug(message.get('position'), message.get('event') != 'out')
    elif action == 'say':
        effects.manager.text.say(message.get('text', ''))
    elif action == 'learn':
        key, position = message.get('key'), message.get('position')
        if key and effects.parse_position(position)[0]:
            settings = effects.load()
            settings["learned_ports"] = dict(settings["learned_ports"], **{key: position})
            effects.save(settings)
            effects.load()
            if last_plug.get('key') == key:
                last_plug['position'], last_plug['how'] = position, 'learned'
    return {'last_plug': dict(last_plug), 'panels': sorted(effects.manager.panels)}

extra_control_handlers['effects'] = handle_effects_request

class LowBatteryNotice:
    """Says so on the text line once when the battery drops to 20% (again after charging)."""
    LEVEL = 20

    def __init__(self):
        self.said = False
        self.next_check = 0.0

    def check(self, now):
        if now < self.next_check:
            return
        self.next_check = now + 30.0
        try:
            import psutil
            battery = psutil.sensors_battery()
        except Exception:
            battery = None
        if battery is None:
            return
        if battery.power_plugged or battery.percent > self.LEVEL + 2:
            self.said = False
        elif battery.percent <= self.LEVEL and not self.said:
            self.said = True
            if effects.manager.settings["text_battery"]:
                effects.manager.say(f"Battery {int(battery.percent)}%")

low_battery = LowBatteryNotice()

def set_tv_effect(enabled):
    """Tray checkbox: the old-TV effect when the panels turn off and back on."""
    settings = app_settings.load_settings()
    settings["tv_effect"] = bool(enabled)
    app_settings.save_settings(settings)
    log.info("Old-TV effect " + ("on" if enabled else "off"))

def is_tv_effect():
    return bool(app_settings.load_settings().get("tv_effect", True))

def play_tv_effect(queues_and_frames):
    """Send each panel its old-TV frames, a frame every animations.TV_FRAME_SECONDS, all panels together.

    queues_and_frames: [(drawing queue, [frames])]. Blocks until the longest list has played."""
    steps = max((len(frames) for _, frames in queues_and_frames), default=0)
    for i in range(steps):
        for draw_queue, frames in queues_and_frames:
            if i < len(frames):
                draw_queue.put((frames[i], False))
        time.sleep(animations.TV_FRAME_SECONDS)

# pynput key helpers: Windows reports left/right Alt as alt_l / alt_r (or alt_gr)
def is_alt_key(key):
    if Key is None:
        return False
    return key in tuple(getattr(Key, name) for name in ('alt', 'alt_l', 'alt_r', 'alt_gr') if hasattr(Key, name))

def key_char(key):
    char = getattr(key, 'char', None)
    if char:
        return char.lower()
    # With Alt held, Windows may report only the virtual key code
    vk = getattr(key, 'vk', None)
    if vk is not None and 0x41 <= vk <= 0x5A:
        return chr(vk).lower()
    return None

def find_keyboard_device():
    """Auto-detect keyboard input device from /dev/input/event*"""
    if evdev is None:
        return None
    try:
        devices = [evdev.InputDevice(path) for path in evdev.list_devices()]
        for device in devices:
            # Look for devices with keyboard capabilities (EV_KEY with standard keys)
            capabilities = device.capabilities(verbose=False)
            if evdev.ecodes.EV_KEY in capabilities:
                keys = capabilities[evdev.ecodes.EV_KEY]
                # Check if device has typical keyboard keys (letters and modifiers)
                has_letters = any(k in keys for k in range(evdev.ecodes.KEY_Q, evdev.ecodes.KEY_P + 1))
                has_modifiers = any(k in keys for k in [evdev.ecodes.KEY_LEFTALT, evdev.ecodes.KEY_RIGHTALT])
                if has_letters and has_modifiers:
                    return device.path
    except Exception as e:
        log.warning(f"Warning: Could not auto-detect keyboard device: {e}")
    return None

def evcode_keyloop():
    global freeze_app_switching, evdev_next_key_pressed
    kbd_path = find_keyboard_device()
    keys_pressed = set()
    if kbd_path:
        try:
            device = evdev.InputDevice(kbd_path)
            for event in device.read_loop():
                if event.type == ecodes.EV_KEY:
                    if event.value == 1:
                        keys_pressed.add(event.code)
                    elif event.value == 0:
                        keys_pressed.discard(event.code)

                    if (ecodes.KEY_LEFTALT in keys_pressed or ecodes.KEY_RIGHTALT in keys_pressed) and ecodes.KEY_F in keys_pressed and shortcuts.is_enabled('f'):
                        freeze_app_switching = True
                    elif (ecodes.KEY_LEFTALT in keys_pressed or ecodes.KEY_RIGHTALT in keys_pressed) and ecodes.KEY_U in keys_pressed and shortcuts.is_enabled('u'):
                        freeze_app_switching = False

                    if (ecodes.KEY_LEFTALT in keys_pressed or ecodes.KEY_RIGHTALT in keys_pressed) and ecodes.KEY_N in keys_pressed and shortcuts.is_enabled('n'):
                        evdev_next_key_pressed = True
        except (PermissionError, FileNotFoundError, OSError) as e:
            log.warning(f"Warning: Cannot access keyboard device {kbd_path}: {e}")
                
if evdev is not None:
    Thread(target=evcode_keyloop, daemon=True).start()


def get_config(args):
    # Check for --config-file program arg first, and then CONFIG_FILE environment variable (used by NixOS module),
    # then config-local.yaml / config.yaml (next to the app on Linux, in %APPDATA%\led_mon on Windows)
    config_file = platform_utils.resolve_config_file(args.config_file)
    with open(config_file, 'r') as f:
        config = safe_load(f)
    # Optional left-port / right-port (e.g. COM4 or /dev/ttyACM0) and left-location / right-location
    # (USB position, e.g. 1-4.2) override panel auto-detection
    shared_state.set_overrides_from_config(config)
    return config

def find_led_devices():
    led_devices = discover_led_devices() or []
    if IS_WINDOWS and not led_devices:
        # When started at login, USB devices may still be enumerating
        for _ in range(15):
            time.sleep(2)
            led_devices = discover_led_devices() or []
            if led_devices:
                break
    return led_devices

def list_apps(base_apps, plugin_apps, quads):
    max_len = max(map(lambda x: len(x), base_apps + plugin_apps))
    print("Installed Apps:")
    print("   " + "Name".ljust(max_len+3, ' ') + "Source".ljust(12, " ") + "Configuration")
    configured_apps = defaultdict(list)
    for quad in quads.values():
        for app in quad:
            del app["app"] # Just the list name in the yaml file, value will be set to None
            configured_apps[app["name"]].append(app)
    for app in [*base_apps , *plugin_apps]:
        if not app in configured_apps:
            print(f"   {app.ljust(max_len+3, ' ')}", end='')
            print(f"{'plugin app'.ljust(12, ' ')if app in plugin_apps else 'base app'.ljust(12, ' ')}", end='')
            print(f"None")
        for _app in configured_apps[app]:
            print(f"   {app.ljust(max_len+3, ' ')}", end='')
            print(f"{'plugin app'.ljust(12, ' ')if app in plugin_apps else 'base app'.ljust(12, ' ')}", end='')
            print(f"{_app}")
        
device = None
def app(args, base_apps, plugin_apps):    
    # Initialize evdev device for key listening if not disabled
    global device
    if not args.no_key_listener and evdev is not None:
        # Try to auto-detect keyboard device first
        kbd_path = find_keyboard_device()
        if kbd_path:
            try:
                device = evdev.InputDevice(kbd_path)
                log.info(f"Using keyboard device: {kbd_path}")
            except (PermissionError, FileNotFoundError, OSError) as e:
                log.warning(f"Warning: Cannot access keyboard device {kbd_path}: {e}")
                log.warning("Try running: sudo usermod -a -G input $USER (then log out and back in)")
                log.warning("Or use --no-key-listener to disable keyboard monitoring.")
                args.no_key_listener = True
                device = None
        else:
            log.warning("Warning: Could not find a suitable keyboard device for evdev monitoring.")
            log.warning("Key listening will be limited to pynput only (if available).")
            device = None

    ################################################################################
    ### Parse config file to enable control of apps by quadrant and by time slice ##
    ################################################################################
    config = get_config(args)
    # Ripples, sliding and the text line (Effects tab)
    try:
        effects.load()
    except Exception as e:
        log.error(f"Could not read the effects settings: {e}")
    # Track index of active app, for cycling through apps in a quadrant by time slice
    app_idx = defaultdict(int)
    # Each app optionally specifes how long it stays active before cycling to the next one
    app_duration = defaultdict(int)

    def apply_config(config):
        nonlocal duration, quads, top_left, bottom_left, top_right, bottom_right
        duration = config['duration'] #Default config to be applied if not set in an app
        quads = config['quadrants']
        top_left,  bottom_left, top_right, bottom_right, = \
            quads['top-left'], quads['bottom-left'], quads['top-right'], quads['bottom-right']
        app_idx.clear()
        for quad in quads:
            app_idx[quad] = 0
        app_duration.clear()
        for quad in quads.items():
            for app in quad[1]:
                # Keyed by quadrant too, so the same app can have a different duration in each quadrant
                app_duration[(quad[0], app["name"])] = int(app.get("duration", duration))

    duration = quads = top_left = bottom_left = top_right = bottom_right = None
    apply_config(config)

    if args.list_apps:
        list_apps(base_apps, plugin_apps, quads)
        sys.exit()

    # Only one instance may drive the panels; a second launch exits silently (Windows)
    if not platform_utils.acquire_single_instance():
        log.info("White Rabbit is already running")
        sys.exit(0)

    led_devices = find_led_devices()
    if not len(led_devices):
        log.error("No LED devices found")
        if use_tray(args):
            platform_utils.show_error("No LED Matrix panels were found. Check that the modules are installed, "
                                      "and that no other program (such as the LED Matrix GUI) is using them.")
        sys.exit(0)
    elif len(led_devices) == 1:
        log.warning(f"Only one LED device found ({led_devices[0]}). The right panel will be drawn when one appears")
    else:
        log.info(f"Found LED devices: Left: {led_devices[0]}, Right: {led_devices[1]}")
    # Use the port name when the USB location is unknown
    locations = list(map(lambda x: x[0] or x[1], led_devices))
    drawing_queues = []
    
    # Track key presses to reveal app ID in each quadrant or panel
    global alt_pressed
    alt_pressed = False
    global i_pressed
    i_pressed = False
    global n_pressed
    n_pressed = False
    global evdev_next_key_pressed
    evdev_next_key_pressed = False
    
    # Set up monitors and brightness parameters
    min_background_brightness = 12
    max_background_brightness = 35
    min_foreground_brightness = 24
    max_foreground_brightness = 160
    # Follows the screen brightness unless a level was picked with the brightness slider
    brightness = app_settings.Brightness()

    cpu_monitor = CPUMonitor()
    memory_monitor = MemoryMonitor()
    battery_monitor = BatteryMonitor()
    disk_monitor = DiskMonitor()
    network_monitor = NetworkMonitor()

    # One drawing thread per side. Each finds its panel again when the modules are moved to other
    # slots, and the right one also starts drawing when a second panel shows up later (some slots
    # come up a few seconds after the first one).
    left_drawing_queue = LatestFrameQueue()
    left_drawing_thread = DrawingThread(locations[0], left_drawing_queue, side_index=0)
    left_drawing_thread.set_animate(False)
    left_drawing_thread.start()    
    drawing_queues.append(left_drawing_queue)

    right_drawing_queue = LatestFrameQueue()
    right_drawing_thread = DrawingThread(locations[1] if len(locations) > 1 else None, right_drawing_queue,
                                         side_index=1)
    right_drawing_thread.set_animate(False)
    right_drawing_thread.start()
    drawing_queues.append(right_drawing_queue)
    drawing_threads = [left_drawing_thread, right_drawing_thread]
    
    app_functions = make_base_app_functions(cpu_monitor, memory_monitor, battery_monitor, disk_monitor, network_monitor)

    panel_scope_conflicts_logged = set()
    suppressed_panel_apps = {
        'top-left': None,
        'bottom-left': None,
        'top-right': None,
        'bottom-right': None,
    }

    def get_panel_scope_owner(panel_name, top_quadrant_name, top_app, bottom_quadrant_name, bottom_app):
        top_claims_panel = top_app.get("display", True) and top_app.get("scope", None) == "panel"
        bottom_claims_panel = bottom_app.get("display", True) and bottom_app.get("scope", None) == "panel"

        if top_claims_panel and bottom_claims_panel:
            conflict_key = (panel_name, top_app.get("name"), bottom_app.get("name"))
            if conflict_key not in panel_scope_conflicts_logged:
                panel_scope_conflicts_logged.add(conflict_key)
                log.warning(
                    f"Both {top_quadrant_name} ({top_app.get('name')}) and {bottom_quadrant_name} ({bottom_app.get('name')}) "
                    f"claim scope='panel' on {panel_name} panel; preferring {top_quadrant_name}."
                )
            return top_quadrant_name, top_app
        if top_claims_panel:
            return top_quadrant_name, top_app
        if bottom_claims_panel:
            return bottom_quadrant_name, bottom_app
        return None, None

    def maybe_dispose_suppressed_panel_app(quadrant_name):
        if quadrant_name is None:
            return
        app = quads[quadrant_name][app_idx[quadrant_name]]
        dispose_fn = app.get('dispose-fn', None)
        if dispose_fn is None:
            return

        signature = (app.get('name'), app_idx[quadrant_name], dispose_fn)
        if suppressed_panel_apps.get(quadrant_name) == signature:
            return

        func = app_functions.get(dispose_fn)
        if func is None:
            log.error(f"Unrecognized dispose function '{dispose_fn}' for suppressed app '{app.get('name')}' in {quadrant_name}")
            suppressed_panel_apps[quadrant_name] = signature
            return

        try:
            func(**build_app_kwargs(app))
            suppressed_panel_apps[quadrant_name] = signature
        except Exception as e:
            log.error(f"Error disposing suppressed app {app.get('name')} in {quadrant_name}: {e}")
    
    def on_press(key):
        global alt_pressed, i_pressed, n_pressed, freeze_app_switching
        # If pynput is unavailable, this callback will not be used; guard anyway
        if not PYNPUT_AVAILABLE:
            return
        try:
            char = key_char(key)
            if char in shortcuts.LETTERS and not shortcuts.is_enabled(char):
                return
            if char == 'i':
                i_pressed = True
            elif char == 'n':
                # Advance once per Alt+N press (matches the evdev listener), not on every loop while held
                if alt_pressed and not n_pressed:
                    request_next_app()
                n_pressed = True
            elif char == 'f' and alt_pressed:
                freeze_app_switching = True
            elif char == 'u' and alt_pressed:
                freeze_app_switching = False
            elif char == 'm' and alt_pressed:
                toggle_music_mode()
            elif is_alt_key(key):
                alt_pressed = True
        except Exception:
            # Be defensive; ignore unexpected pynput key objects
            pass

    def on_release(key):
        global alt_pressed, i_pressed, n_pressed
        if not PYNPUT_AVAILABLE:
            return
        try:
            char = key_char(key)
            if char == 'i':
                i_pressed = False
            elif char == 'n':
                n_pressed = False
            elif is_alt_key(key):
                alt_pressed = False
        except Exception:
            pass
        
    #################################################
    ###      Load app functions from plugins      ###
    #################################################
    if not re.search(r"--disable-plugins|-dp", str(sys.argv)):
        # Try to find plugins directory - either in current dir or installed location
        plugins_dir = platform_utils.plugins_dir()
        for file in os.listdir(plugins_dir):
            if file.endswith('_plugin.py'):
                module_name = re.sub("_plugin.py", "", file)
                file_path = os.path.join(plugins_dir, file)
                spec = importlib.util.spec_from_file_location(module_name, file_path)
                module = importlib.util.module_from_spec(spec)
                sys.modules[module_name] = module
                spec.loader.exec_module(module)

                for obj in module.app_funcs:
                    app_functions[obj["name"]] = obj["fn"]


    # Track last draw time, to enable cycling through them for each quadrannt
    base_time_map = {
        # Each dict stores last draw time for each app
        'top-left': defaultdict(lambda: time.monotonic()),
        'bottom-left': defaultdict(lambda: time.monotonic()),
        'top-right': defaultdict(lambda: time.monotonic()),
        'bottom-right': defaultdict(lambda: time.monotonic()),
    }

    # Used to detect that the key listener was activated, for restoring anination mode after ID display
    global latch_key_combo
    latch_key_combo = False
    music_running = False
    music_args = {}

    def stop_music():
        nonlocal music_running
        if music_running and 'equalizer_dispose' in app_functions:
            for side in ('left', 'right'):
                try:
                    app_functions['equalizer_dispose'](side=side)
                except Exception as e:
                    log.error(f"Error stopping music mode on the {side} panel: {e}")
        music_running = False

    def run_music(foreground_value):
        """Start (or keep running) the audio visualizer on every panel for music mode."""
        nonlocal music_running
        if 'equalizer' not in app_functions:
            log.error("Music mode needs the audio visualizer plugin, which isn't loaded")
            set_music_mode(False)
            return
        if music_restart.is_set():
            music_restart.clear()
            stop_music()
        if not music_running:
            # Read the Audio tab's choices when music mode starts (or the tab changed them)
            music_args.clear()
            music_args.update(app_settings.load_music())
        music_running = True
        for side in ('left', 'right')[:len(drawing_queues)]:
            try:
                app_functions['equalizer'](None, np.zeros((9,34), dtype = int), foreground_value, 0,
                                           **dict(music_args, side=side))
            except Exception as e:
                log.error(f"Error running music mode on the {side} panel: {e}")

    def dispose_active_apps():
        # Music mode's visualizers draw on their own too
        stop_music()
        for quadrant, apps in quads.items():
            app = apps[app_idx[quadrant]]
            dispose_fn = app.get('dispose-fn', None)
            if dispose_fn and dispose_fn in app_functions:
                try:
                    app_functions[dispose_fn](**build_app_kwargs(app))
                except Exception as e:
                    log.error(f"Error disposing app {app.get('name')} in {quadrant}: {e}")

    def reload_config():
        try:
            new_config = get_config(args)
        except Exception as e:
            log.error(f"Could not reload config, keeping the current one: {e}")
            return
        dispose_active_apps()
        apply_config(new_config)
        for quadrant_times in base_time_map.values():
            quadrant_times.clear()
        for quadrant in suppressed_panel_apps:
            suppressed_panel_apps[quadrant] = None
        log.info("Config reloaded")

    previewing = False
    blanked = False
    panels_asleep = False
    # The last picture sent to each panel (None when an app draws on the panel itself), for the old-TV effect
    last_grids = {}

    def handle_control_request(message):
        cmd = message.get('cmd')
        if cmd == 'status':
            reply = {'ports': [t.port_name for t in drawing_threads if t.port_name],
                     'config_file': platform_utils.resolve_config_file(args.config_file),
                     'brightness': brightness.get()}
            if platform_utils.IS_WINDOWS:
                from led_mon import sensor_sources
                reply['sensor_source'] = sensor_sources.current().status()
            return reply
        if cmd == 'brightness':
            return {'brightness': brightness.set(message.get('follow_screen'), message.get('level'))}
        if cmd == 'reload':
            clear_preview()
            reload_config_event.set()
            return {}
        if cmd == 'preview':
            show_preview(message.get('frames', {}), float(message.get('seconds', 30)))
            return {}
        if cmd == 'resume':
            clear_preview()
            return {}
        if cmd == 'identify':
            request_identify()
            return {}
        if cmd == 'music':
            reply = handle_music_request(message)
            # What music mode hears, so the Audio tab can show it
            plugin = sys.modules.get('equalizer')
            if music_running and plugin is not None and hasattr(plugin, 'hearing'):
                reply['hearing'] = plugin.hearing()
            return reply
        if cmd == 'shortcuts':
            # The window turned shortcuts on or off in settings.json
            enabled = shortcuts.reload()
            start_hotkeys()
            return {'shortcuts': enabled}
        if cmd == 'power':
            # The window's "Turn off when..." checkboxes
            return handle_power_request(message)
        if cmd == 'sensor_source':
            # The window changed the temperature and fan source in settings.json, or asks what is being read
            from led_mon import sensor_sources
            source = sensor_sources.current()
            if message.get('reload'):
                source.reload()
            return {'sensor_source': source.status()}
        if cmd == 'quit':
            # The White Rabbit window is about to install an update: quit like the tray's Quit, after replying
            def quit_now():
                from led_mon import tray
                if not tray.quit_from_elsewhere():
                    shutdown_event.set()
            threading.Timer(0.5, quit_now).start()
            return {}
        if cmd in extra_control_handlers:
            return extra_control_handlers[cmd](message)
        return {'ok': False, 'error': f"Unknown command {cmd}"}

    # The White Rabbit window talks to the panels through this channel on Linux too
    control.serve(handle_control_request)

    def render_iteration(args):
        global latch_key_combo, next_key_fired, freeze_app_switching, evdev_next_key_pressed
        nonlocal previewing, blanked, panels_asleep, music_running
        try:
            if reload_config_event.is_set():
                reload_config_event.clear()
                reload_config()

            # Animation stopped from the tray, or the lid is closed: turn the panels off once, then wait
            lid_sleep = power_sleep.is_set()
            if display_paused.is_set() or lid_sleep:
                if not blanked:
                    # Stop apps that draw on their own (the equalizer) so they don't light the panels
                    dispose_active_apps()
                    if is_tv_effect():
                        # Old TV turning off: the picture squeezes into a line, then a dot that fades
                        play_tv_effect([(q, animations.tv_off_frames(last_grids.get(i), shared_state.foreground_value or 160))
                                        for i, q in enumerate(drawing_queues)])
                    for draw_queue in drawing_queues:
                        # Stop any panel animation, then draw a dark frame once it has stopped
                        draw_queue.put((np.zeros((9,34), dtype = int), False))
                        draw_queue.put((np.zeros((9,34), dtype = int), None))
                    blanked = True
                if lid_sleep and not panels_asleep:
                    # Also send the panels to sleep, so they turn their LEDs off. The first frame
                    # after the lid opens wakes them up again.
                    for draw_queue in drawing_queues:
                        draw_queue.put(SLEEP_PANEL)
                    panels_asleep = True
                time.sleep(0.25)
                # Restart panel animations when the display comes back, as after the ID display
                latch_key_combo = True
                return
            # Coming back on after the panels were off: play the old-TV switch-on into the first pictures
            tv_on = blanked and is_tv_effect()
            blanked = False
            panels_asleep = False

            screen_brightness = brightness.ratio(get_monitor_brightness)
            background_value = int(screen_brightness * (max_background_brightness - min_background_brightness) + min_background_brightness)
            foreground_value = int(screen_brightness * (max_foreground_brightness - min_foreground_brightness) + min_foreground_brightness)
            shared_state.foreground_value = foreground_value
            effects.manager.foreground = max(0, min(255, foreground_value))
            low_battery.check(time.monotonic())
            if effects.manager.settings["text_line"] and effects.manager.settings["text_song"]:
                # Keep asking what is playing, so a new song can be shown on the text line
                now_playing.watcher.want()
            
            # Clamp brightness values to the valid byte range
            background_value = max(0, min(255, background_value))
            foreground_value = max(0, min(255, foreground_value))

            grid = np.zeros((9,34), dtype = int)
            
            # Check for key combo using both evdev (if available) and pynput
            active_keys = device.active_keys(verbose=True) if device else []
            evdev_id_key_pressed = True if (MODIFIER_KEYS[0] in active_keys or MODIFIER_KEYS[1] in active_keys) and KEY_I in active_keys and device else False
            pynput_id_key_pressed = i_pressed and alt_pressed
            id_key_combo_active = (evdev_id_key_pressed or pynput_id_key_pressed) and not args.no_key_listener and shortcuts.is_enabled('i')
            # Tray "Identify panels" works even with --no-key-listener
            id_key_combo_active = id_key_combo_active or time.monotonic() < id_display_until
            shared_state.id_key_press_active = id_key_combo_active
            # evdev_next_key_pressed is a one-shot flag set by the evdev listener, pynput Alt+N and the tray
            next_key_combo_active = evdev_next_key_pressed

            # Track when an app is changed in either panel, used to manage animation state
            idx_changed = {
                left_drawing_queue: False,
            }
            if len(drawing_queues) > 1:
                idx_changed[right_drawing_queue] = False
            # A set of apps to be (potentially) disposed
            apps_to_dispose = []

            active_apps = {
                'top-left': top_left[app_idx['top-left']],
                'bottom-left': bottom_left[app_idx['bottom-left']],
                'top-right': top_right[app_idx['top-right']],
                'bottom-right': bottom_right[app_idx['bottom-right']],
            }

            left_owner_quadrant, _ = get_panel_scope_owner(
                "left",
                'top-left', active_apps['top-left'],
                'bottom-left', active_apps['bottom-left']
            )
            right_owner_quadrant, _ = get_panel_scope_owner(
                "right",
                'top-right', active_apps['top-right'],
                'bottom-right', active_apps['bottom-right']
            )

            suppressed_quadrants_pre_rotation = set()
            if left_owner_quadrant == 'top-left':
                suppressed_quadrants_pre_rotation.add('bottom-left')
            elif left_owner_quadrant == 'bottom-left':
                suppressed_quadrants_pre_rotation.add('top-left')
            if right_owner_quadrant == 'top-right':
                suppressed_quadrants_pre_rotation.add('bottom-right')
            elif right_owner_quadrant == 'bottom-right':
                suppressed_quadrants_pre_rotation.add('top-right')
            # Keep quadrant schedules aligned even when one quadrant is suppressed by
            # a sibling panel-scope app; otherwise manual Alt+N can create lasting skew.
            now = time.monotonic()
            # Now playing, set to skip, with nothing playing: both halves of its panel go on to the next page
            skipping_quadrants = set()
            for owner_quadrant in (left_owner_quadrant, right_owner_quadrant):
                if owner_quadrant is None:
                    continue
                owner = quads[owner_quadrant][app_idx[owner_quadrant]]
                if (owner['name'] == 'now-playing' and len(quads[owner_quadrant]) > 1
                        and now - base_time_map[owner_quadrant][owner['name']] >= 2
                        and now_playing.should_skip(owner.get('args'), now)):
                    panel_side = 'left' if 'left' in owner_quadrant else 'right'
                    skipping_quadrants.update(q for q in quads if panel_side in q)
            for quadrant, apps in quads.items():
                    app = apps[app_idx[quadrant]]
                    suppressed_for_rotation = quadrant in suppressed_quadrants_pre_rotation
                    can_force_next = next_key_combo_active
                    should_advance = (now - base_time_map[quadrant][app['name']] >= int(app_duration[(quadrant, app['name'])])) or can_force_next
                    if quadrant in skipping_quadrants:
                        should_advance = True
                    if should_advance and not freeze_app_switching:
                            if can_force_next:
                                evdev_next_key_pressed = False
                            # Suppressed quadrants still rotate state, but should not
                            # trigger draw-side animation/dispose transitions.
                            if not suppressed_for_rotation:
                                if 'left' in quadrant:
                                    idx_changed[left_drawing_queue] = True
                                elif len(drawing_queues) > 1:
                                    idx_changed[right_drawing_queue] = True
                                if 'dispose-fn' in app:
                                        apps_to_dispose.append(app)
                            app_idx[quadrant] = (app_idx[quadrant] + 1) % len(quads[quadrant])
                            app = apps[app_idx[quadrant]]
                            base_time_map[quadrant][app['name']] = now
            left_top_app = top_left[app_idx['top-left']]
            left_bottom_app = bottom_left[app_idx['bottom-left']]
            right_top_app = top_right[app_idx['top-right']]
            right_bottom_app = bottom_right[app_idx['bottom-right']]

            left_owner_quadrant, left_owner_app = get_panel_scope_owner(
                "left",
                'top-left', left_top_app,
                'bottom-left', left_bottom_app
            )
            right_owner_quadrant, right_owner_app = get_panel_scope_owner(
                "right",
                'top-right', right_top_app,
                'bottom-right', right_bottom_app
            )

            left_args = [left_owner_app] if left_owner_app else [left_top_app, left_bottom_app]
            right_args = [right_owner_app] if right_owner_app else [right_top_app, right_bottom_app]

            left_suppressed_quadrant = None
            if left_owner_quadrant == 'top-left':
                left_suppressed_quadrant = 'bottom-left'
            elif left_owner_quadrant == 'bottom-left':
                left_suppressed_quadrant = 'top-left'

            right_suppressed_quadrant = None
            if right_owner_quadrant == 'top-right':
                right_suppressed_quadrant = 'bottom-right'
            elif right_owner_quadrant == 'bottom-right':
                right_suppressed_quadrant = 'top-right'

            currently_suppressed_quadrants = set(filter(None, [left_suppressed_quadrant, right_suppressed_quadrant]))
            for quadrant in suppressed_panel_apps:
                if quadrant not in currently_suppressed_quadrants:
                    suppressed_panel_apps[quadrant] = None

            # Capture animating state so we can restart it if necessary after stopping it for ID display
            animating_left = any(arg.get("animate", False) for arg in left_args)
            animating_right = any(arg.get("animate", False) for arg in right_args)
            
            if id_key_combo_active:
                # Show app IDs for each quadrant or panel
                draw_outline_border(grid, background_value)
                # If app takes up entire panel, draw the panel app ID only
                if identify_name('left'):
                    grid = name_grid(identify_name('left'), foreground_value)
                elif left_owner_app:
                    draw_id(grid, left_owner_app['name'], foreground_value, args=left_owner_app.get('args', None))
                else:
                    draw_ids(grid, left_args[0]['name'], left_args[1]['name'], foreground_value,
                        targs=left_args[0].get('args', None), bargs=left_args[1].get('args', None))
                left_drawing_queue.put((grid, False))
                
                if len(drawing_queues) > 1:  # Right panel exists
                    grid = np.zeros((9,34), dtype = int)
                    draw_outline_border(grid, background_value)
                    if identify_name('right'):
                        grid = name_grid(identify_name('right'), foreground_value)
                    elif right_owner_app:
                        draw_id(grid, right_owner_app['name'], foreground_value, args=right_owner_app.get('args', None))
                    else:
                        draw_ids(grid, right_args[0]['name'], right_args[1]['name'], foreground_value,
                            targs=right_args[0].get('args', None), bargs=right_args[1].get('args', None))
                    right_drawing_queue.put((grid, False))
                time.sleep(0.1)
                latch_key_combo = True
                return

            # A still frame sent from the layout editor takes over both panels until it times out
            frames = active_preview()
            if frames is not None:
                if not previewing:
                    # Stop apps that draw on their own (the equalizer) so they don't overwrite the frame
                    dispose_active_apps()
                    previewing = True
                for i, draw_queue in enumerate(drawing_queues):
                    frame = frames.get('left' if i == 0 else 'right')
                    if frame is not None:
                        draw_queue.put((np.array(frame, dtype=int), False))
                time.sleep(0.25)
                latch_key_combo = True
                return
            previewing = False

            # Music mode: the audio visualizer on both panels instead of the layout's apps
            if music_mode.is_set():
                # The music draws on the panels itself, so there's no picture to squeeze when they turn off
                last_grids.clear()
                if not music_running:
                    # Stop the layout's own self-drawing apps (an audio visualizer) first
                    dispose_active_apps()
                run_music(foreground_value)
                time.sleep(0.1)
                latch_key_combo = True
                return
            if music_running:
                stop_music()
                # The layout's apps come back; an app that draws on its own starts again by itself
                for quadrant in suppressed_panel_apps:
                    suppressed_panel_apps[quadrant] = None
                for draw_queue, changed in idx_changed.items():
                    idx_changed[draw_queue] = True

            outputs = []
            for i, draw_queue in enumerate(drawing_queues):
                grid = np.zeros((9,34), dtype = int)
                if i == 0:
                    panel = 'left'
                    _args = left_args
                    suppressed_quadrant = left_suppressed_quadrant
                else:
                    panel = 'right'
                    _args = right_args
                    suppressed_quadrant = right_suppressed_quadrant

                maybe_dispose_suppressed_panel_app(suppressed_quadrant)
                # For persistent-draw apps, we don't submit the grid to the drawing queue
                persistent_draw, animate = render_panel(grid, _args, app_functions, foreground_value, background_value, panel)
                do_animate = None
                if idx_changed[draw_queue]:
                    do_animate = animate
                    idx_changed[draw_queue] = False
                    if not animate and not persistent_draw and shown_apps.get(panel) != apps_key(_args):
                        # Slide the new app in over the old one (Effects tab). Not when the same app
                        # comes round again (a quadrant with one app still "changes" each time it's due)
                        effects.manager.start_slide(panel)
                shown_apps[panel] = apps_key(_args)
                        
                # Restart animation if it was stopped for ID display
                if latch_key_combo:
                    if (animating_left and panel == 'left') or (animating_right and panel == 'right'):
                        do_animate = True
                last_grids[i] = None if persistent_draw else grid
                if not persistent_draw:
                    outputs.append((draw_queue, grid, do_animate))
            if tv_on:
                play_tv_effect([(q, animations.tv_on_frames(g, foreground_value)[:-1]) for q, g, _ in outputs])
            for draw_queue, grid, do_animate in outputs:
                draw_queue.put((grid, do_animate))
            latch_key_combo = False
            for app in apps_to_dispose:
                dispose_fn = app.get('dispose-fn', None)
                if dispose_fn:
                    func = app_functions[dispose_fn]
                    kwargs = build_app_kwargs(app)
                    func(**kwargs)
            del apps_to_dispose
            # Animations move smoothly at about 25 frames a second; everything else is drawn 10 times a second
            smooth = any(a.get('name') in SMOOTH_APPS for a in left_args + right_args)
            time.sleep(0.04 if smooth else 0.1)
        except KeyboardInterrupt:
            raise
        except Exception as e:
            log.error(f"Error in main loop: {e}")
            time.sleep(1.0)
            
    # Informative message if pynput is not available
    if not PYNPUT_AVAILABLE and not IS_WINDOWS and not args.no_key_listener:
        log.warning("Info: pynput is unavailable; running in evdev-only mode. Use Ctrl+C to exit.")

    hotkey_thread = None
    hotkey_lock = threading.Lock()

    def start_hotkeys():
        """Register the shortcuts that are turned on (again, after a change), on Windows."""
        nonlocal hotkey_thread
        if not IS_WINDOWS or args.no_key_listener:
            return
        from led_mon import hotkeys
        actions = {
            'i': request_identify,
            'n': request_next_app,
            'f': lambda: set_freeze(True),
            'u': lambda: set_freeze(False),
            'm': toggle_music_mode,
        }
        with hotkey_lock:
            if hotkey_thread is not None:
                hotkey_thread.stop()
                hotkey_thread.join(3)
                hotkey_thread = None
            bindings = {letter: action for letter, action in actions.items() if shortcuts.is_enabled(letter)}
            if bindings:
                hotkey_thread = hotkeys.start(bindings)
            else:
                log.info("Keyboard shortcuts: all turned off")

    def run_main_loop():
        global power_watcher
        platform_utils.init_com_for_thread()
        # Windows: turn the panels off when the lid closes, the screen turns off or the laptop screen isn't used
        from led_mon import power_events
        power_watcher = power_events.PowerWatcher(on_power_change)
        power_watcher.start()
        # Windows: ripples when something is plugged in
        global plug_watcher
        from led_mon import plug_events
        plug_watcher = plug_events.PlugWatcher(on_plug, learned=learned_ports)
        plug_watcher.start()
        start_hotkeys()
        # Prefer pynput if available; fall back silently if it fails to start (common on Wayland)
        if PYNPUT_AVAILABLE and not args.no_key_listener:
            try:
                with Listener(on_press=on_press, on_release=on_release):
                    while not shutdown_event.is_set():
                        render_iteration(args)
            except Exception as e:
                log.error(f"Warning: pynput listener could not be started ({e}). Falling back to evdev-only mode.")
                while not shutdown_event.is_set():
                    render_iteration(args)
        else:
            while not shutdown_event.is_set():
                render_iteration(args)

    def shutdown():
        """Turn the panels dark, stop drawing and release both serial ports (tray Quit)."""
        dispose_active_apps()
        for draw_queue in drawing_queues:
            stop_drawing_queue(draw_queue, blank=True)
        for thread in drawing_threads:
            thread.join(timeout=3)
        close_all_ports()
        fps.stop_all()

    if use_tray(args):
        from led_mon import tray
        main_loop_thread = Thread(target=run_main_loop, daemon=True)
        main_loop_thread.start()
        from led_mon.brightness_popup import TrayBrightnessPopup
        brightness_popup = TrayBrightnessPopup(brightness)
        tray.run_tray(
            identify=request_identify,
            next_app=request_next_app,
            set_freeze=set_freeze,
            is_frozen=is_frozen_switching,
            reload_config=reload_config_event.set,
            config_dir=platform_utils.user_config_dir(),
            log_file=platform_utils.log_file_path(),
            quit=shutdown_event.set,
            edit_layout=lambda: open_layout_editor(args),
            set_paused=set_paused,
            is_paused=is_paused,
            set_turn_off_with_lid=set_turn_off_with_lid,
            is_turn_off_with_lid=is_turn_off_with_lid,
            set_turn_off_when_screen_unused=set_turn_off_when_screen_unused,
            is_turn_off_when_screen_unused=is_turn_off_when_screen_unused,
            set_music_mode=set_music_mode,
            is_music_mode=is_music_mode,
            set_tv_effect=set_tv_effect,
            is_tv_effect=is_tv_effect,
            show_brightness=brightness_popup.show,
            lhm_path=app_settings.load_settings().get("libre_hardware_monitor_path", ""),
        )
        # The tray returns after Quit
        brightness_popup.stop()
        shutdown_event.set()
        main_loop_thread.join(timeout=5)
        shutdown()
    else:
        run_main_loop()

    log.info("Exiting")

def open_layout_editor(args):
    # The layout editor is the first tab of the White Rabbit window
    editor_args = ['--window']
    if args.config_file:
        editor_args += ['--config-file', args.config_file]
    platform_utils.launch_self(*editor_args)

def use_tray(args):
    # Windows runs windowless with a tray icon; --no-tray keeps the console behaviour for debugging
    return IS_WINDOWS and not args.no_tray

def stop_drawing_queue(draw_queue, blank=False):
    if blank:
        # Stop any panel animation and draw a dark frame, so the panels don't keep showing the last frame
        try:
            draw_queue.put((np.zeros((9,34), dtype = int), False), timeout=1)
            draw_queue.put((np.zeros((9,34), dtype = int), None), timeout=1)
            # Wait for the dark frame to be drawn rather than dropping it to make room below
            draw_queue.put(None, timeout=2)
            return
        except queue.Full:
            log.warning("Could not turn a panel dark before exiting")
    # The queue is small and may be full, so make room for the exit sentinel
    while True:
        try:
            draw_queue.put_nowait(None)
            return
        except queue.Full:
            try:
                draw_queue.get_nowait()
            except queue.Empty:
                pass

def main(args):
    # A windowless Windows exe has no console; reuse the one it was started from for --help / --list-apps
    platform_utils.attach_parent_console()
    base_apps = ["cpu", "cpu-graph", "gpu-graph", "net", "disk", "disk-net", "mem-bat", "fps", "fps-num", "snap"]
    plugin_apps = []
    ###############################################################
    ###  Load additional app names from plugins for listing insalled apps  ###
    if not re.search(r"--disable-plugins|-dp", str(sys.argv)):
        # Try to find plugins directory - either in current dir or installed location
        plugins_dir = platform_utils.plugins_dir()
        for file in os.listdir(plugins_dir):
            if file.endswith('_plugin.py'):
                module_name = re.sub("_plugin.py", "", file)
                file_path = os.path.join(plugins_dir, file)
                spec = importlib.util.spec_from_file_location(module_name, file_path)
                module = importlib.util.module_from_spec(spec)
                sys.modules[module_name] = module
                spec.loader.exec_module(module)
                plugin_apps += map(lambda o: o["name"], module.app_funcs)
    #################################################################
    parser = ArgumentParser(prog="FW LED System Monitor", add_help=False,
                            description="Displays system performance metrics in the Framework 16 LED Matrix input module",
                            formatter_class=ArgumentDefaultsHelpFormatter)
    mode_group = parser.add_argument_group()
    mode_group.add_argument("--help", "-h", action="help",
        help="Show this help message and exit")
    
    mode_group.add_argument("--no-key-listener", "-nkl", action="store_true", help="Do not listen for key presses")
    mode_group.add_argument("--disable-plugins", "-dp", action="store_true", help="Do not load any plugin code")
    mode_group.add_argument("--list-apps", "-la", action="store_true", help="List the installed apps, and exit")
    mode_group.add_argument("--config-file", "-cf", default=None, help="Absolute path to custom config file")
    mode_group.add_argument("--edit-layout", action="store_true", help="Open the layout editor window instead of running the monitor")
    mode_group.add_argument("--diagnose", action="store_true",
        help="Print the LED panels, serial ports and helper apps that are found, and exit")
    if IS_WINDOWS:
        mode_group.add_argument("--no-tray", action="store_true", help="Run in the console without a tray icon")
    
    args = parser.parse_args()
    if args.diagnose:
        from led_mon import diagnose
        diagnose.run(args.config_file)
        return
    if args.edit_layout:
        from led_mon.editor import run_editor
        run_editor(args)
        return
    app(args, base_apps, plugin_apps)

if __name__ == "__main__":
    platform_utils.setup_logging()
    main(sys.argv)


