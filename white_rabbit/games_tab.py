# Playing the games (white_rabbit/games.py) on the LED panels from the Games & patterns tab.
#
# The game runs on a worker thread and sends each new picture to the ticked panels in greyscale, the
# way the Math tab does. The keys go to the preview in the window: click it (it is focused for you
# when a game starts) and use the arrow keys, space, and Esc or P to pause. The game pauses by
# itself when the preview loses the keyboard, so clicking somewhere else never loses you a life.

# Built In Dependencies
import time
import queue
import logging
import threading
import tkinter as tk
from tkinter import ttk

# External Dependencies
import numpy as np

# Internal Dependencies
from white_rabbit import games, span
from white_rabbit.math_tab import Preview, send_frames

log = logging.getLogger(__name__)

FRAME_SECONDS = 1 / 30
PREVIEW_MS = 40
KEYS = {"Left": games.LEFT, "Right": games.RIGHT, "Up": games.UP, "Down": games.DOWN,
        "space": games.ACTION, "Return": games.ACTION,
        "a": games.LEFT, "d": games.RIGHT, "w": games.UP, "s": games.DOWN}
PAUSE_KEYS = ("Escape", "p", "P")
# On some systems a held key sends release-press pairs; a key only counts as let go after this long
RELEASE_GRACE_MS = 40


