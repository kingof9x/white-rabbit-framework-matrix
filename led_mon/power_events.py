# Turning the panels off when the lid closes, the screen turns off or the PC goes to sleep, and when the
# laptop's own screen isn't in use (docked with the lid shut, or Win+P "Second screen only").
#
# The panel firmware wakes up on every command it gets, and the monitor sends frames several times a
# second, so the panels would stay lit with the lid shut. Windows tells us about the lid, the screen and
# sleep through WM_POWERBROADCAST messages to a (hidden) window; this module listens for them on its own
# thread and calls on_change(state) whenever something that can turn the panels off changes.
#
# "Is the built-in screen in use?" is asked with QueryDisplayConfig: the laptop's own screen is an internal
# (eDP) display, and it drops out of the active displays when Windows only uses an external monitor. That is
# checked when Windows says the displays changed (WM_DISPLAYCHANGE), after the lid opens or closes, and every
# few seconds as a safety net.
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
WM_DISPLAYCHANGE = 0x007E
WM_TIMER = 0x0113

# Event name for "the built-in screen is unused" (value True) or "in use / can't tell" (False)
BUILTIN_SCREEN_UNUSED = "builtin_screen_unused"
# How often the built-in screen check runs anyway (milliseconds), in case a display change is missed
DISPLAY_CHECK_MS = 5000
# A second check this long after a display or lid change, as Windows can finish rearranging screens later
DISPLAY_RECHECK_MS = 1500

# DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY values (wingdi.h) for a screen built into the laptop
OUTPUT_LVDS = 6
OUTPUT_DISPLAYPORT_EMBEDDED = 11
OUTPUT_UDI_EMBEDDED = 13
OUTPUT_INTERNAL = 0x80000000
BUILT_IN_OUTPUTS = (OUTPUT_LVDS, OUTPUT_DISPLAYPORT_EMBEDDED, OUTPUT_UDI_EMBEDDED, OUTPUT_INTERNAL)

DISPLAY_OFF = 0
LID_CLOSED = 0


def builtin_screen_unused(active_outputs, all_outputs):
    """True when the laptop has a built-in screen and Windows isn't showing anything on it.

    active_outputs: the output technology of each screen Windows is using now; all_outputs: of every screen it
    knows about, used or not. False when the built-in screen is in use, when there is no built-in screen
    (a desktop PC), or when Windows uses no screen at all (lid closed, nothing plugged in: the lid handles that).
    """
    active = list(active_outputs or [])
    if not active:
        return False
    if any(o in BUILT_IN_OUTPUTS for o in active):
        return False
    return any(o in BUILT_IN_OUTPUTS for o in (all_outputs or []))


class PowerState:
    """What Windows last told us.

    should_sleep: the lid is closed, the screen is off or the PC is going to sleep ("Turn off when lid closes").
    builtin_unused: the laptop's own screen isn't in use ("Turn off when laptop screen isn't used").
    """

    def __init__(self):
        self.lid_closed = False
        self.display_off = False
        self.suspending = False
        self.builtin_unused = False

    @property
    def should_sleep(self):
        return self.lid_closed or self.display_off or self.suspending

    def wants_sleep(self, with_lid=True, when_screen_unused=True):
        """Should the panels be off, given the two settings?"""
        return bool((with_lid and self.should_sleep) or (when_screen_unused and self.builtin_unused))

    def _key(self):
        return self.should_sleep, self.builtin_unused

    def handle(self, event, value=None):
        """Apply one power message. Returns True when should_sleep or builtin_unused changed."""
        before = self._key()
        if event == PBT_APMSUSPEND:
            self.suspending = True
        elif event in (PBT_APMRESUMESUSPEND, PBT_APMRESUMEAUTOMATIC):
            self.suspending = False
        elif event == GUID_LIDSWITCH_STATE_CHANGE:
            self.lid_closed = value == LID_CLOSED
        elif event == GUID_CONSOLE_DISPLAY_STATE:
            # Dimmed (2) still counts as on: the screen is still showing something
            self.display_off = value == DISPLAY_OFF
        elif event == BUILTIN_SCREEN_UNUSED:
            self.builtin_unused = bool(value)
        return self._key() != before

    def describe(self):
        reasons = [name for name, on in (("lid closed", self.lid_closed), ("screen off", self.display_off),
                                         ("going to sleep", self.suspending),
                                         ("laptop screen not in use", self.builtin_unused)) if on]
        return ", ".join(reasons) or "lid open and screen on"


