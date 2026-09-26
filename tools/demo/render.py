#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "opencv-python-headless", "pillow"]
# ///
"""Turn the tour's raw 4K recording into the finished video: an eased camera that
zooms to where the action is, captions, key presses, a title and an outro.

    tools/demo/render.py [OUT.mp4]   # default .cache/demo/screenie-demo.mp4

Also writes docs/media/screenie-demo.mp4, a smaller encode for the README.

Reads .cache/demo/raw.mkv and timeline.json from `tools/demo/run.py video`. The
recording is 3840x2160, the desktop 1920x1080 logical; the video is 1920x1080 at 60 fps.
Each frame is warped at 4K and box-filtered down, so text stays crisp while zooming.
"""

import json
import math
import subprocess
import sys
from functools import lru_cache
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

MOVE = 1.1  # seconds a camera move takes
TITLE = 3.0  # seconds of title over the opening
OUTRO_IN = 0.6  # the outro starts this long after its mark
CAPTION_BOTTOM = H - 96

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
        ["ffmpeg", "-v", "error", "-ss", f"{start:.4f}", "-i", str(path), "-f", "rawvideo",
         "-pix_fmt", "bgr24", "-"],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=SW * SH * 3,
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


# The camera.


class Camera:
    """Where the video looks: eased moves between focus targets from the timeline."""

    def __init__(self, events):
        self.moves = []  # (start, from, to)
        state = (W / 2, H / 2, 1.0)
        for e in events:
            if e["kind"] != "focus":
                continue
            start = e["t"]
            current = self.at(start) if self.moves else state
            self.moves.append((start, current, self.target(e.get("rect"), e.get("zoom"))))

    @staticmethod
    def target(rect, zoom):
        if not rect:
            return (W / 2, H / 2, 1.0)
        x0, y0, x1, y1 = rect
        z = zoom or min(0.85 * W / (x1 - x0), 0.85 * H / (y1 - y0))
        z = max(1.0, min(z, 2.4))
        vw, vh = W / z, H / z
        cx = min(max((x0 + x1) / 2, vw / 2), W - vw / 2)
        cy = min(max((y0 + y1) / 2, vh / 2), H - vh / 2)
        return (cx, cy, z)

    def at(self, t):
        state = (W / 2, H / 2, 1.0)
        for start, frm, to in self.moves:
            if t < start:
                break
            k = ease((t - start) / MOVE)
            # Zoom in log space, so zooming in and out feel alike.
            z = math.exp(math.log(frm[2]) + (math.log(to[2]) - math.log(frm[2])) * k)
            # Pan so the point under the camera moves along with the zoom.
            state = (frm[0] + (to[0] - frm[0]) * k, frm[1] + (to[1] - frm[1]) * k, z)
        return state


def view(frame, cx, cy, z):
    """The 1920x1080 view of a 4K frame centred on logical (cx, cy) at zoom z."""
    if abs(z - 1) < 1e-4 and abs(cx - W / 2) < 1e-3 and abs(cy - H / 2) < 1e-3:
        big = frame
    else:
        m = np.float32([[z, 0, SW / 2 - z * cx * 2], [0, z, SH / 2 - z * cy * 2]])
        big = cv2.warpAffine(frame, m, (SW, SH), flags=cv2.INTER_CUBIC, borderMode=cv2.BORDER_REPLICATE)
    return cv2.resize(big, (W, H), interpolation=cv2.INTER_AREA)


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


def frost(frame, x, y, w, h, radius):
    """Blur what's behind a panel, for a frosted-glass look."""
    x0, y0, x1, y1 = max(x, 0), max(y, 0), min(x + w, W), min(y + h, H)
    if x1 > x0 and y1 > y0:
        frame[y0:y1, x0:x1] = cv2.GaussianBlur(frame[y0:y1, x0:x1], (0, 0), radius)


def panel(w, h, radius, fill, border=(255, 255, 255, 28), shadow=True):
    """A rounded panel with a soft shadow, as RGBA; returns (image, pad)."""
    pad = 24 if shadow else 0
    img = Image.new("RGBA", (w + 2 * pad, h + 2 * pad), (0, 0, 0, 0))
    if shadow:
        sh = Image.new("RGBA", img.size, (0, 0, 0, 0))
        ImageDraw.Draw(sh).rounded_rectangle((pad, pad + 6, pad + w, pad + h + 6), radius, fill=(0, 0, 0, 110))
        img = Image.alpha_composite(img, sh.filter(ImageFilter.GaussianBlur(12)))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle((pad, pad, pad + w - 1, pad + h - 1), radius, fill=fill, outline=border, width=1)
    return img, pad


