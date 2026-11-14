# Noticing things being plugged into the laptop's ports and unplugged, and working out which port.
#
# Windows tells a (hidden) window about every USB device that arrives or leaves (WM_DEVICECHANGE) and about
# the charger (WM_POWERBROADCAST / PBT_APMPOWERSTATUSCHANGE). For each event this module works out where the
# port is, as 'left-back', 'right-middle' and so on, and calls on_plug(kind, position, plugged_in, info).
#
# Where the port is:
#   - The charger: framework_tool --pd-info says which USB-C port is charging the laptop (Right Back, Right
#     Middle, Left Middle, Left Back on the Framework 16). When it can't run, the port picked on the
#     Effects tab the last time is used.
#   - A USB device: a port picked for it on the Effects tab (remembered by the USB root port the device is
#     on), else the side of the laptop Windows reports for the port (ACPI _PLD), else unknown.
#   Unknown ports ripple from the middle of the panels.

# Built In Dependencies
import re
import time
import logging
import threading
import subprocess

# Internal Dependencies
from led_mon.platform_utils import IS_WINDOWS

log = logging.getLogger(__name__)

GUID_DEVINTERFACE_USB_DEVICE = "A5DCBF10-6530-11D2-901F-00C04FB951ED"
FRAMEWORK_VID = 0x32AC
AUDIO_CARD_PID = 0x0010
# Framework input modules (keyboards, numpad, macropad, LED matrix): moving them isn't plugging anything in
INPUT_MODULE_PIDS = set(range(0x0012, 0x0030))
# Devices arriving right after waking up are the ones that were already there
QUIET_AFTER_RESUME_SEC = 10.0
# A hub or dock brings several devices at once: one ripple per side for those
SAME_SIDE_GAP_SEC = 1.5

WM_DEVICECHANGE = 0x0219
DBT_DEVICEARRIVAL = 0x8000
DBT_DEVICEREMOVECOMPLETE = 0x8004
DBT_DEVTYP_DEVICEINTERFACE = 5
WM_POWERBROADCAST = 0x0218
PBT_APMPOWERSTATUSCHANGE = 0x000A
PBT_APMRESUMESUSPEND = 0x0007
PBT_APMRESUMEAUTOMATIC = 0x0012

# framework_tool --pd-info port names on the Framework 16
PD_PORT_RE = re.compile(r"^USB-C Port\s+(\d+)\s*\(([^)]*)\)", re.IGNORECASE)
PD_ROLE_RE = re.compile(r"^\s*Role:\s*(\w+)", re.IGNORECASE)


def instance_id_from_interface(name):
    r"""'\\?\USB#VID_32AC&PID_0010#123#{a5dcbf10-...}' -> 'USB\VID_32AC&PID_0010\123'."""
    if not name:
        return None
    text = name
    for prefix in ("\\\\?\\", "\\??\\", "\\\\.\\"):
        if text.startswith(prefix):
            text = text[len(prefix):]
            break
    if "#{" in text:
        text = text[:text.rindex("#{")]
    return text.replace("#", "\\").upper()


def vid_pid(instance_id):
    """(vid, pid) from a USB instance id, or (None, None)."""
    m = re.search(r"VID_([0-9A-F]{4})&PID_([0-9A-F]{4})", instance_id or "", re.IGNORECASE)
    if not m:
        return None, None
    return int(m.group(1), 16), int(m.group(2), 16)


def is_ignored(instance_id):
    vid, pid = vid_pid(instance_id)
    return vid == FRAMEWORK_VID and pid in INPUT_MODULE_PIDS


def is_audio_card(instance_id):
    return vid_pid(instance_id) == (FRAMEWORK_VID, AUDIO_CARD_PID)


def location_key(location_paths):
    """The USB root port a device is on, from its location paths, e.g.
    'PCIROOT(0)#PCI(0801)#PCI(0004)#USBROOT(0)#USB(2)#USB(1)' -> 'PCIROOT(0)#PCI(0801)#PCI(0004)#USBROOT(0)#USB(2)'.
    Devices in the same port (also behind a hub in it) share a key."""
    for path in location_paths or []:
        m = re.match(r"^(.*?#USBROOT\(\d+\)#USB\(\d+\))", path or "", re.IGNORECASE)
        if m:
            return m.group(1).upper()
    return None


