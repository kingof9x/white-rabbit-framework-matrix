# Built In Dependencies
import time
import math
import threading
import importlib.util
import sys
import os
import re
import json
import queue
import logging

# Internal Dependencies
from led_mon.commands import Commands, send_command, do_animate
from led_mon.patterns import lightning_bolt_bot, lightning_bolt_top, lookup_table, id_patterns, symbols, numerals, icons, digits_3_x_5
from led_mon.platform_utils import package_dir, plugins_dir
from led_mon.serial_ports import open_port, port_lock
from led_mon.shared_state import is_led_matrix_port
from led_mon import effects

# External Dependencies
import numpy as np
from serial.tools import list_ports

log = logging.getLogger(__name__)

# Put this on a drawing queue to send the panel to sleep (LEDs off). Any later frame wakes it again.
SLEEP_PANEL = "sleep"

# Correct table orientation for visual orientation when drawn
for i in range(lookup_table.shape[0]):
    lookup_table[i] = lookup_table[i].T


def spiral_index(fill_ratio):
    return int(round(fill_ratio * 9.999999 - 0.5))

## App Draw Functions ##

# Takes up 15 rows, 7 columns, starting at y,1
# For bottom segment, the 16th row will be empty
def draw_spiral_vals(grid, cpu_values, fill_value, y):
    y += 1
    for i, v in enumerate(cpu_values):
        column_number = i % 2
        row_number = i // 2
        fill_grid = lookup_table[spiral_index(v)]
        grid[1+column_number*4:4+column_number*4, y+row_number*4:y+3+row_number*4] = fill_grid * fill_value

# Takes up 2 rows, 7 columns, starting at y, 1
def draw_memory(grid, memory_ratio, fill_value, y):
    lit_pixels = 7 * 2 * memory_ratio
    pixels_bottom = int(round(lit_pixels / 2))
    pixels_top = int(round((lit_pixels - 0.49) / 2))
    grid[1:1+pixels_top,y+1] = fill_value
    grid[1:1+pixels_bottom,y+2] = fill_value

# Takes up 12 (top segment) or 13 (bottom segment) rows, 7 columns, starting at y,1
def draw_battery(grid, battery_ratio, battery_plugged, fill_value, y,
        battery_low_thresh = 0.07, battery_low_flash_time = 2, charging_pulse_time = 3):
    if y == 19: # Placement on bottom
        bot = 33
        num_rows = 13
        lightning_bolt = lightning_bolt_bot
    else: # Placement on top (y == 3)
        bot = 16
        num_rows = 12
        lightning_bolt = lightning_bolt_top
    bat_top = y + 1
    bat_bot = bat_top + num_rows
    lit_pixels = int(round(num_rows * 7 * battery_ratio))
    pixels_base = lit_pixels // 7
    remainder = lit_pixels % 7
    if battery_ratio <= battery_low_thresh and not battery_plugged:
        if time.time() % battery_low_flash_time * 2 < battery_low_flash_time: # This will flash the battery indicator if too low
            return
    for i in range(7):
        pixels_col = pixels_base
        if i < remainder:
            pixels_col += 1
        grid[i+1,bot-pixels_col:bot] = fill_value
    if battery_plugged:
        pulse_amount = math.sin(time.time() / charging_pulse_time)
        grid[1:8,bat_top:bat_bot][lightning_bolt] -= np.rint(fill_value + 10 * pulse_amount).astype(int)
        indices = grid[1:8,bat_top:bat_bot] < 0
        grid[1:8,bat_top:bat_bot][indices] = -grid[1:8,bat_top:bat_bot][indices]
    
# Takes up 16 (top segment) or 17 (bottom segment) rows, 3 columns, starting at y,1
def draw_bar(grid, bar_ratio, bar_value, bar_x_offset = 1, y=0):
    bar_width = 3
    bar_height = 16
    lit_pixels = int(round(bar_height * bar_width * bar_ratio))
    pixels_base = lit_pixels // bar_width
    remainder = lit_pixels % bar_width
    for i in range(bar_width):
        pixels_col = pixels_base
        if i < remainder:
            pixels_col += 1
        if y == 16:
            grid[bar_x_offset+i,33-pixels_col:33] = bar_value
        else:
            grid[bar_x_offset+i,1:1+pixels_col] = bar_value
            
