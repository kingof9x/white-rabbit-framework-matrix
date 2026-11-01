# The Audio tab: music mode, and the look and sound input the audio visualizer uses for it.
#
# Changes are saved straight away (as "music" in settings.json) and the monitor picks them up while
# music mode is on. The preview plays the real sound through the same code the panels use, so it
# doubles as a check that the microphone is heard.

# Built In Dependencies
import math
import time
import logging
import threading
import tkinter as tk
from tkinter import ttk

# External Dependencies
import numpy as np

# Internal Dependencies
from led_mon import app_settings, shortcuts
from led_mon.equalizer_files import looks
from white_rabbit.tabs import HINT, section

log = logging.getLogger(__name__)

DEFAULT_MICROPHONE = "Default microphone"
FRAME_MS = 50
DOT = 7
STEP = 8
PANEL_GAP = 18
MARGIN = 10
LED_OFF = (34, 34, 34)
LED_ON = (250, 246, 232)


def led_color(level):
    level = max(0.0, min(1.0, level)) ** 0.6 if level > 0 else 0.0
    return "#%02x%02x%02x" % tuple(int(off + (on - off) * level) for off, on in zip(LED_OFF, LED_ON))


def loudness_db(samples):
    peak = float(np.abs(np.asarray(samples)).max()) if len(samples) else 0.0
    return 20.0 * math.log10(peak) if peak > 0 else -120.0


def hearing_text(hearing):
    """One line about what music mode is hearing, from the monitor's reply."""
    if not hearing:
        return "Music mode is starting..."
    parts = []
    for side in ("left", "right"):
        h = hearing.get(side)
        if not h:
            continue
        device = h.get("device") or "no sound device yet"
        if h.get("problem"):
            state = h["problem"]
        elif h.get("silent"):
            state = "too quiet"
        else:
            state = "hearing sound"
        parts.append(f"{side} panel: {state} ({device})")
    return "Music mode is listening. " + "; ".join(parts) if parts else "Music mode is starting..."


