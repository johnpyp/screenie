#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow"]
# ///
"""Turn the tour's 4K stills (`tools/demo/run.py stills`) into the README's images:
the hero (the whole desktop over three close-ups, on the demo wallpaper) and close-ups
for the highlights under it. Writes docs/media/*.webp.

    tools/demo/stills.py
"""

import subprocess
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parents[2]
STILLS = ROOT / ".cache" / "demo" / "stills"
WALLPAPER = ROOT / ".cache" / "demo" / "assets" / "wallpaper.png"
FONTS = ROOT / "assets" / "fonts"
OUT = ROOT / "docs" / "media"

# name: (still, logical crop (x0, y0, x1, y1) or None for all of it, output width)
# Both close-ups are 4:3, so they line up in the README's table.
IMAGES = {
    "select-area": ("select-area", (0, 360, 840, 990), 1200),
    "pixelate": ("pixelate", (600, 350, 1320, 890), 1200),
}


# The hero: the desktop across the top, three close-ups under it, each with a caption.
# The close-ups are cropped tighter than the highlights', since they're shown smaller.
HERO_TOP = ("annotate", None, "Annotate in place", "The capture stays where you took it")
HERO_ROW = [
    ("select-window", (800, 50, 1480, 560), "Select anything", "An area, a window or a screen"),
    ("recording", (830, 600, 1470, 1080), "Record", "A ring marks it, a pill runs it"),
    ("card-hover", (1440, 720, 1920, 1080), "Preview cards", "Copy, save, annotate or dismiss"),
]
HERO_WIDTH = 2000
PAD, GAP = 64, 36
RADIUS = 18
TEXT, SUBTEXT = (205, 214, 244), (147, 153, 178)


def image(still, crop, width):
    img = Image.open(STILLS / f"{still}.png").convert("RGB")
    if crop:
        img = img.crop(tuple(v * 2 for v in crop))  # the stills are at 2x
    if img.width > width:
        img = img.resize((width, round(img.height * width / img.width)), Image.LANCZOS)
    return img


def font(weight, size):
    return ImageFont.truetype(str(FONTS / f"Inter-{weight}.otf"), size)


def rounded(size, radius):
    mask = Image.new("L", size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size[0] - 1, size[1] - 1), radius, fill=255)
    return mask


def tile(canvas, img, x, y):
    """A screenshot with rounded corners, a soft shadow and a hairline edge."""
    w, h = img.size
    shadow = Image.new("L", canvas.size, 0)
    shadow.paste(rounded((w, h), RADIUS), (x, y + 14))
    shadow = shadow.filter(ImageFilter.GaussianBlur(28)).point(lambda v: v * 0.7)
    canvas.paste((0, 0, 0), (0, 0), shadow)
    canvas.paste(img, (x, y), rounded((w, h), RADIUS))
    edge = ImageChops.subtract(
        rounded((w, h), RADIUS), rounded((w, h), RADIUS).filter(ImageFilter.MinFilter(3))
    )
    canvas.paste((255, 255, 255), (x, y), edge.point(lambda v: v * 0.12))


def caption(draw, x, y, title, subtitle):
    draw.text((x, y), title, font=font("SemiBold", 38), fill=TEXT)
    draw.text((x, y + 52), subtitle, font=font("Regular", 30), fill=SUBTEXT)
    return y + 52 + 40


def hero():
    inner = HERO_WIDTH - 2 * PAD
    top = image(*HERO_TOP[:2], inner)
    row_w = (inner - 2 * GAP) // 3
    row = [image(still, crop, row_w) for still, crop, *_ in HERO_ROW]

    caption_h = 52 + 40
    height = PAD + top.height + 28 + caption_h + GAP + row[0].height + 28 + caption_h + PAD
    if not WALLPAPER.exists():
        subprocess.run(
            ["uv", "run", "-q", "--script", str(ROOT / "tools/demo/wallpaper.py"), str(WALLPAPER)],
            check=True,
        )
    backdrop = Image.open(WALLPAPER).convert("RGB")
    scale = max(HERO_WIDTH / backdrop.width, height / backdrop.height)
    backdrop = backdrop.resize(
        (round(backdrop.width * scale), round(backdrop.height * scale)), Image.LANCZOS
    )
    canvas = backdrop.crop((0, 0, HERO_WIDTH, height)).filter(ImageFilter.GaussianBlur(24))

    draw = ImageDraw.Draw(canvas)
    tile(canvas, top, PAD, PAD)
    y = caption(draw, PAD + 4, PAD + top.height + 28, *HERO_TOP[2:])
    y += GAP
    for i, (img, (*_, title, subtitle)) in enumerate(zip(row, HERO_ROW)):
        x = PAD + i * (row_w + GAP)
        tile(canvas, img, x, y)
        caption(draw, x + 4, y + img.height + 28, title, subtitle)

    # Round the whole card, so it sits on light and dark pages alike.
    canvas.putalpha(rounded(canvas.size, 36))
    return canvas


def save(img, name):
    out = OUT / f"{name}.webp"
    img.save(out, quality=90, method=6)
    print(f"{out.relative_to(ROOT)}  {img.width}x{img.height}  {out.stat().st_size // 1024} KB")


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for name, (still, crop, width) in IMAGES.items():
        save(image(still, crop, width), name)
    save(hero(), "hero")


if __name__ == "__main__":
    main()
