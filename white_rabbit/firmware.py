# Firmware that comes with White Rabbit, and flashing it onto a panel.
#
# The release build downloads two firmware files into res/releases/:
#   white-rabbit/ledmatrix.uf2      kingof9x/inputmodule-rs: startup animation setting, extra Game of Life patterns
#   framework-0.2.0/ledmatrix.uf2   Framework's official 0.2.0 release

# Built In Dependencies
import os
import sys
import time
import logging

# Internal Dependencies
from inputmodule import uf2conv
from inputmodule.inputmodule import bootloader_jump

log = logging.getLogger(__name__)

BUNDLED = [
    ("white-rabbit", "White Rabbit firmware (startup animation setting, extra Game of Life patterns)"),
    ("framework-0.2.0", "Framework's official firmware 0.2.0"),
]
FIRMWARE_FILE = "ledmatrix.uf2"
BOOTLOADER_WAIT_SECONDS = 10


def resource_dir():
    """Folder with res/: inside the exe when frozen, otherwise the repository."""
    if hasattr(sys, "_MEIPASS"):
        return sys._MEIPASS
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def bundled_firmware(base=None):
    """[(label, path)] of the firmware files that are actually there."""
    base = base or os.path.join(resource_dir(), "res", "releases")
    found = []
    for folder, label in BUNDLED:
        path = os.path.join(base, folder, FIRMWARE_FILE)
        if os.path.isfile(path):
            found.append((label, path))
    return found


def wait_for_drives(seconds=BOOTLOADER_WAIT_SECONDS, get_drives=uf2conv.get_drives, sleep=time.sleep):
    """The RPI-RP2 drives of modules in bootloader mode, waiting up to seconds for one to appear."""
    deadline = time.monotonic() + seconds
    while True:
        drives = get_drives()
        if drives or time.monotonic() >= deadline:
            return drives
        sleep(0.5)


def flash(panel, path, status=lambda text: None, get_drives=uf2conv.get_drives, jump=bootloader_jump):
    """Put the panel into bootloader mode and copy the firmware onto its drive. Returns a message.

    Raises RuntimeError when no bootloader drive shows up.
    """
    with open(path, "rb") as f:
        firmware = f.read()
    drives = get_drives()
    if not drives:
        status(f"Putting the {panel.side} panel into bootloader mode...")
        jump(panel)
        drives = wait_for_drives(get_drives=get_drives)
    if not drives:
        raise RuntimeError("No RPI-RP2 drive appeared. Try Enter bootloader, then flash again.")
    for drive in drives:
        status(f"Copying the firmware to {drive}...")
        log.info(f"Flashing {path} onto {drive}")
        # Not os.path.join: on Windows that gives "E:NEW.UF2", relative to E:'s current folder
        uf2conv.write_file(drive + "/NEW.UF2", firmware)
    return f"Flashed {len(drives)} panel{'s' if len(drives) > 1 else ''}. It restarts with the new firmware in a few seconds."