def pld_panel(buffer):
    """Which side of the laptop an ACPI _PLD buffer puts a port on: 'left', 'right' or None.
    Also returns the raw panel, vertical and horizontal position numbers for the log."""
    if not buffer or len(buffer) < 10:
        return None, None
    bits = int.from_bytes(bytes(buffer[:16]).ljust(16, b"\0"), "little")
    panel = (bits >> 67) & 0x7
    vertical = (bits >> 70) & 0x3
    horizontal = (bits >> 72) & 0x3
    side = {2: "left", 3: "right"}.get(panel)
    return side, {"panel": panel, "vertical": vertical, "horizontal": horizontal}


def parse_pd_info(text):
    """The port charging the laptop from framework_tool --pd-info output, as 'right-back' etc., or None."""
    port_name = None
    for line in (text or "").splitlines():
        m = PD_PORT_RE.match(line.strip())
        if m:
            port_name = m.group(2)
            continue
        m = PD_ROLE_RE.match(line)
        if m and port_name and m.group(1).lower() == "sink":
            words = port_name.lower().split()
            side = next((w for w in words if w in ("left", "right")), None)
            spot = next((w for w in words if w in ("back", "middle", "front")), None)
            if side:
                return f"{side}-{spot}" if spot else side
    return None


def resolve_position(key, learned, pld_side):
    """Where a USB device's port is, and how we know: (position or None, 'learned' | 'side' | 'unknown')."""
    if key and key in (learned or {}):
        return learned[key], "learned"
    if pld_side:
        return pld_side, "side"
    return None, "unknown"


class PlugWatcher:
    """Calls on_plug(kind, position, plugged_in, info) from its own thread.

    kind: 'charger', 'headphones' or 'usb'. info: {'name', 'key', 'how'} for the Effects tab.
    """

    def __init__(self, on_plug, learned=None):
        self.on_plug = on_plug
        self.learned = learned or (lambda: {})
        self.present = {}            # instance id: (key, position, how, name, kind)
        self.ac_online = None
        self.charger_position = None
        self.quiet_until = 0.0
        self.last_side_time = {}
        self._hwnd = None
        self._thread = None
        self._lock = threading.Lock()

    def start(self):
        if not IS_WINDOWS:
            return False
        self._thread = threading.Thread(target=self._run, name="plug-events", daemon=True)
        self._thread.start()
        return True

    def stop(self):
        if self._hwnd:
            try:
                import ctypes
                ctypes.windll.user32.PostMessageW(self._hwnd, 0x0010, 0, 0)   # WM_CLOSE
            except Exception:
                pass

    def _run(self):
        try:
            self.ac_online = _ac_online()
            for instance_id in _present_usb_devices():
                self._remember(instance_id)
            _message_loop(self)
        except Exception as e:
            log.error(f"Could not listen for things being plugged in: {e}")

    # Events (also called by tests)
    def _remember(self, instance_id):
        props = _device_properties(instance_id)
        key = location_key(props.get("location_paths"))
        side, raw = pld_panel(props.get("pld"))
        position, how = resolve_position(key, self.learned(), side)
        kind = "headphones" if is_audio_card(instance_id) else "usb"
        entry = (key, position, how, props.get("name") or instance_id, kind)
        with self._lock:
            self.present[instance_id] = entry
        return entry, raw

    def resumed(self):
        self.quiet_until = time.monotonic() + QUIET_AFTER_RESUME_SEC

    def device_arrived(self, instance_id):
        if not instance_id or is_ignored(instance_id):
            return
        (key, position, how, name, kind), raw = self._remember(instance_id)
        log.info(f"Plugged in: {name} ({instance_id}) at {key}, port {position or 'unknown'} ({how}), _PLD {raw}")
        self._fire(kind, position, True, {"name": name, "key": key, "how": how})

    def device_removed(self, instance_id):
        if not instance_id or is_ignored(instance_id):
            return
        with self._lock:
            entry = self.present.pop(instance_id, None)
        if entry is None:
            return
        key, position, how, name, kind = entry
        # A port corrected since it was plugged in
        position = (self.learned() or {}).get(key, position) if key else position
        log.info(f"Unplugged: {name} from port {position or 'unknown'}")
        self._fire(kind, position, False, {"name": name, "key": key, "how": how})

    def power_changed(self):
        online = _ac_online()
        if online is None or online == self.ac_online:
            return
        self.ac_online = online
        if online:
            threading.Thread(target=self._charger_in, daemon=True).start()
        else:
            self._fire("charger", self.charger_position, False,
                       {"name": "Charger", "key": "charger", "how": "remembered"})

    def _charger_in(self):
        position, how = charger_port(), "framework_tool"
        if position is None:
            position, how = (self.learned() or {}).get("charger"), "learned"
            if position is None:
                how = "unknown"
        self.charger_position = position
        log.info(f"Charger plugged in at port {position or 'unknown'} ({how})")
        self._fire("charger", position, True, {"name": "Charger", "key": "charger", "how": how})

    def _fire(self, kind, position, plugged_in, info):
        now = time.monotonic()
        if now < self.quiet_until:
            return
        if kind != "charger" and plugged_in:
            side = (position or "").split("-")[0] or None
            if now - self.last_side_time.get(side, -99) < SAME_SIDE_GAP_SEC:
                return
            self.last_side_time[side] = now
        try:
            self.on_plug(kind, position, plugged_in, info)
        except Exception as e:
            log.error(f"Error playing the plug-in effect: {e}")


