# The LED panels as the White Rabbit window sees them: which is left and which is right,
# and which of them the user has ticked at the bottom of the window. Each side can also have a name
# the user picked ("panel_names" in settings.json), shown in the window and by Identify panels.

# Built In Dependencies
import logging
from dataclasses import dataclass

# Internal Dependencies
from led_mon import app_settings, shared_state

log = logging.getLogger(__name__)


@dataclass
class Panel:
    """One LED Matrix module. Has the .device and .name the inputmodule functions expect."""
    side: str            # 'left' or 'right' (or 'panel 3' and so on, just in case)
    device: str          # serial port, e.g. COM4
    location: str = None  # USB position, e.g. 1-4.2
    serial_number: str = ""
    nickname: str = ""   # the name the user gave this side, if any

    @property
    def name(self):
        return self.device

    @property
    def label(self):
        if self.nickname:
            return f"{self.nickname} ({self.side}, {self.device})"
        return f"{self.side.capitalize()} panel ({self.device})"

    @property
    def called(self):
        """How messages name the panel: 'left panel', or 'left panel (Desk)' once it has a name."""
        return f"{self.side} panel ({self.nickname})" if self.nickname else f"{self.side} panel"


def find_panels(config=None, ports=None):
    """The connected panels in left, right order, using the layout's left/right port settings."""
    from serial.tools import list_ports
    if config is not None:
        shared_state.set_overrides_from_config(config)
    try:
        ports = list(list_ports.comports()) if ports is None else ports
    except Exception as e:
        log.error(f"Could not list the serial ports: {e}")
        return []
    found = shared_state.discover_led_devices(ports) or []
    serials = {p.device.lower(): (getattr(p, "serial_number", "") or "") for p in ports}
    sides = ["left", "right"]
    names = app_settings.load_panel_names()
    panels = []
    for i, (location, device) in enumerate(found):
        side = sides[i] if i < len(sides) else f"panel {i + 1}"
        panels.append(Panel(side, device, location, serials.get(device.lower(), ""), names.get(side, "")))
    return panels
