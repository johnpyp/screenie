"""Driving the demo desktop: smooth, human-looking input, and a timeline of what
happened when, for the video's zooms and captions.

Coordinates are logical (1920x1080). Used by `tools/demo/run.py`.
"""

import json
import math
import os
import random
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEMO = ROOT / ".cache" / "demo"
WLINPUT = ROOT / "target" / "release" / "wlinput"
ENV_FILE = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")) / "screenie-demo.env"


def desktop_env():
    """The running demo desktop's environment (see desktop.sh)."""
    env = dict(os.environ)
    for line in ENV_FILE.read_text().splitlines():
        if line.startswith("export "):
            key, _, value = line[len("export ") :].partition("=")
            env[key] = value
        elif line.startswith("unset "):
            env.pop(line.split()[1], None)
    return env


def ease_in_out(t):
    """Smoothstep-like, with a gentler start and a soft landing."""
    return t * t * t * (t * (6 * t - 15) + 10)


class Input:
    """One `wlinput -` for the whole run: a single virtual pointer and keyboard."""

    def __init__(self, env):
        self.proc = subprocess.Popen(
            [str(WLINPUT), "-"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, text=True
        )
        assert self.proc.stdout.readline().strip() == "ready"
        self.pos = (960.0, 540.0)
        self.rng = random.Random(3)

    def do(self, line):
        self.proc.stdin.write(line + "\n")
        self.proc.stdin.flush()
        answer = self.proc.stdout.readline().strip()
        if answer != "ok":
            raise RuntimeError(f"wlinput {line[:60]}…: {answer}")

    def warp(self, x, y):
        self.do(f"move {x:.2f} {y:.2f}")
        self.pos = (x, y)

    def path(self, x, y, ms, arc):
        """Points along an eased, slightly curved path from here to (x, y), at 120 Hz."""
        x0, y0 = self.pos
        dx, dy = x - x0, y - y0
        dist = math.hypot(dx, dy)
        # A hand moves in a gentle arc, not a ruler line.
        bend = arc * dist * (1 if self.rng.random() < 0.5 else -1)
        nx, ny = (-dy / dist, dx / dist) if dist else (0, 0)
        cx, cy = x0 + dx / 2 + nx * bend, y0 + dy / 2 + ny * bend
        steps = max(2, int(ms / 1000 * 120))
        for i in range(1, steps + 1):
            t = ease_in_out(i / steps)
            u = 1 - t
            yield (u * u * x0 + 2 * u * t * cx + t * t * x, u * u * y0 + 2 * u * t * cy + t * t * y)

    def glide(self, x, y, ms=None, arc=0.08):
        """Move the pointer to (x, y) smoothly. The duration follows the distance."""
        dist = math.hypot(x - self.pos[0], y - self.pos[1])
        if ms is None:
            ms = min(1100, 320 + dist * 0.55)
        interval = 1000 / 120
        chain = " , ".join(f"move {px:.2f} {py:.2f} , sleep {interval:.0f}" for px, py in self.path(x, y, ms, arc))
        if chain:
            self.do(chain)
        self.pos = (x, y)

    def click(self, x=None, y=None, pause=0.12):
        if x is not None:
            self.glide(x, y)
            time.sleep(pause)
        self.do("down left , sleep 70 , up left")

    def drag(self, x, y, ms=None, arc=0.02):
        self.do("down left")
        time.sleep(0.08)
        self.glide(x, y, ms, arc)
        time.sleep(0.12)
        self.do("up left")

    def key(self, *names):
        self.do("key " + " ".join(names))

    def combo(self, *keys):
        """E.g. combo("ctrl", "c"): hold the modifiers, tap the last."""
        *mods, last = keys
        self.do(" , ".join([f"hold {m}" for m in mods] + ["sleep 40", f"key {last}", "sleep 40"] + [f"release {m}" for m in reversed(mods)]))

    def type(self, text, cps=16):
        for ch in text:
            self.do(f"type {ch}" if ch != " " else "key space")
            time.sleep(1 / cps * self.rng.uniform(0.6, 1.4))

    def close(self):
        self.proc.stdin.close()
        self.proc.wait()


class Timeline:
    """What happened when, relative to the recording's first frame."""

    def __init__(self):
        self.t0 = time.monotonic()
        self.events = []

    def start(self):
        self.t0 = time.monotonic()

    def now(self):
        return time.monotonic() - self.t0

    def mark(self, kind, **data):
        self.events.append({"t": round(self.now(), 3), "kind": kind, **data})

    def save(self, path):
        Path(path).write_text(json.dumps(self.events, indent=1))
