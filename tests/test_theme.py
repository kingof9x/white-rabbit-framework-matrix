# The window's look: reading the Windows accent colour, picking light/dark/classic, recolouring the
# Sun Valley pictures, and (where a desktop is available) applying it to a real window.
import colorsys

import pytest

from led_mon import theme


def windows(dark=False, high_contrast=False, accent=("#005fb8", "#60cdff")):
    return {"dark": dark, "accent": accent, "high_contrast": high_contrast, "text_scale": 1.0}


def test_accent_palette_uses_dark1_for_light_mode_and_light2_for_dark_mode():
    # Windows' default blue AccentPalette: 8 RGBA colours, lightest first
    colors = ["99ebff", "4cc2ff", "0091f8", "0078d4", "0067c0", "003e92", "001a68", "f7630c"]
    data = b"".join(bytes.fromhex(c) + b"\x00" for c in colors)
    assert theme.accent_from_palette(data) == ("#0067c0", "#4cc2ff")
    assert theme.accent_from_palette(b"") is None
    assert theme.accent_from_palette(None) is None


def test_single_accent_colour_gives_a_darker_and_a_lighter_shade():
    light, dark = theme.accent_from_color(0xFFD47800)  # 0xAABBGGRR for #0078d4
    l_light = colorsys.rgb_to_hls(*(c / 255 for c in theme._rgb(light)))[1]
    l_dark = colorsys.rgb_to_hls(*(c / 255 for c in theme._rgb(dark)))[1]
    assert l_light < l_dark


def test_resolve_follows_windows_unless_a_choice_is_made():
    assert theme.resolve("system", windows(dark=False)) == ("light", "#005fb8")
    assert theme.resolve("system", windows(dark=True)) == ("dark", "#60cdff")
    assert theme.resolve("dark", windows(dark=False))[0] == "dark"
    assert theme.resolve("light", windows(dark=True))[0] == "light"
    assert theme.resolve("classic", windows())[0] == "classic"
    # High contrast always uses the plain Windows controls, which follow its colours
    assert theme.resolve("dark", windows(high_contrast=True))[0] == "classic"


def test_choice_is_saved_in_settings(tmp_path):
    path = str(tmp_path / "settings.json")
    assert theme.load_choice(path) == "system"
    theme.save_choice("dark", path)
    assert theme.load_choice(path) == "dark"
    theme.save_choice("purple", path)
    assert theme.load_choice(path) == "system"


def test_environment_stands_in_for_windows_settings(monkeypatch):
    monkeypatch.setenv("WHITE_RABBIT_APPEARANCE", "dark")
    monkeypatch.setenv("WHITE_RABBIT_ACCENT", "#0f7b0f,#6ccb5f")
    monkeypatch.setenv("WHITE_RABBIT_TEXT_SCALE", "1.5")
    found = theme.read_windows()
    assert found["dark"] and found["accent"] == ("#0f7b0f", "#6ccb5f") and found["text_scale"] == 1.5


def test_recolor_swaps_the_blue_and_leaves_other_colours():
    from PIL import Image
    image = Image.new("RGBA", (3, 1))
    image.putpixel((0, 0), (0x00, 0x5f, 0xb8, 255))   # Sun Valley's light blue
    image.putpixel((1, 0), (0xc4, 0x2b, 0x1c, 255))   # red stays red
    image.putpixel((2, 0), (0x80, 0x80, 0x80, 128))   # grey stays grey
    out = theme.recolor_sprites(image, "#005fb8", "#0f7b0f")
    r, g, b, a = out.getpixel((0, 0))
    assert g > r and g > b and a == 255
    assert out.getpixel((1, 0)) == (0xc4, 0x2b, 0x1c, 255)
    assert out.getpixel((2, 0)) == (0x80, 0x80, 0x80, 128)


def test_window_follows_each_choice(tmp_path, monkeypatch):
    tk = pytest.importorskip("tkinter")
    pytest.importorskip("sv_ttk")
    try:
        root = tk.Tk()
    except tk.TclError:
        pytest.skip("no desktop to open a window on")
    from tkinter import ttk
    from led_mon import app_settings
    monkeypatch.setattr(app_settings, "settings_path", lambda: str(tmp_path / "settings.json"))
    monkeypatch.setenv("WHITE_RABBIT_APPEARANCE", "light")
    try:
        look = theme.install(root, "system", follow=False)
        hint = ttk.Label(root, text="hint", foreground="#555555")
        hint.pack()
        slider = theme.Slider(root, from_=1, to=10)
        slider.pack()
        slider.set(7)
        assert slider.get() == 7
        root.update()
        style = ttk.Style(root)
        assert look.mode == "light" and style.lookup(".", "background") == "#f3f3f3"
        assert str(hint.cget("foreground")) == theme.PALETTES["light"]["text2"]
        look.set_choice("dark")
        root.update()
        assert look.mode == "dark" and style.lookup(".", "background") == "#202020"
        assert str(hint.cget("foreground")) == theme.PALETTES["dark"]["text2"]
        assert theme.load_choice() == "dark"
        look.set_choice("classic")
        root.update()
        assert look.mode == "classic"
        assert look.icon("home") is not None
    finally:
        theme._current = None
        root.destroy()
