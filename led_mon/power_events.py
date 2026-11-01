# Turning the panels off when the lid closes, the screen turns off or the PC goes to sleep.
#
# The panel firmware wakes up on every command it gets, and the monitor sends frames several times a
# second, so the panels would stay lit with the lid shut. Windows tells us about the lid, the screen and
# sleep through WM_POWERBROADCAST messages to a (hidden) window; this module listens for them on its own
# thread and calls on_change(should_sleep) whenever the answer to "should the panels be off?" changes.
#
# Other systems: nothing to do here. The firmware's SLEEP# pin and USB suspend handle real sleep there.

# Built In Dependencies
import logging
import threading

# Internal Dependencies
from led_mon.platform_utils import IS_WINDOWS

log = logging.getLogger(__name__)

# Power setting GUIDs (winnt.h)
GUID_CONSOLE_DISPLAY_STATE = "6FE69556-704A-47A0-8F24-C28D936FDA47"  # 0 off, 1 on, 2 dimmed
GUID_LIDSWITCH_STATE_CHANGE = "BA3E0F4D-B817-4094-A2D1-D56379E6A0F3"  # 0 closed, 1 open

WM_POWERBROADCAST = 0x0218
PBT_APMSUSPEND = 0x0004
PBT_APMRESUMESUSPEND = 0x0007
PBT_APMRESUMEAUTOMATIC = 0x0012
PBT_POWERSETTINGCHANGE = 0x8013

DISPLAY_OFF = 0
LID_CLOSED = 0


class PowerState:
    """What Windows last told us. The panels should be off while any of these is true."""

    def __init__(self):
        self.lid_closed = False
        self.display_off = False
        self.suspending = False

    @property
    def should_sleep(self):
        return self.lid_closed or self.display_off or self.suspending

    def handle(self, event, value=None):
        """Apply one power message. Returns True when should_sleep changed."""
        before = self.should_sleep
        if event == PBT_APMSUSPEND:
            self.suspending = True
        elif event in (PBT_APMRESUMESUSPEND, PBT_APMRESUMEAUTOMATIC):
            self.suspending = False
        elif event == GUID_LIDSWITCH_STATE_CHANGE:
            self.lid_closed = value == LID_CLOSED
        elif event == GUID_CONSOLE_DISPLAY_STATE:
            # Dimmed (2) still counts as on: the screen is still showing something
            self.display_off = value == DISPLAY_OFF
        return self.should_sleep != before

    def describe(self):
        reasons = [name for name, on in (("lid closed", self.lid_closed), ("screen off", self.display_off),
                                         ("going to sleep", self.suspending)) if on]
        return ", ".join(reasons) or "lid open and screen on"


class PowerWatcher:
    """Calls on_change(should_sleep) from its own thread when the panels should turn off or back on."""

    def __init__(self, on_change):
        self.on_change = on_change
        self.state = PowerState()
        self._thread = None
        self._hwnd = None

    def handle(self, event, value=None):
        if self.state.handle(event, value):
            log.info(f"Power: {self.state.describe()}, panels " + ("off" if self.state.should_sleep else "on"))
            try:
                self.on_change(self.state.should_sleep)
            except Exception as e:
                log.error(f"Error turning the panels off or on: {e}")

    def start(self):
        if not IS_WINDOWS:
            return False
        self._thread = threading.Thread(target=self._run, name="power-events", daemon=True)
        self._thread.start()
        return True

    def stop(self):
        if self._hwnd:
            try:
                import ctypes
                WM_CLOSE = 0x0010
                ctypes.windll.user32.PostMessageW(self._hwnd, WM_CLOSE, 0, 0)
            except Exception:
                pass

    def _run(self):
        try:
            _windows_message_loop(self)
        except Exception as e:
            log.error(f"Could not listen for the lid closing (panels will stay on): {e}")


