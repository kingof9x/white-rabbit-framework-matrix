# Sharing the LED panels between the monitor (tray process) and the White Rabbit window.
#
# Windows lets only one program open a panel's serial port, and the monitor keeps both open. So while
# the monitor runs, the window sends its commands through the monitor's control channel
# (led_mon/control.py) instead of opening the ports itself:
#
#   serial   write bytes to a panel and optionally read the reply
#   hold     lend panels to the window: the monitor stops drawing on them
#   release  give them back to the monitor
#
# serve() adds these commands to the monitor; the rest of this file is the window's side.
# When the monitor isn't running the window opens the ports directly, as the old LED Matrix Control app did.

# Built In Dependencies
import os
import time
import logging
import threading

# Internal Dependencies
from led_mon import control, serial_ports

# External Dependencies
import serial  # pyserial

log = logging.getLogger(__name__)

# Longest wait for a panel's reply. Old firmware doesn't answer some commands at all.
MAX_READ_SECONDS = 3.0


# Monitor side

def _serial(message):
    port = message["port"]
    s = serial_ports.open_port(port, control=True)
    try:
        with serial_ports.port_lock(s):
            if message.get("reset_input"):
                s.reset_input_buffer()
            # Each command is written on its own, as the window wrote it: the panel's firmware reads one
            # command per USB read, so commands sent together in one write would be lost after the first
            chunks = message.get("writes")
            if chunks is None:
                chunks = [message.get("write") or ""]
            for chunk in chunks:
                data = bytes.fromhex(chunk)
                if data:
                    s.write(data)
            size = int(message.get("read") or 0)
            if not size:
                return {"data": ""}
            old_timeout = s.timeout
            timeout = message.get("timeout")
            s.timeout = MAX_READ_SECONDS if timeout is None else min(float(timeout), MAX_READ_SECONDS)
            try:
                return {"data": s.read(size).hex()}
            finally:
                s.timeout = old_timeout
    finally:
        s.close()


def _serial_many(message):
    """Write commands to several panels at once, taking turns between them.

    A panel is busy for a moment after each command, so while one panel works on its command the
    next panel already gets its own: two panels take about as long as one, instead of twice as long.
    """
    items = [(item["port"], [bytes.fromhex(c) for c in item.get("writes", [])]) for item in message["ports"]]
    handles = [serial_ports.open_port(port, control=True) for port, _ in items]
    try:
        # Lock in a fixed order, so two of these at once can't each wait for the other's port
        locks = sorted(((port.lower(), serial_ports.port_lock(s)) for (port, _), s in zip(items, handles)),
                       key=lambda pair: pair[0])
        for _, lock in locks:
            lock.acquire()
        try:
            interleave(handles, [chunks for _, chunks in items])
        finally:
            for _, lock in reversed(locks):
                lock.release()
    finally:
        for s in handles:
            s.close()
    return {}


def interleave(handles, chunk_lists):
    """Write chunk_lists[i] to handles[i], one command per panel in turn."""
    for step in range(max((len(c) for c in chunk_lists), default=0)):
        for s, chunks in zip(handles, chunk_lists):
            if step < len(chunks) and chunks[step]:
                s.write(chunks[step])


def _hold(message):
    serial_ports.hold(message.get("ports", []), owner=message.get("pid"))
    return {"held": list(serial_ports.held())}


def _release(message):
    return {"released": serial_ports.release(message.get("ports"))}


def _pid_alive(pid):
    try:
        import psutil
        return psutil.pid_exists(pid)
    except Exception:
        return True


def _watch_holders(stop, interval=2.0):
    """Give panels back to the monitor when the window that held them has closed or crashed."""
    while not stop.wait(interval):
        for port, owner in serial_ports.held().items():
            if isinstance(owner, int) and not _pid_alive(owner):
                log.info(f"The window holding {port} has closed")
                serial_ports.release([port])


