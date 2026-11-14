# The Home and Settings pages of the White Rabbit window, laid out like the Windows 11 Settings app:
# a column of cards, one setting per card, with an icon, a title, a line of help and its control.

# Built In Dependencies
import tkinter as tk
from tkinter import ttk

# Internal Dependencies
from led_mon import shortcuts, theme as theme_mod

THEME_HINT = ("Follows Windows' light or dark mode and accent colour. Classic is the plain Windows look "
              "White Rabbit used to have; Windows high contrast always uses it.")


def section_title(parent, text):
    label = ttk.Label(parent, text=text, style="Section.TLabel")
    label.pack(anchor="w", pady=(18, 6))
    return label


class Row(ttk.Frame):
    """One card: icon, title and help text on the left, a control (made by right(row)) on the right.

    command: the whole card can be clicked (with a > at the right end), like the Settings app's links to
    another page."""

    def __init__(self, parent, theme, icon, title, help_text="", right=None, command=None):
        super().__init__(parent, style="Card.TFrame", padding=(theme.px(16), theme.px(12)))
        self.pack(fill="x", pady=(0, 4))
        self.command = command
        self.icon = ttk.Label(self, image=theme.icon(icon, 20), style="CardIcon.TLabel")
        self.icon.pack(side="left", padx=(0, theme.px(16)))
        if command is not None:
            self.chevron = ttk.Label(self, image=theme.icon("chevron", 14, "text2"), style="CardIcon.TLabel")
            self.chevron.pack(side="right", padx=(theme.px(12), 0))
        self.control = right(self) if right else None
        if self.control is not None:
            self.control.pack(side="right", padx=(theme.px(12), 0))
        words = ttk.Frame(self, style="CardInner.TFrame")
        words.pack(side="left", fill="x", expand=True)
        self.words = words
        self.title = ttk.Label(words, text=title, style="Card.TLabel")
        self.title.pack(anchor="w")
        self.help = ttk.Label(words, text=help_text, style="CardHint.TLabel", justify="left")
        if help_text:
            self.help.pack(anchor="w", fill="x")
        words.bind("<Configure>", lambda e: self.help.configure(wraplength=max(200, e.width - 4)))
        if command is not None:
            self.configure(cursor="hand2", takefocus=1)
            for widget in (self, words, self.title, self.help, self.icon, self.chevron):
                widget.bind("<Enter>", lambda e: self.hover(True), add="+")
                widget.bind("<Leave>", lambda e: self.hover(False), add="+")
                widget.bind("<Button-1>", lambda e: self.command(), add="+")
            self.bind("<Return>", lambda e: self.command())
            self.bind("<space>", lambda e: self.command())

    def hover(self, on):
        state = ["active"] if on else ["!active"]
        for widget in (self, self.words, self.title, self.help, self.icon, self.chevron):
            widget.state(state)


