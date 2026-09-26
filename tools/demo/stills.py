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
ASSETS = ROOT / ".cache" / "demo" / "assets"
FONTS = ROOT / "assets" / "fonts"
OUT = ROOT / "docs" / "media"

# name: (still, logical crop (x0, y0, x1, y1) or None for all of it, output width)
# Both close-ups are 4:3, so they line up in the README's table.
IMAGES = {
    "select-area": ("select-area", (0, 360, 840, 990), 1200),
    "pixelate": ("pixelate", (600, 350, 1320, 890), 1200),
}


# The hero: a title, the desktop across the top, and three views of the rest under it,
# 16:9 like the desktop so the row reads as a row of screens. Logical crops, as above.
HERO_TITLE = ("screenie", "Screenshots and screen recordings for Wayland")
HERO_TOP = ("annotate", None, "Annotate in place")
HERO_ROW = [
    ("select-window", None, "Select anything", "An area, a window or a screen"),
    ("recording", (800, 450, 1920, 1080), "Record", "A ring marks it, a pill runs it"),
    ("card-hover", (1200, 675, 1920, 1080), "Preview cards", "Copy, save, annotate or dismiss"),
]
HERO_WIDTH = 2000
PAD, GAP = 72, 40  # around the edges, and between tiles in a row
ROW_GAP = 80  # between a caption and the row under it
RADIUS = 18
TEXT, SUBTEXT = (224, 228, 246), (147, 153, 178)


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


def tracked(draw, xy, text, font, fill, tracking=0.0, anchor="la"):
    """Text with letter spacing (a fraction of the size), for display sizes."""
    widths = [font.getlength(c) for c in text]
    width = sum(widths) + tracking * font.size * (len(text) - 1)
    x, y = xy
    if anchor[0] == "m":
        x -= width / 2
    for c, w in zip(text, widths):
        draw.text((x, y), c, font=font, fill=fill, anchor="l" + anchor[1])
        x += w + tracking * font.size


# Captions under tiles: a title and an optional subtitle, as (size, space above baseline).
CAPTION_TITLE = ("SemiBold", 38, 60)
CAPTION_SUBTITLE = ("Regular", 30, 46)
CAPTION_BOTTOM = 12  # below the last baseline, for descenders


def caption_height(subtitle):
    return CAPTION_TITLE[2] + (CAPTION_SUBTITLE[2] if subtitle else 0) + CAPTION_BOTTOM


def caption(draw, x, y, title, subtitle=None):
    weight, size, above = CAPTION_TITLE
    y += above
    tracked(draw, (x, y), title, font(weight, size), TEXT, -0.01, "ls")
    if subtitle:
        weight, size, above = CAPTION_SUBTITLE
        y += above
        draw.text((x, y), subtitle, font=font(weight, size), fill=SUBTEXT, anchor="ls")


def backdrop(width, height):
    """The demo wallpaper, drawn at this size so its colours spread over all of it, and
    blurred: the desktop the screenshots come from, out of focus."""
    path = ASSETS / f"backdrop-{width}x{height}.png"
    if not path.exists():
        script = ROOT / "tools/demo/wallpaper.py"
        subprocess.run(
            ["uv", "run", "-q", "--script", str(script), str(path), str(width), str(height)],
            check=True,
        )
    return Image.open(path).convert("RGB").filter(ImageFilter.GaussianBlur(28))


def wordmark(canvas, x, baseline, text, size):
    """The name: display-tracked, lit from above with a faint gradient, on a soft shadow."""
    f = font("Bold", size)
    mask = Image.new("L", canvas.size, 0)
    tracked(ImageDraw.Draw(mask), (x, baseline), text, f, 255, -0.035, "ms")
    shadow = mask.filter(ImageFilter.GaussianBlur(size / 6)).point(lambda v: v * 0.45)
    canvas.paste((0, 0, 0), (0, round(size / 16)), shadow)
    # From the top of the ascenders to the baseline; descenders keep the last colour.
    top, bottom = (250, 250, 255), (192, 198, 238)
    ascent = f.getmetrics()[0]
    fill = Image.new("RGB", canvas.size, bottom)
    ramp = Image.linear_gradient("L").resize((canvas.width, ascent))
    ramp_rgb = Image.merge(
        "RGB", [ramp.point(lambda v, a=a, b=b: a + (b - a) * v / 255) for a, b in zip(top, bottom)]
    )
    fill.paste(ramp_rgb, (0, round(baseline - ascent)))
    canvas.paste(fill, (0, 0), mask)


def hero():
    inner = HERO_WIDTH - 2 * PAD
    top = image(*HERO_TOP[:2], inner)
    row_w = (inner - 2 * GAP) // 3
    row = [image(still, crop, row_w) for still, crop, *_ in HERO_ROW]

    name, tagline = HERO_TITLE
    name_size, tagline_size, tagline_gap = 112, 40, 64
    title_h = name_size + tagline_gap
    height = (
        PAD + title_h + PAD
        + top.height + caption_height(None) + ROW_GAP
        + max(img.height for img in row) + caption_height(True) + PAD
    )  # fmt: skip
    canvas = backdrop(HERO_WIDTH, height)
    draw = ImageDraw.Draw(canvas)

    y = PAD + name_size
    wordmark(canvas, HERO_WIDTH / 2, y, name, name_size)
    y += tagline_gap
    draw.text(
        (HERO_WIDTH / 2, y), tagline, font=font("Regular", tagline_size), fill=SUBTEXT, anchor="ms"
    )

    y += PAD
    tile(canvas, top, PAD, y)
    caption(draw, PAD + 2, y + top.height, HERO_TOP[2])
    y += top.height + caption_height(None) + ROW_GAP
    for i, (img, (*_, title, subtitle)) in enumerate(zip(row, HERO_ROW)):
        x = PAD + i * (row_w + GAP)
        tile(canvas, img, x, y)
        caption(draw, x + 2, y + img.height, title, subtitle)

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
