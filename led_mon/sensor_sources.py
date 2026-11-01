# Where the Temperatures and Fans apps get their readings on Windows (psutil can't read them there).
# The choice is "sensor_source" in settings.json, picked on the Panels tab:
#   auto                    try Framework Control, then Libre Hardware Monitor, then framework_tool
#   framework_control       Framework Control's local service (no admin rights needed)
#   libre_hardware_monitor  Libre Hardware Monitor's sensors through WMI (it must run as admin)
#   framework_tool          framework_tool --thermal (White Rabbit must run as admin)

# Built In Dependencies
import threading
import time
import logging

# Internal Dependencies
from led_mon import app_settings

log = logging.getLogger(__name__)

SETTING = "sensor_source"
AUTO = "auto"
FRAMEWORK_CONTROL = "framework_control"
LIBRE_HARDWARE_MONITOR = "libre_hardware_monitor"
FRAMEWORK_TOOL = "framework_tool"

# (key, label) in the order shown on the Panels tab
SOURCES = (
    (AUTO, "Automatic"),
    (FRAMEWORK_CONTROL, "Framework Control"),
    (LIBRE_HARDWARE_MONITOR, "Libre Hardware Monitor"),
    (FRAMEWORK_TOOL, "framework_tool"),
)
LABELS = dict(SOURCES)
AUTO_ORDER = (FRAMEWORK_CONTROL, LIBRE_HARDWARE_MONITOR, FRAMEWORK_TOOL)

# What each choice needs, shown under the choice on the Panels tab
HELP = {
    AUTO: "Tries Framework Control, then Libre Hardware Monitor, then framework_tool, and uses the first "
          "one that gives readings (temperatures and fans can come from different ones).",
    FRAMEWORK_CONTROL: "Needs Framework Control installed; its service runs in the background, so nothing has "
                       "to be open and no admin rights are needed. On a Framework 16 it gives the CPU, APU, "
                       "ambient and charger temperatures and both fans.",
    LIBRE_HARDWARE_MONITOR: "Needs Libre Hardware Monitor open (it can sit in the tray) and running as admin; "
                            "White Rabbit reads its sensors through Windows. Which sensors show depends on the "
                            "laptop: GPU and SSD temperatures are likely, fans may be missing.",
    FRAMEWORK_TOOL: "Needs framework_tool.exe next to WhiteRabbit.exe or on PATH, and White Rabbit itself "
                    "running as admin; otherwise Windows refuses to start it.",
}

# Readings older than this mean no panel is showing the Temperatures or Fans app
STALE_SEC = 10.0


def normalize(choice):
    return choice if choice in LABELS else AUTO


def default_readers():
    """The cached background reader for each source, created on first use."""
    from led_mon import framework_control_sensors, libre_hardware_monitor_sensors, framework_tool
    return {
        FRAMEWORK_CONTROL: framework_control_sensors.sensors,
        LIBRE_HARDWARE_MONITOR: libre_hardware_monitor_sensors.sensors,
        FRAMEWORK_TOOL: framework_tool.thermal,
    }


def read_once_functions():
    """Each source's direct (uncached) read, for --diagnose."""
    from led_mon import framework_control_sensors, libre_hardware_monitor_sensors, framework_tool
    return {
        FRAMEWORK_CONTROL: framework_control_sensors.read_once,
        LIBRE_HARDWARE_MONITOR: libre_hardware_monitor_sensors.read_once,
        FRAMEWORK_TOOL: framework_tool.read_once,
    }


