# The White Rabbit window: the layout editor and the LED Matrix controls in one place.
#
# Tabs: Panels (layout editor), Startup animation, Games & patterns, Firmware.
# The bar at the bottom picks which panels the controls act on, and shows whether each panel is
# showing the system monitor or something from this window. Playing a game or showing a pattern
# borrows the panel from the monitor; "Back to system monitor" returns it.

# Built In Dependencies
import os
import queue
import logging
import threading
import tkinter as tk
from tkinter import ttk

# Internal Dependencies
from led_mon import platform_utils
from white_rabbit import bridge, panels as panels_mod, tabs

log = logging.getLogger(__name__)

TITLE = "White Rabbit"
WINDOW_MUTEX = "WhiteRabbit_window"
WAIT_FOR_MONITOR_SECONDS = 10


class Lender:
    """Keeps track of the panels borrowed from the monitor, and what to put back when returning them."""

    def __init__(self, link):
        self.link = link
        self.lock = threading.Lock()
        # {port: firmware brightness before borrowing, or None}
        self.borrowed = {}
        # Bumped on each borrow, so a delayed automatic return doesn't undo something newer
        self.generation = {}

    def is_borrowed(self, panel):
        with self.lock:
            return panel.device in self.borrowed

    def borrow(self, panels):
        """Stop the monitor drawing on these panels (runs on a worker thread)."""
        from inputmodule.inputmodule import get_brightness
        new = []
        with self.lock:
            for panel in panels:
                self.generation[panel.device] = self.generation.get(panel.device, 0) + 1
                if panel.device not in self.borrowed:
                    new.append(panel)
        for panel in new:
            try:
                level = get_brightness(panel)
            except Exception:
                level = None
            with self.lock:
                self.borrowed[panel.device] = level
        if new:
            self.link.hold([p.device for p in new])
        with self.lock:
            return {p.device: self.generation[p.device] for p in panels}

    def give_back(self, panels, only_generation=None):
        """Stop whatever this window started on these panels and let the monitor draw again (worker thread)."""
        from inputmodule.gui.gui_threading import stop_thread
        from inputmodule.inputmodule import send_command, brightness, CommandVals, GameControlVal
        from inputmodule.inputmodule.ledmatrix import animate
        returning = []
        with self.lock:
            for panel in panels:
                if panel.device not in self.borrowed:
                    continue
                if only_generation and self.generation.get(panel.device) != only_generation.get(panel.device):
                    continue
                returning.append((panel, self.borrowed.pop(panel.device)))
        if not returning:
            return []
        stop_thread()
        for panel, level in returning:
            # A game in the firmware keeps drawing over the monitor, and the clock/countdown change brightness
            for step in (lambda: send_command(panel, CommandVals.GameControl, [GameControlVal.Quit]),
                         lambda: animate(panel, False),
                         lambda: level is not None and brightness(panel, level)):
                try:
                    step()
                except Exception as e:
                    log.debug(f"Resetting {panel.device}: {e}")
        try:
            self.link.release([p.device for p, _ in returning])
        except Exception as e:
            log.warning(f"Could not give the panels back to the monitor: {e}")
        return [p for p, _ in returning]