class PowerWatcher:
    """Calls on_change(state) from its own thread when something that can turn the panels off changes."""

    def __init__(self, on_change):
        self.on_change = on_change
        self.state = PowerState()
        self._thread = None
        self._hwnd = None

    def handle(self, event, value=None):
        if self.state.handle(event, value):
            log.info(f"Power: {self.state.describe()}")
            try:
                self.on_change(self.state)
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


def query_display_outputs():
    """Windows: (active, all) lists of screen output technologies from QueryDisplayConfig, or None on failure."""
    import ctypes
    from ctypes import wintypes

    class LUID(ctypes.Structure):
        _fields_ = [("LowPart", wintypes.DWORD), ("HighPart", wintypes.LONG)]

    class SOURCE_INFO(ctypes.Structure):
        _fields_ = [("adapterId", LUID), ("id", ctypes.c_uint32), ("modeInfoIdx", ctypes.c_uint32),
                    ("statusFlags", ctypes.c_uint32)]

    class TARGET_INFO(ctypes.Structure):
        _fields_ = [("adapterId", LUID), ("id", ctypes.c_uint32), ("modeInfoIdx", ctypes.c_uint32),
                    ("outputTechnology", ctypes.c_uint32), ("rotation", ctypes.c_uint32),
                    ("scaling", ctypes.c_uint32), ("refreshNumerator", ctypes.c_uint32),
                    ("refreshDenominator", ctypes.c_uint32), ("scanLineOrdering", ctypes.c_uint32),
                    ("targetAvailable", wintypes.BOOL), ("statusFlags", ctypes.c_uint32)]

    class PATH_INFO(ctypes.Structure):
        _fields_ = [("sourceInfo", SOURCE_INFO), ("targetInfo", TARGET_INFO), ("flags", ctypes.c_uint32)]

    MODE_INFO_SIZE = 64  # DISPLAYCONFIG_MODE_INFO; only needed as space for Windows to fill
    QDC_ALL_PATHS = 0x1
    QDC_ONLY_ACTIVE_PATHS = 0x2
    ERROR_SUCCESS = 0
    ERROR_INSUFFICIENT_BUFFER = 122

    user32 = ctypes.WinDLL("user32")
    user32.GetDisplayConfigBufferSizes.restype = wintypes.LONG
    user32.GetDisplayConfigBufferSizes.argtypes = [ctypes.c_uint32, ctypes.POINTER(ctypes.c_uint32),
                                                   ctypes.POINTER(ctypes.c_uint32)]
    user32.QueryDisplayConfig.restype = wintypes.LONG
    user32.QueryDisplayConfig.argtypes = [ctypes.c_uint32, ctypes.POINTER(ctypes.c_uint32), ctypes.c_void_p,
                                          ctypes.POINTER(ctypes.c_uint32), ctypes.c_void_p, ctypes.c_void_p]

    def outputs(flags):
        for _ in range(3):  # the screens can change between the two calls
            paths, modes = ctypes.c_uint32(0), ctypes.c_uint32(0)
            if user32.GetDisplayConfigBufferSizes(flags, ctypes.byref(paths), ctypes.byref(modes)) != ERROR_SUCCESS:
                return None
            path_array = (PATH_INFO * max(paths.value, 1))()
            mode_array = (ctypes.c_ubyte * (MODE_INFO_SIZE * max(modes.value, 1)))()
            result = user32.QueryDisplayConfig(flags, ctypes.byref(paths), path_array, ctypes.byref(modes),
                                               mode_array, None)
            if result == ERROR_SUCCESS:
                return [path_array[i].targetInfo.outputTechnology for i in range(paths.value)]
            if result != ERROR_INSUFFICIENT_BUFFER:
                return None
        return None

    active = outputs(QDC_ONLY_ACTIVE_PATHS)
    every = outputs(QDC_ALL_PATHS)
    if active is None or every is None:
        return None
    return active, every