def _windows_message_loop(watcher):
    import ctypes
    import uuid
    from ctypes import wintypes

    user32 = ctypes.WinDLL("user32", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

    LRESULT = ctypes.c_ssize_t
    WNDPROC = ctypes.WINFUNCTYPE(LRESULT, wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM)

    class GUID(ctypes.Structure):
        _fields_ = [("Data1", wintypes.DWORD), ("Data2", wintypes.WORD), ("Data3", wintypes.WORD),
                    ("Data4", ctypes.c_ubyte * 8)]

        @classmethod
        def from_string(cls, text):
            return cls.from_buffer_copy(uuid.UUID(text).bytes_le)

    class POWERBROADCAST_SETTING(ctypes.Structure):
        _fields_ = [("PowerSetting", GUID), ("DataLength", wintypes.DWORD), ("Data", ctypes.c_ubyte * 4)]

    class WNDCLASSW(ctypes.Structure):
        _fields_ = [("style", wintypes.UINT), ("lpfnWndProc", WNDPROC), ("cbClsExtra", ctypes.c_int),
                    ("cbWndExtra", ctypes.c_int), ("hInstance", wintypes.HINSTANCE), ("hIcon", wintypes.HICON),
                    ("hCursor", wintypes.HANDLE), ("hbrBackground", wintypes.HBRUSH),
                    ("lpszMenuName", wintypes.LPCWSTR), ("lpszClassName", wintypes.LPCWSTR)]

    user32.DefWindowProcW.restype = LRESULT
    user32.DefWindowProcW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    user32.RegisterClassW.argtypes = [ctypes.POINTER(WNDCLASSW)]
    user32.RegisterClassW.restype = wintypes.ATOM
    user32.CreateWindowExW.restype = wintypes.HWND
    user32.CreateWindowExW.argtypes = [wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD,
                                       ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                                       wintypes.HWND, wintypes.HMENU, wintypes.HINSTANCE, wintypes.LPVOID]
    user32.RegisterPowerSettingNotification.restype = wintypes.HANDLE
    user32.RegisterPowerSettingNotification.argtypes = [wintypes.HANDLE, ctypes.POINTER(GUID), wintypes.DWORD]
    user32.UnregisterPowerSettingNotification.argtypes = [wintypes.HANDLE]
    user32.GetMessageW.argtypes = [ctypes.POINTER(wintypes.MSG), wintypes.HWND, wintypes.UINT, wintypes.UINT]
    user32.DestroyWindow.argtypes = [wintypes.HWND]
    kernel32.GetModuleHandleW.restype = wintypes.HMODULE
    kernel32.GetModuleHandleW.argtypes = [wintypes.LPCWSTR]

    guids = {GUID_CONSOLE_DISPLAY_STATE: GUID.from_string(GUID_CONSOLE_DISPLAY_STATE),
             GUID_LIDSWITCH_STATE_CHANGE: GUID.from_string(GUID_LIDSWITCH_STATE_CHANGE)}
    by_bytes = {bytes(g): name for name, g in guids.items()}

    WM_CLOSE = 0x0010
    WM_DESTROY = 0x0002

    def wndproc(hwnd, msg, wparam, lparam):
        try:
            if msg == WM_POWERBROADCAST:
                if wparam == PBT_POWERSETTINGCHANGE and lparam:
                    setting = POWERBROADCAST_SETTING.from_address(lparam)
                    name = by_bytes.get(bytes(setting.PowerSetting))
                    if name is not None and setting.DataLength >= 1:
                        watcher.handle(name, setting.Data[0])
                elif wparam in (PBT_APMSUSPEND, PBT_APMRESUMESUSPEND, PBT_APMRESUMEAUTOMATIC):
                    watcher.handle(wparam)
                return 1
            if msg == WM_CLOSE:
                user32.DestroyWindow(hwnd)
                return 0
            if msg == WM_DESTROY:
                user32.PostQuitMessage(0)
                return 0
        except Exception as e:
            log.error(f"Error handling a power message: {e}")
        return user32.DefWindowProcW(hwnd, msg, wparam, lparam)

    callback = WNDPROC(wndproc)  # keep a reference for the window's lifetime
    hinstance = kernel32.GetModuleHandleW(None)
    wc = WNDCLASSW()
    wc.lpfnWndProc = callback
    wc.hInstance = hinstance
    wc.lpszClassName = "WhiteRabbitPowerEvents"
    if not user32.RegisterClassW(ctypes.byref(wc)):
        raise ctypes.WinError(ctypes.get_last_error())
    # A hidden top-level window (not message-only), so the sleep / resume broadcasts reach it too
    hwnd = user32.CreateWindowExW(0, wc.lpszClassName, "White Rabbit power events", 0,
                                  0, 0, 0, 0, None, None, hinstance, None)
    if not hwnd:
        raise ctypes.WinError(ctypes.get_last_error())
    watcher._hwnd = hwnd

    DEVICE_NOTIFY_WINDOW_HANDLE = 0
    registrations = []
    for name, guid in guids.items():
        # Windows sends the current state straight away, then every change
        handle = user32.RegisterPowerSettingNotification(hwnd, ctypes.byref(guid), DEVICE_NOTIFY_WINDOW_HANDLE)
        if handle:
            registrations.append(handle)
        else:
            log.warning(f"Could not register for power setting {name}: {ctypes.WinError(ctypes.get_last_error())}")
    log.info("Listening for the lid closing and the screen turning off")

    msg = wintypes.MSG()
    while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
        user32.TranslateMessage(ctypes.byref(msg))
        user32.DispatchMessageW(ctypes.byref(msg))

    for handle in registrations:
        user32.UnregisterPowerSettingNotification(handle)