class WhiteRabbitWindow:
    def __init__(self, root, args, start_tab=None):
        self.root = root
        self.args = args
        self.link = bridge.Link()
        bridge.install(self.link)
        self.lender = Lender(self.link)
        self.ui_calls = queue.Queue()
        self.monitor_running = False

        root.title(TITLE)
        root.geometry("1340x760")
        root.minsize(1000, 700)
        root.protocol("WM_DELETE_WINDOW", self.on_close)

        self.notebook = ttk.Notebook(root)
        self.notebook.pack(fill="both", expand=True)
        self.bar = ttk.Frame(root, padding=(12, 6, 12, 10))
        self.bar.pack(fill="x")

        self.panels = panels_mod.find_panels(self.read_config())
        self.panel_vars = {}
        self.build_bar()

        self.editor = None
        self.panels_tab = ttk.Frame(self.notebook)
        self.notebook.add(self.panels_tab, text="Panels")
        try:
            from led_mon.editor.window import LayoutEditor
            self.editor = LayoutEditor(root, args, container=self.panels_tab, on_dirty=self.on_layout_dirty)
        except Exception as e:
            log.exception("The layout editor failed to start")
            ttk.Label(self.panels_tab, text=f"The layout editor couldn't start: {e}", padding=20).pack(anchor="w")

        self.startup_tab = tabs.StartupTab(self.notebook, self)
        self.notebook.add(self.startup_tab, text="Startup animation")
        self.play_tab = tabs.PlayTab(self.notebook, self)
        self.notebook.add(self.play_tab, text="Games & patterns")
        self.firmware_tab = tabs.FirmwareTab(self.notebook, self)
        self.notebook.add(self.firmware_tab, text="Firmware")
        tab = {"startup": self.startup_tab, "play": self.play_tab, "firmware": self.firmware_tab}.get(start_tab)
        if tab is not None:
            self.notebook.select(tab)

        self.root.after(100, self.poll_ui_calls)
        self.root.after(200, self.check_monitor)
        self.refresh_modules()

    def read_config(self):
        try:
            from yaml import safe_load
            with open(platform_utils.resolve_config_file(getattr(self.args, "config_file", None))) as f:
                return safe_load(f)
        except Exception as e:
            log.warning(f"Could not read the layout for the panel order: {e}")
            return None

    # Bottom bar
    def build_bar(self):
        ttk.Label(self.bar, text="Controls act on:").pack(side="left")
        if not self.panels:
            ttk.Label(self.bar, text="no LED Matrix panels found", foreground="#a33").pack(side="left", padx=6)
        for panel in self.panels:
            var = tk.BooleanVar(value=True)
            ttk.Checkbutton(self.bar, text=panel.label, variable=var).pack(side="left", padx=6)
            self.panel_vars[panel.device] = var
        self.back_button = ttk.Button(self.bar, text="Back to system monitor", command=self.give_back_all)
        self.back_button.pack(side="right")
        self.bar_status = ttk.Label(self.bar, text="", foreground="#555555")
        self.bar_status.pack(side="right", padx=10)
        self.update_bar()

    def update_bar(self):
        borrowed = [p for p in self.panels if self.lender.is_borrowed(p)]
        if not self.monitor_running:
            text = "The system monitor isn't running"
        elif borrowed:
            text = "Showing White Rabbit controls on: " + ", ".join(f"{p.side} panel" for p in borrowed)
        else:
            text = "Panels are showing the system monitor"
        self.bar_status.configure(text=text)
        self.back_button.state(["!disabled"] if borrowed else ["disabled"])

    def selected_panels(self):
        return [p for p in self.panels if self.panel_vars[p.device].get()]

    # Threads: tkinter may only be used from the thread that runs the window
    def call_soon(self, fn):
        self.ui_calls.put(fn)

    def poll_ui_calls(self):
        while True:
            try:
                fn = self.ui_calls.get_nowait()
            except queue.Empty:
                break
            try:
                fn()
            except Exception as e:
                log.error(f"Window update failed: {e}")
        self.root.after(100, self.poll_ui_calls)

    def in_background(self, work, done=None, error=None):
        """Run work() on a worker thread, then done(result) or error(exception) on the window's thread."""
        def run():
            try:
                result = work()
            except Exception as e:
                log.exception("Background action failed")
                if error is not None:
                    self.call_soon(lambda: error(e))
                return
            if done is not None:
                self.call_soon(lambda: done(result))
        threading.Thread(target=run, daemon=True).start()

    def check_monitor(self):
        def done(running):
            self.monitor_running = running
            self.update_bar()
        self.in_background(lambda: self.link.running(fresh=True), done)
        self.root.after(5000, self.check_monitor)

    # Acting on panels
    def act(self, action, status=None, panels=None, borrow=True, return_after=None, one_panel=False):
        """Run action(panel) for the chosen panels on a worker thread.

        borrow: take the panels from the monitor first. return_after: give them back after that many
        seconds. status(text) shows the result. Returns False if no panel is chosen.
        """
        status = status or (lambda text: None)
        panels = self.selected_panels() if panels is None else panels
        if not panels:
            status("Tick a panel at the bottom of the window first.")
            return False
        if one_panel and len(panels) != 1:
            status("Tick exactly one panel at the bottom of the window for this.")
            return False

        def work():
            generations = self.lender.borrow(panels) if borrow else None
            self.call_soon(self.update_bar)
            problems = []
            for panel in panels:
                try:
                    action(panel)
                except Exception as e:
                    problems.append(f"{panel.side} panel: {e}")
            return generations, problems

        def done(result):
            generations, problems = result
            status("Didn't work on the " + "; ".join(problems) if problems else "Done.")
            if return_after and generations:
                self.root.after(int(return_after * 1000), lambda: self.give_back(panels, generations))
            self.update_bar()

        self.in_background(work, done, lambda e: status(f"That didn't work: {e}"))
        return True

    def give_back(self, panels, only_generation=None):
        self.in_background(lambda: self.lender.give_back(panels, only_generation), lambda _: self.update_bar())

    def give_back_all(self):
        self.give_back(self.panels)

    def refresh_modules(self):
        """Read each panel's firmware version and startup setting, then update the tabs."""
        def work():
            from inputmodule.inputmodule import get_version
            from inputmodule.inputmodule.ledmatrix import get_startup_animation
            info = {}
            for panel in self.panels:
                try:
                    startup = get_startup_animation(panel)
                    info[panel.device] = {"version": get_version(panel), "startup": startup, "in_use": False}
                except (IOError, OSError) as e:
                    log.info(f"Could not read {panel.device}: {e}")
                    info[panel.device] = {"version": None, "startup": None, "in_use": True}
            return info

        def done(info):
            self.module_info = info
            self.startup_tab.show_modules(info)
            self.firmware_tab.show_modules(info)
        self.module_info = {}
        self.in_background(work, done)

    def on_layout_dirty(self, dirty):
        self.notebook.tab(self.panels_tab, text="Panels *" if dirty else "Panels")

    def on_close(self):
        if self.editor is not None and not self.editor.close():
            return
        borrowed = [p for p in self.panels if self.lender.is_borrowed(p)]
        if borrowed:
            # Give the panels back before closing; the monitor also notices a closed window by itself
            try:
                self.lender.give_back(borrowed)
            except Exception as e:
                log.warning(f"Could not give the panels back: {e}")
        self.root.destroy()