def charger_port(runner=None):
    """Which port the charger is in, from framework_tool --pd-info, or None."""
    try:
        if runner is None:
            from led_mon import framework_tool, platform_utils
            tool = framework_tool.find_framework_tool()
            if not tool:
                return None
            result = subprocess.run([tool, "--pd-info"], capture_output=True, text=True, timeout=5,
                                    stdin=subprocess.DEVNULL, creationflags=platform_utils.subprocess_flags())
            output = result.stdout
        else:
            output = runner()
        return parse_pd_info(output)
    except Exception as e:
        log.debug(f"framework_tool --pd-info failed: {e}")
        return None


# Windows parts
def _ac_online():
    if not IS_WINDOWS:
        return None
    import ctypes

    class SYSTEM_POWER_STATUS(ctypes.Structure):
        _fields_ = [("ACLineStatus", ctypes.c_ubyte), ("BatteryFlag", ctypes.c_ubyte),
                    ("BatteryLifePercent", ctypes.c_ubyte), ("SystemStatusFlag", ctypes.c_ubyte),
                    ("BatteryLifeTime", ctypes.c_ulong), ("BatteryFullLifeTime", ctypes.c_ulong)]
    status = SYSTEM_POWER_STATUS()
    if not ctypes.windll.kernel32.GetSystemPowerStatus(ctypes.byref(status)):
        return None
    return {0: False, 1: True}.get(status.ACLineStatus)


def _devpropkey(guid_text, pid):
    import ctypes
    import uuid
    from ctypes import wintypes

    class DEVPROPKEY(ctypes.Structure):
        _fields_ = [("fmtid", ctypes.c_ubyte * 16), ("pid", wintypes.ULONG)]
    key = DEVPROPKEY()
    key.fmtid[:] = uuid.UUID(guid_text).bytes_le
    key.pid = pid
    return key


DEVPKEY_LOCATION_PATHS = ("A45C254E-DF1C-4EFD-8020-67D146A850E0", 37)
DEVPKEY_PHYSICAL_LOCATION = ("540B947E-8B40-45BC-A8A2-6A0B894CBDA2", 9)
DEVPKEY_BUS_REPORTED_DESC = ("540B947E-8B40-45BC-A8A2-6A0B894CBDA2", 4)
DEVPKEY_DEVICE_DESC = ("A45C254E-DF1C-4EFD-8020-67D146A850E0", 2)


