#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "opencv-python-headless", "pillow"]
# ///
"""Turn the tour's raw 4K recording into the finished video: captions, key presses, a
title and an outro over the recording.

    tools/demo/render.py [OUT.mp4]   # default .cache/demo/screenie-demo.mp4

Also writes docs/media/screenie-demo.mp4, a smaller encode for the README.

Reads .cache/demo/raw.mkv and timeline.json from `tools/demo/run.py video`. The
recording is 3840x2160, the desktop 1920x1080 logical; the video is 1920x1080 at 60 fps.
Each frame is warped at 4K and box-filtered down, so text stays crisp while zooming.
"""

import json
import subprocess
import sys
from functools import cache
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parents[2]
DEMO = ROOT / ".cache" / "demo"
FONTS = ROOT / "assets" / "fonts"
FPS = 60
W, H = 1920, 1080  # the video, and the desktop in logical pixels
SW, SH = 3840, 2160  # the recording

TITLE = 3.0  # seconds of title over the opening
OUTRO_IN = 0.6  # the outro starts this long after its mark
CAPTION_TOP = 66  # under the bar
MARGIN = 40

TEXT = (205, 214, 244)
SUBTEXT = (166, 173, 200)
MAUVE = (203, 166, 247)


def font(weight, size):
    return ImageFont.truetype(str(FONTS / f"Inter-{weight}.otf"), size)


def ease(t):
    t = min(max(t, 0.0), 1.0)
    return t * t * t * (t * (6 * t - 15) + 10)


def ease_out_back(t):
    t = min(max(t, 0.0), 1.0)
    c = 1.4
    return 1 + (c + 1) * (t - 1) ** 3 + c * (t - 1) ** 2


# The recording.


def frames(path, start):
    """BGR frames of the recording from `start` seconds."""
    proc = subprocess.Popen(
        [
            "ffmpeg",
            "-v",
            "error",
            "-ss",
            f"{start:.4f}",
            "-i",
            str(path),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "bgr24",
            "-",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        bufsize=SW * SH * 3,
    )
    size = SW * SH * 3
    try:
        while True:
            buf = proc.stdout.read(size)
            if len(buf) < size:
                break
            yield np.frombuffer(buf, np.uint8).reshape(SH, SW, 3)
    finally:
        proc.kill()
        proc.wait()


def healed(source, longest=2):
    """Smooth over frames the capture missed. When the screen doesn't deliver a new frame
    in time, the recorder repeats the last one: a hitch in the middle of a motion. A run
    of up to `longest` repeats between two changing frames becomes a crossfade."""

    def thumb(f):
        return cv2.resize(f, (240, 135), interpolation=cv2.INTER_AREA).astype(np.int16)

    def same(a, b):
        return np.abs(a - b).max() <= 2

    prev = None  # the last new frame, and its thumbnail
    moving = False  # whether it changed from the frame before
    held = []  # repeats of it, not yet sent
    for frame in source:
        t = thumb(frame)
        if prev is None:
            prev = (frame, t)
            yield frame
            continue
        if same(t, prev[1]):
            held.append(frame)
            if len(held) > longest:  # a real pause, not a hitch
                yield from held
                held = []
                moving = False
            continue
        if held and moving:
            n = len(held) + 1
            for k in range(1, n):
                yield cv2.addWeighted(prev[0], 1 - k / n, frame, k / n, 0)
        else:
            yield from held
        held = []
        moving = True
        prev = (frame, t)
        yield frame
    yield from held


def find_sync(path, park):
    """The time of the first frame where the pointer has left `park` (run.py warps it
    away as the timeline starts)."""
    x, y = int(park[0] * 2), int(park[1] * 2)
    first = None
    for i, frame in enumerate(frames(path, 0)):
        patch = frame[y - 8 : y + 64, x - 8 : x + 48].astype(np.int16)
        if first is None:
            first = patch
        elif np.abs(patch - first).mean() > 6:
            return i / FPS
        if i > FPS * 8:
            break
    raise SystemExit("no sync jump found in the recording")


def view(frame):
    """A 4K frame at 1920x1080."""
    return cv2.resize(frame, (W, H), interpolation=cv2.INTER_AREA)


# Overlays: RGBA images blended onto the frame.


def blend(frame, overlay, x, y, alpha=1.0):
    """Alpha-blend an RGBA PIL image onto a BGR frame at (x, y)."""
    if alpha <= 0:
        return
    ov = np.asarray(overlay)
    h, w = ov.shape[:2]
    x0, y0 = max(int(x), 0), max(int(y), 0)
    x1, y1 = min(int(x) + w, W), min(int(y) + h, H)
    if x1 <= x0 or y1 <= y0:
        return
    ov = ov[y0 - int(y) : y1 - int(y), x0 - int(x) : x1 - int(x)]
    a = ov[..., 3:4].astype(np.float32) / 255 * alpha
    rgb = ov[..., 2::-1].astype(np.float32)  # RGB -> BGR
    roi = frame[y0:y1, x0:x1].astype(np.float32)
    frame[y0:y1, x0:x1] = (roi * (1 - a) + rgb * a).astype(np.uint8)


@cache
def caption_image(text):
    """Plain text, right-aligned in the top-right corner, with a soft shadow so it
    reads over anything: nothing that could pass for part of screenie's interface."""
    f = font("SemiBold", 34)
    pad = 28
    w, h = int(f.getlength(text)) + 2 * pad, 44 + 2 * pad
    shadow = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).text(
        (w - pad, h / 2 + 2), text, font=f, fill=(0, 0, 0, 230), anchor="rm"
    )
    shadow = shadow.filter(ImageFilter.GaussianBlur(9))
    img = Image.alpha_composite(shadow, shadow.filter(ImageFilter.GaussianBlur(3)))
    ImageDraw.Draw(img).text((w - pad, h / 2), text, font=f, fill=(245, 245, 250, 255), anchor="rm")
    return img, pad


