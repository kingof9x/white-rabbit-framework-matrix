# The window's look: follows Windows' light or dark mode, accent colour, text size and high contrast,
# the way the Windows 11 Settings app does.
#
# Built on the Sun Valley ttk theme (sv-ttk, MIT), with its blue swapped for the Windows accent colour.
# "Classic" uses the plain Windows controls (the look White Rabbit had before) and is used automatically
# when Windows high contrast is on, so high contrast colours are followed exactly.
# The choice (system, light, dark or classic) is saved as "app_theme" in settings.json.

# Built In Dependencies
import os
import base64
import io
import colorsys
import logging
import tkinter as tk
from tkinter import ttk, font as tkfont

# Internal Dependencies
from led_mon import app_settings, platform_utils

log = logging.getLogger(__name__)

SETTING = "app_theme"
CHOICES = ("system", "light", "dark", "classic")
CHOICE_LABELS = {
    "system": "Use my Windows setting",
    "light": "Light",
    "dark": "Dark",
    "classic": "Classic",
}
POLL_MS = 2000

# Windows 11 colours, by mode
PALETTES = {
    "light": dict(bg="#f3f3f3", card="#fbfbfb", card_hover="#f5f5f5", stroke="#e5e5e5", nav_hover="#eaeaea",
                  nav_sel="#e9e9e9", text="#1b1b1b", text2="#5f5f5f", text3="#8a8a8a", control="#ffffff",
                  ok="#0f7b0f", warn="#9d5d00", error="#c42b1c", on_accent="#ffffff", group_stroke="#d9d9d9"),
    "dark": dict(bg="#202020", card="#2b2b2b", card_hover="#323232", stroke="#1d1d1d", nav_hover="#2d2d2d",
                 nav_sel="#2d2d2d", text="#ffffff", text2="#c5c5c5", text3="#9a9a9a", control="#373737",
                 ok="#6ccb5f", warn="#fce100", error="#ff99a4", on_accent="#000000", group_stroke="#3a3a3a"),
}
# Windows' default blue accent: (light mode, dark mode)
DEFAULT_ACCENT = ("#005fb8", "#60cdff")
# The blue built into the Sun Valley pictures, swapped for the Windows accent
SPRITE_ACCENT = {"light": "#005fb8", "dark": "#57c8ff"}

# Colours written into the code before the app had themes, by what they mean
OLD_COLORS = {
    "#555555": "text2", "#777777": "text2", "#2f6f3e": "ok", "#a15c00": "warn",
    "#a33": "error", "#aa3333": "error", "#1a5fb4": "accent",
}

# Fonts, by name: (size in pixels at 100%, bold). The Windows 11 type ramp.
FONTS = {
    "WRCaption": (12, False),
    "WRBody": (14, False),
    "WRStrong": (14, True),
    "WRBodyLarge": (18, False),
    "WRSubtitle": (20, True),
    "WRTitle": (28, True),
}

# Icons: (Segoe Fluent Icons code, stand-in character for when that font isn't there)
ICONS = {
    "home": ("\ue80f", "\u2302"),
    "monitor": ("\ue7f4", "\u25a6"),
    "games": ("\ue7fc", "\u25b6"),
    "music": ("\ue8d6", "\u266a"),
    "math": ("\ue8ef", "\u2211"),
    "draw": ("\ue70f", "\u270e"),
    "effects": ("\ue945", "\u2738"),
    "startup": ("\ue7e8", "\u2726"),
    "settings": ("\ue713", "\u2699"),
    "chevron": ("\ue76c", "\u203a"),
    "firmware": ("\ue950", "\u25a3"),
    "keyboard": ("\ue765", "\u2328"),
    "info": ("\ue946", "\u2139"),
    "theme": ("\ue790", "\u25d0"),
    "identify": ("\ue890", "\u25ce"),
    "back": ("\ue7a7", "\u21ba"),
    "panels": ("\ue7f8", "\u25a5"),
    "rename": ("\ue8ac", "\u270e"),
    "heart": ("\ueb51", "\u2665"),
}


