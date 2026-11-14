# The Math tab: graphs of functions and simulations (Game of Life and other rules, traffic, gas
# particles, one-dimensional automata, and more maths like Langton's Ant and sorting) played on the LED panels, with a preview in the window.
#
# With both panels ticked they act as one 18 x 34 picture; with one ticked it is 9 x 34.
# "Both panels" under the preview can instead leave out the columns hidden behind the keyboard,
# or mirror the right panel onto the left (white_rabbit/span.py).
# The maths is in white_rabbit/math_sims.py.

# Built In Dependencies
import time
import logging
import threading
import tkinter as tk
from tkinter import ttk

# External Dependencies
import numpy as np

# Internal Dependencies
from white_rabbit import math_sims as sims
from white_rabbit import math_effects as effects
from white_rabbit import span
from led_mon.theme import Slider

log = logging.getLogger(__name__)

HINT = "#555555"
LED_SIZE = 8          # preview pixels per LED
PANEL_GAP = 18        # preview pixels between the two panels
PREVIEW_MS = 50
CUSTOM_RULE = "Your own rule (type B and S)"
INSPIRED_BY = "https://github.com/mariobx/Framework-LED-Matrix"


def section(parent, title):
    frame = ttk.LabelFrame(parent, text=title, padding=8)
    frame.pack(fill="x", padx=12, pady=4)
    return frame


def number(entry, default, low=None, high=None, cast=float):
    """The number typed in an entry, or default if it isn't one."""
    try:
        value = cast(entry.get())
    except (ValueError, tk.TclError):
        return default
    if low is not None:
        value = max(low, value)
    if high is not None:
        value = min(high, value)
    return value


def frame_commands(columns):
    """The commands that show 9 columns of greyscale (each 34 values, 0-255) on a panel, all at once.

    The panel starts each frame with every column dark, so dark columns are left out.
    """
    from inputmodule.inputmodule import FWK_MAGIC, CommandVals
    commands = []
    for x in range(sims.PANEL_WIDTH):
        column = [int(v) for v in columns[:, x]]
        if any(column):
            commands.append(bytes(FWK_MAGIC + [CommandVals.StageGreyCol, x] + column))
    commands.append(bytes(FWK_MAGIC + [CommandVals.DrawGreyColBuffer, 0x00]))
    return commands


def send_frame(panel, columns):
    """Show 9 columns of greyscale (each 34 values, 0-255) on one panel, all at once."""
    from inputmodule.inputmodule import open_serial
    with open_serial(panel.device) as s:
        for command in frame_commands(columns):
            s.write(command)


def send_frames(link, panels, frame):
    """Show a frame that spans the panels (9 columns each, left to right) on all of them at once."""
    from white_rabbit import bridge
    bridge.write_many(link, {
        panel.device: frame_commands(frame[:, i * sims.PANEL_WIDTH:(i + 1) * sims.PANEL_WIDTH])
        for i, panel in enumerate(panels)})


