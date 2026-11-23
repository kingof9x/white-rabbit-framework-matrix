"""Makes White Rabbit's logo files from the rabbit in rabbits.py: res/WhiteRabbit.ico (the app icon, every
size Windows asks for) and the window's pictures (rust/wr-window/ui/icons/app.png and rabbit.png).
The tray icon is drawn by the app itself (rust/wr-monitor/src/icon.rs, the same rabbits).

    pip install pillow
    python res/logo/make_logo.py
"""
import os
from PIL import Image, ImageDraw, ImageFilter
from rabbits import R16, R12, R10, grid

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.join(HERE, "..", "..")


def rabbit_tile(size):
    g = {16: R12, 24: R10, 32: R12, 48: R16}[size]
    rows = grid(g); n = len(rows); cell = max(1, int(size * 0.85 / n + 0.1))
    S = 8
    tile = Image.new("RGBA", (size * S, size * S), (0, 0, 0, 0))
    ImageDraw.Draw(tile).rounded_rectangle([0, 0, size * S - 1, size * S - 1], radius=size * S * 0.18, fill=(18, 18, 18, 255),
                                           outline=(70, 70, 70, 255), width=S)
    tile = tile.resize((size, size), Image.BOX)
    off = (size - n * cell) // 2
    for y, r in enumerate(rows):
        for x, c in enumerate(r):
            if c == '#':
                for dy in range(cell):
                    for dx in range(cell):
                        tile.putpixel((off + x * cell + dx, off + y * cell + dy), (255, 255, 255, 255))
    return tile


def rabbit_full(size=256):
    """Big icon: a black LED panel with the 16x16 rabbit lit in white LEDs."""
    S = 4; W = size * S
    rows = grid(R16); n = len(rows)
    img = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    pad = W * 0.04
    d.rounded_rectangle([pad, pad, W - pad, W - pad], radius=W * 0.12, fill=(18, 18, 18, 255),
                        outline=(58, 58, 58, 255), width=int(W * 0.012))
    inner = W - 2 * pad; cell = inner * 0.86 / n; start = pad + (inner - cell * n) / 2
    glow = Image.new("RGBA", (W, W), (0, 0, 0, 0)); gd = ImageDraw.Draw(glow)
    for y, r in enumerate(rows):
        for x, c in enumerate(r):
            cx = start + (x + .5) * cell; cy = start + (y + .5) * cell; rad = cell * 0.36
            if c == '#':
                gd.ellipse([cx - rad * 1.6, cy - rad * 1.6, cx + rad * 1.6, cy + rad * 1.6], fill=(255, 255, 255, 70))
                d.ellipse([cx - rad, cy - rad, cx + rad, cy + rad], fill=(255, 255, 255, 255))
            else:
                d.ellipse([cx - rad, cy - rad, cx + rad, cy + rad], fill=(44, 44, 44, 255))
    glow = glow.filter(ImageFilter.GaussianBlur(cell * 0.35))
    base = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    base.alpha_composite(img); base.alpha_composite(glow)
    return base.resize((size, size), Image.LANCZOS)


def main():
    # Small sizes: plain pixels on a dark tile, so the rabbit stays sharp. Big sizes: the rabbit lit on an LED panel.
    images = [rabbit_tile(s) for s in (16, 24, 32, 48)] + [rabbit_full(s) for s in (64, 128, 256)]
    big = images[-1]
    big.save(os.path.join(ROOT, "res", "WhiteRabbit.ico"), format="ICO",
             sizes=[im.size for im in images], append_images=images[:-1])
    icons = os.path.join(ROOT, "rust", "wr-window", "ui", "icons")
    rabbit_full(64).save(os.path.join(icons, "app.png"))
    rabbit_full(64).save(os.path.join(icons, "rabbit.png"))


if __name__ == "__main__":
    main()
