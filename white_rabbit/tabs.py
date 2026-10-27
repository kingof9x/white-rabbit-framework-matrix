# The control tabs of the White Rabbit window, built on the inputmodule package
# (the same code as Framework's LED Matrix Control app, ledmatrixgui).

# Built In Dependencies
import os
import threading
import webbrowser
import tkinter as tk
from tkinter import ttk, filedialog, messagebox

# Internal Dependencies
from inputmodule.inputmodule import send_command, CommandVals, Game, GameControlVal
from inputmodule.inputmodule.ledmatrix import (
    PATTERNS,
    STARTUP_ANIMATIONS,
    StartupMode,
    animate,
    pattern,
    percentage,
    set_startup_animation,
    show_string,
)
from white_rabbit import firmware

HINT = "#555555"
# Seconds the panels stay with this window after saving the startup animation, so it can play once
STARTUP_PREVIEW_SECONDS = 10

GAME_OF_LIFE = [
    ("Current picture", 0), ("Pattern 1", 1), ("Blinker", 2), ("Toad", 3), ("Beacon", 4), ("Glider", 5),
    ("R-pentomino", 7), ("Acorn", 8), ("Diehard", 9), ("Light spaceship", 10), ("Medium spaceship", 11),
    ("Heavy spaceship", 12), ("Spaceship fleet", 13), ("Octagon", 14), ("Tumbler", 15), ("Caterer", 16),
]
FIRMWARE_RELEASES = "https://github.com/kingof9x/inputmodule-rs/releases"


def section(parent, title):
    frame = ttk.LabelFrame(parent, text=title, padding=8)
    frame.pack(fill="x", padx=12, pady=6)
    return frame


