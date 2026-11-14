# The White Rabbit window: the layout editor and the LED Matrix controls in one place, laid out like the
# Windows 11 Settings app and following the Windows look (see led_mon/theme.py).
#
# Pages, picked in the menu on the left: Home, System monitor (the layout editor), Games & patterns, Music,
# Math, Startup animation and Settings (which opens Firmware, Keyboard shortcuts and About).
# The bottom of the menu picks which panels the controls act on, and shows whether the panels are
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
from white_rabbit import bridge, info_tab, math_tab, music, panels as panels_mod, tabs

log = logging.getLogger(__name__)

TITLE = "White Rabbit"


def window_title():
    """'White Rabbit 1.1': the title bar shows which version is running."""
    version = info_tab.app_version()
    return f"{TITLE} {version}" if version else TITLE


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


# The menu on the left: (page key, icon, label). Firmware, Keyboard shortcuts and About open from Settings.
NAV_PAGES = (
    ("home", "home", "Home"),
    ("monitor", "monitor", "System monitor"),
    ("games", "games", "Games & patterns"),
    ("music", "music", "Music"),
    ("math", "math", "Math"),
    ("startup", "startup", "Startup animation"),
    ("settings", "settings", "Settings"),
)
SUB_PAGES = {"firmware": ("settings", "Firmware"), "shortcuts": ("settings", "Keyboard shortcuts"),
             "about": ("settings", "About White Rabbit")}
# --tab names (older names too) -> pages
START_PAGES = {"panels": "monitor", "startup": "startup", "play": "games", "games": "games", "math": "math",
               "audio": "music", "music": "music", "firmware": "firmware", "shortcuts": "shortcuts",
               "info": "about", "about": "about", "settings": "settings", "home": "home"}