def load_choice(path=None):
    choice = app_settings.load_settings(path).get(SETTING)
    return choice if choice in CHOICES else "system"


def save_choice(choice, path=None):
    settings = app_settings.load_settings(path)
    settings[SETTING] = choice if choice in CHOICES else "system"
    return app_settings.save_settings(settings, path)


# Reading Windows' settings
def _hex(r, g, b):
    return f"#{int(r):02x}{int(g):02x}{int(b):02x}"


def _rgb(color):
    color = color.lstrip("#")
    if len(color) == 3:
        color = "".join(c * 2 for c in color)
    return tuple(int(color[i:i + 2], 16) for i in (0, 2, 4))


def accent_from_palette(data):
    """Windows' AccentPalette (8 RGBA colours, lightest first) -> (light mode accent, dark mode accent).

    Windows apps use the 'Dark 1' shade on light backgrounds and 'Light 2' on dark ones."""
    if not data or len(data) < 32:
        return None
    colors = [_hex(*data[i * 4:i * 4 + 3]) for i in range(8)]
    return colors[4], colors[1]


def accent_from_color(abgr):
    """A single accent colour (DWM AccentColor, 0xAABBGGRR) -> (light mode, dark mode) shades."""
    r, g, b = abgr & 0xff, (abgr >> 8) & 0xff, (abgr >> 16) & 0xff
    h, light, s = colorsys.rgb_to_hls(r / 255, g / 255, b / 255)
    shade = [_hex(*(c * 255 for c in colorsys.hls_to_rgb(h, lum, s))) for lum in (min(light, 0.38), max(light, 0.7))]
    return shade[0], shade[1]


def _read_registry(key_path, name, default=None):
    import winreg
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, key_path) as key:
            return winreg.QueryValueEx(key, name)[0]
    except OSError:
        return default


def _high_contrast():
    import ctypes

    class HIGHCONTRAST(ctypes.Structure):
        _fields_ = [("cbSize", ctypes.c_uint), ("dwFlags", ctypes.c_uint), ("lpszDefaultScheme", ctypes.c_wchar_p)]
    hc = HIGHCONTRAST()
    hc.cbSize = ctypes.sizeof(HIGHCONTRAST)
    SPI_GETHIGHCONTRAST, HCF_HIGHCONTRASTON = 0x42, 1
    if not ctypes.windll.user32.SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, ctypes.byref(hc), 0):
        return False
    return bool(hc.dwFlags & HCF_HIGHCONTRASTON)


def read_windows():
    """Windows' look settings: {'dark', 'accent': (light, dark), 'high_contrast', 'text_scale'}.

    Away from Windows (or in tests), WHITE_RABBIT_APPEARANCE=light|dark, WHITE_RABBIT_ACCENT=#light,#dark,
    WHITE_RABBIT_TEXT_SCALE=1.25 and WHITE_RABBIT_HIGH_CONTRAST=1 stand in for them."""
    found = {"dark": False, "accent": DEFAULT_ACCENT, "high_contrast": False, "text_scale": 1.0}
    if platform_utils.IS_WINDOWS:
        try:
            found["dark"] = _read_registry(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                                           "AppsUseLightTheme", 1) == 0
            accent = accent_from_palette(_read_registry(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent",
                                                        "AccentPalette"))
            if accent is None:
                color = _read_registry(r"Software\Microsoft\Windows\DWM", "AccentColor")
                accent = accent_from_color(color) if isinstance(color, int) else None
            found["accent"] = accent or DEFAULT_ACCENT
            scale = _read_registry(r"Software\Microsoft\Accessibility", "TextScaleFactor", 100)
            found["text_scale"] = max(1.0, min(2.25, float(scale) / 100))
            found["high_contrast"] = _high_contrast()
        except Exception as e:
            log.debug(f"Could not read the Windows look settings: {e}")
    env = os.environ
    if env.get("WHITE_RABBIT_APPEARANCE") in ("light", "dark"):
        found["dark"] = env["WHITE_RABBIT_APPEARANCE"] == "dark"
    if env.get("WHITE_RABBIT_ACCENT", "").count(",") == 1:
        found["accent"] = tuple(env["WHITE_RABBIT_ACCENT"].split(","))
    if env.get("WHITE_RABBIT_TEXT_SCALE"):
        found["text_scale"] = float(env["WHITE_RABBIT_TEXT_SCALE"])
    if env.get("WHITE_RABBIT_HIGH_CONTRAST"):
        found["high_contrast"] = env["WHITE_RABBIT_HIGH_CONTRAST"] == "1"
    return found


