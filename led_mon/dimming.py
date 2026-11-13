# "Keep every shade when the panels are dim" (Effects tab).
#
# Normally the monitor dims the panels by drawing with smaller pixel values, so at low brightness
# many shades become the same value. With this on, and White Rabbit firmware that has the
# GlobalCurrent command (0x30), the pixel values are stretched to the full range and the panel
# is dimmed by lowering the LED current instead. The light is the same; the shades are kept.
# Firmware without the command doesn't answer the query, and is left as it was.

# Built In Dependencies
import logging

log = logging.getLogger(__name__)

GLOBAL_CURRENT = 0x30
FULL_CURRENT = 255
QUERY_TIMEOUT = 0.3

# Per port, when the port isn't a shared one: {id: {"supported": bool|None, "current": int|None}}
_plain_ports = {}


def _state(s):
    entry = getattr(s, "_entry", None)
    if entry is not None:
        if not hasattr(entry, "dimming"):
            entry.dimming = {"supported": None, "current": None}
        return entry.dimming
    return _plain_ports.setdefault(id(s), {"supported": None, "current": None})


def _held(s):
    try:
        from led_mon.serial_ports import is_held
        return not getattr(s, "_control", False) and is_held(s.port)
    except Exception:
        return False


def supports(s):
    """True if the panel's firmware answers the GlobalCurrent query. Asked once per port."""
    state = _state(s)
    if state["supported"] is None and not _held(s):
        old = getattr(s, "timeout", None)
        try:
            s.reset_input_buffer()
            s.timeout = QUERY_TIMEOUT
            s.write(bytearray([0x32, 0xAC, GLOBAL_CURRENT]))
            answer = s.read(32)
            state["supported"] = len(answer) == 32
        except Exception as e:
            log.debug(f"Asking the panel about dimming: {e}")
            state["supported"] = False
        finally:
            try:
                s.timeout = old
            except Exception:
                pass
        log.info(f"Panel {getattr(s, 'port', '?')}: dimming by LED current "
                 f"{'supported' if state['supported'] else 'not supported (older firmware)'}")
    return bool(state["supported"])


def _send_current(s, value):
    state = _state(s)
    if state["current"] != value:
        s.write(bytearray([0x32, 0xAC, GLOBAL_CURRENT, value]))
        state["current"] = value


def prepare(s, grid, enabled, foreground):
    """The grid to send. With dimming by current on and supported, stretched to the full range,
    and the panel's current set to match the brightness. Otherwise the grid as it was, and the
    panel's current back to full if it was lowered."""
    import numpy as np
    state = _state(s)
    if not enabled or not foreground or foreground <= 0:
        if state["current"] not in (None, FULL_CURRENT):
            _send_current(s, FULL_CURRENT)
        return grid
    if not supports(s):
        return grid
    foreground = max(1, min(255, int(foreground)))
    _send_current(s, foreground)
    return np.clip(np.rint(np.asarray(grid, dtype=float) * (255.0 / foreground)), 0, 255)


def reset_entry(entry):
    """Full current again on a shared port about to be lent to the White Rabbit window."""
    state = getattr(entry, "dimming", None)
    if state and state["current"] not in (None, FULL_CURRENT):
        try:
            with entry.lock:
                entry.serial.write(bytearray([0x32, 0xAC, GLOBAL_CURRENT, FULL_CURRENT]))
            entry.shown = None
        except Exception as e:
            log.debug(f"Resetting the dimming on {entry.port_name}: {e}")
        state["current"] = None
