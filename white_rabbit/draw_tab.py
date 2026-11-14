# The Draw tab: draw on one panel's 9 x 34 LEDs in shades of grey, make an animation out of
# several frames, import a PNG or GIF, and save drawings for the Picture app to show on a panel.
#
# The idea of a drawing editor comes from Deo's LED Matrix Manager (MIT licence,
# github.com/Deovandski/Deo-s-Framework16-LED-Matrix-Manager); this is our own code.
# The files and the import are in led_mon/drawings.py, so the monitor can play them too.

# Built In Dependencies
import time
import logging
import tkinter as tk
from tkinter import ttk, filedialog, messagebox

# External Dependencies
import numpy as np

# Internal Dependencies
from led_mon import drawings, platform_utils

log = logging.getLogger(__name__)

HINT = "#555555"
LED = 16              # editor pixels per LED
THUMB = 3             # frame list pixels per LED
UNDO_STEPS = 50
# The brush shades offered as swatches (the slider picks any other)
SHADES = (255, 192, 128, 80, 40, 16)
TOOLS = (("pen", "Pen"), ("eraser", "Eraser"), ("fill", "Fill area"))
SIZES = (1, 2, 3)
INSPIRED_BY = "https://github.com/Deovandski/Deo-s-Framework16-LED-Matrix-Manager"


def led_colour(value):
    """How an LED of this brightness (0-255) looks in the window: off LEDs stay faintly visible."""
    level = 0x22 + int(value) * (0xFF - 0x22) // 255
    return f"#{level:02x}{level:02x}{level:02x}"


def flood_fill(frame, row, col, value):
    """Fill the area of the same brightness around (row, col), like a paint bucket."""
    target = frame[row, col]
    if target == value:
        return frame
    out = frame.copy()
    stack = [(row, col)]
    while stack:
        r, c = stack.pop()
        if 0 <= r < drawings.HEIGHT and 0 <= c < drawings.WIDTH and out[r, c] == target:
            out[r, c] = value
            stack.extend(((r + 1, c), (r - 1, c), (r, c + 1), (r, c - 1)))
    return out


def section(parent, title):
    frame = ttk.LabelFrame(parent, text=title, padding=8)
    frame.pack(fill="x", pady=(0, 8))
    return frame