def _device_properties(instance_id):
    """{'location_paths': [...], 'pld': bytes or None, 'name': str} for a device instance id."""
    out = {"location_paths": [], "pld": None, "name": None}
    if not IS_WINDOWS:
        return out
    import ctypes
    from ctypes import wintypes
    cfg = ctypes.WinDLL("cfgmgr32")
    devinst = wintypes.DWORD()
    if cfg.CM_Locate_DevNodeW(ctypes.byref(devinst), ctypes.c_wchar_p(instance_id), 0) != 0:
        return out

    def get(node, key):
        prop_type = wintypes.ULONG()
        size = wintypes.ULONG(0)
        k = _devpropkey(*key)
        cfg.CM_Get_DevNode_PropertyW(node, ctypes.byref(k), ctypes.byref(prop_type), None, ctypes.byref(size), 0)
        if not size.value:
            return None
        buf = (ctypes.c_ubyte * size.value)()
        if cfg.CM_Get_DevNode_PropertyW(node, ctypes.byref(k), ctypes.byref(prop_type), buf,
                                        ctypes.byref(size), 0) != 0:
            return None
        return bytes(buf)

    def strings(raw):
        if raw is None:
            return []
        return [s for s in raw.decode("utf-16-le", "ignore").split("\0") if s]

    node = devinst.value
    out["name"] = next(iter(strings(get(node, DEVPKEY_BUS_REPORTED_DESC)) or strings(get(node, DEVPKEY_DEVICE_DESC))), None)
    # Walk up to the port: the device itself, then its parents (a hub in the port, ...)
    for _ in range(6):
        if not out["location_paths"]:
            out["location_paths"] = strings(get(node, DEVPKEY_LOCATION_PATHS))
        if out["pld"] is None:
            out["pld"] = get(node, DEVPKEY_PHYSICAL_LOCATION)
        if out["location_paths"] and out["pld"] is not None:
            break
        parent = wintypes.DWORD()
        if cfg.CM_Get_Parent(ctypes.byref(parent), node, 0) != 0:
            break
        node = parent.value
    return out


def _present_usb_devices():
    """Instance ids of the USB devices plugged in now."""
    if not IS_WINDOWS:
        return []
    import ctypes
    import uuid
    from ctypes import wintypes
    cfg = ctypes.WinDLL("cfgmgr32")
    guid = (ctypes.c_ubyte * 16)(*uuid.UUID(GUID_DEVINTERFACE_USB_DEVICE).bytes_le)
    size = wintypes.ULONG(0)
    if cfg.CM_Get_Device_Interface_List_SizeW(ctypes.byref(size), guid, None, 0) != 0 or not size.value:
        return []
    buf = ctypes.create_unicode_buffer(size.value)
    if cfg.CM_Get_Device_Interface_ListW(guid, None, buf, size.value, 0) != 0:
        return []
    names = [n for n in ctypes.wstring_at(ctypes.addressof(buf), size.value).split("\0") if n]
    return [instance_id_from_interface(n) for n in names]