def resolve(choice, windows):
    """(mode, accent) for a theme choice: mode is light, dark or classic."""
    if windows["high_contrast"] or choice == "classic":
        mode = "classic"
    elif choice in ("light", "dark"):
        mode = choice
    else:
        mode = "dark" if windows["dark"] else "light"
    accent = windows["accent"][1] if mode == "dark" else windows["accent"][0]
    return mode, accent


def recolor_sprites(image, old_accent, new_accent):
    """Swap the Sun Valley blue in a picture (PIL RGBA) for another colour, keeping its light and shade."""
    import numpy as np
    rgba = np.asarray(image.convert("RGBA")).astype(np.float32) / 255
    rgb, alpha = rgba[..., :3], rgba[..., 3:]
    hsv = np.asarray(image.convert("RGB").convert("HSV")).astype(np.float32) / 255
    old_h, old_s, old_v = colorsys.rgb_to_hsv(*(c / 255 for c in _rgb(old_accent)))
    new_h, new_s, new_v = colorsys.rgb_to_hsv(*(c / 255 for c in _rgb(new_accent)))
    hue_gap = np.abs(((hsv[..., 0] - old_h) + 0.5) % 1.0 - 0.5)
    mask = (hsv[..., 1] > 0.3) & (hue_gap < 25 / 360) & (alpha[..., 0] > 0)
    if not mask.any():
        return image
    s = np.clip(hsv[..., 1] * (new_s / max(old_s, 1e-3)), 0, 1)
    v = np.clip(hsv[..., 2] * (new_v / max(old_v, 1e-3)), 0, 1)
    # HSV -> RGB for the swapped pixels only
    h6 = np.full(s.shape, new_h * 6)
    i = np.floor(h6).astype(int) % 6
    f = h6 - np.floor(h6)
    p, q, t = v * (1 - s), v * (1 - s * f), v * (1 - s * (1 - f))
    choices = [np.stack(c, -1) for c in ((v, t, p), (q, v, p), (p, v, t), (p, q, v), (t, p, v), (v, p, q))]
    new_rgb = np.choose(i[..., None], choices)
    rgb = np.where(mask[..., None], new_rgb, rgb)
    out = np.concatenate([rgb, alpha], -1)
    from PIL import Image
    return Image.fromarray((out * 255 + 0.5).astype(np.uint8), "RGBA")


def _photo_data(image):
    buffer = io.BytesIO()
    image.save(buffer, format="PNG")
    return base64.b64encode(buffer.getvalue()).decode("ascii")


