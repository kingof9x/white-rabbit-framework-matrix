# The Effects tab: a ripple when something is plugged in, the text line along the top of the panels,
# sliding between apps, and dimming. The monitor plays the effects (led_mon/effects.py); this tab saves the
# settings in settings.json, asks the monitor to reload them, and plays the same effects in its preview.

# Built In Dependencies
import time
import logging
import tkinter as tk
from tkinter import ttk

# External Dependencies
import numpy as np

# Internal Dependencies
from led_mon import effects, drawing
from white_rabbit.tabs import HINT, section
from white_rabbit.math_tab import ScrollFrame
from white_rabbit.music import led_color, DOT, STEP, PANEL_GAP, MARGIN
from led_mon.theme import Slider

log = logging.getLogger(__name__)

FRAME_MS = 33
POSITIONS = [f"{side}-{spot}" for side in effects.SIDES for spot in effects.SPOTS]
HOW = {
    "learned": "the port you picked for it",
    "framework_tool": "as Framework's framework_tool reported",
    "side": "Windows only says which side",
    "remembered": "where it was plugged in",
    "unknown": "White Rabbit couldn't tell which port",
}


def sample_frames():
    """A typical monitor picture for the preview: CPU and memory on the left, disk and network on the right."""
    fg, bg = 160, 35
    left = np.zeros((effects.WIDTH, effects.HEIGHT), dtype=int)
    drawing.draw_app("cpu", left, [0.9, 0.35, 0.6, 0.2, 0.75, 0.45, 0.3, 0.55], fg, 0)
    drawing.draw_app_border("cpu", left, bg, 0)
    drawing.draw_app("mem", left, 0.5, fg, 16)
    drawing.draw_app_border("mem", left, bg, 16)
    right = np.zeros((effects.WIDTH, effects.HEIGHT), dtype=int)
    drawing.draw_app("disk-net", right, (0.2, 0.8), (1.0, 0.4), fg, 0)
    drawing.draw_app_border("disk-net", right, bg, 0)
    drawing.draw_app("disk-net", right, (0.55, 0.05), (0.3, 0.95), fg, 16)
    drawing.draw_app_border("disk-net", right, bg, 16)
    return {"left": left, "right": right}