class GamePlayer:
    """The Games section of the Games & patterns tab, and the preview the keys go to."""

    def __init__(self, tab, buttons_parent, preview_parent):
        self.tab = tab
        self.window = tab.window
        self.session = None
        self.run_id = 0
        self.events = queue.Queue()
        self.held = set()
        self.release_jobs = {}
        self.latest = None
        self.drawn = None
        self.scores = games.load_high_scores()
        self.saved_score = None
        # Whether the game should be paused, as the window sees it (the game thread catches up)
        self.want_paused = False

        row = ttk.Frame(buttons_parent)
        row.pack(fill="x")
        for game in games.GAMES:
            ttk.Button(row, text=game.title, command=lambda g=game: self.start(g)).pack(side="left", padx=2)
        self.pause_button = ttk.Button(row, text="Pause", command=self.toggle_pause, state="disabled")
        self.pause_button.pack(side="left", padx=(16, 2))
        ttk.Button(row, text="Stop", command=self.stop).pack(side="left", padx=2)
        self.best_label = ttk.Label(buttons_parent, text=self.best_text(), foreground=tab_hint())
        self.best_label.pack(anchor="w", pady=(6, 0))

        self.title = ttk.Label(preview_parent, text="No game running", font=("TkDefaultFont", 11, "bold"))
        self.title.pack(anchor="w")
        self.score_label = ttk.Label(preview_parent, text="Pick a game on the left.")
        self.score_label.pack(anchor="w", pady=(2, 6))
        self.preview = Preview(preview_parent)
        self.preview.configure(takefocus=1, highlightthickness=2, highlightbackground="#111111",
                               highlightcolor="#e8b65a", cursor="hand2")
        self.preview.pack(anchor="center")
        self.help = ttk.Label(preview_parent, text="", wraplength=200, foreground=tab_hint(), justify="left")
        self.help.pack(anchor="w", pady=(6, 0))
        layout_row = ttk.Frame(preview_parent)
        layout_row.pack(anchor="w", pady=(8, 0))
        ttk.Label(layout_row, text="Both panels:").pack(side="left")
        self.layout = ttk.Combobox(layout_row, values=[label for _, label in span.MODES], state="readonly", width=28)
        self.layout.pack(side="left", padx=4)
        self.layout.bind("<<ComboboxSelected>>", lambda e: self.layout_changed())
        self.layout.bind("<Map>", lambda e: self.show_layout())
        ttk.Label(preview_parent, text="Ledris always plays side by side. The keyboard gap is set in the Math tab.",
                  wraplength=260, foreground=tab_hint(), justify="left").pack(anchor="w", pady=(2, 0))
        self.show_layout()
        for name in ("<KeyPress>", "<KeyRelease>"):
            self.preview.bind(name, self.on_key)
        self.preview.bind("<Button-1>", self.on_click)
        self.preview.bind("<FocusOut>", self.on_focus_out)
        self.preview.after(PREVIEW_MS, self.draw_preview)

    # Both panels: shares the Math tab's setting
    def show_layout(self):
        keys = [key for key, _ in span.MODES]
        self.layout.current(keys.index(span.load()["mode"]))

    def layout_changed(self):
        keys = [key for key, _ in span.MODES]
        layout = span.load()
        layout["mode"] = keys[max(0, self.layout.current())]
        span.save(layout)
        # A game that's running starts again with the new layout
        if self.session is not None and self.session.game.spans and not self.session.game.over:
            self.start(type(self.session.game))

    # Text
    def best_text(self):
        parts = [f"{g.title} {self.scores[g.key]}" for g in games.GAMES if self.scores.get(g.key)]
        return "Best scores: " + ", ".join(parts) if parts else "Best scores show up here once you've played."

    def update_labels(self):
        session = self.session
        if session is None:
            return
        game = session.game
        if self.want_paused and not game.over:
            text = "Paused. Press Esc or P (or click the game) to carry on."
        elif game.over:
            text = f"Game over: {game.score}." + (" A new best!" if session.new_best else f" Best: {session.best}.")
            text += " Press space to play again."
        else:
            extra = f"   Level {game.level}" if hasattr(game, "level") else ""
            text = f"Score {game.score}   Best {session.best}{extra}"
        self.score_label.configure(text=text)
        self.pause_button.configure(text="Carry on" if self.want_paused else "Pause",
                                    state="disabled" if game.over else "normal")
        if game.over and self.saved_score is not game:
            self.saved_score = game
            if games.save_high_score(game.key, game.score):
                self.scores[game.key] = game.score
                self.best_label.configure(text=self.best_text())

    # Keys
    def on_key(self, event):
        key = KEYS.get(event.keysym)
        if event.type == tk.EventType.KeyPress:
            if event.keysym in PAUSE_KEYS:
                self.toggle_pause()
                return "break"
            if key is None:
                return None
            job = self.release_jobs.pop(key, None)
            if job is not None:
                self.preview.after_cancel(job)
            elif key not in self.held:
                self.held.add(key)
            if self.session and self.want_paused:
                self.set_paused(False)
            elif self.session and self.session.game.over and key == games.ACTION and self.session.finished:
                self.start(type(self.session.game))
            else:
                self.events.put(key)
            return "break"
        if key is not None:
            self.release_jobs[key] = self.preview.after(RELEASE_GRACE_MS, lambda k=key: self.let_go(k))
            return "break"
        return None

    def let_go(self, key):
        self.release_jobs.pop(key, None)
        self.held.discard(key)

    def on_click(self, event=None):
        self.preview.focus_set()
        if self.session and self.want_paused:
            self.set_paused(False)

    def on_focus_out(self, event=None):
        self.held.clear()
        if self.session and not self.session.game.over:
            self.set_paused(True)

    def toggle_pause(self):
        self.set_paused(not self.want_paused)

    def set_paused(self, paused):
        if self.session is None:
            return
        self.want_paused = paused
        self.events.put(("pause", paused))
        if not paused:
            self.preview.focus_set()
        self.update_labels()

    # Running
    def start(self, game_class):
        from inputmodule.gui.gui_threading import reset_thread, set_status
        panels = self.window.selected_panels()
        count = len(panels) if panels else 2
        layout = span.load() if game_class.spans else {}
        self.run_id += 1
        run_id = self.run_id
        # Ends a clock, equalizer or Math tab animation, and any earlier game
        reset_thread()
        set_status("game")
        self.events = queue.Queue()
        self.held.clear()
        self.want_paused = False
        self.session = games.GameSession(
            game_class(span.picture_width(count, layout)), best=self.scores.get(game_class.key, 0),
            to_panels=lambda grid: span.to_panels(grid, count, layout), panel_width=games.PANEL_WIDTH * count)
        self.saved_score = None
        self.title.configure(text=game_class.title)
        where = " and ".join(p.called for p in panels)
        self.help.configure(text=game_class.controls + " Esc pauses."
                            + ("" if panels else " No panel is ticked, so it only shows here."))
        self.tab.show(f"Playing {game_class.title} on the {where}." if panels else f"Playing {game_class.title} in the window.")
        self.update_labels()
        self.preview.focus_set()
        threading.Thread(target=self.run, args=(run_id, panels, self.session), daemon=True).start()

    def still_running(self, run_id):
        from inputmodule.gui.gui_threading import get_status, is_thread_stopped
        return run_id == self.run_id and get_status() == "game" and not is_thread_stopped()

    def run(self, run_id, panels, session):
        from inputmodule.gui.gui_threading import is_thread_stopped, reset_thread
        from inputmodule.inputmodule.ledmatrix import animate
        try:
            if panels:
                self.window.lender.borrow(panels)
                self.window.call_soon(self.window.update_bar)
                for panel in panels:
                    animate(panel, False)
            last = time.monotonic()
            sent = None
            state = None
            while self.still_running(run_id):
                while True:
                    try:
                        event = self.events.get_nowait()
                    except queue.Empty:
                        break
                    if isinstance(event, tuple):
                        session.set_paused(event[1])
                    else:
                        session.press(event)
                now = time.monotonic()
                session.update(min(0.1, now - last), set(self.held))
                last = now
                frame = np.clip(np.rint(session.frame() * 255), 0, 255).astype(int)
                if sent is None or not np.array_equal(frame, sent):
                    if panels:
                        send_frames(self.window.link, panels, frame)
                    sent = frame
                    self.latest = frame
                now_state = (session.game.score, session.paused, session.game.over, session.finished,
                             getattr(session.game, "level", None))
                if now_state != state:
                    state = now_state
                    self.window.call_soon(self.update_labels)
                pause = FRAME_SECONDS - (time.monotonic() - now)
                if pause > 0:
                    time.sleep(pause)
        except Exception as e:
            log.exception("The game stopped")
            message = f"The game stopped: {e}"
            if run_id == self.run_id:
                self.window.call_soon(lambda: self.tab.show(message))
        finally:
            if is_thread_stopped() and run_id == self.run_id:
                reset_thread()

    def stop(self):
        self.run_id += 1
        self.session = None
        self.title.configure(text="No game running")
        self.score_label.configure(text="Pick a game on the left.")
        self.pause_button.configure(text="Pause", state="disabled")
        panels = self.window.selected_panels()
        if any(self.window.lender.is_borrowed(p) for p in panels):
            self.window.give_back(panels)
            self.tab.show("Stopped. The system monitor has the panels again.")
        else:
            self.tab.show("Stopped.")

    def draw_preview(self):
        frame = self.latest
        if frame is not None and frame is not self.drawn:
            self.drawn = frame
            self.preview.draw(frame)
        self.preview.after(PREVIEW_MS, self.draw_preview)


def tab_hint():
    from white_rabbit.tabs import HINT
    return HINT
