# "Start with Windows": an entry under HKCU\Software\Microsoft\Windows\CurrentVersion\Run.
#
# The installer writes the same value name on a first install, so the tray menu and the installer agree.
# Task Manager > Startup apps can also disable the entry; it records that under
# ...\Explorer\StartupApproved\Run, and we treat a disabled entry as off.

#
# The Microsoft Store app can't use that key (Windows keeps a packaged app's registry writes to itself).
# Its package declares a startup task instead (TaskId below, in installer/msix/AppxManifest.xml), which
# Windows lists in Task Manager > Startup apps and Settings > Apps > Startup; we switch it with WinRT.

# Built In Dependencies
import os
import sys
import asyncio
import logging
import threading

# Internal Dependencies
from led_mon import platform_utils

log = logging.getLogger(__name__)

RUN_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"
APPROVED_KEY = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
VALUE_NAME = "WhiteRabbit"


class WinRegistry:
    """Reads and writes string/binary values under HKEY_CURRENT_USER."""

    def __init__(self):
        import winreg
        self.winreg = winreg

    def get(self, key, name):
        try:
            with self.winreg.OpenKey(self.winreg.HKEY_CURRENT_USER, key) as k:
                return self.winreg.QueryValueEx(k, name)[0]
        except FileNotFoundError:
            return None

    def set(self, key, name, value):
        with self.winreg.CreateKey(self.winreg.HKEY_CURRENT_USER, key) as k:
            self.winreg.SetValueEx(k, name, 0, self.winreg.REG_SZ, value)

    def delete(self, key, name):
        try:
            with self.winreg.OpenKey(self.winreg.HKEY_CURRENT_USER, key, 0, self.winreg.KEY_SET_VALUE) as k:
                self.winreg.DeleteValue(k, name)
        except FileNotFoundError:
            pass


STARTUP_TASK_ID = "WhiteRabbitStartup"
WINRT_TIMEOUT = 15
# Windows.ApplicationModel.StartupTaskState
TASK_DISABLED, TASK_DISABLED_BY_USER, TASK_ENABLED, TASK_DISABLED_BY_POLICY, TASK_ENABLED_BY_POLICY = range(5)
# Windows.ApplicationModel.Activation.ActivationKind.StartupTask
ACTIVATION_STARTUP_TASK = 1020


class StartupBlocked(Exception):
    """The user or a policy turned White Rabbit off in Windows' own startup list; only they can turn it back on."""


def _run_winrt(make_coro):
    """Run a WinRT call on a thread of its own, with its own COM apartment and event loop."""
    result = {}

    def work():
        try:
            try:
                import winrt.runtime
                winrt.runtime.init_apartment(winrt.runtime.ApartmentType.MULTI_THREADED)
            except Exception:
                pass  # already set up on this thread
            result["value"] = asyncio.run(asyncio.wait_for(make_coro(), WINRT_TIMEOUT))
        except BaseException as e:
            result["error"] = e

    thread = threading.Thread(target=work, name="startup-task", daemon=True)
    thread.start()
    thread.join(WINRT_TIMEOUT + 5)
    if thread.is_alive():
        raise TimeoutError("Windows didn't answer about the startup task")
    if "error" in result:
        raise result["error"]
    return result["value"]


class StoreStartupTask:
    """The Store package's startup task (Windows.ApplicationModel.StartupTask)."""

    def _task_class(self):
        from winrt.windows.applicationmodel import StartupTask
        return StartupTask

    def state(self):
        StartupTask = self._task_class()

        async def get_state():
            task = await StartupTask.get_async(STARTUP_TASK_ID)
            return int(task.state)
        return _run_winrt(get_state)

    def enable(self):
        """Returns the state after asking; Windows refuses if the user turned it off in Task Manager."""
        StartupTask = self._task_class()

        async def request():
            task = await StartupTask.get_async(STARTUP_TASK_ID)
            return int(await task.request_enable_async())
        return _run_winrt(request)

    def disable(self):
        StartupTask = self._task_class()

        async def turn_off():
            task = await StartupTask.get_async(STARTUP_TASK_ID)
            task.disable()
            return int(task.state)
        return _run_winrt(turn_off)


def _store_task(task):
    if task is not None:
        return task
    if platform_utils.is_store_package():
        return StoreStartupTask()
    return None


def started_by_startup_task():
    """True when Windows started the Store app at login (its startup task), so it should stay in the tray.

    The startup task can't pass --background, so the Store app asks Windows how it was started.
    """
    if not platform_utils.is_store_package():
        return False
    try:
        from winrt.windows.applicationmodel import AppInstance
        import winrt.windows.applicationmodel.activation  # noqa: F401  (the type Windows answers with)

        async def kind():
            args = AppInstance.get_activated_event_args()
            return int(args.kind) if args is not None else None
        return _run_winrt(kind) == ACTIVATION_STARTUP_TASK
    except Exception as e:
        log.warning(f"Could not tell how Windows started White Rabbit: {e}")
        return False


def supported():
    return platform_utils.IS_WINDOWS


def _registry(registry):
    return registry if registry is not None else WinRegistry()


def launch_command():
    """The command Windows runs at login: this exe, or pythonw with the repo's main.py when run from source.

    --background starts the monitor and tray icon without opening the window.
    """
    if platform_utils.is_frozen():
        return f'"{sys.executable}" --background'
    python = sys.executable
    pythonw = os.path.join(os.path.dirname(python), "pythonw.exe")
    if os.path.isfile(pythonw):
        python = pythonw
    script = os.path.join(os.path.dirname(platform_utils.package_dir()), "main.py")
    return f'"{python}" "{script}" --background'


def _disabled_in_task_manager(registry):
    # The first byte is even (02, 06) when enabled and odd (03) when disabled
    value = registry.get(APPROVED_KEY, VALUE_NAME)
    return isinstance(value, (bytes, bytearray)) and len(value) > 0 and value[0] % 2 == 1


def is_enabled(registry=None, task=None):
    task = _store_task(task) if registry is None else task
    if task is not None:
        return task.state() in (TASK_ENABLED, TASK_ENABLED_BY_POLICY)
    registry = _registry(registry)
    if not registry.get(RUN_KEY, VALUE_NAME):
        return False
    return not _disabled_in_task_manager(registry)


def set_enabled(enabled, registry=None, command=None, task=None):
    """Adds this app to, or removes it from, the programs Windows starts at login."""
    task = _store_task(task) if registry is None else task
    if task is not None:
        if not enabled:
            task.disable()
            return
        state = task.enable()
        if state == TASK_DISABLED_BY_USER:
            raise StartupBlocked("it's turned off in Task Manager > Startup apps. Turn it on there.")
        if state == TASK_DISABLED_BY_POLICY:
            raise StartupBlocked("a policy on this computer doesn't allow it.")
        return
    registry = _registry(registry)
    if enabled:
        registry.set(RUN_KEY, VALUE_NAME, command or launch_command())
    else:
        registry.delete(RUN_KEY, VALUE_NAME)
    # Clear any enabled/disabled state left by Task Manager so ours is the one that counts
    registry.delete(APPROVED_KEY, VALUE_NAME)