class DrawTab(ttk.Frame):
    def __init__(self, master, window):
        super().__init__(master, padding=10)
        self.window = window
        self.drawing = drawings.Drawing()
        self.index = 0
        self.name = ""
        self.saved_state = self.state_key()
        self.undo_stack = []
        self.redo_stack = []
        self.last_cell = None
        self.stroke_open = False
        self.playing = False
        self.play_started = 0.0
        self.push_pending = False
        self.next_push = None
        # The panels this tab is showing on, while 'Show on the ticked panels' is on
        self.live_panels = []

        self.tool = tk.StringVar(value="pen")
        self.size = tk.IntVar(value=1)
        self.shade = tk.IntVar(value=255)
        self.live = tk.BooleanVar(value=False)
        self.fit = tk.StringVar(value=drawings.FIT_LABELS[drawings.FIT_WHOLE])
        self.invert = tk.BooleanVar(value=False)

        self.status = ttk.Label(self, text="Draw with the left mouse button; the right button rubs out.",
                                foreground=HINT)
        self.status.pack(side="bottom", anchor="w", pady=(6, 0))

        tools = ttk.Frame(self, width=250)
        tools.pack(side="left", fill="y")
        middle = ttk.Frame(self)
        middle.pack(side="left", fill="y", padx=16)
        right = ttk.Frame(self)
        right.pack(side="left", fill="both", expand=True)

        self.build_tools(tools)
        self.build_editor(middle)
        self.build_frames(right)
        bottom = ttk.Frame(right)
        bottom.pack(fill="both", expand=True)
        left_col = ttk.Frame(bottom)
        left_col.pack(side="left", fill="y", padx=(0, 8))
        right_col = ttk.Frame(bottom)
        right_col.pack(side="left", fill="both", expand=True)
        self.build_import(left_col)
        self.build_panels(left_col)
        self.build_saved(right_col)

        for key, fn in (("<Control-z>", self.undo), ("<Control-y>", self.redo)):
            self.bind_all(key, lambda e, fn=fn: self.shortcut(e, fn), add="+")
        self.refresh_saved()
        self.redraw()
        self.after(50, self.tick)

    # Building the tab
    def build_tools(self, parent):
        brush = section(parent, "Brush")
        for key, label in TOOLS:
            ttk.Radiobutton(brush, text=label, value=key, variable=self.tool).pack(anchor="w")
        ttk.Label(brush, text="Size").pack(anchor="w", pady=(8, 2))
        sizes = ttk.Frame(brush)
        sizes.pack(anchor="w")
        for s in SIZES:
            ttk.Radiobutton(sizes, text=f"{s} x {s}", value=s, variable=self.size).pack(side="left", padx=(0, 6))

        shades = section(parent, "Brightness")
        self.swatches = tk.Canvas(shades, width=6 * 34, height=34, highlightthickness=0, background="#111111")
        self.swatches.pack(anchor="w")
        for i, value in enumerate(SHADES):
            x = i * 34
            self.swatches.create_rectangle(x + 4, 4, x + 30, 30, fill=led_colour(value), width=0, tags=(f"s{value}",))
            self.swatches.tag_bind(f"s{value}", "<Button-1>", lambda e, v=value: self.set_shade(v))
        self.swatch_ring = self.swatches.create_rectangle(0, 0, 0, 0, outline="#4ea1ff", width=2)
        self.shade_scale = tk.Scale(shades, from_=1, to=255, orient="horizontal", length=200, showvalue=True,
                                    variable=self.shade, command=lambda v: self.show_swatch())
        self.shade_scale.pack(anchor="w", pady=(4, 0))
        self.show_swatch()

        picture = section(parent, "Whole frame")
        grid = ttk.Frame(picture)
        grid.pack(anchor="w")
        buttons = [("Clear", self.clear), ("Fill", self.fill_all), ("Swap dark/light", self.invert_frame),
                   ("Mirror", lambda: self.transform(lambda f: f[:, ::-1])),
                   ("Upside down", lambda: self.transform(lambda f: f[::-1, :])), ("Undo", self.undo)]
        for i, (label, fn) in enumerate(buttons):
            ttk.Button(grid, text=label, command=fn, width=14).grid(row=i // 2, column=i % 2, padx=2, pady=2, sticky="w")
        ttk.Label(picture, text="Move it").pack(anchor="w", pady=(6, 2))
        arrows = ttk.Frame(picture)
        arrows.pack(anchor="w")
        for col, (label, dy, dx) in enumerate((("Up", -1, 0), ("Down", 1, 0), ("Left", 0, -1), ("Right", 0, 1))):
            ttk.Button(arrows, text=label, width=6,
                       command=lambda dy=dy, dx=dx: self.transform(lambda f: np.roll(f, (dy, dx), axis=(0, 1)))
                       ).grid(row=0, column=col, padx=1)

    def build_editor(self, parent):
        w = drawings.WIDTH * LED + 12
        h = drawings.HEIGHT * LED + 12
        # A fixed size, so a long name doesn't push the rest of the tab around
        self.frame_label = ttk.Label(parent, text="", foreground=HINT, width=1, wraplength=w)
        self.frame_label.pack(anchor="w", fill="x")
        self.canvas = tk.Canvas(parent, width=w, height=h, background="#111111", highlightthickness=0, cursor="pencil")
        self.canvas.pack(anchor="n", pady=(4, 0))
        self.cells = []
        for r in range(drawings.HEIGHT):
            line = []
            for c in range(drawings.WIDTH):
                x, y = 6 + c * LED, 6 + r * LED
                line.append(self.canvas.create_rectangle(x + 1, y + 1, x + LED - 1, y + LED - 1, width=0,
                                                         fill=led_colour(0)))
            self.cells.append(line)
        for button, erase in (("1", False), ("3", True)):
            self.canvas.bind(f"<Button-{button}>", lambda e, erase=erase: self.press(e, erase))
            self.canvas.bind(f"<B{button}-Motion>", lambda e, erase=erase: self.drag(e, erase))
            self.canvas.bind(f"<ButtonRelease-{button}>", lambda e: self.release())

    def build_frames(self, parent):
        frames = section(parent, "Frames (more than one makes an animation)")
        self.strip = tk.Canvas(frames, height=drawings.HEIGHT * THUMB + 26, background="#1b1b1b", highlightthickness=0)
        self.strip.pack(fill="x")
        scroll = ttk.Scrollbar(frames, orient="horizontal", command=self.strip.xview)
        scroll.pack(fill="x")
        self.strip.configure(xscrollcommand=scroll.set)
        self.strip.bind("<Button-1>", self.strip_click)
        row = ttk.Frame(frames)
        row.pack(fill="x", pady=(6, 0))
        for label, fn in (("Add blank", self.add_blank), ("Copy this frame", self.duplicate), ("Delete", self.delete_frame),
                          ("Move earlier", lambda: self.move_frame(-1)), ("Move later", lambda: self.move_frame(1))):
            ttk.Button(row, text=label, command=fn).pack(side="left", padx=(0, 4))
        row2 = ttk.Frame(frames)
        row2.pack(fill="x", pady=(6, 0))
        ttk.Label(row2, text="Each frame shows for").pack(side="left")
        self.delay = tk.Spinbox(row2, from_=drawings.MIN_DELAY_MS, to=drawings.MAX_DELAY_MS, increment=10, width=6,
                                command=self.delay_changed)
        self.delay.pack(side="left", padx=4)
        self.delay.bind("<FocusOut>", lambda e: self.delay_changed())
        self.delay.bind("<Return>", lambda e: self.delay_changed())
        ttk.Label(row2, text="milliseconds").pack(side="left")
        self.play_button = ttk.Button(row2, text="Play", command=self.toggle_play, width=8)
        self.play_button.pack(side="left", padx=(16, 0))

    def build_import(self, parent):
        box = section(parent, "Import a picture")
        ttk.Button(box, text="Import a PNG or GIF...", command=self.import_picture).pack(anchor="w")
        ttk.Label(box, text="Shape").pack(anchor="w", pady=(8, 2))
        ttk.Combobox(box, textvariable=self.fit, state="readonly", width=30,
                     values=[drawings.FIT_LABELS[f] for f in drawings.FITS]).pack(anchor="w")
        ttk.Checkbutton(box, text="Swap dark and light (for dark drawings on white)", variable=self.invert).pack(anchor="w", pady=(6, 0))
        ttk.Label(box, text="The picture is shrunk to 9 x 34 LEDs and turned grey. A GIF keeps all its frames "
                  "and its speed. It replaces what is in the editor (Undo brings it back).",
                  foreground=HINT, wraplength=330, justify="left").pack(anchor="w", pady=(6, 0))

    def build_panels(self, parent):
        box = section(parent, "On the panels")
        ttk.Checkbutton(box, text="Show on the ticked panels while I draw", variable=self.live,
                        command=self.live_changed).pack(anchor="w")
        ttk.Label(box, text="To keep a drawing on a panel, save it, then put the Picture app on the panel "
                  "in the Panels tab and pick the drawing there.",
                  foreground=HINT, wraplength=330, justify="left").pack(anchor="w", pady=(6, 0))

    def build_saved(self, parent):
        box = ttk.LabelFrame(parent, text="My drawings", padding=8)
        box.pack(fill="both", expand=True, pady=(0, 8))
        name_row = ttk.Frame(box)
        name_row.pack(fill="x")
        ttk.Label(name_row, text="Name").pack(side="left")
        self.name_entry = ttk.Entry(name_row, width=24)
        self.name_entry.pack(side="left", padx=6, fill="x", expand=True)
        ttk.Button(name_row, text="Save", command=self.save).pack(side="left")
        list_row = ttk.Frame(box)
        list_row.pack(fill="both", expand=True, pady=(6, 0))
        self.saved_list = tk.Listbox(list_row, height=8, activestyle="none", exportselection=False)
        self.saved_list.pack(side="left", fill="both", expand=True)
        bar = ttk.Scrollbar(list_row, orient="vertical", command=self.saved_list.yview)
        bar.pack(side="left", fill="y")
        self.saved_list.configure(yscrollcommand=bar.set)
        self.saved_list.bind("<Double-Button-1>", lambda e: self.open_selected())
        buttons = ttk.Frame(box)
        buttons.pack(fill="x", pady=(6, 0))
        for label, fn in (("Open", self.open_selected), ("New", self.new_drawing), ("Delete", self.delete_selected),
                          ("Show folder", self.show_folder)):
            ttk.Button(buttons, text=label, command=fn).pack(side="left", padx=(0, 4))

    # Showing things
    def shortcut(self, event, fn):
        # Typing in the name or delay box keeps its own undo
        if self.is_showing() and not isinstance(event.widget, (tk.Entry, ttk.Entry, tk.Spinbox)):
            fn()

    def is_showing(self):
        try:
            return self.window.notebook.select() == str(self)
        except Exception:
            return False

    def show(self, text):
        self.status.configure(text=text)

    def set_shade(self, value):
        self.shade.set(value)
        self.show_swatch()
        if self.tool.get() == "eraser":
            self.tool.set("pen")

    def show_swatch(self):
        value = self.shade.get()
        if value in SHADES:
            x = SHADES.index(value) * 34
            self.swatches.coords(self.swatch_ring, x + 2, 2, x + 32, 32)
        else:
            self.swatches.coords(self.swatch_ring, -10, -10, -10, -10)

    def current(self):
        return self.drawing.frames[self.index]

    def redraw(self, frame=None):
        frame = self.current() if frame is None else frame
        for r in range(drawings.HEIGHT):
            for c in range(drawings.WIDTH):
                self.canvas.itemconfigure(self.cells[r][c], fill=led_colour(frame[r, c]))
        count = len(self.drawing.frames)
        if not self.name:
            name = "Not saved yet"
        else:
            name = self.name + (" *" if self.state_key() != self.saved_state else "")
        self.frame_label.configure(text=f"{name}\nFrame {self.index + 1} of {count}")
        self.draw_strip()
        self.push_soon()

    def draw_strip(self):
        from PIL import Image, ImageTk
        self.strip.delete("all")
        self.thumbs = []
        step = drawings.WIDTH * THUMB + 10
        for i, frame in enumerate(self.drawing.frames):
            shown = (0x22 + frame.astype(np.int32) * (0xFF - 0x22) // 255).astype(np.uint8)
            image = Image.fromarray(shown, "L").resize((drawings.WIDTH * THUMB, drawings.HEIGHT * THUMB), Image.NEAREST)
            photo = ImageTk.PhotoImage(image)
            self.thumbs.append(photo)
            x = 8 + i * step
            if i == self.index:
                self.strip.create_rectangle(x - 3, 3, x + drawings.WIDTH * THUMB + 2, 6 + drawings.HEIGHT * THUMB,
                                            outline="#4ea1ff", width=2)
            self.strip.create_image(x, 5, image=photo, anchor="nw")
            self.strip.create_text(x + drawings.WIDTH * THUMB // 2, 14 + drawings.HEIGHT * THUMB, text=str(i + 1),
                                   fill="#bbbbbb", font=("Segoe UI", 8))
        self.strip.configure(scrollregion=(0, 0, 16 + len(self.drawing.frames) * step, 1))

    def strip_click(self, event):
        x = self.strip.canvasx(event.x) - 8
        i = int(x // (drawings.WIDTH * THUMB + 10))
        if 0 <= i < len(self.drawing.frames):
            self.stop_playing()
            self.index = i
            self.redraw()

    # Undo
    def state_key(self):
        return (self.drawing.delay_ms, tuple(f.tobytes() for f in self.drawing.frames))

    def remember(self):
        self.undo_stack.append(([f.copy() for f in self.drawing.frames], self.index, self.drawing.delay_ms))
        del self.undo_stack[:-UNDO_STEPS]
        self.redo_stack.clear()

    def restore(self, source, target):
        if not source:
            return
        target.append(([f.copy() for f in self.drawing.frames], self.index, self.drawing.delay_ms))
        frames, index, delay = source.pop()
        self.drawing = drawings.Drawing(frames, delay)
        self.index = min(index, len(frames) - 1)
        self.show_delay()
        self.redraw()

    def undo(self):
        self.restore(self.undo_stack, self.redo_stack)

    def redo(self):
        self.restore(self.redo_stack, self.undo_stack)

    # Drawing with the mouse
    def cell_at(self, event):
        c = (event.x - 6) // LED
        r = (event.y - 6) // LED
        if 0 <= r < drawings.HEIGHT and 0 <= c < drawings.WIDTH:
            return int(r), int(c)
        return None

    def press(self, event, erase):
        self.stop_playing()
        cell = self.cell_at(event)
        if cell is None:
            return
        self.remember()
        self.stroke_open = True
        if self.tool.get() == "fill" and not erase:
            self.drawing.frames[self.index] = flood_fill(self.current(), *cell, self.shade.get())
            self.redraw()
            return
        self.last_cell = cell
        self.paint(cell, erase)

    def drag(self, event, erase):
        if not self.stroke_open or self.tool.get() == "fill" and not erase:
            return
        cell = self.cell_at(event)
        if cell is None or cell == self.last_cell:
            return
        # Fill in between, so quick strokes don't leave gaps
        (r0, c0), (r1, c1) = self.last_cell or cell, cell
        steps = max(abs(r1 - r0), abs(c1 - c0), 1)
        for i in range(1, steps + 1):
            self.paint((round(r0 + (r1 - r0) * i / steps), round(c0 + (c1 - c0) * i / steps)), erase)
        self.last_cell = cell

    def release(self):
        self.stroke_open = False
        self.last_cell = None
        self.redraw()

    def paint(self, cell, erase):
        value = 0 if erase or self.tool.get() == "eraser" else self.shade.get()
        size = self.size.get()
        r, c = cell
        frame = self.current()
        top, left = r - (size - 1) // 2, c - (size - 1) // 2
        for rr in range(max(0, top), min(drawings.HEIGHT, top + size)):
            for cc in range(max(0, left), min(drawings.WIDTH, left + size)):
                frame[rr, cc] = value
                self.canvas.itemconfigure(self.cells[rr][cc], fill=led_colour(value))
        self.push_soon()

    # Whole-frame buttons
    def transform(self, fn):
        self.stop_playing()
        self.remember()
        self.drawing.frames[self.index] = np.ascontiguousarray(fn(self.current())).astype(np.uint8)
        self.redraw()

    def clear(self):
        self.transform(lambda f: np.zeros_like(f))

    def fill_all(self):
        self.transform(lambda f: np.full_like(f, self.shade.get()))

    def invert_frame(self):
        self.transform(lambda f: 255 - f)

    # Frames
    def add_blank(self):
        self.change_frames(lambda frames, i: (frames[:i + 1] + [drawings.blank_frame()] + frames[i + 1:], i + 1))

    def duplicate(self):
        self.change_frames(lambda frames, i: (frames[:i + 1] + [frames[i].copy()] + frames[i + 1:], i + 1))

    def delete_frame(self):
        if len(self.drawing.frames) == 1:
            self.clear()
            return
        self.change_frames(lambda frames, i: (frames[:i] + frames[i + 1:], min(i, len(frames) - 2)))

    def move_frame(self, step):
        def move(frames, i):
            j = i + step
            if not 0 <= j < len(frames):
                return frames, i
            frames = list(frames)
            frames[i], frames[j] = frames[j], frames[i]
            return frames, j
        self.change_frames(move)

    def change_frames(self, fn):
        self.stop_playing()
        self.remember()
        frames, index = fn(list(self.drawing.frames), self.index)
        if len(frames) > drawings.MAX_FRAMES:
            self.undo_stack.pop()
            self.show(f"A drawing can have up to {drawings.MAX_FRAMES} frames.")
            return
        self.drawing.frames = frames
        self.index = index
        self.redraw()

    def show_delay(self):
        self.delay.delete(0, "end")
        self.delay.insert(0, str(self.drawing.delay_ms))

    def delay_changed(self):
        value = drawings.clamp_delay(self.delay.get())
        if value != self.drawing.delay_ms:
            self.remember()
            self.drawing.delay_ms = value
        self.show_delay()
        self.redraw()

    def toggle_play(self):
        if self.playing:
            self.stop_playing()
        elif len(self.drawing.frames) > 1:
            self.playing = True
            self.play_started = time.monotonic()
            self.play_button.configure(text="Stop")
        else:
            self.show("Add a second frame to make an animation.")

    def stop_playing(self):
        if self.playing:
            self.playing = False
            self.play_button.configure(text="Play")
            self.redraw()

    def tick(self):
        if self.live.get() and self.live_panels and not self.push_pending and \
                not any(self.window.lender.is_borrowed(p) for p in self.live_panels):
            # 'Back to system monitor' (or another tab) took the panels back
            self.live.set(False)
            self.live_panels = []
        if self.playing:
            frame = self.drawing.frame_at(time.monotonic() - self.play_started)
            self.show_playing(frame)
        self.after(max(20, min(100, self.drawing.delay_ms // 2)), self.tick)

    def show_playing(self, frame):
        for r in range(drawings.HEIGHT):
            for c in range(drawings.WIDTH):
                self.canvas.itemconfigure(self.cells[r][c], fill=led_colour(frame[r, c]))
        self.push_soon(frame)

    # Import
    def import_picture(self):
        path = filedialog.askopenfilename(parent=self, title="Import a picture",
                                          filetypes=[("Pictures", "*.png *.gif *.jpg *.jpeg *.bmp"), ("All files", "*.*")])
        if path:
            self.import_from(path)

    def import_from(self, path):
        fit = next((f for f, label in drawings.FIT_LABELS.items() if label == self.fit.get()), drawings.FIT_WHOLE)
        try:
            imported = drawings.import_image(path, fit, self.invert.get())
        except Exception as e:
            self.show(f"Couldn't import that picture: {e}")
            return
        self.stop_playing()
        self.remember()
        self.drawing = imported
        self.index = 0
        self.show_delay()
        self.redraw()
        count = len(imported.frames)
        self.show(f"Imported {count} frame{'s' if count != 1 else ''}." + (" Press Play to watch it." if count > 1 else ""))

    # Saving
    def refresh_saved(self, select=None):
        self.saved_list.delete(0, "end")
        for i, name in enumerate(drawings.list_names()):
            self.saved_list.insert("end", name)
            if name == select:
                self.saved_list.selection_set(i)
                self.saved_list.see(i)

    def selected_name(self):
        chosen = self.saved_list.curselection()
        return self.saved_list.get(chosen[0]) if chosen else None

    def save(self):
        name = drawings.safe_name(self.name_entry.get())
        if not name:
            self.show("Type a name for the drawing first.")
            self.name_entry.focus_set()
            return
        if name != self.name and name in drawings.list_names():
            if not messagebox.askyesno("Replace drawing?", f"There is already a drawing called {name}. Replace it?",
                                       parent=self):
                return
        try:
            drawings.save(name, self.drawing)
        except Exception as e:
            self.show(f"Couldn't save: {e}")
            return
        self.name = name
        self.saved_state = self.state_key()
        self.refresh_saved(select=name)
        self.redraw()
        self.show(f"Saved {name}. The Picture app on the Panels tab can show it now.")

    def ok_to_lose_changes(self):
        if self.state_key() == self.saved_state:
            return True
        return messagebox.askyesno("Lose changes?", "This drawing has changes that aren't saved. Carry on anyway?",
                                   parent=self)

    def open_selected(self):
        name = self.selected_name()
        if not name or not self.ok_to_lose_changes():
            return
        try:
            drawing = drawings.load(name)
        except Exception as e:
            self.show(f"Couldn't open {name}: {e}")
            return
        self.stop_playing()
        self.drawing, self.index, self.name = drawing, 0, name
        self.undo_stack.clear()
        self.redo_stack.clear()
        self.saved_state = self.state_key()
        self.name_entry.delete(0, "end")
        self.name_entry.insert(0, name)
        self.show_delay()
        self.redraw()
        self.show(f"Opened {name}.")

    def new_drawing(self):
        if not self.ok_to_lose_changes():
            return
        self.stop_playing()
        self.drawing, self.index, self.name = drawings.Drawing(), 0, ""
        self.undo_stack.clear()
        self.redo_stack.clear()
        self.saved_state = self.state_key()
        self.name_entry.delete(0, "end")
        self.show_delay()
        self.redraw()

    def delete_selected(self):
        name = self.selected_name()
        if not name:
            return
        if not messagebox.askyesno("Delete drawing?", f"Delete {name}? A panel showing it goes blank.", parent=self):
            return
        try:
            drawings.delete(name)
        except Exception as e:
            self.show(f"Couldn't delete {name}: {e}")
            return
        if name == self.name:
            self.name = ""
            self.saved_state = None
        self.refresh_saved()
        self.redraw()
        self.show(f"Deleted {name}.")

    def show_folder(self):
        import os
        os.makedirs(drawings.folder(), exist_ok=True)
        platform_utils.open_path(drawings.folder())

    # Showing on the panels while drawing
    def live_changed(self):
        if self.live.get():
            if not self.window.selected_panels():
                self.show("Tick a panel at the bottom of the window first.")
                self.live.set(False)
                return
            self.push_soon()
            self.show("The ticked panels show the drawing. 'Back to system monitor' at the bottom gives them back.")
        else:
            panels, self.live_panels = self.live_panels, []
            if panels:
                self.window.give_back(panels)

    def push_soon(self, frame=None):
        """Send the frame to the panels soon (at most one send at a time, the newest frame wins)."""
        if not self.live.get():
            return
        self.next_push = (self.current() if frame is None else frame).copy()
        if self.push_pending:
            return
        self.push_pending = True
        panels = self.window.selected_panels()
        self.live_panels = panels

        def work():
            from inputmodule.gui.gui_threading import reset_thread, set_status
            from inputmodule.inputmodule.ledmatrix import animate
            from white_rabbit.math_tab import send_frames
            new = [p for p in panels if not self.window.lender.is_borrowed(p)]
            if new:
                reset_thread()
                set_status("draw")
                self.window.lender.borrow(new)
                self.window.call_soon(self.window.update_bar)
                for panel in new:
                    animate(panel, False)
            while True:
                frame, self.next_push = self.next_push, None
                if frame is None:
                    break
                send_frames(self.window.link, panels, np.tile(frame, (1, len(panels))))

        def done(_=None):
            self.push_pending = False
            if self.next_push is not None:
                self.push_soon(self.next_push)

        def failed(e):
            self.push_pending = False
            self.show(f"Couldn't show it on the panels: {e}")
        self.window.in_background(work, done, failed)
