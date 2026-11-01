# Built In Dependencies
import os
import numpy as np
import threading
import logging
import time

# Internal dependencies
from led_mon.patterns import letters_5_x_6, numerals
from led_mon import drawing
from led_mon.equalizer_files.visualize import Equalizer
from led_mon.equalizer_files import looks
from led_mon.shared_state import discover_led_devices

log = logging.getLogger(__name__)
LOG_LEVELS = {
    "debug": logging.DEBUG,
    "info": logging.INFO,
    "warning": logging.WARNING,
    "error": logging.ERROR,
}

log_level = LOG_LEVELS[os.environ.get("LOG_LEVEL", "warning").lower()]
log.setLevel(log_level)

####  Monitor functions ####

equalizers = {}
equalizer_retry_after = {}
EQUALIZER_RETRY_BACKOFF_SEC = 30

def parse_float_arg(kwargs, key, default):
    value = kwargs.get(key, default)
    try:
        return float(value)
    except (TypeError, ValueError):
        log.warning(f"Invalid audio visualizer arg '{key}={value}', using default {default}.")
        return float(default)

def normalize_input_mode(value):
    return looks.normalize_input_mode(value)

def visualizer_settings(kwargs):
    """The look and input settings from an app entry's args (or music mode's settings)."""
    return looks.normalize(kwargs)

def run_equalizer(_, grid, foreground_value, idx, **kwargs):
    side = kwargs.get('side', None)
    if side not in ('left', 'right'):
        log.error(f"Unexpected audio visualizer side arg '{side}'. Expected 'left' or 'right'.")
        return

    if side in equalizers:
        return
    if time.time() < equalizer_retry_after.get(side, 0):
        return

    settings = visualizer_settings(kwargs)
    silent_pulse_after_sec = min(parse_float_arg(kwargs, 'zero-frame-delay-sec', 4.0),
                                 parse_float_arg(kwargs, 'silent-pulse-after-sec', 12.0))
    silent_pulse_period_sec = parse_float_arg(kwargs, 'silent-pulse-period-sec', 3.8)
    silent_pulse_reveal_sec = parse_float_arg(kwargs, 'silent-pulse-reveal-sec', 5.0)

    # device tuple => ('<location>', '<serial device>')
    devices: tuple[str, str] = discover_led_devices() or []
    channel = 0 if side == 'left' else 1
    if len(devices) <= channel:
        log.warning(f"Audio visualizer requested for {side} side, but matching LED device was not discovered.")
        return
    device = devices[channel]

    # Use the port name when the USB location is unknown (e.g. on Windows)
    eq = Equalizer(device_location=device[0] or device[1])
    equalizers[side] = eq

    def _run():
        try:
            ok = eq.run(
                settings=settings,
                side=side,
                device_name=device[1],
                silent_pulse_after_sec=silent_pulse_after_sec,
                silent_pulse_period_sec=silent_pulse_period_sec,
                silent_pulse_reveal_sec=silent_pulse_reveal_sec,
            )
            if ok is False:
                equalizer_retry_after[side] = time.time() + EQUALIZER_RETRY_BACKOFF_SEC
        except Exception as e:
            log.error(f"Audio visualizer runtime error on {side} side: {e}")
            equalizer_retry_after[side] = time.time() + EQUALIZER_RETRY_BACKOFF_SEC
        finally:
            eq.stop()
            if equalizers.get(side) is eq:
                del equalizers[side]

    eq_thread = threading.Thread(target=_run, daemon=True)
    eq_thread.start()
    if len(equalizers) > 2:
        log.info(f"run_equalizer: Active equalizers: {len(equalizers)}")
    
def hearing():
    """What each panel's audio visualizer hears right now: {side: {'device', 'problem', 'loudness', 'silent'}}."""
    return {side: dict(eq.hearing) for side, eq in list(equalizers.items()) if eq.hearing}

def dispose_equalizer(**kwargs):
    side = kwargs.get('side', None)
    if side is None:
        log.error("dispose_equalizer called without required 'side' arg.")
        return

    eq = equalizers.pop(side, None)
    if eq is not None:
        eq.stop()
    equalizer_retry_after.pop(side, None)
    if len(equalizers) > 2:
        log.info(f"dispose_equalizer: Active equalizers: {len(equalizers)}")

#### Implement high-level drawing functions to be called by app functions below ####

draw_app = getattr(drawing, 'draw_app')

#### Implement low-level drawing functions ####
# These functions will be dynamically imported by drawing.py and called by their corresponding app function

# No implementations needed since the script draws continuoulsy on grid until dispose function is called
direct_draw_funcs = {
    "equalizer": {
        "fn": lambda *x: None,
        "border":lambda *x: None
    }
}

# Implement app functions that call your direct_draw functions
# These functions will be dynamically imported by led_system_monitor.py. They call the direct_draw_funcs
# defined above, providing additional capabilities that can be targeted to panel quadrants

app_funcs = [
    {
        "name": "equalizer",
        "fn": run_equalizer
    },
    {
        "name": "equalizer_dispose",
        "fn": dispose_equalizer
    }
]

# Provide id patterns that identify your apps
# These items will be dynamically imported by drawing.py

id_patterns = {
    # "AV": audio visualizer
    "equalizer": np.concatenate((np.zeros((10,9)), letters_5_x_6["A"], np.zeros((2,9)), letters_5_x_6["V"], np.zeros((10,9)))).T,
    "equalizer_paused": np.concatenate((np.zeros((5,9)), letters_5_x_6["A"], np.zeros((2,9)), letters_5_x_6["V"],  np.zeros((3,9)), numerals["0"], np.zeros((5,9)))).T,

}