def _message_loop(watcher):
    import ctypes
    import uuid
    from ctypes import wintypes

    user32 = ctypes.WinDLL("user32", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    LRESULT = ctypes.c_ssize_t
    WNDPROC = ctypes.WINFUNCTYPE(LRESULT, wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM)

    class WNDCLASSW(ctypes.Structure):
        _fields_ = [("style", wintypes.UINT), ("lpfnWndProc", WNDPROC), ("cbClsExtra", ctypes.c_int),
                    ("cbWndExtra", ctypes.c_int), ("hInstance", wintypes.HINSTANCE), ("hIcon", wintypes.HICON),
                    ("hCursor", wintypes.HANDLE), ("hbrBackground", wintypes.HBRUSH),
                    ("lpszMenuName", wintypes.LPCWSTR), ("lpszClassName", wintypes.LPCWSTR)]

    class DEV_BROADCAST_HDR(ctypes.Structure):
        _fields_ = [("dbch_size", wintypes.DWORD), ("dbch_devicetype", wintypes.DWORD),
                    ("dbch_reserved", wintypes.DWORD)]

    class DEV_BROADCAST_DEVICEINTERFACE_W(ctypes.Structure):
        _fields_ = [("dbcc_size", wintypes.DWORD), ("dbcc_devicetype", wintypes.DWORD),
                    ("dbcc_reserved", wintypes.DWORD), ("dbcc_classguid", ctypes.c_ubyte * 16),
                    ("dbcc_name", ctypes.c_wchar * 1)]

    user32.DefWindowProcW.restype = LRESULT
    user32.DefWindowProcW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    user32.RegisterClassW.argtypes = [ctypes.POINTER(WNDCLASSW)]
    user32.RegisterClassW.restype = wintypes.ATOM
    user32.CreateWindowExW.restype = wintypes.HWND
    user32.CreateWindowExW.argtypes = [wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD,
                                       ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                                       wintypes.HWND, wintypes.HMENU, wintypes.HINSTANCE, wintypes.LPVOID]
    user32.RegisterDeviceNotificationW.restype = wintypes.HANDLE
    user32.RegisterDeviceNotificationW.argtypes = [wintypes.HANDLE, wintypes.LPVOID, wintypes.DWORD]
    user32.UnregisterDeviceNotification.argtypes = [wintypes.HANDLE]
    user32.GetMessageW.argtypes = [ctypes.POINTER(wintypes.MSG), wintypes.HWND, wintypes.UINT, wintypes.UINT]
    user32.DestroyWindow.argtypes = [wintypes.HWND]
    user32.UnregisterClassW.argtypes = [wintypes.LPCWSTR, wintypes.HINSTANCE]
    kernel32.GetModuleHandleW.restype = wintypes.HMODULE
    kernel32.GetModuleHandleW.argtypes = [wintypes.LPCWSTR]

    def interface_name(lparam):
        hdr = DEV_BROADCAST_HDR.from_address(lparam)
        if hdr.dbch_devicetype != DBT_DEVTYP_DEVICEINTERFACE:
            return None
        offset = DEV_BROADCAST_DEVICEINTERFACE_W.dbcc_name.offset
        return ctypes.wstring_at(lparam + offset)

    def wndproc(hwnd, msg, wparam, lparam):
        try:
            if msg == WM_DEVICECHANGE and lparam and wparam in (DBT_DEVICEARRIVAL, DBT_DEVICEREMOVECOMPLETE):
                instance_id = instance_id_from_interface(interface_name(lparam))
                handler = watcher.device_arrived if wparam == DBT_DEVICEARRIVAL else watcher.device_removed
                # Off the window thread: reading the device's properties can take a moment
                threading.Thread(target=handler, args=(instance_id,), daemon=True).start()
                return 1
            if msg == WM_POWERBROADCAST:
                if wparam == PBT_APMPOWERSTATUSCHANGE:
                    threading.Thread(target=watcher.power_changed, daemon=True).start()
                elif wparam in (PBT_APMRESUMESUSPEND, PBT_APMRESUMEAUTOMATIC):
                    watcher.resumed()
                return 1
            if msg == 0x0010:    # WM_CLOSE
                user32.DestroyWindow(hwnd)
                return 0
            if msg == 0x0002:    # WM_DESTROY
                user32.PostQuitMessage(0)
                return 0
        except Exception as e:
            log.error(f"Error handling a plug message: {e}")
        return user32.DefWindowProcW(hwnd, msg, wparam, lparam)

    callback = WNDPROC(wndproc)
    hinstance = kernel32.GetModuleHandleW(None)
    wc = WNDCLASSW()
    wc.lpfnWndProc = callback
    wc.hInstance = hinstance
    wc.lpszClassName = f"WhiteRabbitPlugEvents{id(watcher)}"
    if not user32.RegisterClassW(ctypes.byref(wc)):
        raise ctypes.WinError(ctypes.get_last_error())
    # A hidden top-level window: power broadcasts don't reach message-only windows
    hwnd = user32.CreateWindowExW(0, wc.lpszClassName, "White Rabbit plug events", 0,
                                  0, 0, 0, 0, None, None, hinstance, None)
    if not hwnd:
        raise ctypes.WinError(ctypes.get_last_error())
    watcher._hwnd = hwnd

    flt = DEV_BROADCAST_DEVICEINTERFACE_W()
    flt.dbcc_size = ctypes.sizeof(DEV_BROADCAST_DEVICEINTERFACE_W)
    flt.dbcc_devicetype = DBT_DEVTYP_DEVICEINTERFACE
    flt.dbcc_classguid[:] = uuid.UUID(GUID_DEVINTERFACE_USB_DEVICE).bytes_le
    DEVICE_NOTIFY_WINDOW_HANDLE = 0
    registration = user32.RegisterDeviceNotificationW(hwnd, ctypes.byref(flt), DEVICE_NOTIFY_WINDOW_HANDLE)
    if not registration:
        log.warning(f"Could not register for USB device messages: {ctypes.WinError(ctypes.get_last_error())}")
    log.info("Listening for things being plugged in")

    msg = wintypes.MSG()
    while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
        user32.TranslateMessage(ctypes.byref(msg))
        user32.DispatchMessageW(ctypes.byref(msg))

    if registration:
        user32.UnregisterDeviceNotification(registration)
    watcher._hwnd = None
    user32.UnregisterClassW(wc.lpszClassName, hinstance)