class PortTeacher:
    """Walks through the six expansion card ports: plug something into each and White Rabbit remembers it.

    Each USB port of the laptop is its own place to Windows (the same for every device plugged into it),
    so one plug per port is enough. The monitor sees the plug and says where it was; this learns that."""

    POLL_MS = 600

    def __init__(self, tab):
        self.tab = tab
        self.step = 0
        self.done = {}            # position: the port's key
        self.waiting_since = time.time()
        self.closed = False
        self.dialog = tk.Toplevel(tab.winfo_toplevel())
        self.dialog.title("Teach White Rabbit my ports")
        self.dialog.transient(tab.winfo_toplevel())
        self.dialog.resizable(False, False)
        self.dialog.protocol("WM_DELETE_WINDOW", self.close)
        body = ttk.Frame(self.dialog, padding=14)
        body.pack(fill="both", expand=True)
        ttk.Label(body, text="Use something small, like a USB stick, mouse or phone cable. For each port: unplug it "
                  "from the last port, then plug it in here. Ports with a display or storage card can be skipped.",
                  wraplength=380, justify="left").pack(anchor="w")
        self.ask = ttk.Label(body, text="", font=("Segoe UI", 12, "bold"))
        self.ask.pack(anchor="w", pady=(10, 2))
        self.heard = ttk.Label(body, text="", foreground=HINT, wraplength=380, justify="left")
        self.heard.pack(anchor="w")
        grid = ttk.Frame(body)
        grid.pack(anchor="w", pady=(10, 0))
        self.marks = {}
        for col, side in enumerate(effects.SIDES):
            ttk.Label(grid, text=f"{side.capitalize()} side", font=("Segoe UI", 9, "bold")).grid(row=0, column=col, sticky="w", padx=(0, 30))
            for row, spot in enumerate(effects.SPOTS):
                mark = ttk.Label(grid, text=f"○  {spot}")
                mark.grid(row=row + 1, column=col, sticky="w", padx=(0, 30))
                self.marks[f"{side}-{spot}"] = mark
        buttons = ttk.Frame(body)
        buttons.pack(fill="x", pady=(12, 0))
        ttk.Button(buttons, text="Done", command=self.close).pack(side="right")
        self.skip_button = ttk.Button(buttons, text="Skip this port", command=self.skip)
        self.skip_button.pack(side="right", padx=6)
        ttk.Button(buttons, text="Forget all and start over", command=self.start_over).pack(side="left")
        self.show_step()
        self.dialog.after(self.POLL_MS, self.poll)

    def show_step(self):
        for position, mark in self.marks.items():
            spot = position.split("-")[1]
            if position in self.done:
                mark.configure(text=f"✓  {spot}" if self.done[position] else f"–  {spot} (skipped)")
            elif self.step < len(POSITIONS) and POSITIONS[self.step] == position:
                mark.configure(text=f"▶  {spot}")
            else:
                mark.configure(text=f"○  {spot}")
        if self.step >= len(POSITIONS):
            learned = sum(1 for key in self.done.values() if key)
            self.ask.configure(text="All done")
            self.heard.configure(text=f"White Rabbit knows {learned} of your ports now. The ripple starts from them "
                                      "from now on. You can run this again any time.")
            self.skip_button.state(["disabled"])
            return
        position = POSITIONS[self.step]
        self.ask.configure(text=f"Plug something into the {effects.position_label(position).lower()} port")
        self.waiting_since = time.time()

    def advance(self, key):
        self.done[POSITIONS[self.step]] = key
        self.step += 1
        self.show_step()

    def skip(self):
        if self.step < len(POSITIONS):
            self.heard.configure(text="")
            self.advance(None)

    def start_over(self):
        self.tab.ask_monitor({"action": "forget_ports"}, done=lambda reply: None)
        self.done = {}
        self.step = 0
        self.heard.configure(text="Forgot every port White Rabbit had learned.")
        self.show_step()

    def poll(self):
        if self.closed:
            return
        self.tab.ask_monitor({"action": "status"}, done=self.got_reply)
        self.dialog.after(self.POLL_MS, self.poll)

    def got_reply(self, reply):
        if self.closed:
            return
        if reply is None:
            self.heard.configure(text="The monitor isn't running, so White Rabbit can't see plugs. Start it on the Home page.")
            return
        self.tab.got_reply(reply)
        plug = reply.get("last_plug") or {}
        if (self.step >= len(POSITIONS) or not plug.get("plugged_in") or plug.get("kind") == "charger"
                or plug.get("time", 0) < self.waiting_since):
            return
        key = plug.get("key")
        name = plug.get("name") or "Something"
        if not key:
            self.heard.configure(text=f"{name} was plugged in, but Windows didn't say where. Try something else.")
            self.waiting_since = time.time()
            return
        same = next((p for p, k in self.done.items() if k == key), None)
        if same is not None:
            self.heard.configure(text=f"That is the same port as {effects.position_label(same).lower()}. "
                                      "Unplug it and use the port shown.")
            self.waiting_since = time.time()
            return
        position = POSITIONS[self.step]
        self.tab.ask_monitor({"action": "learn", "key": key, "position": position})
        self.heard.configure(text=f"Got it: {name}. That is the {effects.position_label(position).lower()} port.")
        self.advance(key)

    def close(self):
        self.closed = True
        self.dialog.destroy()