class WhiteRabbitWindow:
    def __init__(self, root, args, start_tab=None):
        from led_mon import theme as theme_mod
        from white_rabbit import pages
        self.root = root
        self.args = args
        self.link = bridge.Link()
        bridge.install(self.link)
        self.lender = Lender(self.link)
        self.ui_calls = queue.Queue()
        self.monitor_running = False
        self.module_info = {}
        self.theme = theme_mod.current() or theme_mod.install(root)
        px = self.theme.px

        root.title(window_title())
        width = min(px(1480), root.winfo_screenwidth() - px(40))
        height = min(px(900), root.winfo_screenheight() - px(80))
        root.geometry(f"{width}x{height}")
        root.minsize(min(px(1180), width), min(px(700), height))
        root.protocol("WM_DELETE_WINDOW", self.on_close)

        # The menu on the left, and the page on the right with its title above it
        self.nav = ttk.Frame(root, padding=(px(8), px(10), px(8), px(10)), width=px(290))
        self.nav.pack(side="left", fill="y")
        self.nav.pack_propagate(False)
        self.main = ttk.Frame(root, padding=(px(16), 0, px(24), px(12)))
        self.main.pack(side="left", fill="both", expand=True)
        self.header = ttk.Frame(self.main)
        self.header.pack(fill="x", pady=(px(14), px(10)))
        self.crumb = ttk.Label(self.header, style="Crumb.TLabel", cursor="hand2")
        self.crumb_arrow = ttk.Label(self.header, style="Crumb.TLabel", text=" \u203a ")
        self.page_title = ttk.Label(self.header, style="Title.TLabel")
        self.page_title.pack(side="left")
        self.crumb.bind("<Button-1>", lambda e: self.show_page(self.crumb_page))
        self.crumb.bind("<Enter>", lambda e: self.crumb.state(["active"]))
        self.crumb.bind("<Leave>", lambda e: self.crumb.state(["!active"]))
        self.crumb_page = None
        self.body = ttk.Frame(self.main)
        self.body.pack(fill="both", expand=True)

        self.panels = panels_mod.find_panels(self.read_config())
        self.panel_vars = {}
        self.build_nav()

        self.pages = {}
        self.page = None
        self.editor = None
        self.panels_tab = ttk.Frame(self.body)
        self.pages["monitor"] = self.panels_tab
        try:
            from led_mon.editor.window import LayoutEditor
            self.editor = LayoutEditor(root, args, container=self.panels_tab, on_dirty=self.on_layout_dirty)
        except Exception as e:
            log.exception("The layout editor failed to start")
            ttk.Label(self.panels_tab, text=f"The layout editor couldn't start: {e}", padding=20).pack(anchor="w")

        self.startup_tab = tabs.StartupTab(self.body, self)
        self.play_tab = tabs.PlayTab(self.body, self)
        self.math_tab = math_tab.MathTab(self.body, self)
        self.music_tab = music.MusicTab(self.body, self)
        self.firmware_tab = tabs.FirmwareTab(self.body, self)
        self.shortcuts_tab = tabs.ShortcutsTab(self.body, self)
        self.info_tab = info_tab.InfoTab(self.body, self)
        self.home_page = pages.HomePage(self.body, self)
        self.settings_page = pages.SettingsPage(self.body, self)
        self.pages.update(home=self.home_page, games=self.play_tab, music=self.music_tab, math=self.math_tab,
                          startup=self.startup_tab, settings=self.settings_page, firmware=self.firmware_tab,
                          shortcuts=self.shortcuts_tab, about=self.info_tab)
        self.theme.refresh_tree(root)
        self.theme.title_bar(root)
        self.show_page(START_PAGES.get(start_tab, "home"))

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

    # The menu on the left
    def build_nav(self):
        theme, px = self.theme, self.theme.px
        top = ttk.Frame(self.nav)
        top.pack(fill="x", padx=px(8), pady=(px(4), px(14)))
        try:
            from PIL import ImageTk
            from led_mon.tray import make_icon_image
            self.logo = ImageTk.PhotoImage(make_icon_image(px(20)), master=self.root)
            ttk.Label(top, image=self.logo).pack(side="left", padx=(0, px(10)))
        except Exception as e:
            log.debug(f"No logo: {e}")
        ttk.Label(top, text=window_title(), style="NavHint.TLabel").pack(side="left")

        self.nav_items = {}
        for key, icon, label in NAV_PAGES:
            item = ttk.Label(self.nav, text="  " + label, image=theme.icon(icon, 16), style="Nav.TLabel",
                             takefocus=1, cursor="hand2")
            item.pack(fill="x", pady=px(1))
            item.bind("<Button-1>", lambda e, key=key: self.show_page(key))
            item.bind("<Return>", lambda e, key=key: self.show_page(key))
            item.bind("<space>", lambda e, key=key: self.show_page(key))
            item.bind("<Enter>", lambda e, item=item: item.state(["active"]))
            item.bind("<Leave>", lambda e, item=item: item.state(["!active"]))
            self.nav_items[key] = item

        # Which panels the controls act on, at the bottom of the menu
        self.bar = ttk.Frame(self.nav, padding=(px(10), 0, px(6), 0))
        self.bar.pack(side="bottom", fill="x")
        ttk.Label(self.bar, text="Controls act on", style="Section.TLabel").pack(anchor="w", pady=(0, px(4)))
        if not self.panels:
            ttk.Label(self.bar, text="No LED Matrix panels found", foreground="#a33").pack(anchor="w")
        self.panel_checks = {}
        for panel in self.panels:
            var = tk.BooleanVar(value=True)
            check = ttk.Checkbutton(self.bar, text=panel.label, variable=var)
            check.pack(anchor="w", pady=px(1))
            self.panel_vars[panel.device] = var
            self.panel_checks[panel.device] = check
        self.bar_status = ttk.Label(self.bar, text="", style="NavHint.TLabel", wraplength=px(250), justify="left")
        self.bar_status.pack(anchor="w", fill="x", pady=(px(8), px(6)))
        self.back_button = ttk.Button(self.bar, text="Back to system monitor", command=self.give_back_all)
        self.back_button.pack(anchor="w", fill="x")
        self.update_bar()

    def show_page(self, key):
        if key not in self.pages:
            key = "home"
        if self.page == key:
            return
        if self.page is not None:
            self.pages[self.page].pack_forget()
        self.page = key
        self.pages[key].pack(fill="both", expand=True)
        parent, title = SUB_PAGES.get(key, (None, None))
        nav_key = parent or key
        for name, item in self.nav_items.items():
            item.state(["selected"] if name == nav_key else ["!selected"])
        title = title or dict((k, label) for k, _, label in NAV_PAGES)[key]
        if parent:
            self.crumb_page = parent
            self.crumb.configure(text=dict((k, label) for k, _, label in NAV_PAGES)[parent])
            self.crumb.pack(side="left", before=self.page_title)
            self.crumb_arrow.pack(side="left", before=self.page_title)
        else:
            self.crumb.pack_forget()
            self.crumb_arrow.pack_forget()
        self.page_title.configure(text=title)
        # The Music page listens to the sound only while it is showing
        try:
            self.music_tab.set_visible(key == "music")
        except Exception as e:
            log.error(f"Could not start the Music page's preview: {e}")
        show = getattr(self.pages[key], "page_shown", None)
        if callable(show):
            show()

    def update_bar(self):
        borrowed = [p for p in self.panels if self.lender.is_borrowed(p)]
        if not self.monitor_running:
            text = "The system monitor isn't running"
        elif borrowed:
            text = "Showing White Rabbit controls on: " + ", ".join(p.called for p in borrowed)
        elif getattr(getattr(self, "music_tab", None), "music_on", False):
            text = "Panels are showing music mode"
        else:
            text = "Panels are showing the system monitor"
        self.bar_status.configure(text=text)
        self.back_button.state(["!disabled"] if borrowed else ["disabled"])
        home = getattr(self, "home_page", None)
        if home is not None:
            home.update_panels()

    def name_panels(self):
        """A small window to give the left and right panels names; Identify panels shows them."""
        from led_mon import app_settings
        names = app_settings.load_panel_names()
        dialog = tk.Toplevel(self.root)
        dialog.title("Name panels")
        dialog.transient(self.root)
        dialog.resizable(False, False)
        body = ttk.Frame(dialog, padding=14)
        body.pack(fill="both", expand=True)
        entries = {}
        for row, side in enumerate(app_settings.PANEL_SIDES):
            ttk.Label(body, text=f"{side.capitalize()} panel:").grid(row=row, column=0, sticky="w", pady=3)
            entry = ttk.Entry(body, width=24)
            entry.insert(0, names.get(side, ""))
            entry.grid(row=row, column=1, sticky="w", padx=6, pady=3)
            entries[side] = entry
        ttk.Label(body, text=f"Identify panels (on the Home page or in the tray menu) shows the first "
                             f"{app_settings.PANEL_NAME_LETTERS} letters on the panel itself. "
                             "Leave a name empty to go back to plain left and right.",
                  foreground="#555555", wraplength=300, justify="left"
                  ).grid(row=2, column=0, columnspan=2, sticky="w", pady=(8, 8))
        buttons = ttk.Frame(body)
        buttons.grid(row=3, column=0, columnspan=2, sticky="e")

        def save():
            saved = app_settings.clean_panel_names({side: e.get() for side, e in entries.items()})
            app_settings.save_panel_names(saved)
            for panel in self.panels:
                panel.nickname = saved.get(panel.side, "")
                check = self.panel_checks.get(panel.device)
                if check is not None:
                    check.configure(text=panel.label)
            self.update_bar()
            dialog.destroy()
            if self.monitor_running:
                from led_mon.editor import preview
                self.in_background(preview.identify)

        ttk.Button(buttons, text="Save and show", command=save).pack(side="right")
        ttk.Button(buttons, text="Cancel", command=dialog.destroy).pack(side="right", padx=6)
        dialog.bind("<Return>", lambda e: save())
        dialog.bind("<Escape>", lambda e: dialog.destroy())
        entries["left"].focus_set()

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
            status('Tick a panel under "Controls act on", bottom left, first.')
            return False
        if one_panel and len(panels) != 1:
            status('Tick exactly one panel under "Controls act on", bottom left, for this.')
            return False

        def work():
            generations = self.lender.borrow(panels) if borrow else None
            self.call_soon(self.update_bar)
            problems = []
            for panel in panels:
                try:
                    action(panel)
                except Exception as e:
                    problems.append(f"{panel.called}: {e}")
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
            self.home_page.update_panels()
        self.in_background(work, done)

    def on_layout_dirty(self, dirty):
        self.nav_items["monitor"].configure(text="  System monitor *" if dirty else "  System monitor")

    def on_close(self):
        if self.editor is not None and not self.editor.close():
            return
        borrowed = [p for p in self.panels if self.lender.is_borrowed(p)]
        self.music_tab.set_visible(False)
        if borrowed:
            # Give the panels back before closing; the monitor also notices a closed window by itself
            try:
                self.lender.give_back(borrowed)
            except Exception as e:
                log.warning(f"Could not give the panels back: {e}")
        self.root.destroy()

    def quit_for_update(self, start_installer):
        """Close the window and the monitor, then start_installer() (the Info tab's Download and install)."""
        if self.editor is not None and not self.editor.close():
            return
        self.music_tab.set_visible(False)

        def work():
            import time
            borrowed = [p for p in self.panels if self.lender.is_borrowed(p)]
            if borrowed:
                try:
                    self.lender.give_back(borrowed)
                except Exception as e:
                    log.warning(f"Could not give the panels back: {e}")
            if self.link.running(fresh=True):
                self.link.request({"cmd": "quit"}, timeout=5)
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline and self.link.running(fresh=True):
                    time.sleep(0.5)
            start_installer()

        def done(_):
            log.info("Closing for the update")
            self.root.destroy()

        def failed(e):
            self.info_tab.show(f"Couldn't start the installer: {e}")
        self.in_background(work, done, failed)


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
    hwnd = user32.FindWindowW(None, window_title())
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
    if os.environ.get("WHITE_RABBIT_UI_SCALE"):
        # Screenshots away from Windows: draw as Windows would at this display scaling (1.5 = 150%)
        root.tk.call("tk", "scaling", float(os.environ["WHITE_RABBIT_UI_SCALE"]) * 96 / 72)
    from led_mon import theme
    theme.install(root)
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
        def check_sensor_source():
            # Show the Temperatures app's settings, pick each temperature and fan source and a refresh
            # time, and check they are saved
            editor = window.editor
            if editor is None or not platform_utils.IS_WINDOWS:
                return
            from led_mon import app_settings, sensor_sources
            from led_mon.editor import catalog
            editor.settings.show(catalog.new_app("temp", "left"), "Left panel, top half", "left")
            if not hasattr(editor, "sensor_source_var"):
                log.error("Smoke test failed: the Temperatures app's settings have no Read from choice")
                return
            for key, label in list(sensor_sources.SOURCES[1:]) + [sensor_sources.SOURCES[0]]:
                editor.sensor_source_var.set(label)
                editor.choose_sensor_source(key)
                saved = app_settings.load_settings().get(sensor_sources.SETTING)
                if saved != key:
                    log.error(f"Smoke test failed: picked {label}, settings.json has {saved}")
                else:
                    log.info(f"Smoke test: temperature and fan source {label} saved; help: {editor.sensor_help.cget('text')}")
            editor.choose_sensor_refresh(5)
            saved = app_settings.load_settings().get(sensor_sources.REFRESH_SETTING)
            if saved != 5:
                log.error(f"Smoke test failed: picked 5 seconds, settings.json has {saved}")
            else:
                log.info("Smoke test: temperature and fan refresh of 5 seconds saved")

        root.after(3000, check_sensor_source)

        def check_weather_location():
            # Show the Weather app's settings with Location set to Windows Location (asks Windows in the background)
            editor = window.editor
            if editor is None or not platform_utils.IS_WINDOWS:
                return
            try:
                from led_mon.editor import catalog
                entry = catalog.new_app("weather", "left")
                entry["args"]["location_source"] = "windows"
                editor.settings.show(entry, "Left panel, whole panel", "left")
                log.info("Smoke test: weather location settings shown")
            except Exception as e:
                log.error(f"Smoke test failed: the Weather app's location settings: {e}")
        root.after(4500, check_weather_location)

        def check_info_tab():
            # The build adds a VERSION file for the Info tab and the update check
            version = window.info_tab.version
            if version:
                log.info(f"Smoke test: Info tab shows version {version}, title bar '{root.title()}'")
            else:
                log.error("Smoke test failed: the Info tab has no version")
        root.after(5000, check_info_tab)

        def check_look():
            # Each page opens, and the look can be changed to each choice and back
            from led_mon import theme
            look = theme.current()
            try:
                for key in window.pages:
                    window.show_page(key)
                window.show_page("home")
                saved = look.choice
                for choice in theme.CHOICES:
                    look.set_choice(choice)
                    root.update_idletasks()
                    log.info(f"Smoke test: look {choice} shows as {look.mode}, accent {look.accent}")
                look.set_choice(saved)
            except Exception as e:
                log.exception(f"Smoke test failed: changing the look: {e}")
        root.after(5500, check_look)

        def finish():
            if window.editor is not None:
                window.editor.stop.set()
            root.destroy()
        root.after(8000, finish)
    root.mainloop()
