# Temperatures and fan speeds on Windows, read from Framework Control (https://github.com/ozturkkl/framework-control).
# Its Windows service runs framework_tool with system rights and shares the result on
# http://127.0.0.1:<port>/api/thermal, so White Rabbit gets the values without admin rights.

# Built In Dependencies
import json
import threading
import time
import logging
import urllib.request

# Internal Dependencies
from led_mon import external_apps

log = logging.getLogger(__name__)

CACHE_SEC = 2.0
TIMEOUT_SEC = 1.0
# The CPU first on the panel, then the APU, the GPU module, then the rest (ambient, charger, ...)
_NAME_ORDER = (("cpu", 0), ("apu", 1), ("gpu", 2))


def _rank(name):
    name = name.lower()
    for part, rank in _NAME_ORDER:
        if name.startswith(part):
            return rank
    return 3


def parse_thermal(data):
    """Turn /api/thermal JSON ({"temps": {name: C}, "fans": [{name, rpm}]}) into ([temps in C], [fan RPMs])."""
    temps = []
    for name, value in (data.get("temps") or {}).items():
        try:
            value = float(value)
        except (TypeError, ValueError):
            continue
        # 0: the sensor isn't powered, e.g. the GPU module's sensors while it is off
        if value > 0:
            temps.append((_rank(name), name, value))
    fans = []
    for fan in data.get("fans") or []:
        try:
            fans.append(max(0, int(fan.get("rpm"))))
        except (TypeError, ValueError, AttributeError):
            continue
    return [t for _, _, t in sorted(temps)], fans


def _fetch():
    """/api/thermal JSON from the Framework Control service, or None when it isn't installed."""
    port = external_apps.framework_control_port()
    if not port:
        return None
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/api/thermal", timeout=TIMEOUT_SEC) as response:
        return json.load(response)


class FrameworkControlSensors:
    """Cached readings. Refreshes in the background so the main loop never waits on the service."""

    def __init__(self, fetcher=None):
        self._fetcher = fetcher or _fetch
        self._lock = threading.Lock()
        self._values = ([], [])
        self._last_refresh = 0.0
        self._refreshing = False
        self._available = None

    def _set_available(self, available, reason=""):
        if available != self._available:
            self._available = available
            if available:
                log.info("Reading temperatures and fans from Framework Control")
            else:
                log.info(f"Framework Control isn't available{reason}; "
                         "temperatures and fans fall back to framework_tool")

    def _refresh(self):
        try:
            data = self._fetcher()
            values = parse_thermal(data) if data else ([], [])
            self._set_available(bool(data), "" if data else " (not installed)")
        except Exception as e:
            values = ([], [])
            self._set_available(False, f" ({e})")
        finally:
            with self._lock:
                self._values = values
                self._refreshing = False

    @property
    def answered(self):
        """True once the first refresh has finished, whether or not Framework Control was there."""
        return self._available is not None

    def get(self):
        """Return ([temps], [fan RPMs]) from the last reading, starting a refresh if it is stale."""
        with self._lock:
            now = time.monotonic()
            if not self._refreshing and now - self._last_refresh >= CACHE_SEC:
                self._refreshing = True
                self._last_refresh = now
                threading.Thread(target=self._refresh, daemon=True).start()
            return self._values


sensors = FrameworkControlSensors()


def windows_readings():
    """([temps], [fan RPMs]) for the temperature and fan apps on Windows.

    Each list comes from Framework Control when it has values, otherwise from framework_tool,
    which only works when White Rabbit runs as admin.
    """
    from led_mon.framework_tool import thermal
    temps, fans = sensors.get()
    # Until Framework Control's first answer is in, don't start framework_tool just in case
    if sensors.answered and (not temps or not fans):
        tool_temps, tool_fans = thermal.get()
        temps = temps or tool_temps
        fans = fans or tool_fans
    return temps, fans