# Rows inside a half panel's border: 15 for the top half (rows 1-15), 16 for the bottom (17-32)
def half_rows(y):
    return range(y + 1, 16) if y == 0 else range(y + 1, 33)

# Lights a horizontal bar from column 1, up to 7 pixels long; a partly filled last pixel is dimmer
def draw_hbar(grid, ratio, fill_value, row, width = 7, start = 1):
    length = max(0.0, min(1.0, ratio)) * width
    full = int(length)
    grid[start:start+full, row] = fill_value
    if full < width:
        grid[start+full, row] = int(round(fill_value * (length - full)))

# One horizontal bar per core, for CPUs with more cores than the eight spirals fit.
# More cores than rows (not on any Framework 16 CPU) are averaged in pairs, and so on.
def draw_core_bars(grid, core_values, fill_value, y):
    rows = list(half_rows(y))
    per_row = -(-len(core_values) // len(rows)) # Ceiling division
    values = [sum(core_values[i:i+per_row]) / len(core_values[i:i+per_row])
              for i in range(0, len(core_values), per_row)]
    for row, v in zip(rows, values):
        draw_hbar(grid, v, fill_value, row)

# The cpu app: a spiral per core when there are up to eight, otherwise a bar per core
cpu_core_count = 0
def draw_cpu_vals(grid, cpu_values, fill_value, y):
    global cpu_core_count
    cpu_core_count = len(cpu_values)
    if len(cpu_values) <= 8:
        draw_spiral_vals(grid, cpu_values, fill_value, y)
    else:
        draw_core_bars(grid, cpu_values, fill_value, y)

# Scrolling graph filling a half panel: one row per reading, newest at the top,
# each row a bar as long as the reading (None draws an empty row)
def draw_history_graph(grid, values, fill_value, y):
    for row, v in zip(half_rows(y), values):
        if v is not None:
            draw_hbar(grid, v, fill_value, row)

# Two short bars that grow upwards from row bottom, 3 columns wide at bar_x_offset
def draw_short_bar(grid, bar_ratio, bar_value, bar_x_offset, top, bottom):
    bar_width = 3
    bar_height = bottom - top + 1
    lit_pixels = int(round(bar_height * bar_width * max(0.0, min(1.0, bar_ratio))))
    pixels_base = lit_pixels // bar_width
    remainder = lit_pixels % bar_width
    for i in range(bar_width):
        pixels_col = pixels_base
        if i < remainder:
            pixels_col += 1
        if pixels_col:
            grid[bar_x_offset+i, bottom+1-pixels_col:bottom+1] = bar_value

# Disk on the upper part of the half panel (read left, write right), network on the
# lower part (upload left, download right), like the separate disk and net apps
def draw_disk_net(grid, disk_values, net_values, fill_value, y):
    rows = list(half_rows(y))
    disk_top, disk_bottom = rows[0], rows[6]
    net_top, net_bottom = rows[8], rows[-1]
    draw_short_bar(grid, disk_values[0], fill_value, 1, disk_top, disk_bottom)
    draw_short_bar(grid, disk_values[1], fill_value, 5, disk_top, disk_bottom)
    draw_short_bar(grid, net_values[0], fill_value, 1, net_top, net_bottom)
    draw_short_bar(grid, net_values[1], fill_value, 5, net_top, net_bottom)

# FPS as numbers in a 9 x 16 area: 3x5 digits drawn twice as tall (3 x 10), side by side.
# 100-199 draws the 1 as a single column so the digits keep a gap; 200 and up use every column,
# with the middle digit dimmer so the three stay apart. A "?" means FPS can't be measured.
def fps_number_columns(fps):
    value = max(0, min(999, int(round(fps or 0))))
    if value >= 200:
        return [(str(value // 100), 0, False), (str(value // 10 % 10), 3, True), (str(value % 10), 6, False)]
    if value >= 100:
        return [("|", 0, False), (str(value // 10 % 10), 2, False), (str(value % 10), 6, False)]
    if value >= 10:
        return [(str(value // 10), 1, False), (str(value % 10), 5, False)]
    return [(str(value), 3, False)]

def draw_fps_number(grid, fps, available, fill_value, top):
    """The number in rows top .. top+9."""
    if not available:
        grid[:, top + 2:top + 8] = symbols["?"].T * fill_value
        return
    for ch, x, dim in fps_number_columns(fps):
        value = int(round(fill_value * 0.55)) if dim else fill_value
        if ch == "|":
            grid[x, top:top + 10] = value
        else:
            grid[x:x + 3, top:top + 10] = np.repeat(digits_3_x_5[ch], 2, axis=0).T * value

# Highest FPS on the histogram's scale: the first of these at or above the highest reading, so the
# bars don't change size every time the highest reading changes
FPS_SCALES = (30, 60, 90, 120, 144, 165, 240, 360, 500, 1000)

def fps_scale(history):
    top = max([v for v in history if v] + [1.0])
    return next((s for s in FPS_SCALES if s >= top), FPS_SCALES[-1])

# The fps app, on the whole panel: the FPS as numbers on the top half, and below them a histogram:
# one column per second across the full width, newest on the right, scrolling left. Each column
# grows up from the bottom row; a partly filled top pixel is dimmer.
FPS_HISTOGRAM_TOP = 18      # rows 18-33: 16 pixels tall
FPS_HISTOGRAM_COLUMNS = 9   # the last 9 seconds
def draw_fps(grid, fps, history, available, fill_value):
    draw_fps_number(grid, fps, available, fill_value, 3)
    scale = fps_scale(history)
    height = grid.shape[1] - FPS_HISTOGRAM_TOP
    for i, v in enumerate(list(history)[:FPS_HISTOGRAM_COLUMNS]):
        if not v:
            continue
        x = FPS_HISTOGRAM_COLUMNS - 1 - i
        length = max(0.0, min(1.0, v / scale)) * height
        full = int(length)
        bottom = grid.shape[1]
        grid[x, bottom - full:bottom] = fill_value
        if full < height and length > full:
            grid[x, bottom - full - 1] = int(round(fill_value * (length - full)))

# The fps-num app: just the FPS as numbers, in the top or bottom half
def draw_fps_half(grid, fps, available, fill_value, y):
    draw_fps_number(grid, fps, available, fill_value, 3 if y == 0 else 20)

warned = set()
def draw_snapshot(grid, fill_value, **kwargs):
    global warned
    path = kwargs.get('path', None)
    panel = kwargs.get('panel', None)
    file = kwargs.get('file', None)
    current_dir = package_dir()
    path = os.path.join(current_dir, path)
    subdirs = [ f.name for f in os.scandir(path) if f.is_dir() and f.name in ['left', 'right']]
    subdir = panel if panel in subdirs else ''
    try:
        with open(os.path.join(path, subdir, file)) as f:
            snap = json.load(f)
            grid[:,:] = np.array(snap).T * fill_value
    except FileNotFoundError as e:
        if not file in warned:
            print(f"File {file} not found")
            warned.add(file)

def draw_chars_list(grid: np.ndarray, chars: list[str], fill_value: int, y: int):
    char_map = {**numerals, **symbols, **icons}
    grid = grid.T
    for char in chars:
        if char in char_map:
            try:
                l = len(char_map[char])
                # For three-digit temps (e.g. with Kelvin) only part of the condition icon can be rendered
                grid[y:y+l, :] = char_map[char][:34-y] * fill_value
                y += l + 1 # +1 for spacing
            except KeyError:
                raise Exception(f"Character {char} not found in numerals, symbols, or icons dictionary")
            except Exception as e:
                raise e
    grid = grid.T
    
    
## Border Draw Functions ##
    
# Draws a border around a 16 (top segment) or a 17 (bottom segment)
# x 9 grid, divided into a 2 x 4 grid. For the bottom segment,
# the last grid will have an extra row
def draw_8_x_8_grid(grid, border_value, y):
    height = 16 if y == 0 else 17
    grid[:, y] = border_value # Top
    grid[:, y+height] = border_value # Bottom
    
    grid[0, y:y+height] = border_value # Left
    grid[8, y:y+height] = border_value # Right
    grid[4, y:y+height] = border_value # Middle
    
    # Horizontal grid borders
    grid[:, y+4] = border_value
    grid[:, y+8] = border_value
    grid[:, y+12] = border_value
    
# Draws a border around a 16 (top segment) or a 17 (bottom segment)
# x 9 grid, split horizontally into two sections at the specified column
def draw_2_x_1_horiz_grid(grid, border_value, y, x_split_idx=4):
    height = 16 if y == 0 else 17
    grid[:, y] = border_value # Top
    grid[:, y+height] = border_value # Bottom

    grid[0, y:y+height] = border_value # Left
    grid[8, y:y+height] = border_value # Right
    grid[x_split_idx, y:y+height] = border_value # Middle
    
# Draws a border around a 16 (top segment) or a 17 (bottom segment),
# row section, split vertically into two sections at the specified row
def draw_1_x_2_vert_grid(grid, border_value, y, y_split_idx = 3):
    height = 16 if y == 0 else 17
    grid[:, y] = border_value # Top
    grid[:, y+height] = border_value # Bottom
    grid[:, y+y_split_idx] = border_value # Middle

    grid[0, y:y+height] = border_value # Left
    grid[8, y:y+height] = border_value # Right
    
# Draws a plain border around a 16 (top segment) or a 17 (bottom segment) row section
def draw_half_outline(grid, border_value, y):
    height = 16 if y == 0 else 17
    grid[:, y] = border_value # Top
    grid[:, y+height] = border_value # Bottom
    grid[0, y:y+height] = border_value # Left
    grid[8, y:y+height] = border_value # Right

# The cpu app's border: the 2 x 4 grid around spirals, or a plain outline around core bars
def draw_cpu_border(grid, border_value, y):
    if cpu_core_count <= 8:
        draw_8_x_8_grid(grid, border_value, y)
    else:
        draw_half_outline(grid, border_value, y)

# The disk-net app's border: split into left and right bars, and into a disk and a network part
def draw_disk_net_border(grid, border_value, y):
    draw_2_x_1_horiz_grid(grid, border_value, y)
    grid[:, y+8] = border_value

# Draws a border around the entire panel, split
# vertically into two equal segments
def draw_outline_border(grid, border_value):
    grid[:, 0] = border_value # Top
    grid[:, 16] = border_value # Middle
    grid[:, 33] = border_value # Bottom
    grid[0, :] = border_value # Left
    grid[8, :] = border_value # Right
    
# Maps an app arg value to abstract app and border draw functions
direct_draw_funcs = {
    "cpu": {
        "fn": draw_cpu_vals,
        "border": draw_cpu_border
    },
    "cpu-graph": {
        "fn": draw_history_graph,
        "border": draw_half_outline
    },
    "gpu-graph": {
        "fn": draw_history_graph,
        "border": draw_half_outline
    },
    "disk-net": {
        "fn": draw_disk_net,
        "border": draw_disk_net_border
    },
    "disk": {
        "fn": draw_bar,
        "border": draw_2_x_1_horiz_grid
    },
    "net": {
        "fn": draw_bar,
        "border": draw_2_x_1_horiz_grid
    },
    "mem": {
        "fn": draw_memory,
        "border": draw_1_x_2_vert_grid
    },
    "bat": {
        "fn": draw_battery,
        "border": draw_1_x_2_vert_grid
    },
    "now-playing": {
        "fn": lambda *x, **k: None, # Drawn by led_mon.now_playing
        "border": lambda *x: None # Whole panel, no border
    },
    "battery-sand": {
        "fn": lambda *x, **k: None, # Drawn by led_mon.sand
        "border": draw_half_outline
    },
    "fps": {
        "fn": draw_fps,
        "border": lambda *x: None # Whole panel, no border
    },
    "fps-num": {
        "fn": draw_fps_half,
        "border": lambda *x: None # Just the numbers
    },
    "snap": {
        "fn": draw_snapshot,
        "border": lambda *x: None # No border
    },
    #noop
    "none": {
        "fn": lambda *x: x,
        "border": lambda *x: x
    }
}

# Draws the app for the specified arg value
def draw_app(app, *arguments, **kwargs):
    direct_draw_funcs[app].get('fn')(*arguments, **kwargs)
    
# Draws the border for the specified arg value
def draw_app_border(app, *arguments):
    direct_draw_funcs[app].get('border')(*arguments)
            
# Draw the IDs of apps currently assigned to the top and bottom of a panel
def draw_ids(grid, top, bottom, fill_value, targs=None, bargs=None):
    if isinstance(targs, list):
        t_merged_dict = {k: v for d in targs if isinstance(d, dict) for k, v in d.items()}
        id_override = t_merged_dict.get('id_key_override', None)
        if id_override:
            if t_merged_dict.get(id_override[0], False):
                top = id_override[1]
            else:
                top = id_override[2]
        b_merged_dict = {k: v for d in bargs if isinstance(d, dict) for k, v in d.items()}
        id_override = b_merged_dict.get('id_key_override', None)
        if id_override:
            if b_merged_dict.get(id_override[0], False):
                bottom = id_override[1]
            else:
                bottom = id_override[2]
    fill_grid_top = id_patterns[top]
    fill_grid_bot = id_patterns[bottom]
    grid[1:8, 1:16] = fill_grid_top * fill_value
    grid[1:8, 18:-1] = fill_grid_bot * fill_value
    
# Draw the ID of the app currently assigned to the full panel
def draw_id(grid, id, fill_value, args=None):
    id_override = args.get('id_key_override', None)
    if id_override:
        if args.get(id_override[0], False):
            id = id_override[1]
        else:
            id = id_override[2]
    fill_grid = id_patterns[id]
    grid[:,:] = fill_grid * fill_value

# A frame that's already on the panel is sent again after this many seconds anyway. The panel
# firmware goes to sleep after 60 seconds without any command, so this keeps it awake.
RESEND_UNCHANGED_SEC = 5.0


def draw_to_LEDs(s, grid):
    # Ensure all values are valid bytes before sending
    safe_grid = np.clip(grid, 0, 255).astype(np.uint8)
    key = safe_grid.tobytes()
    remembers = hasattr(s, 'shows')
    # Hold the port for the whole frame so another thread sharing it can't interleave commands
    with port_lock(s):
        if remembers and s.shows(key, RESEND_UNCHANGED_SEC):
            # Already on the panel: sending it again would only keep the panel busy
            return
        for i in range(safe_grid.shape[0]):
            column = safe_grid[i, :]
            # The panel starts every frame with all columns dark, so dark columns needn't be sent
            if not column.any():
                continue
            params = bytearray([i]) + bytearray(column.tolist())
            send_command(s, Commands.StageCol, parameters=params)
        send_command(s, Commands.FlushCols)
        if remembers:
            s.remember_shown(key)


class LatestFrameQueue:
    """A drawing queue that keeps only the newest frame waiting.

    A panel takes a moment to draw each frame. With an ordinary queue the frames waiting behind it are
    already out of date when their turn comes, so the panel shows the past. Here a new frame replaces
    the one still waiting. Other items (SLEEP_PANEL, the None that stops the thread) keep their place.
    Has the parts of queue.Queue the app uses; put() never blocks because it is never full.
    """

    def __init__(self):
        self._items = []
        self._ready = threading.Condition()

    def put(self, item, block=True, timeout=None):
        with self._ready:
            last = self._items[-1] if self._items else None
            # Replace the waiting frame, unless that would lose a change to the panel's animation
            if isinstance(item, tuple) and isinstance(last, tuple) and item[1] in (None, last[1]):
                self._items[-1] = (item[0], last[1])
            else:
                self._items.append(item)
            self._ready.notify()

    def put_nowait(self, item):
        self.put(item, block=False)

    def get(self, block=True, timeout=None):
        with self._ready:
            if not block:
                timeout = 0
            if not self._ready.wait_for(lambda: self._items, timeout):
                raise queue.Empty
            return self._items.pop(0)

    def get_nowait(self):
        return self.get(block=False)

    def qsize(self):
        with self._ready:
            return len(self._items)

    def empty(self):
        return self.qsize() == 0


def init_device(location = "1-3.2", fatal=True):
    # location is a USB location prefix (e.g. 1-3.2), or a port name (e.g. COM4) when the
    # location is unknown or the port was pinned with left-port / right-port in the config
    try:
        device_list = list_ports.comports()
        for device in device_list:
            if device.device.lower() == str(location).lower():
                return open_port(device.device)
        for device in device_list:
            if device.location and device.location.startswith(location) and is_led_matrix_port(device):
                return open_port(device.device)
        raise RuntimeError(f"No LED panel device found for location prefix '{location}'")
    except Exception as e:
        if fatal:
            log.error(f"Error getting comm ports for LED panel USB devices: {e}")
            sys.exit(1)
        raise


# {port name in lower case: the DrawingThread drawing on it}, so two threads never pick the same panel
_claimed_ports = {}
_claim_guard = threading.Lock()


def _claim(port_name, owner):
    """Mark port_name as owner's panel. False if another drawing thread already has it."""
    key = port_name.lower()
    with _claim_guard:
        holder = _claimed_ports.get(key)
        if holder is not None and holder is not owner:
            return False
        _claimed_ports[key] = owner
        return True


def _unclaim(owner):
    with _claim_guard:
        for key in [k for k, v in _claimed_ports.items() if v is owner]:
            del _claimed_ports[key]


def _claimed_by_other(port_name, owner):
    with _claim_guard:
        holder = _claimed_ports.get(port_name.lower())
        return holder is not None and holder is not owner


def _find_panel_port(port_location, side_index, owner, device_list=None):
    """The port this drawing thread should draw on now, or None.

    First the panel at the thread's own USB location (or port name). If no panel is there any more,
    for example because the modules were moved to other slots, the panel that is now on this side
    (left = first, right = second, as at startup), as long as another drawing thread isn't using it.
    """
    from led_mon.shared_state import discover_led_devices
    if device_list is None:
        device_list = list_ports.comports()
    if port_location:
        for device in device_list:
            if device.device.lower() == str(port_location).lower() and not _claimed_by_other(device.device, owner):
                return device.device
        for device in device_list:
            if (device.location and device.location.startswith(port_location) and is_led_matrix_port(device)
                    and not _claimed_by_other(device.device, owner)):
                return device.device
    if side_index is None:
        return None
    found = discover_led_devices(device_list) or []
    if side_index < len(found):
        port = found[side_index][1]
        if not _claimed_by_other(port, owner):
            return port
    # The panel on this side is taken (e.g. the other panel moved onto this side): use any free one
    for _, port in found:
        if not _claimed_by_other(port, owner):
            return port
    return None


class DrawingThread(threading.Thread):
    def __init__(self, port_location, input_queue, side_index=None, effect_side=None):
        """Draws the frames put in input_queue on one LED panel.

        port_location is the panel's USB location (or port name) at startup; it may be None when the
        panel isn't connected yet. side_index (0 = left, 1 = right) lets the thread find its panel
        again after the modules were moved to other slots, or when it is plugged in later.
        """
        super().__init__()
        self.daemon = True
        self.port_location = port_location
        self.side_index = side_index
        self.input_queue = input_queue
        self.animate_active= False
        self._reconnect_backoff_sec = 0.5
        self._max_reconnect_backoff_sec = 8.0
        self._next_reconnect_time = 0.0
        self._warned_missing = False
        self.serial_port = None
        # Which panel's effects (ripples, sliding, text line; see effects.py) go over these frames
        self.effect_side = effect_side or (effects.SIDES[side_index] if side_index in (0, 1) else None)
        self.last_grid = None
        if port_location is not None or side_index is None:
            if side_index is None:
                self.serial_port = init_device(self.port_location)
            else:
                self._attempt_reconnect(force=True)

    @property
    def port_name(self):
        """The serial port this thread is drawing on now, or None while its panel is missing."""
        return self.serial_port.port if self.serial_port is not None else None

    def set_animate(self, animate):
        if self.serial_port is not None:
            do_animate(self.serial_port, animate=animate)

    def _close_serial_port(self):
        if self.serial_port is None:
            return
        try:
            self.serial_port.close()
        except Exception as e:
            log.debug(f"Error closing serial port during reconnect: {e}")
        finally:
            self.serial_port = None
            _unclaim(self)
            if self.effect_side and self.side_index is not None:
                effects.manager.panel_present(self.effect_side, False)

    def _attempt_reconnect(self, force=False):
        now = time.monotonic()
        if not force and now < self._next_reconnect_time:
            return False
        try:
            if self.side_index is None:
                self.serial_port = init_device(self.port_location, fatal=False)
            else:
                port = _find_panel_port(self.port_location, self.side_index, self)
                if port is None or not _claim(port, self):
                    raise RuntimeError(f"No free LED panel found for location '{self.port_location}'")
                try:
                    self.serial_port = open_port(port)
                except Exception:
                    _unclaim(self)
                    raise
                # Remember where the panel is now, so the next reconnect looks there first
                self.port_location = next((d.location for d in list_ports.comports()
                                           if d.device.lower() == port.lower() and d.location), port)
            do_animate(self.serial_port, animate=self.animate_active)
            if self.effect_side and self.side_index is not None:
                effects.manager.panel_present(self.effect_side, True)
            self._reconnect_backoff_sec = 0.5
            self._next_reconnect_time = 0.0
            self._warned_missing = False
            log.info(f"Connected LED panel {self.port_name} at location {self.port_location}")
            return True
        except Exception as e:
            if self.serial_port is not None:
                self._close_serial_port()
            self._next_reconnect_time = now + self._reconnect_backoff_sec
            self._reconnect_backoff_sec = min(self._reconnect_backoff_sec * 2, self._max_reconnect_backoff_sec)
            if not self._warned_missing:
                log.warning(f"Unable to connect LED panel at {self.port_location}: {e}")
                self._warned_missing = True
            else:
                log.debug(f"Still unable to connect LED panel at {self.port_location}: {e}")
            return False

    def run(self):
        while True:
            try:
                wait = effects.manager.interval(self.effect_side) if self.effect_side else None
                try:
                    item = self.input_queue.get() if wait is None else self.input_queue.get(timeout=wait)
                except queue.Empty:
                    # An effect is playing: draw its next step over the last frame
                    if (self.last_grid is not None and self.serial_port is not None
                            and not self.animate_active and effects.manager.owner.get(self.effect_side) is self):
                        draw_to_LEDs(self.serial_port, effects.manager.apply(
                            self.effect_side, self.last_grid, new_frame=False))
                    continue
                if item is None:  # Sentinel to exit cleanly
                    break
                if item == SLEEP_PANEL:
                    if self.effect_side:
                        effects.manager.forget(self.effect_side)
                    # The firmware fades the panel out and turns its LEDs off until the next command
                    if self.serial_port is not None:
                        send_command(self.serial_port, Commands.Sleep, parameters=[1])
                    continue
                grid, animate = item

                if animate is not None:
                    self.animate_active = animate

                if self.serial_port is None and not self._attempt_reconnect():
                    continue

                if not self.animate_active:
                    self.last_grid = grid
                    if self.effect_side:
                        # The monitor's thread and the audio visualizer's can share a panel: the one that
                        # drew last owns the effects there
                        effects.manager.owner[self.effect_side] = self
                        grid = effects.manager.apply(self.effect_side, grid)
                    draw_to_LEDs(self.serial_port, grid)
                if animate is not None:
                    do_animate(self.serial_port, animate)

            except Exception as e:
                log.error(f"Error in DrawingThread: {e}")
                self._close_serial_port()
                self._attempt_reconnect(force=True)
                time.sleep(1.0)
                continue
            
        # Clean shutdown
        self._close_serial_port()
        log.debug("DrawingThread exited cleanly")
                
###############################################################
###           Load metrics functions from plugins           ###
###############################################################
# Keep this at the end of the module to avoid circular imports
if not re.search(r"--disable-plugins|-dp", str(sys.argv)):
    # Try to find plugins directory - either in current dir or installed location
    _plugins_dir = plugins_dir()
    for file in os.listdir(_plugins_dir):
        if file.endswith('_plugin.py'):
            module_name = re.sub("_plugin.py", "", file)
            file_path = os.path.join(_plugins_dir, file)
            spec = importlib.util.spec_from_file_location(module_name, file_path)
            module = importlib.util.module_from_spec(spec)
            sys.modules[module_name] = module
            spec.loader.exec_module(module)

            for k,v in module.direct_draw_funcs.items():
                direct_draw_funcs[k] = v
                
            from led_mon.drawing import id_patterns
            for k,v in module.id_patterns.items():
                id_patterns[k] = v
            
################################################################