class SensorSource:
    """The chosen source, and the readings for the Temperatures and Fans apps."""

    def __init__(self, path=None, readers=None):
        self.path = path
        self._readers = readers
        self._lock = threading.Lock()
        self.choice = AUTO
        self.temps_from = None
        self.fans_from = None
        self._counts = (0, 0)
        self._last_read = 0.0
        self.reload()

    @property
    def readers(self):
        if self._readers is None:
            self._readers = default_readers()
        return self._readers

    def reload(self):
        """Read the choice from settings.json again (the window changed it)."""
        choice = normalize(app_settings.load_settings(self.path).get(SETTING, AUTO))
        with self._lock:
            changed = choice != self.choice
            self.choice = choice
            if changed:
                self.temps_from = self.fans_from = None
                self._counts = (0, 0)
        if changed:
            log.info(f"Temperatures and fans now come from: {LABELS[choice]}")
        return choice

    def set(self, choice, save=True):
        choice = normalize(choice)
        if save:
            settings = app_settings.load_settings(self.path)
            settings[SETTING] = choice
            app_settings.save_settings(settings, self.path)
        with self._lock:
            self.choice = choice
            self.temps_from = self.fans_from = None
        return choice

    def readings(self):
        """([temps in C], [fan RPMs]) from the chosen source."""
        with self._lock:
            choice = self.choice
        if choice != AUTO:
            temps, fans = self.readers[choice].get()
            temps_from = choice if temps else None
            fans_from = choice if fans else None
        else:
            temps, fans, temps_from, fans_from = [], [], None, None
            for key in AUTO_ORDER:
                reader = self.readers[key]
                source_temps, source_fans = reader.get()
                if not temps and source_temps:
                    temps, temps_from = source_temps, key
                if not fans and source_fans:
                    fans, fans_from = source_fans, key
                # Wait for each source's first answer before starting the next one, so a working
                # Framework Control doesn't also start Libre Hardware Monitor queries or framework_tool
                if (temps and fans) or not reader.answered:
                    break
        with self._lock:
            self.temps_from, self.fans_from = temps_from, fans_from
            self._counts = (len(temps), len(fans))
            self._last_read = time.monotonic()
        return temps, fans

    def status(self):
        """For the Panels tab: the choice, and where the last readings came from."""
        with self._lock:
            reading = time.monotonic() - self._last_read < STALE_SEC
            state = {"choice": self.choice, "reading": reading,
                     "temps_from": self.temps_from, "fans_from": self.fans_from,
                     "temps": self._counts[0], "fans": self._counts[1]}
        if reading:
            keys = [self.choice] if self.choice != AUTO else list(AUTO_ORDER)
            errors = {}
            for key in keys:
                reader = self.readers[key]
                if reader.answered and getattr(reader, "error", ""):
                    errors[key] = reader.error
            state["errors"] = errors
        return state


def describe(status):
    """One or two lines for the Panels tab from status()."""
    if not status:
        return "The monitor isn't running."
    if not status.get("reading"):
        return "No panel is showing Temperatures or Fans right now, so nothing is being read."
    parts = []
    for what, count_key, from_key in (("Temperatures", "temps", "temps_from"), ("Fans", "fans", "fans_from")):
        source = status.get(from_key)
        if source:
            parts.append(f"{what}: {status.get(count_key, 0)} from {LABELS.get(source, source)}")
        else:
            parts.append(f"{what}: none")
    text = "Now reading. " + "; ".join(parts) + "."
    errors = status.get("errors") or {}
    if errors and (not status.get("temps_from") or not status.get("fans_from")):
        text += "\n" + "\n".join(f"{LABELS.get(k, k)}: {v}" for k, v in errors.items())
    return text


_source = None
_source_lock = threading.Lock()


def current():
    """The process-wide SensorSource, loaded from settings.json on first use."""
    global _source
    with _source_lock:
        if _source is None:
            _source = SensorSource()
        return _source


def windows_readings():
    """([temps], [fan RPMs]) for the Temperatures and Fans apps on Windows."""
    return current().readings()


def diagnose_lines(read_once=None, settings_path=None, wait_sec=8.0):
    """The --diagnose section: the chosen source, each source read directly, and what the choice gives."""
    lines = ["Temperatures and fans:"]
    source = SensorSource(settings_path)
    lines.append(f"  Chosen source: {LABELS[source.choice]} ({source.choice})")
    read_once = read_once or read_once_functions()
    for key in AUTO_ORDER:
        try:
            temps, fans = read_once[key]()
            lines.append(f"  {LABELS[key]}: {len(temps)} temperatures {[round(t) for t in temps]}, "
                         f"{len(fans)} fans {fans}")
        except Exception as e:
            lines.append(f"  {LABELS[key]}: not available ({type(e).__name__}: {e})")
    # The same path the panels use, through the background readers
    end = time.monotonic() + wait_sec
    keys = [source.choice] if source.choice != AUTO else list(AUTO_ORDER)
    while True:
        temps, fans = source.readings()
        if (temps and fans) or time.monotonic() >= end or all(source.readers[k].answered for k in keys):
            break
        time.sleep(0.2)
    # One more pass, so a source that just answered is counted
    source.readings()
    lines.append(f"  Panels get: temperatures from {LABELS.get(source.temps_from, 'nowhere')}, "
                 f"fans from {LABELS.get(source.fans_from, 'nowhere')}")
    return lines
