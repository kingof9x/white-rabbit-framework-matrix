# Opening LED panel serial ports.
#
# Windows COM ports are exclusive, so every open() of the same port returns a handle onto one shared
# pyserial object, with writes serialised. White Rabbit does the same on Linux, so that holding a
# panel (below) covers every writer.
#
# Holding a panel: while the White Rabbit window plays a game or shows a pattern on a panel, the
# monitor's writes to that port are dropped, and only control handles (open_port(..., control=True))
# get through. Releasing the panel lets the monitor draw on it again.

# Built In Dependencies
import threading
import logging

# External Dependencies
import serial # pyserial

log = logging.getLogger(__name__)

BAUD_RATE = 115200


class _SharedPort:
    def __init__(self, port_name):
        self.port_name = port_name
        self.serial = serial.Serial(port_name, BAUD_RATE)
        self.lock = threading.RLock()
        self.refs = 0
        self.broken = False

    def close(self):
        try:
            self.serial.close()
        except Exception as e:
            log.debug(f"Error closing {self.port_name}: {e}")


class SharedSerial:
    """A handle onto a shared port. Has the subset of the pyserial API the app uses."""

    def __init__(self, entry, control=False):
        self._entry = entry
        self._closed = False
        self._control = control
        entry.refs += 1

    @property
    def port(self):
        return self._entry.port_name

    @property
    def lock(self):
        return self._entry.lock

    @property
    def is_open(self):
        return not self._closed and not self._entry.broken and self._entry.serial.is_open

    def write(self, data):
        if self._closed:
            raise serial.SerialException(f"{self.port} handle is closed")
        if not self._control and is_held(self.port):
            # The panel is lent to the White Rabbit window; pretend the write went through
            return len(data)
        with self._entry.lock:
            if self._entry.broken:
                raise serial.SerialException(f"{self.port} needs to be reopened")
            try:
                return self._entry.serial.write(data)
            except Exception:
                # e.g. the COM handle was invalidated by sleep: every user must reopen
                self._entry.broken = True
                raise

    def read(self, size=1):
        with self._entry.lock:
            return self._entry.serial.read(size)

    @property
    def timeout(self):
        return self._entry.serial.timeout

    @timeout.setter
    def timeout(self, value):
        self._entry.serial.timeout = value

    def reset_input_buffer(self):
        with self._entry.lock:
            self._entry.serial.reset_input_buffer()

    def close(self):
        if self._closed:
            return
        self._closed = True
        _release(self._entry)


_ports = {}
_ports_guard = threading.Lock()


def _release(entry):
    with _ports_guard:
        entry.refs -= 1
        if entry.refs <= 0:
            if _ports.get(entry.port_name.lower()) is entry:
                del _ports[entry.port_name.lower()]
            entry.close()


def open_port(port_name, control=False):
    """Open an LED panel port: a handle onto one shared pyserial object per port.

    control=True gives a handle that still writes while the panel is held (the White Rabbit window's).
    """
    key = port_name.lower()
    with _ports_guard:
        entry = _ports.get(key)
        if entry is not None and entry.broken:
            # Drop the dead handle so the port can be opened again; holders still get errors and reconnect
            entry.close()
            del _ports[key]
            entry = None
        if entry is None:
            entry = _SharedPort(port_name)
            _ports[key] = entry
        return SharedSerial(entry, control=control)


# {port name in lower case: id of whoever holds it}
_held = {}
_held_guard = threading.Lock()


def hold(port_names, owner=None):
    """Stop the monitor drawing on these panels until release()."""
    with _held_guard:
        for name in port_names:
            _held[name.lower()] = owner
    log.info(f"Panels lent to the White Rabbit window: {', '.join(port_names)}")


def release(port_names=None):
    """Let the monitor draw on these panels again (all of them when port_names is None)."""
    with _held_guard:
        names = list(_held) if port_names is None else [n.lower() for n in port_names]
        released = [n for n in names if n in _held]
        for n in released:
            del _held[n]
    if released:
        log.info(f"Panels back with the monitor: {', '.join(released)}")
    return released


def is_held(port_name):
    with _held_guard:
        return port_name.lower() in _held


def held():
    """{port name: owner} of the held panels."""
    with _held_guard:
        return dict(_held)


def port_lock(s):
    """Lock to hold while sending a multi-command sequence (e.g. a full frame) to a port."""
    lock = getattr(s, 'lock', None)
    return lock if lock is not None else _no_lock


class _NoLock:
    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


_no_lock = _NoLock()


def close_all():
    """Close every shared port (used on Quit so both COM ports are released)."""
    with _ports_guard:
        entries = list(_ports.values())
        _ports.clear()
    for entry in entries:
        entry.close()