class Theme:
    """Applies the look to a Tk window and keeps it in step with Windows."""

    def __init__(self, root, choice=None, follow=True):
        self.root = root
        self.style = ttk.Style(root)
        self.choice = choice if choice in CHOICES else load_choice()
        self.mode = None
        self.accent = None
        self.windows = None
        self.colors = {}
        self.listeners = []
        self.roles = {}           # widget path -> {option: colour role}
        self.elements = set()     # (ttk theme, element) already made
        self.images = {}          # element pictures, updated in place
        self.icons = {}           # (icon, size, colour role) -> PhotoImage, redrawn on theme changes
        self.sprite_sheets = {}
        self.fonts = {}
        self.scale = max(1.0, float(root.tk.call("tk", "scaling")) / (96 / 72))
        self.icon_font = self._find_icon_font()
        for cls in ("TLabel", "Label", "Scale", "Listbox", "Canvas", "Text", "Toplevel"):
            root.bind_class(cls, "<Map>", self._on_map, add="+")
        self.apply(force=True)
        if follow:
            root.after(POLL_MS, self._poll)

    # Choosing
    def set_choice(self, choice):
        self.choice = choice if choice in CHOICES else "system"
        save_choice(self.choice)
        self.apply()

    def on_change(self, fn):
        """fn() runs after every change of look (to redraw custom pictures)."""
        self.listeners.append(fn)

    def _poll(self):
        try:
            self.apply()
        except Exception as e:
            log.warning(f"Could not follow the Windows look: {e}")
        self.root.after(POLL_MS, self._poll)

    # Applying
    def apply(self, force=False):
        windows = read_windows()
        mode, accent = resolve(self.choice, windows)
        if not force and (mode, accent, windows["text_scale"]) == (self.mode, self.accent,
                                                                   self.windows and self.windows["text_scale"]):
            return False
        self.mode, self.accent, self.windows = mode, accent, windows
        self.colors = self._palette(mode, accent, windows["high_contrast"])
        if mode == "classic":
            names = self.style.theme_names()
            self.style.theme_use("vista" if "vista" in names else "clam")
        else:
            import sv_ttk
            sv_ttk.set_theme(mode, self.root)
            # Sun Valley sets its colours when Tk reports the theme change, which would come after ours:
            # set them now instead, then put ours over them
            self.root.tk.eval("bind Tk <<ThemeChanged>> {}")
            self.root.tk.call("configure_colors")
            self._recolor_sv(mode, accent)
        self._fonts(windows["text_scale"], mode)
        self._styles()
        self._palette_tk()
        self.refresh_tree(self.root)
        self.title_bar(self.root)
        for icon in list(self.icons):
            self._draw_icon(*icon)
        for fn in self.listeners:
            try:
                fn()
            except Exception as e:
                log.warning(f"Redrawing after a look change failed: {e}")
        log.info(f"Look: {mode}, accent {accent}, text size {windows['text_scale']:.2f}")
        return True

    def _palette(self, mode, accent, high_contrast):
        if mode != "classic":
            colors = dict(PALETTES[mode])
            colors["accent"] = accent
            return colors

        def system(name, fallback):
            if not platform_utils.IS_WINDOWS:
                return fallback
            try:
                return _hex(*(c // 257 for c in self.root.winfo_rgb(name)))
            except tk.TclError:
                return fallback
        face = system("SystemButtonFace", "#f0f0f0")
        text = system("SystemButtonText", "#000000")
        window = system("SystemWindow", "#ffffff")
        highlight = system("SystemHighlight", "#0078d7")
        return dict(group_stroke=system("SystemButtonShadow", "#a0a0a0"), bg=face, card=window, card_hover=face, stroke=system("SystemButtonShadow", "#a0a0a0"),
                    nav_hover=window, nav_sel=window, text=text,
                    text2=text if high_contrast else "#555555", text3=system("SystemGrayText", "#6d6d6d"),
                    control=window, ok=text if high_contrast else "#2f6f3e",
                    warn=text if high_contrast else "#a15c00", error=text if high_contrast else "#aa3333",
                    accent=system("SystemHotlight", "#1a5fb4") if high_contrast else highlight,
                    on_accent=system("SystemHighlightText", "#ffffff"))

    def _recolor_sv(self, mode, accent):
        """Put the accent colour into Sun Valley's pictures (buttons, switches, check boxes, sliders)."""
        key = (mode, accent)
        if self.sprite_sheets.get(mode) == key:
            return
        try:
            import sv_ttk
            from PIL import Image
            sheet = os.path.join(os.path.dirname(sv_ttk.__file__), "theme", f"spritesheet_{mode}.png")
            image = recolor_sprites(Image.open(sheet), SPRITE_ACCENT[mode], accent)
            photo = tk.PhotoImage(master=self.root, data=_photo_data(image), format="png")
            namespace = f"::ttk::theme::sv_{mode}::I"
            self.root.tk.eval(
                f"foreach {{name x y w h}} $::spriteinfo {{"
                f" ${namespace}($name) copy {photo} -from $x $y [expr {{$x+$w}}] [expr {{$y+$h}}] }}")
            self.sprite_sheets[mode] = key
            self._sheet = photo
        except Exception as e:
            log.warning(f"Could not use the accent colour in the controls: {e}")

    def _family(self, *wanted):
        families = set(tkfont.families(self.root))
        for family in wanted:
            if family in families:
                return family
        return None

    def _fonts(self, text_scale, mode):
        body = self._family("Segoe UI Variable Text", "Segoe UI", "Inter", "DejaVu Sans") or "TkDefaultFont"
        display = self._family("Segoe UI Variable Display Semibold", "Segoe UI Semibold", "Inter Display SemiBold",
                               "Inter Display", "Inter Semi Bold") or body
        strong = self._family("Segoe UI Variable Text Semibold", "Segoe UI Semibold", "Inter Semi Bold") or body
        for name, (px, bold) in FONTS.items():
            family = display if name in ("WRTitle", "WRSubtitle") else (strong if bold else body)
            weight = "bold" if bold and family == body else "normal"
            size = -round(px * self.scale * text_scale)
            if name in tkfont.names(self.root):
                tkfont.nametofont(name, self.root).configure(family=family, size=size, weight=weight)
            else:
                # Kept, because Tk forgets a named font when its Font object goes
                self.fonts[name] = tkfont.Font(self.root, name=name, family=family, size=size, weight=weight)
        # Everything that uses Tk's own fonts, and Sun Valley's
        size = -round((14 if mode != "classic" else 12) * self.scale * text_scale)
        for name in ("TkDefaultFont", "TkTextFont", "TkMenuFont", "TkHeadingFont", "TkCaptionFont",
                     "TkTooltipFont", "TkIconFont", "SunValleyBodyFont"):
            if name in tkfont.names(self.root):
                tkfont.nametofont(name, self.root).configure(
                    family=body, size=size, weight="bold" if name == "TkHeadingFont" else "normal")
        for name, px in (("SunValleyCaptionFont", 12), ("SunValleyBodyStrongFont", 14), ("TkSmallCaptionFont", 12)):
            if name in tkfont.names(self.root):
                tkfont.nametofont(name, self.root).configure(family=strong if "Strong" in name else body,
                                                             size=-round(px * self.scale * text_scale))

    def px(self, n):
        return max(1, round(n * self.scale))

    # Pictures for the cards and the menu on the left
    def _rounded(self, size, radius, fill, outline=None, bar=None):
        from PIL import Image, ImageDraw
        k = 4  # drawn large then shrunk, for smooth corners
        w, h = size
        image = Image.new("RGBA", (w * k, h * k), (0, 0, 0, 0))
        draw = ImageDraw.Draw(image)
        draw.rounded_rectangle([0, 0, w * k - 1, h * k - 1], radius=radius * k, fill=fill,
                               outline=outline, width=k if outline else 0)
        if bar:
            bh = self.px(16)
            top = (h - bh) // 2
            draw.rounded_rectangle([0, top * k, self.px(3) * k - 1, (top + bh) * k - 1], radius=self.px(2) * k,
                                   fill=bar)
        image = image.resize((w, h), Image.LANCZOS)
        # On the window colour, with no see-through pixels: Tk on Windows draws a picture with soft edges
        # by reading the screen back and mixing by hand, which is slow, and these are drawn many times
        return Image.alpha_composite(Image.new("RGBA", (w, h), self.colors["bg"]), image)

    def nav_height(self):
        """Height of an item in the menu on the left: a line of text and its padding."""
        line = tkfont.nametofont("WRBody", self.root).metrics("linespace")
        return max(line, self.px(16)) + 2 * self.px(9)

    def _element_images(self):
        c = self.colors
        r = self.px(4)
        # Tk fills a box by tiling the picture's middle: a big picture needs far fewer copies than a small
        # one, which made pages with many cards or groups take a third of a second to show
        nav = (self.px(160), self.nav_height())
        card = (self.px(160), self.px(160))
        wanted = {
            "nav": self._rounded(nav, r, c["bg"]),
            "nav_hover": self._rounded(nav, r, c["nav_hover"]),
            "nav_sel": self._rounded(nav, r, c["nav_sel"], bar=c["accent"]),
            "card": self._rounded(card, r, c["card"], c["stroke"]),
            "card_hover": self._rounded(card, r, c["card_hover"], c["stroke"]),
            # Groups of controls: just an outline, so what is inside sits on the window colour
            "group": self._rounded(card, r, c["bg"], c["group_stroke"]),
        }
        for name, image in wanted.items():
            data = _photo_data(image)
            if name in self.images:
                # The same picture, so the elements using it change too (its size may change with the text size)
                self.images[name].configure(width=0, height=0, data=data, format="png")
            else:
                self.images[name] = tk.PhotoImage(master=self.root, data=data, format="png")

    def _element(self, name, *args, **kw):
        key = (self.style.theme_use(), name)
        if key not in self.elements:
            self.style.element_create(name, "image", *args, **kw)
            self.elements.add(key)

    def _styles(self):
        c = self.colors
        s = self.style
        self._element_images()
        i = self.images
        b = self.px(6)
        self._element("WRNav.pill", i["nav"], ("selected", i["nav_sel"]), ("active", i["nav_hover"]),
                      border=(self.px(8), self.px(4)), height=self.nav_height(), sticky="nsew")
        self._element("WRCard.box", i["card"], ("active", i["card_hover"]), border=b, sticky="nsew")
        self._element("WRGroup.border", i["group"], border=b, padding=self.px(4), sticky="nsew")
        if self.mode != "classic":
            s.layout("TLabelframe", [("WRGroup.border", {"sticky": "nsew"})])

        sv = self.mode != "classic"
        s.configure(".", background=c["bg"], foreground=c["text"], selectbackground=c["accent"],
                    selectforeground=c["on_accent"], troughcolor=c["bg"])
        for name in ("TFrame", "TLabel", "TLabelframe", "TCheckbutton", "TRadiobutton", "TPanedwindow",
                     "Switch.TCheckbutton", "Toggle.TButton"):
            s.configure(name, background=c["bg"])
        s.configure("TLabelframe.Label", background=c["bg"], foreground=c["text"])
        s.configure("Treeview", background=c["card"], fieldbackground=c["card"], foreground=c["text"])
        s.configure("Hint.TLabel", foreground=c["text2"])
        s.configure("Link.TLabel", foreground=c["accent"])
        s.configure("Title.TLabel", font="WRTitle")
        s.configure("Crumb.TLabel", font="WRTitle", foreground=c["text2"])
        s.map("Crumb.TLabel", foreground=[("active", c["text"])])
        s.configure("Subtitle.TLabel", font="WRSubtitle")
        s.configure("Section.TLabel", font="WRStrong")
        s.configure("Accent.TLabel", foreground=c["accent"])

        # The menu on the left
        s.layout("Nav.TLabel", [("WRNav.pill", {"sticky": "nsew", "children": [
            ("Label.padding", {"sticky": "nsew", "children": [("Label.label", {"sticky": "w"})]})]})])
        s.configure("Nav.TLabel", padding=(self.px(14), 0, self.px(12), 0), font="WRBody",
                    background=c["bg"], foreground=c["text"], compound="left")
        s.configure("NavHint.TLabel", background=c["bg"], foreground=c["text2"], font="WRCaption")

        # Cards: rounded boxes holding one setting each
        s.layout("Card.TFrame", [("WRCard.box", {"sticky": "nsew"})])
        for name, fg, font in (("Card.TLabel", c["text"], "WRBody"), ("CardHint.TLabel", c["text2"], "WRCaption"),
                               ("CardIcon.TLabel", c["text"], "WRBody")):
            s.configure(name, background=c["card"], foreground=fg, font=font)
            s.map(name, background=[("active", c["card_hover"])])
        s.configure("CardInner.TFrame", background=c["card"])
        s.map("CardInner.TFrame", background=[("active", c["card_hover"])])
        for name in ("Card.TButton", "Card.Accent.TButton", "Card.Switch.TCheckbutton", "Card.TCheckbutton",
                     "Card.TRadiobutton", "Card.TCombobox"):
            # The light, dark and border colours fill the box behind switches and check boxes
            s.configure(name, background=c["card"], lightcolor=c["card"], darkcolor=c["card"], bordercolor=c["card"])
        for name in ("TCheckbutton", "TRadiobutton", "Switch.TCheckbutton"):
            s.configure(name, lightcolor=c["bg"], darkcolor=c["bg"], bordercolor=c["bg"])
        if sv:
            s.configure("Accent.TButton", foreground=c["on_accent"])
            s.configure("Card.Accent.TButton", foreground=c["on_accent"])
            s.map("Accent.TButton", foreground=[("disabled", c["text3"]), ("pressed", c["on_accent"])])
            s.map("Card.Accent.TButton", foreground=[("disabled", c["text3"]), ("pressed", c["on_accent"])])

    def _palette_tk(self):
        """Colours for the classic Tk widgets (canvases, lists, sliders, small windows)."""
        c = self.colors
        try:
            self.root.tk.call("tk_setPalette", "background", c["bg"], "foreground", c["text"],
                              "highlightColor", c["accent"], "selectBackground", c["accent"],
                              "selectForeground", c["on_accent"], "activeBackground", c["nav_hover"],
                              "activeForeground", c["text"], "troughColor", c["stroke"],
                              "insertBackground", c["text"])
        except tk.TclError as e:
            log.debug(f"tk_setPalette: {e}")

    # Widgets with colours of their own
    def _on_map(self, event):
        try:
            self.refresh_widget(event.widget)
        except Exception:
            pass

    def refresh_tree(self, widget):
        self.refresh_widget(widget)
        for child in widget.winfo_children():
            self.refresh_tree(child)

    def refresh_widget(self, widget):
        """Swap colours that were written into the code for this look's ones."""
        if not isinstance(widget, tk.Misc):
            return
        path = str(widget)
        roles = self.roles.get(path)
        if roles is None:
            roles = {}
            for option in ("foreground", "fg"):
                try:
                    value = str(widget.cget(option)).lower()
                except tk.TclError:
                    continue
                if value in OLD_COLORS:
                    roles[option] = OLD_COLORS[value]
                break
            self.roles[path] = roles
        c = self.colors
        for option, role in roles.items():
            try:
                widget.configure(**{option: c[role]})
            except tk.TclError:
                pass
        cls = widget.winfo_class()
        try:
            if cls == "Scale":
                widget.configure(background=c["bg"], foreground=c["text"], troughcolor=c["stroke"],
                                 activebackground=c["accent"], highlightthickness=0, borderwidth=0,
                                 sliderrelief="flat")
            elif cls == "Listbox":
                widget.configure(background=c["card"], foreground=c["text"], selectbackground=c["accent"],
                                 selectforeground=c["on_accent"], relief="flat", borderwidth=0,
                                 highlightthickness=1, highlightbackground=c["stroke"], highlightcolor=c["accent"])
            elif cls == "Toplevel":
                widget.configure(background=c["bg"])
                self.title_bar(widget)
        except tk.TclError:
            pass

    # Title bar
    def title_bar(self, window):
        """Dark or light title bar to match (Windows 10 2004 and later), in the window's own colour on Windows 11."""
        if not platform_utils.IS_WINDOWS:
            return
        try:
            import ctypes
            window.update_idletasks()
            hwnd = ctypes.windll.user32.GetParent(window.winfo_id()) or window.winfo_id()
            dwm = ctypes.windll.dwmapi
            dark = ctypes.c_int(1 if self.mode == "dark" else 0)
            for attribute in (20, 19):  # DWMWA_USE_IMMERSIVE_DARK_MODE, and its number before Windows 10 20H1
                if dwm.DwmSetWindowAttribute(hwnd, attribute, ctypes.byref(dark), ctypes.sizeof(dark)) == 0:
                    break
            DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_COLOR_DEFAULT = 35, 36, 0xFFFFFFFF
            for attribute, color in ((DWMWA_CAPTION_COLOR, self.colors["bg"]), (DWMWA_TEXT_COLOR, self.colors["text"])):
                if self.mode == "classic":
                    value = ctypes.c_uint(DWMWA_COLOR_DEFAULT)
                else:
                    r, g, b = _rgb(color)
                    value = ctypes.c_uint(r | (g << 8) | (b << 16))
                dwm.DwmSetWindowAttribute(hwnd, attribute, ctypes.byref(value), ctypes.sizeof(value))
        except Exception as e:
            log.debug(f"Could not colour the title bar: {e}")

    # Icons
    def _find_icon_font(self):
        if platform_utils.IS_WINDOWS:
            fonts = os.path.join(os.environ.get("WINDIR", r"C:\Windows"), "Fonts")
            for name in ("SegoeIcons.ttf", "segmdl2.ttf"):
                path = os.path.join(fonts, name)
                if os.path.exists(path):
                    return path
        return None

    def icon(self, name, size=16, role="text"):
        """A picture of an icon in this look's colour; it is redrawn when the look changes."""
        key = (name, size, role)
        if key not in self.icons:
            self.icons[key] = None
            self._draw_icon(*key)
        return self.icons[key]

    def _draw_icon(self, name, size, role):
        from PIL import Image, ImageDraw, ImageFont, ImageTk
        px = self.px(size)
        glyph, standin = ICONS.get(name, (None, "?"))
        font = None
        if self.icon_font and glyph:
            try:
                font = ImageFont.truetype(self.icon_font, px)
            except OSError:
                font = None
        if font is None:
            glyph = standin
            for candidate in ("DejaVuSans.ttf", "seguisym.ttf", "arial.ttf"):
                try:
                    font = ImageFont.truetype(candidate, round(px * 0.95))
                    break
                except OSError:
                    continue
            else:
                font = ImageFont.load_default()
        image = Image.new("RGBA", (px + 2, px + 2), (0, 0, 0, 0))
        ImageDraw.Draw(image).text(((px + 2) / 2, (px + 2) / 2), glyph, font=font, anchor="mm",
                                   fill=self.colors.get(role, self.colors["text"]))
        photo = self.icons.get((name, size, role))
        if photo is None:
            self.icons[(name, size, role)] = ImageTk.PhotoImage(image, master=self.root)
        else:
            photo.paste(image)


_current = None


def install(root, choice=None, follow=True):
    """Give the window its look; returns the Theme (also from current())."""
    global _current
    _current = Theme(root, choice, follow)
    return _current


def current():
    return _current


def color(role, fallback="#555555"):
    """A colour of the current look (text2, accent, ok, ...), for code that draws by itself."""
    return _current.colors.get(role, fallback) if _current else fallback


class Slider(ttk.Frame):
    """A slider for whole numbers with its value shown beside it, in the look of the other controls.

    Takes the same main options as tk.Scale (from_, to, length, command) so it can stand in for one;
    command(value) runs when the user moves it to a new number."""

    def __init__(self, master, from_=0, to=100, length=200, command=None, variable=None, **ignored):
        super().__init__(master)
        self.command = command
        self.var = variable if variable is not None else tk.DoubleVar(value=from_)
        self.shown = round(float(self.var.get()))
        scale = current().px(1) if current() else 1
        self.scale = ttk.Scale(self, from_=from_, to=to, orient="horizontal", length=round(length * scale),
                               variable=self.var, command=self._moved)
        self.scale.pack(side="left")
        self.value = ttk.Label(self, text=str(self.shown), width=len(str(round(to))) + 1, anchor="e")
        self.value.pack(side="left", padx=(4, 0))
        # The value shown follows the variable, also when other code changes it
        self.var.trace_add("write", lambda *_: self._follow())

    def _follow(self):
        try:
            self.value.configure(text=str(round(float(self.var.get()))))
        except (tk.TclError, ValueError):
            pass

    def _moved(self, _=None):
        value = round(float(self.var.get()))
        if value != self.shown:
            self.shown = value
            self.value.configure(text=str(value))
            if self.command is not None:
                self.command(str(value))

    def configure(self, cnf=None, **kw):
        if "command" in kw:
            self.command = kw.pop("command")
        if cnf or kw:
            return super().configure(cnf, **kw)

    config = configure

    def get(self):
        return round(float(self.var.get()))

    def set(self, value):
        self.var.set(value)
        self.shown = round(float(value))
        self.value.configure(text=str(self.shown))