@cache
def keys_image(label):
    """Keycaps for "Ctrl + C": a cap per key, joined by plus signs."""
    f = font("SemiBold", 26)
    parts = [p.strip() for p in label.split("+")]
    caps = []
    for p in parts:
        tw = int(f.getlength(p))
        cw, ch = max(tw + 36, 56), 56
        cap = Image.new("RGBA", (cw, ch + 5), (0, 0, 0, 0))
        d = ImageDraw.Draw(cap)
        d.rounded_rectangle((0, 5, cw - 1, ch + 4), 12, fill=(12, 12, 20, 235))
        d.rounded_rectangle(
            (0, 0, cw - 1, ch - 1), 12, fill=(49, 50, 68, 245), outline=(108, 112, 134, 200)
        )
        d.text((cw / 2, ch / 2 - 1), p, font=f, fill=TEXT, anchor="mm")
        caps.append(cap)
    plus_w = 34
    w = sum(c.width for c in caps) + plus_w * (len(caps) - 1)
    h = caps[0].height
    pad = 24
    img = Image.new("RGBA", (w + 2 * pad, h + 2 * pad), (0, 0, 0, 0))
    sh = Image.new("RGBA", img.size, (0, 0, 0, 0))
    x = pad
    for i, c in enumerate(caps):
        ImageDraw.Draw(sh).rounded_rectangle(
            (x, pad + 8, x + c.width, pad + h + 8), 12, fill=(0, 0, 0, 120)
        )
        x += c.width + plus_w
    img = Image.alpha_composite(img, sh.filter(ImageFilter.GaussianBlur(10)))
    d = ImageDraw.Draw(img)
    x = pad
    for i, c in enumerate(caps):
        img.alpha_composite(c, (x, pad))
        x += c.width
        if i < len(caps) - 1:
            d.text((x + plus_w / 2, pad + h / 2 - 2), "+", font=f, fill=SUBTEXT, anchor="mm")
            x += plus_w
    return img, pad, w, h


@cache
def title_image():
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    big = font("Bold", 150)
    sub = font("Medium", 38)
    small = font("Medium", 26)
    d.text((W / 2, H / 2 - 70), "screenie", font=big, fill=TEXT, anchor="mm")
    d.text(
        (W / 2, H / 2 + 50),
        "Screenshots and screen recordings for Wayland",
        font=sub,
        fill=SUBTEXT,
        anchor="mm",
    )
    row = "select  ·  annotate  ·  record  ·  one daemon, no scripts"
    d.text((W / 2, H / 2 + 118), row, font=small, fill=(147, 153, 178), anchor="mm")
    # A soft glow behind the wordmark.
    glow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(glow).text(
        (W / 2, H / 2 - 70), "screenie", font=big, fill=MAUVE + (150,), anchor="mm"
    )
    glow = glow.filter(ImageFilter.GaussianBlur(28))
    return Image.alpha_composite(glow, img)