@lru_cache(maxsize=None)
def caption_image(text):
    f = font("SemiBold", 27)
    tw = int(f.getlength(text))
    w, h = tw + 64, 58
    img, pad = panel(w, h, 29, (24, 24, 37, 205))
    d = ImageDraw.Draw(img)
    d.ellipse((pad + 24, pad + h / 2 - 4, pad + 32, pad + h / 2 + 4), fill=MAUVE)
    d.text((pad + 44, pad + h / 2), text, font=f, fill=TEXT, anchor="lm")
    img = img.crop((0, 0, img.width + 12, img.height))  # room for the dot
    return img, pad, w + 12, h


@lru_cache(maxsize=None)
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
        d.rounded_rectangle((0, 0, cw - 1, ch - 1), 12, fill=(49, 50, 68, 245), outline=(108, 112, 134, 200))
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
        ImageDraw.Draw(sh).rounded_rectangle((x, pad + 8, x + c.width, pad + h + 8), 12, fill=(0, 0, 0, 120))
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


@lru_cache(maxsize=None)
def title_image():
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    big = font("Bold", 150)
    sub = font("Medium", 38)
    small = font("Medium", 26)
    d.text((W / 2, H / 2 - 70), "screenie", font=big, fill=TEXT, anchor="mm")
    d.text((W / 2, H / 2 + 50), "Screenshots and screen recordings for Wayland", font=sub, fill=SUBTEXT, anchor="mm")
    row = "select  ·  annotate  ·  record  ·  one daemon, no scripts"
    d.text((W / 2, H / 2 + 118), row, font=small, fill=(147, 153, 178), anchor="mm")
    # A soft glow behind the wordmark.
    glow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(glow).text((W / 2, H / 2 - 70), "screenie", font=big, fill=MAUVE + (150,), anchor="mm")
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
        # Captions: slide up and fade in, fade out, at the bottom centre (clear of the
        # selector's bar below them).
        right = W / 2  # where a key press shows: right of the caption
        for start, stop, text in self.captions:
            if start <= t < stop + 0.3 and t < self.outro:
                a = ease((t - start) / 0.35) * (1 - ease((t - stop) / 0.3))
                img, pad, w, h = caption_image(text)
                x = (W - w) / 2
                y = CAPTION_BOTTOM - h + 12 * (1 - ease((t - start) / 0.35))
                frost(frame, int(x), int(y), w, h, 14 * a + 0.1)
                blend(frame, img, x - pad, y - pad, a)
                right = max(right, (W + w) / 2 + 16) if t < stop else right
        # Keys: pop in beside the caption, then fade.
        for start, label in self.keys:
            if start <= t < start + 1.5:
                p = t - start
                a = ease(p / 0.15) * (1 - ease((p - 1.15) / 0.35))
                img, pad, w, h = keys_image(label)
                s = 0.85 + 0.15 * ease_out_back(p / 0.3)
                im = img.resize((max(1, int(img.width * s)), max(1, int(img.height * s))), Image.LANCZOS)
                x = right - pad * s if right > W / 2 else (W - im.width) / 2
                y = CAPTION_BOTTOM - 58 / 2 - im.height / 2
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

    camera = Camera(events)
    overlays = Overlays(events, end)
    enc = subprocess.Popen(
        ["ffmpeg", "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "bgr24", "-s", f"{W}x{H}",
         "-r", str(FPS), "-i", "-", "-c:v", "libx264", "-preset", "slow", "-crf", "18",
         "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(out)],
        stdin=subprocess.PIPE,
    )
    total = int(end * FPS)
    for i, src in enumerate(healed(frames(raw, offset))):
        if i >= total:
            break
        t = i / FPS
        cx, cy, z = camera.at(t)
        frame = view(src, cx, cy, z)
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
        ["ffmpeg", "-v", "error", "-y", "-i", str(out), "-c:v", "libx264", "-preset", "veryslow",
         "-crf", "26", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(web)],
        check=True,
    )
    print(web)


if __name__ == "__main__":
    main()