def check_builtin_screen(watcher):
    """Ask Windows whether the laptop's own screen is in use and tell the watcher."""
    try:
        found = query_display_outputs()
    except Exception as e:
        log.debug(f"Could not ask Windows which screens are in use: {e}")
        found = None
    watcher.handle(BUILTIN_SCREEN_UNUSED, builtin_screen_unused(*found) if found else False)


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
    user32.UnregisterClassW.argtypes = [wintypes.LPCWSTR, wintypes.HINSTANCE]
    user32.SetTimer.restype = ctypes.c_size_t
    user32.SetTimer.argtypes = [wintypes.HWND, ctypes.c_size_t, wintypes.UINT, ctypes.c_void_p]
    user32.KillTimer.argtypes = [wintypes.HWND, ctypes.c_size_t]
    kernel32.GetModuleHandleW.restype = wintypes.HMODULE
    kernel32.GetModuleHandleW.argtypes = [wintypes.LPCWSTR]

    guids = {GUID_CONSOLE_DISPLAY_STATE: GUID.from_string(GUID_CONSOLE_DISPLAY_STATE),
             GUID_LIDSWITCH_STATE_CHANGE: GUID.from_string(GUID_LIDSWITCH_STATE_CHANGE)}
    by_bytes = {bytes(g): name for name, g in guids.items()}

    WM_CLOSE = 0x0010
    WM_DESTROY = 0x0002
    TIMER_PERIODIC = 1  # the regular built-in screen check
    TIMER_RECHECK = 2   # one extra check shortly after a display or lid change

    def recheck_soon(hwnd):
        user32.SetTimer(hwnd, TIMER_RECHECK, DISPLAY_RECHECK_MS, None)

    def wndproc(hwnd, msg, wparam, lparam):
        try:
            if msg == WM_POWERBROADCAST:
                if wparam == PBT_POWERSETTINGCHANGE and lparam:
                    setting = POWERBROADCAST_SETTING.from_address(lparam)
                    name = by_bytes.get(bytes(setting.PowerSetting))
                    if name is not None and setting.DataLength >= 1:
                        watcher.handle(name, setting.Data[0])
                        if name == GUID_LIDSWITCH_STATE_CHANGE:
                            check_builtin_screen(watcher)
                            recheck_soon(hwnd)
                elif wparam in (PBT_APMSUSPEND, PBT_APMRESUMESUSPEND, PBT_APMRESUMEAUTOMATIC):
                    watcher.handle(wparam)
                return 1
            if msg == WM_DISPLAYCHANGE:
                check_builtin_screen(watcher)
                recheck_soon(hwnd)
                return 0
            if msg == WM_TIMER:
                if wparam == TIMER_RECHECK:
                    user32.KillTimer(hwnd, TIMER_RECHECK)
                check_builtin_screen(watcher)
                return 0
            if msg == WM_CLOSE:
                user32.KillTimer(hwnd, TIMER_PERIODIC)
                user32.KillTimer(hwnd, TIMER_RECHECK)
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
    # One class per watcher: a class keeps the window procedure it was registered with
    wc.lpszClassName = f"WhiteRabbitPowerEvents{id(watcher)}"
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
    check_builtin_screen(watcher)
    user32.SetTimer(hwnd, TIMER_PERIODIC, DISPLAY_CHECK_MS, None)

    msg = wintypes.MSG()
    while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
        user32.TranslateMessage(ctypes.byref(msg))
        user32.DispatchMessageW(ctypes.byref(msg))

    for handle in registrations:
        user32.UnregisterPowerSettingNotification(handle)
    watcher._hwnd = None
    user32.UnregisterClassW(wc.lpszClassName, hinstance)