class Preview(tk.Canvas):
    """What the panels show, drawn in the window (also works with no panels at all)."""

    def __init__(self, master):
        super().__init__(master, background="#111111", highlightthickness=0)
        self.cells = []
        self.width_leds = 0
        self.shown = None
        self.layout(2 * sims.PANEL_WIDTH)

    def layout(self, width_leds):
        self.delete("all")
        self.width_leds = width_leds
        self.shown = None
        panels = max(1, width_leds // sims.PANEL_WIDTH)
        w = panels * sims.PANEL_WIDTH * LED_SIZE + (panels - 1) * PANEL_GAP + 16
        h = sims.HEIGHT * LED_SIZE + 16
        self.configure(width=w, height=h)
        self.cells = []
        for row in range(sims.HEIGHT):
            line = []
            for col in range(width_leds):
                x = 8 + col * LED_SIZE + (col // sims.PANEL_WIDTH) * PANEL_GAP
                y = 8 + row * LED_SIZE
                line.append(self.create_rectangle(x + 1, y + 1, x + LED_SIZE - 1, y + LED_SIZE - 1,
                                                  fill="#222222", width=0))
            self.cells.append(line)

    def draw(self, frame):
        if frame.shape[1] != self.width_leds:
            self.layout(frame.shape[1])
        old = self.shown
        for row, col in zip(*np.nonzero(frame != old) if old is not None else np.indices(frame.shape).reshape(2, -1)):
            v = int(frame[row, col])
            level = 0x22 + v * (0xFF - 0x22) // 255
            self.itemconfigure(self.cells[row][col], fill=f"#{level:02x}{level:02x}{level:02x}")
        self.shown = frame.copy()


class ScrollFrame(ttk.Frame):
    """A frame with a vertical scrollbar that appears when its contents don't fit."""

    def __init__(self, master):
        super().__init__(master)
        self.canvas = tk.Canvas(self, highlightthickness=0, borderwidth=0)
        self.bar = ttk.Scrollbar(self, orient="vertical", command=self.canvas.yview)
        self.canvas.configure(yscrollcommand=self.bar.set)
        self.canvas.pack(side="left", fill="both", expand=True)
        self.inner = ttk.Frame(self.canvas)
        self.window = self.canvas.create_window(0, 0, window=self.inner, anchor="nw")
        self.inner.bind("<Configure>", lambda e: self.fit())
        self.canvas.bind("<Configure>", lambda e: (self.canvas.itemconfigure(self.window, width=e.width), self.fit()))
        self.bind_all("<MouseWheel>", self.wheel, add="+")

    def fit(self):
        self.canvas.configure(scrollregion=self.canvas.bbox("all"))
        if self.inner.winfo_reqheight() > self.canvas.winfo_height() > 1:
            if not self.bar.winfo_ismapped():
                self.bar.pack(side="right", fill="y")
        elif self.bar.winfo_ismapped():
            self.bar.pack_forget()
            self.canvas.yview_moveto(0)

    def wheel(self, event):
        # Only when the mouse is over this frame and there is something to scroll
        widget = self.winfo_containing(event.x_root, event.y_root)
        while widget is not None and widget is not self:
            widget = widget.master
        if widget is self and self.bar.winfo_ismapped():
            self.canvas.yview_scroll(-1 if event.delta > 0 else 1, "units")


class MathTab(ttk.Frame):
    """Graphs and simulations on the LED panels."""

    def __init__(self, master, window):
        super().__init__(master, padding=6)
        self.window = window
        self.run_id = 0
        self.latest = None
        self.drawn = None
        self.rng = np.random.default_rng()
        self.span = span.load()
        self.last_play = None

        bottom = ttk.Frame(self)
        bottom.pack(side="bottom", fill="x", padx=12, pady=6)
        ttk.Label(bottom, text="Speed:").pack(side="left")
        # Read by the animation thread, which mustn't touch tkinter itself
        self.fps_value = 8
        self.fps = Slider(bottom, from_=1, to=30, orient="horizontal", length=200, showvalue=True,
                            command=lambda v: setattr(self, "fps_value", int(float(v))))
        self.fps.set(self.fps_value)
        self.fps.pack(side="left", padx=4)
        ttk.Label(bottom, text="steps per second").pack(side="left")
        ttk.Button(bottom, text="Stop", command=self.stop).pack(side="left", padx=(16, 4))
        self.status = ttk.Label(bottom, text="Pick something to show. Both panels together make one wide picture.",
                                foreground=HINT)
        self.status.pack(side="left", padx=10)

        body = ttk.Frame(self)
        body.pack(fill="both", expand=True)
        right = ttk.Frame(body)
        right.pack(side="right", fill="y", padx=12)
        ttk.Label(right, text="Preview", foreground=HINT).pack(anchor="w")
        self.preview = Preview(right)
        self.preview.pack(anchor="n")
        self.preview_note = ttk.Label(right, text="", foreground=HINT, wraplength=200, justify="left")
        self.preview_note.pack(anchor="w", pady=(4, 0))
        self.build_span(right)
        # Scrolls when the window is small or Windows display scaling makes everything bigger
        scroller = ScrollFrame(body)
        scroller.pack(side="left", fill="both", expand=True)
        left = scroller.inner

        self.build_graphs(section(left, "Graphs"))
        self.build_life(section(left, "Game of Life and other Life-like rules"))
        self.build_traffic(section(left, "Traffic jam (Biham-Middleton-Levine)"))
        self.build_gas(section(left, "Gas particles bouncing in a box (Hardy-Pomeau-Pazzis)"))
        self.build_automata(section(left, "One-line automata (each new row is the next generation)"))
        self.build_effects(section(left, "More maths"))
        extra = ttk.Frame(left)
        extra.pack(fill="x", padx=12, pady=4)
        ttk.Button(extra, text="Random grey noise", command=lambda: self.play(
            "Random grey noise", lambda w: sims.noise_frames(self.rng, w))).pack(side="left")
        ttk.Label(extra, text="Ideas from mariobx's Framework-LED-Matrix project.", foreground=HINT
                  ).pack(side="right")

        self.after(PREVIEW_MS, self.draw_preview)

    # Both panels: side by side, keyboard gap or mirror
    def build_span(self, parent):
        frame = ttk.LabelFrame(parent, text="Both panels", padding=6)
        frame.pack(anchor="w", fill="x", pady=(10, 0))
        labels = [label for _, label in span.MODES]
        keys = [key for key, _ in span.MODES]
        self.span_mode = ttk.Combobox(frame, values=labels, state="readonly", width=28)
        self.span_mode.current(keys.index(self.span["mode"]))
        self.span_mode.pack(anchor="w")
        self.span_mode.bind("<<ComboboxSelected>>", lambda e: self.span_changed())
        gap_row = ttk.Frame(frame)
        gap_row.pack(anchor="w", pady=(6, 0))
        ttk.Label(gap_row, text="Keyboard gap:").pack(side="left")
        self.gap_var = tk.StringVar(value=str(self.span["gap_columns"]))
        self.gap = ttk.Spinbox(gap_row, from_=span.MIN_GAP, to=span.MAX_GAP, width=5, textvariable=self.gap_var,
                               command=self.span_changed)
        self.gap.pack(side="left", padx=4)
        self.gap.bind("<Return>", lambda e: self.span_changed())
        self.gap.bind("<FocusOut>", lambda e: self.span_changed())
        ttk.Label(gap_row, text="columns").pack(side="left")
        self.span_hint = ttk.Label(frame, text="", foreground=HINT, wraplength=230, justify="left")
        self.span_hint.pack(anchor="w", pady=(4, 0))
        self.gap_test = ttk.Button(frame, text="Show the gap test", command=lambda: self.play(
            "Gap test: a dot going across both panels", span.gap_test_frames))
        self.gap_test.pack(anchor="w", pady=(4, 0))
        # The Games tab can change the choice too
        frame.bind("<Map>", lambda e: self.reload_span())
        self.update_span_hint()

    def reload_span(self):
        saved = span.load()
        if saved == self.span:
            return
        self.span = saved
        self.span_mode.current([key for key, _ in span.MODES].index(saved["mode"]))
        self.gap_var.set(str(saved["gap_columns"]))
        self.update_span_hint()

    def update_span_hint(self):
        mode = self.span["mode"]
        self.gap.state(["!disabled"] if mode == span.KEYBOARD_GAP else ["disabled"])
        self.gap_test.state(["!disabled"] if mode == span.KEYBOARD_GAP else ["disabled"])
        if mode == span.KEYBOARD_GAP:
            text = ("The picture carries on behind the keyboard, so things line up when they cross it. "
                    "Use the gap test: if the dot comes out on the other side too soon, make the gap "
                    "bigger; if it takes too long, make it smaller.")
        elif mode == span.MIRROR:
            text = "The right panel shows the picture; the left panel shows it mirrored."
        else:
            text = "The two panels make one picture 18 lights wide, as if they touched."
        self.span_hint.configure(text=text)

    def span_changed(self):
        keys = [key for key, _ in span.MODES]
        new = span.normalize({"mode": keys[max(0, self.span_mode.current())],
                              "gap_columns": number(self.gap, self.span["gap_columns"], span.MIN_GAP,
                                                    span.MAX_GAP, int)})
        self.gap_var.set(str(new["gap_columns"]))
        if new == self.span:
            return
        self.span = new
        span.save(new)
        self.update_span_hint()
        # Start whatever is showing again with the new layout
        if self.last_play is not None and self.still_running(self.run_id):
            self.play(*self.last_play)

    # Sections
    def build_graphs(self, frame):
        ttk.Label(frame, text="Graph:").grid(row=0, column=0, sticky="w")
        self.graph = ttk.Combobox(frame, values=sims.GRAPH_NAMES, state="readonly", width=22)
        self.graph.current(0)
        self.graph.grid(row=0, column=1, sticky="w", padx=4)
        self.graph.bind("<<ComboboxSelected>>", lambda e: self.show_graph())
        ttk.Button(frame, text="Show", command=self.show_graph).grid(row=0, column=2, padx=2)
        ttk.Button(frame, text="Random graphs", command=self.random_graphs).grid(row=0, column=3, padx=2)
        self.axes = tk.BooleanVar(value=True)
        ttk.Checkbutton(frame, text="Show the axes (dim)", variable=self.axes).grid(row=3, column=0, columnspan=5,
                                                                                    sticky="w", pady=(4, 0))

        ttk.Label(frame, text="Your own: y =").grid(row=1, column=0, sticky="w", pady=(6, 0))
        own = ttk.Frame(frame)
        own.grid(row=1, column=1, columnspan=4, sticky="w", pady=(6, 0))
        self.formula = ttk.Entry(own, width=18)
        self.formula.insert(0, "sin(x) * x")
        self.formula.pack(side="left", padx=4)
        ttk.Button(own, text="Show", command=self.show_formula).pack(side="left", padx=(0, 8))
        self.formula.bind("<Return>", lambda e: self.show_formula())
        ttk.Label(own, text="x from").pack(side="left")
        self.x_from = ttk.Entry(own, width=5)
        self.x_from.insert(0, "-10")
        self.x_from.pack(side="left", padx=2)
        ttk.Label(own, text="to").pack(side="left")
        self.x_to = ttk.Entry(own, width=5)
        self.x_to.insert(0, "10")
        self.x_to.pack(side="left", padx=2)
        ttk.Label(frame, text="x runs down the panels and y across. Use + - * / ^, sin, cos, tan, sqrt, exp, log, "
                  "abs, pi... Put t in it to make it move, like sin(x - t).", foreground=HINT, wraplength=720,
                  justify="left").grid(row=2, column=0, columnspan=5, sticky="w", pady=(4, 0))

    def build_life(self, frame):
        ttk.Label(frame, text="Rule:").grid(row=0, column=0, sticky="w")
        self.rule = ttk.Combobox(frame, values=sims.LIFE_RULE_NAMES + [CUSTOM_RULE], state="readonly", width=36)
        self.rule.current(0)
        self.rule.grid(row=0, column=1, sticky="w", padx=4)
        self.rule.bind("<<ComboboxSelected>>", lambda e: self.rule_changed())
        ttk.Button(frame, text="Start", command=self.start_life).grid(row=0, column=2, sticky="w", padx=8)
        custom = ttk.Frame(frame)
        custom.grid(row=2, column=1, columnspan=2, sticky="w", padx=4, pady=(4, 0))
        ttk.Label(custom, text="Born with").pack(side="left")
        self.birth = ttk.Entry(custom, width=6)
        self.birth.pack(side="left", padx=2)
        ttk.Label(custom, text="neighbours, survives with").pack(side="left")
        self.survive = ttk.Entry(custom, width=8)
        self.survive.pack(side="left", padx=2)
        ttk.Label(frame, text="Start from:").grid(row=1, column=0, sticky="w", pady=(4, 0))
        self.life_start = ttk.Combobox(frame, values=sims.STARTS, state="readonly", width=22)
        self.life_start.current(0)
        self.life_start.grid(row=1, column=1, sticky="w", padx=4, pady=(4, 0))
        self.rule_changed()

    def build_traffic(self, frame):
        row = ttk.Frame(frame)
        row.pack(fill="x")
        ttk.Button(row, text="Start", command=self.start_traffic).pack(side="left")
        ttk.Label(row, text="Roads:").pack(side="left", padx=(12, 0))
        self.traffic_density = Slider(row, from_=5, to=80, orient="horizontal", length=140, showvalue=True)
        self.traffic_density.set(35)
        self.traffic_density.pack(side="left", padx=4)
        ttk.Label(row, text="% full").pack(side="left")
        ttk.Label(frame, text="Cars going across (bright) and down (dim) take turns to move. "
                  "Busy roads jam up for good.", foreground=HINT, wraplength=640, justify="left"
                  ).pack(anchor="w", pady=(4, 0))

    def build_gas(self, frame):
        ttk.Button(frame, text="Start", command=self.start_gas).pack(side="left")
        ttk.Label(frame, text="Begin with:").pack(side="left", padx=(12, 0))
        self.gas_start = ttk.Combobox(frame, values=sims.HPP_STARTS, state="readonly", width=16)
        self.gas_start.current(0)
        self.gas_start.pack(side="left", padx=4)
        ttk.Label(frame, text="Amount:").pack(side="left", padx=(12, 0))
        self.gas_density = Slider(frame, from_=5, to=100, orient="horizontal", length=140, showvalue=True)
        self.gas_density.set(30)
        self.gas_density.pack(side="left", padx=4)

    def build_automata(self, frame):
        ttk.Button(frame, text="Start", command=self.start_automaton).pack(side="left")
        self.kind = ttk.Combobox(frame, values=sims.AUTOMATA_KINDS, state="readonly", width=32)
        self.kind.current(0)
        self.kind.pack(side="left", padx=(12, 0))
        self.kind.bind("<<ComboboxSelected>>", lambda e: self.kind_changed())
        ttk.Label(frame, text="Number:").pack(side="left", padx=(12, 0))
        self.rule_number = tk.Spinbox(frame, from_=0, to=255, width=6)
        self.rule_number.pack(side="left", padx=4)
        ttk.Label(frame, text="Start:").pack(side="left", padx=(12, 0))
        self.automaton_start = ttk.Combobox(frame, values=sims.AUTOMATA_STARTS, state="readonly", width=12)
        self.automaton_start.current(0)
        self.automaton_start.pack(side="left", padx=4)
        self.kind_changed()

    def build_effects(self, frame):
        self.effect = ttk.Combobox(frame, values=effects.EFFECT_NAMES, state="readonly", width=30)
        self.effect.current(0)
        self.effect.grid(row=0, column=0, sticky="w")
        self.effect.bind("<<ComboboxSelected>>", lambda e: self.effect_changed())
        ttk.Button(frame, text="Start", command=self.start_effect).grid(row=0, column=1, sticky="w", padx=8)
        self.effect_about = ttk.Label(frame, text="", foreground=HINT, wraplength=640, justify="left")
        self.effect_about.grid(row=1, column=0, columnspan=2, sticky="w", pady=(4, 0))
        self.effect_changed()

    def effect_changed(self):
        self.effect_about.configure(text=effects.EFFECTS[self.effect.current()][3])

    def rule_changed(self):
        name = self.rule.get()
        custom = name == CUSTOM_RULE
        for entry in (self.birth, self.survive):
            entry.state(["!disabled"] if custom else ["disabled"])
        if not custom:
            _, birth, survive = sims.LIFE_RULES[sims.LIFE_RULE_NAMES.index(name)]
            for entry, value in ((self.birth, birth), (self.survive, survive)):
                entry.state(["!disabled"])
                entry.delete(0, "end")
                entry.insert(0, value)
                entry.state(["disabled"])

    def kind_changed(self):
        totalistic = self.kind.get() == sims.AUTOMATA_KINDS[1]
        self.rule_number.configure(to=2186 if totalistic else 255)
        self.rule_number.delete(0, "end")
        self.rule_number.insert(0, "777" if totalistic else "30")

    # Actions
    def show(self, text):
        self.status.configure(text=text)

    def show_graph(self):
        name, formula, x_from, x_to, y_range = sims.graph_by_name(self.graph.get())
        axes = self.axes.get()
        self.play(f"{name}: y = {formula}",
                  lambda w: sims.graph_frames(formula, x_from, x_to, y_range, w, axes))

    def show_formula(self):
        formula = self.formula.get()
        try:
            sims.compile_formula(formula)
        except sims.FormulaError as e:
            self.show(str(e))
            return
        x_from, x_to = number(self.x_from, -10.0), number(self.x_to, 10.0)
        if x_from >= x_to:
            self.show("The 'x from' number has to be smaller than the 'to' number.")
            return
        axes = self.axes.get()
        self.play(f"y = {formula}", lambda w: sims.graph_frames(formula, x_from, x_to, None, w, axes))

    def random_graphs(self):
        axes = self.axes.get()
        self.play("Random graphs", lambda w: sims.random_graphs_frames(
            self.rng, w, frames_each=max(8, self.fps_value * 3), axes=axes, on_change=self.show_soon))

    def start_life(self):
        try:
            birth, survive = sims.parse_counts(self.birth.get()), sims.parse_counts(self.survive.get())
        except ValueError as e:
            self.show(str(e))
            return
        if not birth:
            self.show("Type at least one number for 'Born with', or nothing new will ever appear.")
            return
        start = self.life_start.get()
        name = self.rule.get() if self.rule.get() != CUSTOM_RULE else \
            f"B{''.join(map(str, sorted(birth)))}/S{''.join(map(str, sorted(survive)))}"
        self.play(name, lambda w: sims.life_frames(self.rng, w, birth, survive, start, on_change=self.show_soon))

    def start_traffic(self):
        density = self.traffic_density.get() / 100
        self.play("Traffic", lambda w: sims.bml_frames(self.rng, w, density))

    def start_gas(self):
        start, density = self.gas_start.get(), self.gas_density.get() / 100
        self.play("Gas particles", lambda w: sims.hpp_frames(self.rng, w, start, density, on_change=self.show_soon))

    def start_automaton(self):
        kind = self.kind.get()
        top = 2186 if kind == sims.AUTOMATA_KINDS[1] else 255
        value = number(self.rule_number, None, cast=int)
        if value is None or not 0 <= value <= top:
            self.show(f"Pick a number from 0 to {top}.")
            return
        start = self.automaton_start.get()
        label = f"Rule {value}" if top == 255 else f"Code {value}"
        self.play(label, lambda w: sims.automaton_frames(self.rng, w, kind, value, start))

    def start_effect(self):
        name, make, speed, _ = effects.EFFECTS[self.effect.current()]
        # Each one has a speed it looks best at; the slider still changes it afterwards
        self.fps.set(speed)
        self.fps_value = speed
        if make is effects.sorting_frames:
            self.play(name, lambda w: make(self.rng, w, on_change=self.show_soon))
        else:
            self.play(name, lambda w: make(self.rng, w))

    def show_soon(self, text):
        """For messages from the animation thread."""
        self.window.call_soon(lambda: self.show(text))

    # Running
    def play(self, title, make_frames):
        """Start an animation on the ticked panels (or just the preview if none is ticked)."""
        from inputmodule.gui.gui_threading import reset_thread, set_status
        panels = self.window.selected_panels()
        count = len(panels) if panels else 2
        layout = dict(self.span)
        frames = (span.to_panels(picture, count, layout) for picture in make_frames(span.picture_width(count, layout)))
        self.last_play = (title, make_frames)
        self.run_id += 1
        run_id = self.run_id
        # Ends a clock or equalizer from the other tab, and any earlier animation from this one
        reset_thread()
        set_status("math")
        where = " and ".join(p.called for p in panels)
        self.show(f"{title}, on the {where}." if panels else title)
        self.preview_note.configure(text="" if panels else "No panel is ticked, so this only shows here.")
        threading.Thread(target=self.run, args=(run_id, panels, frames), daemon=True).start()

    def still_running(self, run_id):
        from inputmodule.gui.gui_threading import get_status, is_thread_stopped
        return run_id == self.run_id and get_status() == "math" and not is_thread_stopped()

    def run(self, run_id, panels, frames):
        from inputmodule.gui.gui_threading import is_thread_stopped, reset_thread
        from inputmodule.inputmodule.ledmatrix import animate
        try:
            if panels:
                self.window.lender.borrow(panels)
                self.window.call_soon(self.window.update_bar)
                for panel in panels:
                    animate(panel, False)
            next_at = time.monotonic()
            while self.still_running(run_id):
                frame = next(frames)
                if panels:
                    send_frames(self.window.link, panels, frame)
                # Shown in the window once it's on the panels, so the window doesn't run ahead of them
                self.latest = frame
                next_at += 1.0 / max(1, self.fps_value)
                pause = next_at - time.monotonic()
                if pause > 0:
                    time.sleep(pause)
                else:
                    next_at = time.monotonic()
        except Exception as e:
            log.exception("The Math tab animation stopped")
            if run_id == self.run_id:
                self.show_soon(f"Stopped: {e}")
        finally:
            if is_thread_stopped() and run_id == self.run_id:
                reset_thread()

    def stop(self):
        self.run_id += 1
        panels = self.window.selected_panels()
        if any(self.window.lender.is_borrowed(p) for p in panels):
            self.window.give_back(panels)
            self.show("Stopped. The system monitor has the panels again.")
        else:
            self.show("Stopped.")

    def draw_preview(self):
        frame = self.latest
        if frame is not None and frame is not self.drawn:
            self.drawn = frame
            self.preview.draw(frame)
        self.after(PREVIEW_MS, self.draw_preview)