def serve(handlers):
    """Add the window's commands to the monitor's control channel handlers (a dict)."""
    handlers.update({"serial": _serial, "serial_many": _serial_many, "hold": _hold, "release": _release})
    stop = threading.Event()
    threading.Thread(target=_watch_holders, args=(stop,), daemon=True, name="white_rabbit-holders").start()
    return stop


# Window side

class Link:
    """Remembers for a moment whether the monitor is running, so each command doesn't have to ask."""

    def __init__(self, request=None, ttl=2.0):
        self.request = request or control.request
        self.ttl = ttl
        self._checked = 0.0
        self._status = None
        self._lock = threading.Lock()

    def status(self, fresh=False):
        """The monitor's status ({'ports': [...]}) or None if it isn't running."""
        with self._lock:
            if fresh or time.monotonic() - self._checked > self.ttl:
                reply = self.request({"cmd": "status"}, timeout=1.5)
                self._status = reply if reply and reply.get("ok") else None
                self._checked = time.monotonic()
            return self._status

    def running(self, fresh=False):
        return self.status(fresh) is not None

    def call(self, message, timeout=MAX_READ_SECONDS + 2):
        reply = self.request(message, timeout=timeout)
        if reply is None:
            with self._lock:
                self._checked = 0.0
            raise serial.SerialException("The White Rabbit monitor stopped answering")
        if not reply.get("ok"):
            raise serial.SerialException(reply.get("error", "The monitor couldn't reach the panel"))
        return reply

    def hold(self, ports):
        if ports and self.running():
            self.call({"cmd": "hold", "ports": list(ports), "pid": os.getpid()})

    def release(self, ports=None):
        if self.running():
            self.call({"cmd": "release", "ports": None if ports is None else list(ports)})


class ProxySerial:
    """Stands in for serial.Serial in the inputmodule code, sending through the monitor.

    Writes are collected and sent together with the next read, or when the port is closed. The monitor
    still writes them to the panel one by one.
    """

    def __init__(self, link, port, timeout=None):
        self.link = link
        self.port = port
        self.timeout = timeout
        self._pending = []
        self._reset_input = False

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
        return False

    def reset_input_buffer(self):
        self._reset_input = True

    def write(self, data):
        data = bytes(data)
        if data:
            self._pending.append(data.hex())
        return len(data)

    def _send(self, read=0):
        message = {"cmd": "serial", "port": self.port, "writes": list(self._pending), "read": read,
                   "timeout": self.timeout, "reset_input": self._reset_input}
        self._pending.clear()
        self._reset_input = False
        return bytes.fromhex(self.link.call(message).get("data", ""))

    def read(self, size=1):
        return self._send(read=size)

    def flush(self):
        if self._pending or self._reset_input:
            self._send()

    def close(self):
        self.flush()


def write_many(link, writes_by_port):
    """Send commands ({port: [bytes, ...]}) to several panels at once, taking turns between them."""
    writes_by_port = {port: [bytes(c) for c in chunks] for port, chunks in writes_by_port.items() if chunks}
    if not writes_by_port:
        return
    if link.running():
        link.call({"cmd": "serial_many",
                   "ports": [{"port": port, "writes": [c.hex() for c in chunks]}
                             for port, chunks in writes_by_port.items()]})
        return
    from inputmodule import inputmodule as im
    handles = []
    try:
        for port in writes_by_port:
            handles.append(im.open_serial(port))
        interleave(handles, list(writes_by_port.values()))
    finally:
        for s in handles:
            s.close()


def make_opener(link, direct):
    """An inputmodule serial_opener: through the monitor while it runs, otherwise direct(port, timeout)."""
    def opener(port, timeout=None):
        if link.running():
            return ProxySerial(link, port, timeout)
        return direct(port, timeout)
    return opener


def install(link):
    """Make the inputmodule package talk through the monitor whenever it is running."""
    from inputmodule import inputmodule as im
    im.serial_opener = make_opener(link, im._open_pyserial)