class StartupTab(ttk.Frame):
    """Which animation the panels show when the laptop turns on. Saved on the panel itself."""

    MODES = [
        ("Random: a different one every time", StartupMode.RandomAll),
        ("Always this one:", StartupMode.Fixed),
        ("Random out of these:", StartupMode.RandomSet),
        ("None: the panels stay dark until an app uses them", StartupMode.NoAnimation),
    ]

    def __init__(self, master, window):
        super().__init__(master, padding=6)
        self.window = window
        frame = section(self, "Startup animation")
        ttk.Label(frame, text="Saved on the panels themselves, so it works even when White Rabbit isn't running. "
                  "Needs the White Rabbit firmware (see the Firmware tab).", foreground=HINT,
                  wraplength=760, justify="left").pack(anchor="w", pady=(0, 6))
        self.mode = tk.IntVar(value=StartupMode.RandomAll)
        self.animation = ttk.Combobox(frame, values=STARTUP_ANIMATIONS, state="readonly", width=40)
        self.animation.current(0)
        self.set_vars = [tk.BooleanVar(value=True) for _ in STARTUP_ANIMATIONS]
        self.set_frame = ttk.Frame(frame)
        self.set_boxes = []
        for i, name in enumerate(STARTUP_ANIMATIONS):
            box = ttk.Checkbutton(self.set_frame, text=name, variable=self.set_vars[i])
            box.grid(row=i // 3, column=i % 3, sticky="w", padx=6)
            self.set_boxes.append(box)
        self.radios = []
        for text, mode in self.MODES:
            radio = ttk.Radiobutton(frame, text=text, variable=self.mode, value=mode, command=self.update_enabled)
            radio.pack(anchor="w")
            self.radios.append(radio)
            if mode == StartupMode.Fixed:
                self.animation.pack(anchor="w", padx=24, pady=2)
            elif mode == StartupMode.RandomSet:
                self.set_frame.pack(anchor="w", padx=18, pady=2)
        row = ttk.Frame(frame)
        row.pack(fill="x", pady=(8, 0))
        self.save_button = ttk.Button(row, text="Save", command=self.save)
        self.save_button.pack(side="left")
        self.status = ttk.Label(row, text="Reading the panels...", foreground=HINT)
        self.status.pack(side="left", padx=10)
        self.supported = False
        self.update_enabled()

    def update_enabled(self):
        on = self.supported
        self.animation.configure(state="readonly" if on and self.mode.get() == StartupMode.Fixed else "disabled")
        for box in self.set_boxes:
            box.state(["!disabled"] if on and self.mode.get() == StartupMode.RandomSet else ["disabled"])
        for widget in self.radios + [self.save_button]:
            widget.state(["!disabled"] if on else ["disabled"])

    def show_modules(self, info):
        current = next((i["startup"] for i in info.values() if i["startup"]), None)
        self.supported = current is not None
        if current:
            mode, animation, animation_set = current
            self.mode.set(mode)
            if animation < len(STARTUP_ANIMATIONS):
                self.animation.current(animation)
            for i, var in enumerate(self.set_vars):
                var.set(bool(animation_set & (1 << i)))
            self.status.configure(text="")
        elif any(i["in_use"] for i in info.values()):
            self.status.configure(text="Another program is using the panels. Close it and reopen this window.")
        elif not info:
            self.status.configure(text="No LED Matrix panels found.")
        else:
            self.status.configure(text="The panels need the White Rabbit firmware for this (Firmware tab).")
        self.update_enabled()

    def save(self):
        mask = 0
        for i, var in enumerate(self.set_vars):
            if var.get():
                mask |= 1 << i
        mode = self.mode.get()
        if mode == StartupMode.RandomSet and mask == 0:
            self.status.configure(text="Tick at least one animation.")
            return
        animation = self.animation.current()
        supported = [p for p in self.window.selected_panels()
                     if (self.window.module_info.get(p.device) or {}).get("startup")]
        if not supported:
            self.status.configure(text="None of the ticked panels has the White Rabbit firmware.")
            return

        def done(text):
            self.status.configure(text="Saved. Playing it now." if text == "Done." else text)
        self.window.act(lambda panel: set_startup_animation(panel, mode, animation, mask), done, panels=supported,
                        return_after=STARTUP_PREVIEW_SECONDS)


class PlayTab(ttk.Frame):
    """Games, Game of Life patterns, the firmware's patterns, text and a few moving things."""

    def __init__(self, master, window):
        super().__init__(master, padding=6)
        self.window = window
        self.status = ttk.Label(self, text="These borrow the ticked panels from the system monitor. "
                                "Stop, or Back to system monitor, gives them back.", foreground=HINT)
        self.status.pack(side="bottom", anchor="w", padx=12, pady=6)

        gol = section(self, "Game of Life")
        for i, (name, param) in enumerate(GAME_OF_LIFE):
            ttk.Button(gol, text=name, width=18,
                       command=lambda p=param: self.run(lambda d: send_command(d, CommandVals.StartGame,
                                                                               [Game.GameOfLife, p]))
                       ).grid(row=i // 6, column=i % 6, padx=2, pady=2)
        ttk.Button(gol, text="Stop", command=self.stop).grid(row=3, column=0, padx=2, pady=(8, 2), sticky="w")

        games = section(self, "Games (played in a small window with the arrow keys)")
        ttk.Button(games, text="Snake", command=lambda: self.pygame("snake")).pack(side="left", padx=2)
        ttk.Button(games, text="Ledris", command=lambda: self.pygame("ledris")).pack(side="left", padx=2)

        shapes = section(self, "Patterns and text")
        ttk.Label(shapes, text="Pattern:").grid(row=0, column=0, sticky="w")
        self.pattern = ttk.Combobox(shapes, values=PATTERNS, state="readonly", width=36)
        self.pattern.grid(row=0, column=1, sticky="w", padx=4)
        self.pattern.bind("<<ComboboxSelected>>", lambda e: self.run(lambda d: pattern(d, self.pattern.get())))
        ttk.Label(shapes, text="Text:").grid(row=1, column=0, sticky="w", pady=4)
        text_row = ttk.Frame(shapes)
        text_row.grid(row=1, column=1, sticky="w", padx=4)
        self.text = ttk.Entry(text_row, width=20)
        self.text.pack(side="left")
        ttk.Button(text_row, text="Show", command=lambda: self.run(lambda d: show_string(d, self.text.get().upper()))
                   ).pack(side="left", padx=4)
        ttk.Label(shapes, text="Fill:").grid(row=2, column=0, sticky="w")
        self.fill = tk.Scale(shapes, from_=0, to=100, orient="horizontal", length=260, showvalue=True,
                             command=lambda v: self.run(lambda d: percentage(d, int(float(v)))))
        self.fill.grid(row=2, column=1, sticky="w", padx=4)
        ttk.Button(shapes, text="Scroll the picture", command=lambda: self.run(lambda d: animate(d, True))
                   ).grid(row=3, column=1, sticky="w", padx=4, pady=4)

        moving = section(self, "Clock, countdown and equalizer")
        ttk.Button(moving, text="Clock", command=lambda: self.thread_per_panel("clock")).pack(side="left", padx=2)
        self.seconds = tk.Spinbox(moving, from_=1, to=600, width=5)
        self.seconds.delete(0, "end")
        self.seconds.insert(0, "10")
        ttk.Button(moving, text="Countdown", command=lambda: self.thread_per_panel("countdown")).pack(side="left", padx=(12, 2))
        self.seconds.pack(side="left")
        ttk.Label(moving, text="seconds").pack(side="left", padx=(2, 12))
        ttk.Button(moving, text="Random equalizer", command=lambda: self.thread_per_panel("eq")).pack(side="left", padx=2)
        ttk.Button(moving, text="Stop", command=self.stop).pack(side="left", padx=(12, 2))

    def show(self, text):
        self.status.configure(text=text)

    def run(self, action):
        self.window.act(action, self.show)

    def stop(self):
        self.window.give_back(self.window.selected_panels())
        self.show("Stopped. The system monitor has the panels again.")

    def thread_per_panel(self, name):
        from inputmodule.gui.ledmatrix import clock, countdown, random_eq
        from inputmodule.gui.gui_threading import reset_thread
        seconds = int(self.seconds.get() or 10)
        target = {"clock": clock, "eq": random_eq,
                  "countdown": lambda d: countdown(d, seconds)}[name]

        def start(panel):
            reset_thread()
            threading.Thread(target=target, args=(panel,), daemon=True).start()
        self.window.act(start, self.show)

    def pygame(self, name):
        def start(panels):
            try:
                if name == "snake":
                    from inputmodule.gui.pygames import snake as game
                else:
                    from inputmodule.gui.pygames import ledris as game
                game.main_devices(panels)
            except Exception as e:
                self.window.call_soon(lambda: self.show(f"The game couldn't start: {e}"))

        panels = self.window.selected_panels()
        if self.window.act(lambda panel: None, self.show, panels=panels):
            threading.Thread(target=start, args=(panels,), daemon=True).start()


class FirmwareTab(ttk.Frame):
    """What firmware each panel has, bootloader mode, and flashing the bundled firmware."""

    CHOOSE_FILE = "A .uf2 file on this computer..."

    def __init__(self, master, window):
        super().__init__(master, padding=6)
        self.window = window
        installed = section(self, "Installed firmware")
        self.installed = ttk.Frame(installed)
        self.installed.pack(fill="x")
        ttk.Label(self.installed, text="Reading the panels...", foreground=HINT).pack(anchor="w")
        ttk.Button(installed, text="Check again", command=self.window_refresh).pack(anchor="w", pady=(6, 0))

        update = section(self, "Update firmware")
        ttk.Label(update, text="Close other programs that use the LED panels first. Flashing works on one panel "
                  "at a time: tick only that one at the bottom of the window.", foreground=HINT,
                  wraplength=760, justify="left").pack(anchor="w", pady=(0, 6))
        self.bundled = firmware.bundled_firmware()
        choices = [label for label, _ in self.bundled] + [self.CHOOSE_FILE]
        row = ttk.Frame(update)
        row.pack(fill="x")
        self.choice = ttk.Combobox(row, values=choices, state="readonly", width=70)
        self.choice.current(0)
        self.choice.pack(side="left")
        ttk.Button(row, text="Flash", command=self.flash).pack(side="left", padx=6)
        links = ttk.Frame(update)
        links.pack(fill="x", pady=(6, 0))
        ttk.Button(links, text="Enter bootloader", command=self.bootloader).pack(side="left")
        ttk.Button(links, text="Firmware releases", command=lambda: webbrowser.open(FIRMWARE_RELEASES)).pack(side="left", padx=6)
        self.status = ttk.Label(update, text="", foreground=HINT, wraplength=760, justify="left")
        self.status.pack(anchor="w", pady=(6, 0))

        power = section(self, "Sleep")
        ttk.Button(power, text="Sleep", command=lambda: self.window.act(
            lambda d: send_command(d, CommandVals.Sleep, [True]), self.show)).pack(side="left")
        ttk.Button(power, text="Wake", command=lambda: self.window.act(
            lambda d: send_command(d, CommandVals.Sleep, [False]), self.show)).pack(side="left", padx=6)

    def show(self, text):
        self.status.configure(text=text)

    def window_refresh(self):
        for child in self.installed.winfo_children():
            child.destroy()
        ttk.Label(self.installed, text="Reading the panels...", foreground=HINT).pack(anchor="w")
        self.window.refresh_modules()

    def show_modules(self, info):
        for child in self.installed.winfo_children():
            child.destroy()
        if not self.window.panels:
            ttk.Label(self.installed, text="No LED Matrix panels found.").pack(anchor="w")
        for panel in self.window.panels:
            details = info.get(panel.device) or {}
            if details.get("in_use"):
                text = f"{panel.label}: in use by another program. Close it, then click Check again."
            else:
                startup = "has" if details.get("startup") else "doesn't have"
                text = (f"{panel.label}: version {details.get('version') or 'unknown'}, "
                        f"{startup} the startup animation setting")
            ttk.Label(self.installed, text=text).pack(anchor="w")

    def bootloader(self):
        from inputmodule.inputmodule import bootloader_jump
        if not messagebox.askyesno("Enter bootloader", "The ticked panels go dark and show up as an RPI-RP2 drive, "
                                   "ready for new firmware. Continue?", parent=self):
            return
        self.window.act(bootloader_jump, lambda text: self.show(
            "Copy a .uf2 file onto the RPI-RP2 drive, or use Flash." if text == "Done." else text))

    def firmware_path(self):
        label = self.choice.get()
        if label == self.CHOOSE_FILE:
            return filedialog.askopenfilename(parent=self, title="Choose LED Matrix firmware",
                                              filetypes=[("Firmware", "*.uf2")]) or None
        return dict(self.bundled).get(label)

    def flash(self):
        path = self.firmware_path()
        if not path:
            return
        panels = self.window.selected_panels()
        if len(panels) != 1:
            self.show("Tick exactly one panel at the bottom of the window to flash it.")
            return
        panel = panels[0]
        if not messagebox.askyesno("Flash firmware", f"Flash {os.path.basename(path)} onto the {panel.side} panel?\n\n"
                                   f"{self.choice.get()}", parent=self):
            return
        status = lambda text: self.window.call_soon(lambda: self.show(text))

        def finished(text):
            if text != "Done.":  # otherwise keep the message from firmware.flash
                self.show(text)
            # Give the panel back once it has restarted, then read its new version
            self.window.root.after(6000, lambda: (self.window.give_back([panel]), self.window_refresh()))
        self.window.act(lambda p: status(firmware.flash(p, path, status)), finished, panels=[panel])
