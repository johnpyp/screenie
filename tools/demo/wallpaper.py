#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "pillow"]
# ///
"""Generate the demo desktop's wallpaper: soft Catppuccin Mocha aurora blobs over the
dark base, faint contour lines and a little grain. Deterministic (fixed seed).

    tools/demo/wallpaper.py OUT.png [WIDTH HEIGHT]
"""

import sys

import numpy as np
from PIL import Image


def hex_rgb(h):
    h = h.lstrip("#")
    return np.array([int(h[i : i + 2], 16) for i in (0, 2, 4)], dtype=np.float32) / 255


def smooth_noise(rng, h, w, waves=6, scale=1.0):
    """A smooth field from a few random plane waves: cheap, seamless, no dependencies."""
    y, x = np.mgrid[0:h, 0:w].astype(np.float32)
    x /= w
    y /= w
    field = np.zeros((h, w), np.float32)
    for _ in range(waves):
        angle = rng.uniform(0, np.pi)
        freq = rng.uniform(0.6, 2.2) * scale
        phase = rng.uniform(0, 2 * np.pi)
        field += np.sin((np.cos(angle) * x + np.sin(angle) * y) * freq * 2 * np.pi + phase)
    return field / waves


def main():
    out = sys.argv[1]
    width, height = (int(sys.argv[2]), int(sys.argv[3])) if len(sys.argv) > 3 else (3840, 2160)
    rng = np.random.default_rng(7)
    # Fields are smooth, so compute them small and scale up.
    h, w = height // 4, width // 4
    y, x = np.mgrid[0:h, 0:w].astype(np.float32)
    x /= w
    y /= w  # square units: y runs 0..h/w

    # Warp the plane so the blobs flow instead of sitting as circles.
    wx = x + 0.10 * smooth_noise(rng, h, w, scale=1.2)
    wy = y + 0.10 * smooth_noise(rng, h, w, scale=1.2)

    crust, base = hex_rgb("#11111b"), hex_rgb("#1e1e2e")
    t = np.clip(y / (h / w), 0, 1)[..., None]
    img = crust * t + base * (1 - t)

    blobs = [
        # (x, y as a fraction of the height, radius, color, strength)
        (0.14, 0.22, 0.20, "#cba6f7", 0.85),  # mauve
        (0.62, 0.04, 0.17, "#89b4fa", 0.60),  # blue
        (0.88, 0.70, 0.22, "#f5c2e7", 0.55),  # pink
        (0.36, 0.95, 0.18, "#94e2d5", 0.40),  # teal
        (0.97, 0.05, 0.12, "#b4befe", 0.45),  # lavender
    ]
    height_field = np.zeros((h, w), np.float32)
    for bx, by, r, color, strength in blobs:
        d2 = (wx - bx) ** 2 + (wy - by * h / w) ** 2
        g = np.exp(-d2 / (r * r))
        height_field += g * strength
        img += hex_rgb(color) * (g * strength * 0.42)[..., None]

    # Faint topographic lines over the blobs.
    levels = height_field * 10 + 3.0 * smooth_noise(rng, h, w, waves=8, scale=1.6)
    frac = np.abs(levels - np.round(levels))
    lines = np.clip(1 - frac / 0.05, 0, 1) * (0.35 + np.clip(height_field * 1.5, 0, 0.65))
    img += hex_rgb("#b4befe") * (lines * 0.07)[..., None]

    # Vignette.
    cx, cy = 0.5, 0.5 * h / w
    v = np.sqrt((x - cx) ** 2 + (y - cy) ** 2)
    img *= (1 - 0.35 * np.clip(v / 0.75, 0, 1) ** 2)[..., None]

    # PIL has no float RGB, so upscale each channel as a float image.
    full = np.stack(
        [
            np.asarray(
                Image.fromarray(img[..., c].astype(np.float32), "F").resize(
                    (width, height), Image.BICUBIC
                )
            )
            for c in range(3)
        ],
        axis=-1,
    )
    # Grain, which also dithers away banding in the gradients.
    full += rng.normal(0, 0.012, full.shape[:2]).astype(np.float32)[..., None]
    Image.fromarray((np.clip(full, 0, 1) * 255 + 0.5).astype(np.uint8)).save(out, optimize=True)
    print(out)


if __name__ == "__main__":
    main()
