# The audio visualizer app (called "equalizer" in config files): bars and other looks that move with
# the sound, drawn on one panel. Listening is in audio.py, the looks in looks.py.

# Built-in Dependencies
import subprocess
import time
import os
import re
import threading
from threading import Timer
import argparse
import logging
from enum import Enum
import shutil
import signal
import sys
from pathlib import Path
from led_mon import shared_state
from led_mon.drawing import DrawingThread, LatestFrameQueue

# Internal Dependencies
from led_mon.shared_state import discover_led_devices
from led_mon.patterns import id_patterns
from led_mon.platform_utils import IS_WINDOWS
from led_mon.commands import Commands, send_command
from led_mon.serial_ports import open_port
from led_mon.equalizer_files import audio, looks
import queue

# External Dependencies
import numpy as np
if not IS_WINDOWS:
    try:
        from pulsectl import Pulse
    except ImportError:
        Pulse = None
else:
    Pulse = None


level = logging.WARNING
if os.getenv("LOG_LEVEL", "").lower() == "debug":
    level = logging.DEBUG
elif os.getenv("LOG_LEVEL", "").lower() == "error":
    level = logging.ERROR
elif os.getenv("LOG_LEVEL", "").lower() == "info":
    level = logging.INFO

logging.basicConfig(
    level=level,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)

log = logging.getLogger(__name__)

class DeviceType(Enum):
    SOURCE = 1
    SINK = 2

SOURCE_CHECK_INTERVAL_SEC = 1
DEFAULT_ZERO_FRAME_NOTIFY_DELAY_SEC = 4.0
DEFAULT_SILENT_PULSE_AFTER_SEC = 4.0
DEFAULT_SILENT_PULSE_PERIOD_SEC = 3.8
DEFAULT_SILENT_PULSE_REVEAL_SEC = 5.0
# Sound must last this long to end the silence pulse, so a single click doesn't
RESUME_HOLD_SEC = 0.4

UPDATE_RATE = 0.03 # 33 fps

INPUTMODULE_CONTROL_APP = shutil.which('inputmodule-control')
PACTL_APP = shutil.which('pactl')

def has_inputmodule_control():
    return bool(INPUTMODULE_CONTROL_APP and Path(INPUTMODULE_CONTROL_APP).is_file())

def can_draw_equalizer():
    # The visualizer draws through its own serial handle, so it no longer needs inputmodule-control
    return True

def clamp_positive_float(value, default):
    try:
        parsed = float(value)
        return parsed if parsed > 0 else float(default)
    except (TypeError, ValueError):
        return float(default)

# Lock for drawing the "sound device changed" cue
device_lock = threading.Lock()

def get_notification_pattern(source):
    if re.match(".*headphone.*|.*Audio_Expansion.*", source):
        # Wired headphones
        return 'zigzag'
    elif re.match(".*analog-stereo.*", source):
        # Speakers
        return 'all-on'
    elif re.match(".*bluez.*", source):
        # BlueTooth device
        return 'gradient'
    else:
        # Other
        return 'gradient'
    