# The video.


class Overlays:
    def __init__(self, events, end):
        self.captions = []  # (start, end, text)
        current = None
        for e in events:
            if e["kind"] == "caption":
                if current:
                    self.captions.append((current[0], e["t"], current[1]))
                current = (e["t"], e["text"]) if e.get("text") else None
        if current:
            self.captions.append((current[0], end, current[1]))
        self.keys = [(e["t"], e["label"]) for e in events if e["kind"] == "key"]
        self.title = next(e["t"] for e in events if e["kind"] == "title")
        self.outro = next(e["t"] for e in events if e["kind"] == "outro") + OUTRO_IN

    def draw(self, frame, t, end):
        # Title: the desktop comes into focus behind it.
        if t < self.title + TITLE:
            k = (t - self.title) / TITLE
            fade = 1 - ease((k - 0.55) / 0.45)
            self.backdrop(frame, fade)
            blend(frame, title_image(), 0, 0, fade * ease(k / 0.18))
        # Outro: the reverse.
        if t > self.outro:
            k = ease((t - self.outro) / 0.9)
            self.backdrop(frame, k)
            blend(frame, title_image(), 0, 0, k)
        # Captions: top right, fading in with a slight rise, and out.
        for start, stop, text in self.captions:
            if start <= t < stop + 0.3 and t < self.outro:
                a = ease((t - start) / 0.35) * (1 - ease((t - stop) / 0.3))
                img, pad = caption_image(text)
                y = CAPTION_TOP - pad + 10 * (1 - ease((t - start) / 0.35))
                blend(frame, img, W - MARGIN - img.width + pad, y, a)
        # Keys: pop in under the caption, then fade.
        for start, label in self.keys:
            if start <= t < start + 1.5:
                p = t - start
                a = ease(p / 0.15) * (1 - ease((p - 1.15) / 0.35))
                img, pad, w, _h = keys_image(label)
                s = 0.85 + 0.15 * ease_out_back(p / 0.3)
                im = img.resize(
                    (max(1, int(img.width * s)), max(1, int(img.height * s))), Image.LANCZOS
                )
                # Scaled about the caps' right edge, which lines up with the caption's.
                x = W - MARGIN - (pad + w) * s
                y = CAPTION_TOP + 62 - pad * s
                blend(frame, im, x, y, a)

    @staticmethod
    def backdrop(frame, k):
        if k <= 0.001:
            return
        blurred = cv2.GaussianBlur(frame, (0, 0), 1 + 22 * k)
        dark = (blurred.astype(np.float32) * (1 - 0.45 * k)).astype(np.uint8)
        frame[:] = dark


def main():
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else DEMO / "screenie-demo.mp4"
    raw = DEMO / "raw.mkv"
    events = json.loads((DEMO / "timeline.json").read_text())
    sync = next(e for e in events if e["kind"] == "sync")
    offset = find_sync(raw, sync["park"])
    outro = next(e["t"] for e in events if e["kind"] == "outro")
    end = outro + 3.4
    print(f"sync at {offset:.3f}s; {end:.1f}s of video", flush=True)

    overlays = Overlays(events, end)
    enc = subprocess.Popen(
        [
            "ffmpeg",
            "-v",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "bgr24",
            "-s",
            f"{W}x{H}",
            "-r",
            str(FPS),
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-preset",
            "slow",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            str(out),
        ],
        stdin=subprocess.PIPE,
    )
    total = int(end * FPS)
    for i, src in enumerate(healed(frames(raw, offset))):
        if i >= total:
            break
        t = i / FPS
        frame = view(src)
        overlays.draw(frame, t, end)
        # Fade in from, and out to, black.
        fade = min(ease(t / 0.5), 1 - ease((t - (end - 0.6)) / 0.6))
        if fade < 1:
            frame = (frame.astype(np.float32) * fade).astype(np.uint8)
        enc.stdin.write(frame.tobytes())
        if i % 300 == 0:
            print(f"  {t:5.1f}s", flush=True)
    enc.stdin.close()
    enc.wait()
    print(out)
    web = ROOT / "docs" / "media" / "screenie-demo.mp4"
    web.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            "ffmpeg",
            "-v",
            "error",
            "-y",
            "-i",
            str(out),
            "-c:v",
            "libx264",
            "-preset",
            "veryslow",
            "-crf",
            "26",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            str(web),
        ],
        check=True,
    )
    print(web)


if __name__ == "__main__":
    main()