class HomePage(ttk.Frame):
    """What the panels are doing, and the things done most often."""

    def __init__(self, master, window):
        super().__init__(master)
        self.window = window
        theme = window.theme

        section_title(self, "Your panels")
        self.panel_rows = []
        if not window.panels:
            Row(self, theme, "panels", "No LED Matrix panels found",
                "Check that the panels are in place, then open White Rabbit again.")
        for panel in window.panels:
            row = Row(self, theme, "panels", panel.label, "")
            self.panel_rows.append((panel, row))
        Row(self, theme, "rename", "Name your panels", "Identify panels shows the names on the panels themselves.",
            right=lambda r: ttk.Button(r, text="Name panels...", style="Card.TButton", command=window.name_panels))

        section_title(self, "Quick actions")
        self.music_var = tk.BooleanVar(value=False)
        self.music_row = Row(self, theme, "music", "Music mode",
                             f"Both panels show the audio visualizer.  {shortcuts.keys_text('m')}",
                             right=lambda r: ttk.Checkbutton(r, variable=self.music_var, text="",
                                                             style="Card.Switch.TCheckbutton",
                                                             command=self.toggle_music))
        Row(self, theme, "identify", "Identify panels", "Shows each panel's name on it, so you know which is which.",
            right=lambda r: ttk.Button(r, text="Identify", style="Card.TButton", command=self.identify))
        self.back_row = Row(self, theme, "back", "Back to the system monitor",
                            "Stops a game, pattern or Math picture and lets the monitor draw again.",
                            right=lambda r: ttk.Button(r, text="Give the panels back", style="Card.TButton",
                                                       command=window.give_back_all))

        section_title(self, "Make the panels yours")
        for icon, title, help_text, page in (
                ("monitor", "System monitor", "Pick what each panel shows: CPU, memory, battery, clock, weather and more.",
                 "monitor"),
                ("games", "Games & patterns", "Play Snake, Ledris, Breakout and Rabbit run, or show a pattern or text.",
                 "games"),
                ("music", "Music", "Choose how the audio visualizer looks and what it listens to.", "music"),
                ("math", "Math", "Graphs, the Game of Life and other moving maths across both panels.", "math"),
                ("startup", "Startup animation", "What the panels play when the laptop turns on.", "startup")):
            Row(self, theme, icon, title, help_text, command=lambda page=page: window.show_page(page))
        self.status = ttk.Label(self, text="", style="Hint.TLabel")
        self.status.pack(anchor="w", pady=(8, 0))

    def page_shown(self):
        """Called each time the page is shown."""
        self.update_panels()
        music = getattr(self.window, "music_tab", None)
        if music is not None:
            music.check_music_mode()

    def update_panels(self):
        """What each panel is showing, from the window's point of view."""
        window = self.window
        music_on = bool(getattr(getattr(window, "music_tab", None), "music_on", False))
        info = getattr(window, "module_info", {}) or {}
        for panel, row in self.panel_rows:
            row.title.configure(text=panel.label)
            if not window.monitor_running:
                doing = "The system monitor isn't running"
            elif window.lender.is_borrowed(panel):
                doing = "Showing something from this window"
            elif music_on:
                doing = "Showing music mode"
            else:
                doing = "Showing the system monitor"
            version = (info.get(panel.device) or {}).get("version")
            row.help.configure(text=f"{doing}.  Firmware {version}" if version else f"{doing}.")
            if not row.help.winfo_manager():
                row.help.pack(anchor="w", fill="x")
        self.music_var.set(music_on)
        borrowed = any(window.lender.is_borrowed(p) for p in window.panels)
        self.back_row.control.state(["!disabled"] if borrowed else ["disabled"])

    def toggle_music(self):
        music = self.window.music_tab
        wanted = self.music_var.get()
        if bool(music.music_on) != wanted:
            music.toggle_music_mode()
        if music.music_on is None and wanted:
            self.status.configure(text="Music mode needs the system monitor, which isn't running.")
            self.music_var.set(False)

    def identify(self):
        from led_mon.editor import preview

        def done(sent):
            self.status.configure(text="Look at the panels." if sent else
                                  "The system monitor isn't running, so the panels can't be shown.")
        self.window.in_background(preview.identify, done)


class SettingsPage(ttk.Frame):
    """The app's own settings, and the pages that are used less often."""

    def __init__(self, master, window):
        super().__init__(master)
        self.window = window
        theme = window.theme

        section_title(self, "Appearance")
        self.theme_var = tk.StringVar(value=theme_mod.CHOICE_LABELS[theme.choice])

        def theme_box(row):
            box = ttk.Combobox(row, textvariable=self.theme_var, state="readonly", width=24,
                               values=[theme_mod.CHOICE_LABELS[c] for c in theme_mod.CHOICES])
            box.bind("<<ComboboxSelected>>", self.choose_theme)
            return box
        self.theme_row = Row(self, theme, "theme", "App theme", THEME_HINT, right=theme_box)

        section_title(self, "Panels")
        Row(self, theme, "rename", "Panel names", "Give the left and right panels names of your own.",
            right=lambda r: ttk.Button(r, text="Name panels...", style="Card.TButton", command=window.name_panels))
        Row(self, theme, "firmware", "Firmware", "See the firmware on each panel, and install the White Rabbit "
            "firmware or Framework's.", command=lambda: window.show_page("firmware"))

        section_title(self, "App")
        Row(self, theme, "keyboard", "Keyboard shortcuts", "Shortcuts that work anywhere in Windows.",
            command=lambda: window.show_page("shortcuts"))
        Row(self, theme, "info", "About White Rabbit", "Version, updates, credits and the tip jar.",
            command=lambda: window.show_page("about"))

    def choose_theme(self, event=None):
        labels = {label: key for key, label in theme_mod.CHOICE_LABELS.items()}
        self.window.theme.set_choice(labels.get(self.theme_var.get(), "system"))

    def page_shown(self):
        pass