def draw_source_change_cue(source):
    if IS_WINDOWS:
        draw_source_change_cue_native(source)
        return
    if not has_inputmodule_control():
        return
    pattern = get_notification_pattern(source)
    devices= discover_led_devices()
    # Only one instance will usually detect the source change, so it notifies both devices
    cmd_1 = [
            INPUTMODULE_CONTROL_APP,
            '--serial-dev', devices[0][1],
            'led-matrix',
            '--pattern',
            pattern
    ]
    if len(devices) > 1:
        cmd_2 = [
                INPUTMODULE_CONTROL_APP,
                '--serial-dev', devices[1][1],
                'led-matrix',
                '--pattern',
                pattern
        ]
    else:
        cmd_2 = None
    with device_lock:
        if not shared_state.id_key_press_active:
            for _ in range(3):
                subprocess.call(cmd_1, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                if cmd_2:
                    subprocess.call(cmd_2, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    
# Pattern ids from inputmodule-rs (inputmodule-control led-matrix --pattern)
PATTERN_IDS = {'gradient': 1, 'zigzag': 4, 'all-on': 5}

def get_windows_notification_pattern(name):
    name = name.lower()
    if 'headphone' in name or 'headset' in name or 'audio expansion' in name:
        return 'zigzag'
    if 'speaker' in name:
        return 'all-on'
    return 'gradient'

def draw_source_change_cue_native(source):
    # Windows: send the pattern over the already-open (shared) serial handles
    pattern = get_windows_notification_pattern(source)
    devices = discover_led_devices() or []
    with device_lock:
        if shared_state.id_key_press_active:
            return
        for _, port_name in devices:
            try:
                s = open_port(port_name)
                try:
                    for _ in range(3):
                        send_command(s, Commands.Pattern, parameters=[PATTERN_IDS[pattern]])
                finally:
                    s.close()
            except Exception as e:
                log.debug(f"Could not draw source change cue on {port_name}: {e}")

def eq_grid(levels):
    """Same picture as `inputmodule-control led-matrix --eq`: bars growing up and down from the middle row."""
    grid = np.zeros((9, 34), dtype=int)
    row = 34 // 2
    for col, val in enumerate(levels[:9]):
        val = max(0, min(34, int(val)))
        above = val // 2
        below = val - above
        grid[col, row:row + above] = 0xFF
        grid[col, row - below:row] = 0xFF
    return grid

def get_default_device(dev_type: DeviceType):
    with Pulse() as pulse_tmp:  # Temporary connection to query
        server_info = pulse_tmp.server_info()
        new_dev = \
            server_info.default_source_name if dev_type == DeviceType.SOURCE \
            else server_info.default_sink_name if dev_type == DeviceType.SINK \
            else None
        return new_dev
    
class Equalizer():
    
    def __init__(self, device_location):
        self.done = False
        self.device_name = None
        self.last_known_sink = None
        # What it hears, for the Music tab: {'device', 'problem', 'loudness', 'silent'}
        self.hearing = {}
        self.queue = LatestFrameQueue()
        self.drawing_thread = DrawingThread(device_location, self.queue)
        self.drawing_thread.start()
        
    def stop(self):
        if not self.done:
            self.done = True
            try:
                self.queue.put_nowait(None)  # Sentinel to stop DrawingThread
            except queue.Full:
                try:
                    self.queue.get_nowait()
                except queue.Empty:
                    pass
                try:
                    self.queue.put_nowait(None)
                except queue.Full:
                    pass
        device_name = self.device_name if self.device_name else "<unknown>"
        log.debug(f"Stop equalizer on device {device_name}")

    def queue_frame(self, grid, animate=False):
        frame = (grid, animate)
        try:
            self.queue.put_nowait(frame)
        except queue.Full:
            try:
                self.queue.get_nowait()
            except queue.Empty:
                pass
            try:
                self.queue.put_nowait(frame)
            except queue.Full:
                pass
    
    # Pipewire is supposed to automatically make the default source track the default sink's monitor, but the
    # capability is fragile and can sometimes be permanently broken. So we track sink changes and set the default
    # source to its monitor, to ensure continued data flow. We also draw a visual cue identifying the new source.
    def force_monitor_source(self):
        if not PACTL_APP:
            if not hasattr(self, '_pactl_missing_logged'):
                self._pactl_missing_logged = True
                log.warning("pactl was not found on PATH; skipping monitor-source auto-sync for equalizer.")
            return
        try:
            current_sink = get_default_device(DeviceType.SINK)
            if current_sink == self.last_known_sink or current_sink is None:
                if not self.done:
                    Timer(SOURCE_CHECK_INTERVAL_SEC, self.force_monitor_source).start()
                return

            expected_source = f"{current_sink}.monitor"
            current_source = get_default_device(DeviceType.SOURCE)

            if current_source != expected_source and current_source is not None:
                log.info(f"New sink detected: {current_sink}")
                subprocess.run(
                    [PACTL_APP, 'set-default-source', expected_source],
                    check=True,
                    capture_output=True,
                    text=True
                )
                #time.sleep(0.2)  # tiny settle time, may not be needed
                verified = get_default_device(DeviceType.SOURCE)
                if verified == expected_source:
                    log.info(f"Default source changed: {current_source} → {expected_source}")
                    draw_source_change_cue(expected_source)
                else:
                    log.warning(f"Failed to change default source: still {verified}")
                    
            self.last_known_sink = current_sink

        except subprocess.CalledProcessError as e:
            log.error(f"Failed to check/fix default source: {e}")
        except Exception as e:
            log.error(f"Unexpected error in force_monitor_source: {e}")
        if not self.done:
            Timer(SOURCE_CHECK_INTERVAL_SEC, self.force_monitor_source).start()
        
    def cleanup(self, sig=None, frame=None):
        self.stop()

    def draw_inverted_silence_pulse(self, elapsed_sec, pulse_period_sec, reveal_sec):
        pulse_period_sec = clamp_positive_float(pulse_period_sec, DEFAULT_SILENT_PULSE_PERIOD_SEC)
        reveal_sec = clamp_positive_float(reveal_sec, DEFAULT_SILENT_PULSE_REVEAL_SEC)
        base_foreground = max(1, int(shared_state.foreground_value))
        phase = (elapsed_sec / pulse_period_sec) % 1.0
        # Triangle wave avoids the visual "pause" at min/max that a sinusoid can create.
        wave = 1.0 - abs((2.0 * phase) - 1.0)

        x = np.arange(9, dtype=float)[:, None]
        y = np.arange(34, dtype=float)[None, :]
        center_x = 4.0
        center_y = 16.5
        norm_x = (x - center_x) / 4.0
        norm_y = (y - center_y) / 16.5
        radial = np.sqrt((norm_x ** 2) + (norm_y ** 2))
        angle = np.arctan2(norm_y, norm_x)

        # Warp radial distance into a star-like contour.
        star_scale = 1.0 + (0.28 * np.cos(4.0 * angle))
        star_radius = radial / np.clip(star_scale, 0.45, None)
        pulse_radius = 0.22 + (1.20 * wave)
        shell_width = 0.11
        shell = np.exp(-((star_radius - pulse_radius) ** 2) / (2.0 * (shell_width ** 2)))
        fill = np.clip((pulse_radius - star_radius) / max(pulse_radius, 1e-6), 0.0, 1.0)
        shape = np.clip((0.50 * fill) + (0.50 * shell), 0.0, 1.0)

        brightness = 0.30 + (0.70 * wave)
        grid = np.rint(np.clip(base_foreground * shape * brightness, 0, 255)).astype(int)
        grid = np.where(shape > 0.04, np.maximum(grid, 1), grid)

        reveal = np.clip(elapsed_sec / reveal_sec, 0.0, 1.0)
        paused_mask = (id_patterns['equalizer_paused'] > 0).astype(float)
        grid = np.rint(grid * (1.0 - (paused_mask * reveal))).astype(int)
        self.queue_frame(grid, False)

    def run(
        self,
        settings=None,
        side='left',
        device_name=None,
        silent_pulse_after_sec=DEFAULT_SILENT_PULSE_AFTER_SEC,
        silent_pulse_period_sec=DEFAULT_SILENT_PULSE_PERIOD_SEC,
        silent_pulse_reveal_sec=DEFAULT_SILENT_PULSE_REVEAL_SEC,
    ):
        """Draw the visualizer on this panel until stop(). settings: see looks.py."""
        self.device_name = device_name
        settings = looks.normalize(settings)
        silent_pulse_after_sec = clamp_positive_float(silent_pulse_after_sec, DEFAULT_SILENT_PULSE_AFTER_SEC)
        silent_pulse_period_sec = clamp_positive_float(silent_pulse_period_sec, DEFAULT_SILENT_PULSE_PERIOD_SEC)
        silent_pulse_reveal_sec = clamp_positive_float(silent_pulse_reveal_sec, DEFAULT_SILENT_PULSE_REVEAL_SEC)
        visualizer = looks.Visualizer(settings, side)
        input_mode = settings['input-mode']

        # Windows has no PulseAudio/PipeWire monitor source to keep in sync; the capture follows the default output
        if input_mode == 'playback' and not IS_WINDOWS and Pulse is not None:
            self.force_monitor_source()
        try:
            capture = audio.acquire(input_mode, settings['input-device'],
                                    on_device_change=draw_source_change_cue if IS_WINDOWS else None)
        except Exception as e:
            log.error(f"Unable to open the audio visualizer's sound input: {e}")
            self.stop()
            return False
        log.debug(f"Running the audio visualizer ({settings['style']}) on {device_name} ({side}) "
                  f"listening to {input_mode}")

        last_sound = time.monotonic()
        last_frame = last_sound
        pulsing = False
        resume_since = None
        try:
            while not self.done:
                now = time.monotonic()
                dt, last_frame = now - last_frame, now
                grid = visualizer.frame(capture.snapshot(), dt)
                self.hearing = {'device': getattr(capture, 'device_name', None),
                                'problem': getattr(capture, 'problem', None),
                                'loudness': visualizer.loudness, 'silent': visualizer.is_silent(),
                                'input': input_mode}
                if not shared_state.id_key_press_active:
                    if visualizer.is_silent():
                        resume_since = None
                        if now - last_sound >= silent_pulse_after_sec:
                            pulsing = True
                    else:
                        if pulsing:
                            resume_since = resume_since or now
                            if now - resume_since >= RESUME_HOLD_SEC:
                                pulsing = False
                                resume_since = None
                        if not pulsing:
                            last_sound = now
                    if pulsing:
                        self.draw_inverted_silence_pulse(
                            elapsed_sec=now - last_sound,
                            pulse_period_sec=silent_pulse_period_sec,
                            reveal_sec=silent_pulse_reveal_sec,
                        )
                    else:
                        self.queue_frame(looks.to_leds(grid, shared_state.foreground_value), False)
                time.sleep(UPDATE_RATE)
        except KeyboardInterrupt:
            self.cleanup()
        finally:
            audio.release(capture)
        return True
            
if __name__ == '__main__':
    parser = argparse.ArgumentParser(description="LED Matrix Audio Visualizer - one panel")
    parser.add_argument('--channel', choices=['left', 'right'], required=True,
                        help="Which panel to draw on: 'left' or 'right'")
    parser.add_argument('--style', choices=looks.STYLE_KEYS, default='classic')
    parser.add_argument('--microphone', action='store_true', help="Listen to the microphone instead of what plays")
    parser.add_argument('--span-panels', action='store_true', help="One spectrum across both panels")
    args = parser.parse_args()

    devices = discover_led_devices() or []
    index = 0 if args.channel == 'left' else 1
    if len(devices) > index:
        serial_dev = devices[index][1]
        eq = Equalizer(devices[index][0] or serial_dev)
        def stop_and_exit(sig=None, frame=None):
            eq.cleanup()
            sys.exit(0)
        signal.signal(signal.SIGINT, stop_and_exit)
        eq.run({'style': args.style, 'span-panels': args.span_panels,
                'input-mode': 'microphone' if args.microphone else 'playback'},
               side=args.channel, device_name=serial_dev)
    else:
        log.error(f"The {args.channel} LED matrix panel was not found. Please check your connections and try again.")
