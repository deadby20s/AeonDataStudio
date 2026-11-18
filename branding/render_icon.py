"""Renders the app icon (branding/icon-1024.png) from the same geometry as public/logo.svg.

Pillow can't read SVG, so the mark is drawn directly: a 64-unit design scaled to 1024 px,
drawn at 4x and downscaled for smooth edges. Then generate every platform icon with:

    python branding/render_icon.py
    cargo tauri icon branding/icon-1024.png
"""
import os

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

HERE = os.path.dirname(os.path.abspath(__file__))
SIZE = 1024
SS = 4
S = SIZE * SS / 64  # pixels per design unit

TOP = np.array([0x5B, 0x7C, 0xFF], np.float32)  # gradient start (top left)
BOTTOM = np.array([0xA2, 0x4B, 0xF7], np.float32)  # gradient end (bottom right)


def u(v):
    return int(round(v * S))


def rounded_mask(x, y, w, h, r):
    m = Image.new("L", (SIZE * SS, SIZE * SS), 0)
    ImageDraw.Draw(m).rounded_rectangle([u(x), u(y), u(x + w), u(y + h)], radius=u(r), fill=255)
    return m


def main():
    n = SIZE * SS
    yy, xx = np.mgrid[0:n, 0:n].astype(np.float32) / n
    # Diagonal gradient from (6,4) to (58,60) in design units.
    t = np.clip(((xx * 64 - 6) * 52 + (yy * 64 - 4) * 56) / (52 * 52 + 56 * 56), 0, 1)[..., None]
    rgb = TOP * (1 - t) + BOTTOM * t
    # Gloss: white, fading from 32% at the top to 0 at y = 34.
    gloss = np.clip(1 - (yy * 64 - 2) / 32, 0, 1)[..., None] * 0.32
    rgb = rgb * (1 - gloss) + 255 * gloss
    img = Image.fromarray(rgb.astype(np.uint8), "RGB").convert("RGBA")

    shape = rounded_mask(2, 2, 60, 60, 17)
    out = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    out.paste(img, (0, 0), shape)

    d = ImageDraw.Draw(out, "RGBA")
    # Hairline inner edge.
    d.rounded_rectangle([u(2.5), u(2.5), u(61.5), u(61.5)], radius=u(16.5), outline=(255, 255, 255, 56), width=max(1, u(1)))
    # Three data layers narrowing toward the drop.
    for (x, y, w, a) in [(14, 16, 36, 0.95), (19, 26, 26, 0.85), (24, 36, 16, 0.75)]:
        d.rounded_rectangle([u(x), u(y), u(x + w), u(y + 6)], radius=u(3), fill=(255, 255, 255, int(255 * a)))
    # Glow behind the drop.
    glow = Image.new("L", (n, n), 0)
    ImageDraw.Draw(glow).ellipse([u(32 - 6), u(50 - 6), u(32 + 6), u(50 + 6)], fill=200)
    glow = glow.filter(ImageFilter.GaussianBlur(u(3.5)))
    white = Image.new("RGBA", (n, n), (255, 255, 255, 255))
    out = Image.composite(white, out, Image.fromarray((np.asarray(glow) * (np.asarray(shape) > 0)).astype(np.uint8)))
    d = ImageDraw.Draw(out, "RGBA")
    d.ellipse([u(32 - 3.6), u(50 - 3.6), u(32 + 3.6), u(50 + 3.6)], fill=(255, 255, 255, 255))

    final = out.resize((SIZE, SIZE), Image.LANCZOS)
    path = os.path.join(HERE, "icon-1024.png")
    final.save(path)
    final.resize((256, 256), Image.LANCZOS).save(os.path.join(HERE, "icon-256-preview.png"))
    print("wrote", path)


if __name__ == "__main__":
    main()
