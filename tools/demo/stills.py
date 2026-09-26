#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow"]
# ///
"""Turn the tour's 4K stills (`tools/demo/run.py stills`) into the README's images:
the whole desktop for the top, close-ups for the highlights. Writes docs/media/*.webp.

    tools/demo/stills.py
"""

from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
STILLS = ROOT / ".cache" / "demo" / "stills"
OUT = ROOT / "docs" / "media"

# name: (still, logical crop (x0, y0, x1, y1) or None for all of it, output width)
# All the close-ups are 4:3, so they line up in the README's table.
IMAGES = {
    "desktop": ("annotate", None, 1920),
    "select-area": ("select-area", (0, 360, 840, 990), 1200),
    "select-window": ("select-window", (820, 40, 1920, 865), 1200),
    "annotate": ("annotate", (400, 160, 1520, 1000), 1200),
    "pixelate": ("pixelate", (600, 350, 1320, 890), 1200),
    "recording": ("recording", (480, 0, 1920, 1080), 1200),
    "cards": ("card-hover", (1440, 720, 1920, 1080), 960),
}


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for name, (still, crop, width) in IMAGES.items():
        img = Image.open(STILLS / f"{still}.png").convert("RGB")
        if crop:
            img = img.crop(tuple(v * 2 for v in crop))  # the stills are at 2x
        if img.width > width:
            img = img.resize((width, round(img.height * width / img.width)), Image.LANCZOS)
        out = OUT / f"{name}.webp"
        img.save(out, quality=90, method=6)
        print(f"{out.relative_to(ROOT)}  {img.width}x{img.height}  {out.stat().st_size // 1024} KB")


if __name__ == "__main__":
    main()