class MusicTab(ttk.Frame):

    def __init__(self, master, window):
        super().__init__(master, padding=6)
        self.window = window
        self.settings = app_settings.load_music()
        self.capture = None
        self.capture_key = None
        self.visualizers = {}
        self.visible = False
        self.last_frame_time = time.monotonic()
        self.leds = {"left": [], "right": []}
        self.shown = {"left": None, "right": None}
        self.microphones = []
        self.music_on = None

        mode = section(self, "Music mode")
        ttk.Label(mode, text="Shows the audio visualizer on both panels, with the look picked below, until you "
                  f"turn it off. Also in the tray menu, and with {shortcuts.keys_text('m')}.",
                  foreground=HINT, wraplength=900, justify="left").pack(anchor="w", pady=(0, 6))
        row = ttk.Frame(mode)
        row.pack(fill="x")
        self.mode_button = ttk.Button(row, text="Turn music mode on", command=self.toggle_music_mode, width=24)
        self.mode_button.pack(side="left")
        self.mode_status = ttk.Label(row, text="", foreground=HINT)
        self.mode_status.pack(side="left", padx=10)
        self.mode_hearing = ttk.Label(mode, text="", foreground=HINT)
        self.mode_hearing.pack(anchor="w", pady=(4, 0))

        body = ttk.Frame(self)
        body.pack(fill="both", expand=True)
        controls = ttk.Frame(body)
        controls.pack(side="left", fill="both", expand=True)
        self.build_listen(controls)
        self.build_look(controls)
        self.build_preview(body)
        self.status = ttk.Label(self, text="", foreground=HINT)
        self.status.pack(anchor="w", padx=12, pady=6)
        self.show_settings()

    # Building the tab
    def build_listen(self, parent):
        listen = section(parent, "Listen to")
        self.input_mode = tk.StringVar(value=self.settings["input-mode"])
        for mode in looks.INPUT_MODES:
            ttk.Radiobutton(listen, text=looks.INPUT_LABELS[mode], variable=self.input_mode, value=mode,
                            command=self.changed).pack(anchor="w")
        mic_row = ttk.Frame(listen)
        mic_row.pack(fill="x", padx=(20, 0), pady=(2, 4))
        ttk.Label(mic_row, text="Microphone:").pack(side="left")
        self.microphone = ttk.Combobox(mic_row, state="readonly", width=46)
        self.microphone.pack(side="left", padx=4)
        self.microphone.bind("<<ComboboxSelected>>", lambda e: self.changed())
        ttk.Button(mic_row, text="Look again", command=self.find_microphones).pack(side="left")
        check = ttk.Frame(listen)
        check.pack(fill="x", pady=(4, 0))
        ttk.Label(check, text="Sound check:").pack(side="left")
        self.meter = tk.Canvas(check, width=200, height=12, highlightthickness=0, background="#222222")
        self.meter.pack(side="left", padx=6)
        self.meter_bar = self.meter.create_rectangle(0, 0, 0, 12, fill="#7bd88f", width=0)
        self.hearing = ttk.Label(check, text="", foreground=HINT)
        self.hearing.pack(side="left")
        self.fill_microphones([])
        self.find_microphones()

    def build_look(self, parent):
        look = section(parent, "Look")
        row = ttk.Frame(look)
        row.pack(fill="x")
        ttk.Label(row, text="Look:").pack(side="left")
        self.style = ttk.Combobox(row, state="readonly", width=20,
                                  values=[looks.STYLE_LABELS[k] for k in looks.STYLE_KEYS])
        self.style.pack(side="left", padx=4)
        self.style.bind("<<ComboboxSelected>>", lambda e: self.changed())
        self.style_text = ttk.Label(look, text="", foreground=HINT, wraplength=520, justify="left")
        self.style_text.pack(anchor="w", pady=(2, 6))

        self.flags = {}
        for key, text in (("span-panels", "One spectrum across both panels: 18 bars, bass on the left panel"),
                          ("peak-dots", "Peak dots: a dot above each bar that falls slowly"),
                          ("shaded", "Shaded: brighter towards the tips"),
                          ("flip", "Upside down"),
                          ("auto-sensitivity", "Adjust the sensitivity automatically")):
            var = tk.BooleanVar(value=self.settings[key])
            ttk.Checkbutton(look, text=text, variable=var, command=self.changed).pack(anchor="w")
            self.flags[key] = var

        grid = ttk.Frame(look)
        grid.pack(fill="x", pady=(6, 0))
        ttk.Label(grid, text="Falling speed:").grid(row=0, column=0, sticky="w")
        self.fall = ttk.Combobox(grid, state="readonly", width=10,
                                 values=[looks.FALL_LABELS[k] for k in looks.FALL_SPEEDS])
        self.fall.grid(row=0, column=1, sticky="w", padx=4)
        self.fall.bind("<<ComboboxSelected>>", lambda e: self.changed())
        ttk.Label(grid, text="Sensitivity:").grid(row=1, column=0, sticky="w", pady=(4, 0))
        self.sensitivity = tk.Scale(grid, from_=1, to=10, orient="horizontal", length=200, showvalue=True,
                                    command=lambda v: self.changed())
        self.sensitivity.grid(row=1, column=1, sticky="w", padx=4)
        ttk.Label(grid, text="Turn it up if the bars hardly move, down if they're always full.",
                  foreground=HINT).grid(row=1, column=2, sticky="w", padx=6)

    def build_preview(self, parent):
        frame = ttk.LabelFrame(parent, text="Preview", padding=8)
        frame.pack(side="left", fill="y", padx=12, pady=6, anchor="n")
        width = 2 * MARGIN + 2 * 9 * STEP + PANEL_GAP
        height = 2 * MARGIN + 34 * STEP
        self.canvas = tk.Canvas(frame, width=width, height=height, background="#111111", highlightthickness=0)
        self.canvas.pack()
        for p, side in enumerate(("left", "right")):
            x0 = MARGIN + p * (9 * STEP + PANEL_GAP)
            for x in range(9):
                column = []
                for y in range(34):
                    cx, cy = x0 + x * STEP, MARGIN + y * STEP
                    column.append(self.canvas.create_oval(cx, cy, cx + DOT, cy + DOT, fill=led_color(0), width=0))
                self.leds[side].append(column)
        ttk.Label(frame, text="Left panel        Right panel", foreground=HINT).pack()
        ttk.Label(frame, text="Plays the real sound, so you can\nsee what the panels will show.",
                  foreground=HINT, justify="center").pack(pady=(4, 0))

    # Settings
    def show_settings(self):
        s = self.settings
        self.input_mode.set(s["input-mode"])
        self.style.current(looks.STYLE_KEYS.index(s["style"]))
        self.style_text.configure(text=looks.STYLE_DESCRIPTIONS[s["style"]])
        for key, var in self.flags.items():
            var.set(s[key])
        self.fall.current(list(looks.FALL_SPEEDS).index(s["fall-speed"]))
        self._setting_scale = True
        self.sensitivity.set(s["sensitivity"])
        self._setting_scale = False
        self.update_enabled()

    def read_settings(self):
        s = dict(self.settings)
        s["input-mode"] = self.input_mode.get()
        index = self.microphone.current()
        s["input-device"] = self.microphones[index - 1] if index > 0 and index - 1 < len(self.microphones) else (
            s["input-device"] if index < 0 else "")
        if self.style.current() >= 0:
            s["style"] = looks.STYLE_KEYS[self.style.current()]
        for key, var in self.flags.items():
            s[key] = bool(var.get())
        if self.fall.current() >= 0:
            s["fall-speed"] = list(looks.FALL_SPEEDS)[self.fall.current()]
        s["sensitivity"] = int(self.sensitivity.get())
        return looks.normalize(s)

    def update_enabled(self):
        mic = self.input_mode.get() == "microphone"
        self.microphone.configure(state="readonly" if mic else "disabled")

    def changed(self):
        if getattr(self, "_setting_scale", False):
            return
        new = self.read_settings()
        if new == self.settings:
            return
        self.settings = new
        self.style_text.configure(text=looks.STYLE_DESCRIPTIONS[new["style"]])
        self.update_enabled()
        if not app_settings.save_music(new):
            self.status.configure(text="Couldn't save the change (see the log).")
            return
        self.status.configure(text="Saved. Music mode uses this look from now on.")
        self.visualizers = {}
        if self.visible:
            self.listen()
        # The monitor starts the visualizer again with the new look, if music mode is on
        self.window.in_background(lambda: self.window.link.request({"cmd": "music", "action": "reload"}, timeout=3))

    # Microphones
    def find_microphones(self):
        from led_mon.equalizer_files.audio import list_microphones
        self.window.in_background(list_microphones, self.fill_microphones)

    def fill_microphones(self, names):
        self.microphones = list(names)
        values = [DEFAULT_MICROPHONE] + self.microphones
        wanted = self.settings["input-device"]
        if wanted and wanted not in self.microphones:
            # Picked before, but not plugged in now
            self.microphones.append(wanted)
            values.append(f"{wanted} (not found)")
        self.microphone.configure(values=values)
        self.microphone.current(self.microphones.index(wanted) + 1 if wanted else 0)

    # Music mode
    def toggle_music_mode(self):
        action = "off" if self.music_on else "on"
        self.mode_status.configure(text="...")

        def work():
            if action == "on":
                # Panels lent to this window (a game, a pattern, the Math tab) don't show the monitor's
                # drawing, so give them back first
                self.window.lender.give_back(self.window.panels)
                self.window.call_soon(self.window.update_bar)
            return self.window.link.request({"cmd": "music", "action": action}, timeout=5)
        self.window.in_background(work, self.show_music_mode)

    def check_music_mode(self):
        self.window.in_background(lambda: self.window.link.request({"cmd": "music", "action": "status"}, timeout=3),
                                  self.show_music_mode)

    def show_music_mode(self, reply):
        if not reply or "music_mode" not in reply:
            self.music_on = None
            self.mode_button.configure(text="Turn music mode on")
            self.mode_status.configure(text="The system monitor isn't running, so music mode can't start.")
            return
        self.music_on = bool(reply["music_mode"])
        self.window.update_bar()
        self.mode_button.configure(text="Turn music mode off" if self.music_on else "Turn music mode on")
        self.mode_status.configure(text="On: both panels show the audio visualizer." if self.music_on else
                                   "Off: the panels show your layout.")
        self.mode_hearing.configure(text=hearing_text(reply.get("hearing")) if self.music_on else "")

    def poll_music_mode(self):
        """While the tab shows, keep the music mode line up to date (what the panels hear)."""
        if not self.visible:
            return
        self.check_music_mode()
        self.after(2000, self.poll_music_mode)

    # The preview
    def set_visible(self, visible):
        if visible == self.visible:
            return
        self.visible = visible
        if visible:
            self.poll_music_mode()
            self.listen()
            self.last_frame_time = time.monotonic()
            self.after(FRAME_MS, self.tick)
        else:
            self.stop_listening()

    def listen(self):
        """Start recording what the settings say to listen to (if it isn't already)."""
        from led_mon.equalizer_files import audio
        key = (self.settings["input-mode"], self.settings["input-device"])
        if self.capture is not None and self.capture_key == key:
            return
        self.stop_listening()
        try:
            self.capture = audio.acquire(*key)
            self.capture_key = key
        except Exception as e:
            log.warning(f"The Audio tab couldn't listen: {e}")
            self.hearing.configure(text=f"Couldn't listen: {e}")

    def stop_listening(self):
        if self.capture is not None:
            from led_mon.equalizer_files import audio
            try:
                audio.release(self.capture)
            except Exception as e:
                log.debug(f"Stopping the Audio tab's recording: {e}")
        self.capture = None
        self.capture_key = None

    def tick(self):
        if not self.visible:
            return
        try:
            self.draw_frame()
        except Exception as e:
            log.error(f"Audio tab preview failed: {e}")
        self.after(FRAME_MS, self.tick)

    def draw_frame(self):
        now = time.monotonic()
        dt, self.last_frame_time = now - self.last_frame_time, now
        samples = self.capture.snapshot() if self.capture is not None else np.zeros((looks.FFT_SIZE, 2))
        for side in ("left", "right"):
            visualizer = self.visualizers.get(side)
            if visualizer is None:
                visualizer = self.visualizers[side] = looks.Visualizer(self.settings, side)
            self.draw_panel(side, visualizer.frame(samples, min(dt, 0.5)))
        self.show_sound_check(samples, self.visualizers["left"].is_silent())

    def draw_panel(self, side, grid):
        levels = np.round(np.asarray(grid) * 20) / 20
        last = self.shown[side]
        for x in range(9):
            for y in range(34):
                value = float(levels[x][y])
                if last is None or float(last[x][y]) != value:
                    self.canvas.itemconfigure(self.leds[side][x][y], fill=led_color(value))
        self.shown[side] = levels

    def show_sound_check(self, samples, silent):
        db = loudness_db(samples[-2048:])
        fraction = max(0.0, min(1.0, (db + 80.0) / 80.0))
        self.meter.coords(self.meter_bar, 0, 0, int(200 * fraction), 12)
        if self.capture is None:
            return
        name = self.capture.device_name or "the sound device"
        if self.capture.problem:
            text = self.capture.problem
        elif db <= -100:
            text = ("Nothing is playing" if self.settings["input-mode"] == "playback" else "Silence") + f" ({name})"
        elif silent:
            text = (f"Listening to {name}, but it's too quiet for the bars. Play music nearer the microphone, "
                    "or turn the sensitivity up." if self.settings["input-mode"] == "microphone" else
                    f"Listening to {name}, but it's too quiet for the bars. Turn the volume or the sensitivity up.")
        else:
            text = f"Hearing sound from {name}"
        self.hearing.configure(text=text)