def _focus_open_window():
    """If the window is already open (Windows), bring it to the front. Returns True if it was."""
    if not platform_utils.IS_WINDOWS:
        return False
    import ctypes
    kernel32 = ctypes.windll.kernel32
    user32 = ctypes.windll.user32
    kernel32.CreateMutexW.restype = ctypes.c_void_p
    handle = kernel32.CreateMutexW(None, False, WINDOW_MUTEX)
    if kernel32.GetLastError() != 183:  # ERROR_ALREADY_EXISTS
        # Keep the mutex for as long as this window runs
        global _window_mutex
        _window_mutex = handle
        return False
    hwnd = user32.FindWindowW(None, TITLE)
    if hwnd:
        SW_RESTORE = 9
        user32.ShowWindow(hwnd, SW_RESTORE)
        user32.SetForegroundWindow(hwnd)
    return True


_window_mutex = None


def wait_for_monitor(link, seconds=WAIT_FOR_MONITOR_SECONDS):
    """Started together with the monitor: let it open the panels first, so the two don't collide."""
    import time
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if link.running(fresh=True):
            return True
        time.sleep(0.5)
    return False


def run_window(args, start_tab=None, wait=False):
    platform_utils.attach_parent_console()
    if _focus_open_window():
        return
    if wait:
        wait_for_monitor(bridge.Link())
    platform_utils.enable_dpi_awareness()
    root = tk.Tk()
    try:
        from led_mon.tray import make_icon_image
        from PIL import ImageTk
        icon = ImageTk.PhotoImage(make_icon_image(64))
        root.iconphoto(True, icon)
    except Exception:
        pass
    window = WhiteRabbitWindow(root, args, start_tab)
    if os.environ.get("WHITE_RABBIT_SMOKE_TEST"):
        # CI: check that the window opens with all its tabs, then close
        def finish():
            if window.editor is not None:
                window.editor.stop.set()
            root.destroy()
        root.after(8000, finish)
    root.mainloop()