class EffectsTab(ttk.Frame):

    def __init__(self, master, window):
        super().__init__(master, padding=6)
        self.window = window
        self.settings = effects.load_settings()
        self.vars = {}
        self.visible = False
        self.last_plug = {}
        self.leds = {"left": [], "right": []}
        self.shown = {"left": None, "right": None}
        self.preview = effects.Effects()
        self.preview.configure(self.settings)
        for side in effects.SIDES:
            self.preview.panel_present(side)
        self.frames = sample_frames()
        self.slide_flip = False

        body = ttk.Frame(self)
        body.pack(fill="both", expand=True)
        # Scrolls when the window is small or Windows display scaling makes everything bigger
        scroller = ScrollFrame(body)
        scroller.pack(side="left", fill="both", expand=True)
        controls = scroller.inner
        self.build_plug(controls)
        self.build_text(controls)
        self.build_changing(controls)
        self.build_dimming(controls)
        self.build_preview(body)
        self.status = ttk.Label(controls, text="", foreground=HINT)
        self.status.pack(anchor="w", padx=12)

    # Settings
    def flag(self, parent, key, text, **pack):
        var = tk.BooleanVar(value=self.settings[key])
        self.vars[key] = var
        box = ttk.Checkbutton(parent, text=text, variable=var, command=self.changed)
        box.pack(**(pack or {"anchor": "w"}))
        return box

    def changed(self):
        s = dict(self.settings)
        for key, var in self.vars.items():
            s[key] = var.get()
        s["text_speed"] = int(self.speed.get())
        s["slide"] = self.slide_var.get() == "slide"
        # Learned ports may have changed in the monitor since this tab read them
        s["learned_ports"] = effects.load_settings().get("learned_ports", {})
        self.settings = effects.normalize(s)
        effects.save(self.settings)
        self.preview.configure(self.settings)
        self.update_enabled()
        self.ask_monitor({"action": "reload"})
        self.status.configure(text="Saved.")

    def update_enabled(self):
        plug = self.vars["plug_animation"].get()
        for box in self.plug_boxes:
            box.state(["!disabled"] if plug else ["disabled"])
        text = self.vars["text_line"].get()
        for box in self.text_boxes:
            box.state(["!disabled"] if text else ["disabled"])

    def ask_monitor(self, message, done=None):
        def work():
            return self.window.link.request(dict(message, cmd="effects"), timeout=3)
        self.window.in_background(work, done or self.got_reply)

    def got_reply(self, reply):
        if reply and reply.get("last_plug") is not None:
            self.show_last_plug(reply["last_plug"])

    # Plugging in
    def build_plug(self, parent):
        frame = section(parent, "When something is plugged in")
        ttk.Label(frame, text="A ripple runs across the panel from the port you plug something into, and the "
                  "picture drains out through the port when you unplug it. The panel goes back to what it was "
                  "showing after about two seconds.", foreground=HINT, wraplength=640,
                  justify="left").pack(anchor="w")
        self.flag(frame, "plug_animation", "Play the ripple and the drain", anchor="w", pady=(6, 0))
        row = ttk.Frame(frame)
        row.pack(anchor="w", pady=(4, 0))
        ttk.Label(row, text="For:").pack(side="left")
        self.plug_boxes = [
            self.flag(row, "plug_charger", "the charger", side="left", padx=(6, 10)),
            self.flag(row, "plug_headphones", "headphones (audio card)", side="left", padx=(0, 10)),
            self.flag(row, "plug_usb", "every other USB device", side="left"),
        ]
        row = ttk.Frame(frame)
        row.pack(anchor="w", pady=(8, 0))
        ttk.Label(row, text="Try it at:").pack(side="left")
        self.try_position = ttk.Combobox(row, state="readonly", width=14,
                                         values=[effects.position_label(p) for p in POSITIONS])
        self.try_position.current(POSITIONS.index("right-middle"))
        self.try_position.pack(side="left", padx=6)
        ttk.Button(row, text="Plug in", command=lambda: self.try_plug(True)).pack(side="left")
        ttk.Button(row, text="Unplug", command=lambda: self.try_plug(False)).pack(side="left", padx=6)

        last = ttk.Frame(frame)
        last.pack(anchor="w", fill="x", pady=(8, 0))
        self.last_text = ttk.Label(last, text="Nothing plugged in or out since White Rabbit started.",
                                   foreground=HINT, wraplength=640, justify="left")
        self.last_text.pack(anchor="w")
        fix = ttk.Frame(frame)
        fix.pack(anchor="w", pady=(4, 0))
        ttk.Label(fix, text="Wrong port? It was really:").pack(side="left")
        self.fix_position = ttk.Combobox(fix, state="readonly", width=14,
                                         values=[effects.position_label(p) for p in POSITIONS])
        self.fix_position.pack(side="left", padx=6)
        self.fix_button = ttk.Button(fix, text="Remember", command=self.learn)
        self.fix_button.pack(side="left")
        self.fix_button.state(["disabled"])
        ttk.Label(frame, text="White Rabbit remembers the port you pick, for that device's port from then on.",
                  foreground=HINT).pack(anchor="w", pady=(2, 0))
        teach = ttk.Frame(frame)
        teach.pack(anchor="w", pady=(8, 0))
        ttk.Button(teach, text="Teach White Rabbit my ports...", command=self.teach_ports).pack(side="left")
        ttk.Label(teach, text="  Windows doesn't say which port is which, so plug something into each one once.",
                  foreground=HINT).pack(side="left")

    def try_plug(self, plugged_in):
        position = POSITIONS[self.try_position.current()]
        self.preview.plug(position, plugged_in)
        self.ask_monitor({"action": "try", "position": position, "event": "in" if plugged_in else "out"})

    def show_last_plug(self, plug):
        self.last_plug = plug or {}
        if not plug:
            return
        what = plug.get("name") or "Something"
        verb = "plugged in" if plug.get("plugged_in") else "unplugged"
        when = time.strftime("%H:%M", time.localtime(plug.get("time", time.time())))
        where = effects.position_label(plug.get("position"))
        how = HOW.get(plug.get("how"), "")
        self.last_text.configure(text=f"Last: {what}, {verb} at {when}. Ripple came from: {where}"
                                      + (f" ({how})." if how else "."))
        if plug.get("key"):
            self.fix_button.state(["!disabled"])
            if plug.get("position") in POSITIONS:
                self.fix_position.current(POSITIONS.index(plug["position"]))

    def learn(self):
        if not self.last_plug.get("key") or self.fix_position.current() < 0:
            return
        position = POSITIONS[self.fix_position.current()]
        self.ask_monitor({"action": "learn", "key": self.last_plug["key"], "position": position})
        self.status.configure(text=f"Remembered: {effects.position_label(position)}.")

    def teach_ports(self):
        PortTeacher(self)

    # Text line
    def build_text(self, parent):
        frame = section(parent, "Text line")
        ttk.Label(frame, text="A line of small letters that scrolls across the top of both panels, over whatever "
                  "they show, then gets out of the way.", foreground=HINT, wraplength=640,
                  justify="left").pack(anchor="w")
        self.flag(frame, "text_line", "Show the text line", anchor="w", pady=(6, 0))
        row = ttk.Frame(frame)
        row.pack(anchor="w", pady=(4, 0))
        ttk.Label(row, text="Say when:").pack(side="left")
        self.text_boxes = [
            self.flag(row, "text_song", "a new song starts", side="left", padx=(6, 10)),
            self.flag(row, "text_charger", "the charger goes in or out", side="left", padx=(0, 10)),
            self.flag(row, "text_battery", "the battery is low", side="left"),
        ]
        row = ttk.Frame(frame)
        row.pack(anchor="w", pady=(6, 0))
        ttk.Label(row, text="Your own message:").pack(side="left")
        self.message = ttk.Entry(row, width=34)
        self.message.insert(0, "Back in 5 minutes")
        self.message.pack(side="left", padx=6)
        self.message.bind("<Return>", lambda e: self.say())
        ttk.Button(row, text="Show it now", command=self.say).pack(side="left")
        row = ttk.Frame(frame)
        row.pack(anchor="w", pady=(6, 0))
        ttk.Label(row, text="Scroll speed:").pack(side="left")
        self.speed = Slider(row, from_=1, to=10, orient="horizontal", length=200, showvalue=True)
        self.speed.set(self.settings["text_speed"])
        self.speed.configure(command=lambda v: self.changed())
        self.speed.pack(side="left", padx=6)

    def say(self):
        text = self.message.get().strip()
        if text:
            self.preview.text.say(text)
            self.ask_monitor({"action": "say", "text": text})

    # Changing apps and dimming
    def build_changing(self, parent):
        frame = section(parent, "Changing apps")
        self.slide_var = tk.StringVar(value="slide" if self.settings["slide"] else "cut")
        ttk.Radiobutton(frame, text="Slide the next app in from the bottom", value="slide",
                        variable=self.slide_var, command=self.slide_changed).pack(anchor="w")
        ttk.Radiobutton(frame, text="Cut straight to the next app", value="cut",
                        variable=self.slide_var, command=self.slide_changed).pack(anchor="w")

    def slide_changed(self):
        self.changed()
        if self.slide_var.get() == "slide":
            self.preview.start_slide("left")
            self.preview.start_slide("right")
            self.slide_flip = not self.slide_flip

    def build_dimming(self, parent):
        frame = section(parent, "Dimming")
        self.flag(frame, "full_shades", "Keep every shade when the panels are dim")
        ttk.Label(frame, text="Dark pictures stop losing detail at low brightness. Needs White Rabbit firmware "
                  "with dimming (update it in Settings > Firmware); older firmware ignores this.",
                  foreground=HINT, wraplength=640, justify="left").pack(anchor="w", pady=(2, 0))

    # Preview
    def build_preview(self, parent):
        frame = ttk.LabelFrame(parent, text="Preview", padding=8)
        frame.pack(side="left", fill="y", padx=12, pady=6, anchor="n")
        width = 2 * MARGIN + 2 * 9 * STEP + PANEL_GAP
        height = 2 * MARGIN + 34 * STEP
        self.canvas = tk.Canvas(frame, width=width, height=height, background="#111111", highlightthickness=0)
        self.canvas.pack()
        for p, side in enumerate(effects.SIDES):
            x0 = MARGIN + p * (9 * STEP + PANEL_GAP)
            for x in range(9):
                column = []
                for y in range(34):
                    cx, cy = x0 + x * STEP, MARGIN + y * STEP
                    column.append(self.canvas.create_oval(cx, cy, cx + DOT, cy + DOT, fill=led_color(0), width=0))
                self.leds[side].append(column)
        ttk.Label(frame, text="Left panel        Right panel", foreground=HINT).pack()
        ttk.Label(frame, text="Plays what \"Plug in\", \"Unplug\"\nand \"Show it now\" do.",
                  foreground=HINT, justify="center").pack(pady=(4, 0))
        self.update_enabled()

    def set_visible(self, visible):
        if visible == self.visible:
            return
        self.visible = visible
        if visible:
            self.ask_monitor({"action": "status"})
            self.after(FRAME_MS, self.tick)
            self.after(2000, self.poll)

    def poll(self):
        if not self.visible:
            return
        self.ask_monitor({"action": "status"})
        self.after(2000, self.poll)

    def tick(self):
        if not self.visible:
            return
        try:
            for side in effects.SIDES:
                frame = self.frames[side]
                if self.slide_flip:
                    frame = frame[:, ::-1]
                grid = self.preview.apply(side, frame)
                self.draw_panel(side, np.asarray(grid) / 255.0)
        except Exception as e:
            log.error(f"Effects preview failed: {e}")
        self.after(FRAME_MS, self.tick)

    def draw_panel(self, side, levels):
        levels = np.round(levels * 20) / 20
        last = self.shown[side]
        for x in range(9):
            for y in range(34):
                value = float(levels[x][y])
                if last is None or float(last[x][y]) != value:
                    self.canvas.itemconfigure(self.leds[side][x][y], fill=led_color(value))
        self.shown[side] = levels
