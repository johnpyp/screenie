#!/usr/bin/env python3
"""Play the feature tour on the demo desktop (tools/demo/desktop.sh start first).

    tools/demo/run.py video    # record it: .cache/demo/raw.mkv + timeline.json
    tools/demo/run.py stills   # the same tour, stopping for screenshots: .cache/demo/stills/

Then `tools/demo/render.py` turns the recording into the finished video.

Coordinates are logical (the desktop is 1920x1080 at 2x). Window positions come from
sway; card positions from screenie's card layout (see `Cards`).
"""

import json
import os
import shutil
import signal
import subprocess
import sys
import time

from demo import DEMO, ROOT, Input, Timeline, desktop_env

MODE = sys.argv[1] if len(sys.argv) > 1 else "video"
ENV = desktop_env()
STILLS = DEMO / "stills"
# Where the pointer waits before the recording's sync jump (see render.py).
PARK, SYNC = (1700, 300), (1480, 420)
# Where the pointer rests to press PrtSc. Headless sway draws the pointer into the
# screen's image, so the frozen screen would show a stray copy of it; here the
# selector's toolbar covers it.
UNDER_TOOLBAR = (925, 1010)


def sway_windows():
    tree = json.loads(subprocess.check_output(["swaymsg", "-t", "get_tree"], env=ENV))
    rects = {}

    def walk(node):
        if node.get("app_id"):
            r = node["rect"]
            rects[node["app_id"]] = (r["x"], r["y"], r["x"] + r["width"], r["y"] + r["height"])
        for child in node.get("nodes", []) + node.get("floating_nodes", []):
            walk(child)

    walk(tree)
    return rects


class Cards:
    """The preview card stack in the bottom-right corner, mirroring screenie's layout
    (crates/screenie-app/src/preview.rs): 236 wide at most, 204x116 at least, 18 from the
    screen's edges, 12 apart, newest at the bottom."""

    def __init__(self):
        self.sizes = []  # oldest first

    def add(self, w_px, h_px):
        fit = min(236 / w_px, 236 / h_px, 1.0)
        self.sizes.append((min(max(w_px * fit, 204), 236), min(max(h_px * fit, 116), 236)))

    def remove(self, index):
        self.sizes.pop(index)

    def rect(self, index):
        """Card `index` (0 = oldest) as (x0, y0, x1, y1)."""
        bottom = 1080 - 18
        for i in range(len(self.sizes) - 1, index, -1):
            bottom -= self.sizes[i][1] + 12
        w, h = self.sizes[index]
        return (1920 - 18 - w, bottom - h, 1920 - 18, bottom)

    def buttons(self, index):
        x0, y0, x1, y1 = self.rect(index)
        cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
        return {
            "close": (x0 + 22, y0 + 21),
            "edit": (x1 - 22, y0 + 21),
            "copy": (cx - 26, cy),
            "save": (cx + 27, cy),
        }


class Tour:
    def __init__(self):
        self.input = Input(ENV)
        self.timeline = Timeline()
        self.cards = Cards()
        self.recorder = None
        self.load = subprocess.Popen(
            [sys.executable, str(ROOT / "tools/demo/load.py"), "12"], start_new_session=True
        )

    # Pacing and annotations for the video.

    def wait(self, seconds):
        time.sleep(seconds)

    def caption(self, text):
        self.timeline.mark("caption", text=text)

    def focus(self, rect=None, zoom=None):
        """Point the video's camera at a logical rect, or back at the whole screen."""
        self.timeline.mark("focus", rect=rect, zoom=zoom)

    def key(self, *keys, label=None):
        self.timeline.mark("key", label=label or " + ".join(k.capitalize() for k in keys))
        if len(keys) == 1:
            self.input.key(keys[0])
        else:
            self.input.combo(*keys)

    def still(self, name):
        if MODE == "stills":
            time.sleep(0.4)
            subprocess.run(["grim", str(STILLS / f"{name}.png")], env=ENV, check=True)
            print("still", name)

    # Recording.

    def start(self):
        self.input.warp(*PARK)
        if MODE == "video":
            raw = DEMO / "raw.mkv"
            raw.unlink(missing_ok=True)
            self.recorder = subprocess.Popen(
                ["wf-recorder", "-y", "-c", "libx264", "-p", "preset=ultrafast", "-p", "crf=10",
                 "-r", "60", "-f", str(raw)],
                env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
            time.sleep(2.0)
        else:
            shutil.rmtree(STILLS, ignore_errors=True)
            STILLS.mkdir(parents=True)
        # The sync jump: render.py finds this frame and starts the timeline there.
        self.input.warp(*SYNC)
        self.timeline.start()
        self.timeline.mark("sync", park=PARK, to=SYNC)

    def finish(self):
        if self.recorder:
            self.recorder.send_signal(signal.SIGINT)
            self.recorder.wait()
        os.killpg(self.load.pid, signal.SIGTERM)
        self.input.close()
        if MODE == "video":
            self.timeline.save(DEMO / "timeline.json")

    # The tour.

    def run(self):
        self.start()
        win = sway_windows()
        self.timeline.mark("title")
        self.wait(3.2)
        self.area()
        self.window(win)
        self.annotate()
        self.pixelate()
        self.record(win)
        self.screen()
        self.outro()
        self.finish()

    def area(self):
        self.caption("Select any area, on a frozen screen")
        self.input.glide(*UNDER_TOOLBAR)
        self.wait(0.2)
        self.key("print", label="PrtSc")
        self.wait(0.9)
        self.focus((20, 440, 720, 660), zoom=1.7)
        self.input.glide(30, 478, ms=700)
        self.wait(0.35)
        # Captured on release.
        self.input.do("down left")
        self.wait(0.1)
        self.input.glide(630, 614, ms=1500, arc=0.03)
        self.wait(0.5)
        self.still("select-area")
        self.input.do("up left")
        self.cards.add(1200, 272)
        self.focus(None)
        self.wait(0.5)

        self.caption("Every capture lands in a preview card")
        card = self.cards.rect(0)
        self.focus((card[0] - 180, card[1] - 120, 1920, 1080), zoom=1.9)
        self.input.glide(*self.cards.buttons(0)["copy"], ms=1100)
        self.wait(0.6)
        self.still("card-hover")
        self.input.click()
        self.wait(1.1)

    def window(self, win):
        self.focus(None)
        self.caption("Or snap to a window")
        self.input.glide(*UNDER_TOOLBAR, ms=800)
        self.wait(0.2)
        self.key("print", label="PrtSc")
        self.wait(0.9)
        btop, code = win["term-btop"], win["term-code"]
        self.input.glide(1300, 860, ms=700)
        self.wait(0.7)
        self.input.glide(1250, 380, ms=900)
        self.wait(0.6)
        self.still("select-window")
        self.input.click()
        w, h = (code[2] - code[0]) * 2, (code[3] - code[1]) * 2
        self.cards.add(w, h)
        self.wait(1.3)

    def annotate(self):
        self.caption("Annotate right from the card")
        card = self.cards.rect(1)
        self.focus((card[0] - 200, card[1] - 200, 1920, 1080), zoom=1.7)
        self.input.glide(*self.cards.buttons(1)["edit"], ms=1000)
        self.wait(0.5)
        self.input.click()
        self.cards.remove(1)  # the edit takes the capture off its card
        self.wait(1.0)
        self.focus(None)
        # The editor centres the capture, its toolbars under it.
        self.caption("Arrows, shapes, text and numbered steps")
        self.key("r")
        self.input.glide(636, 556, ms=800)
        self.input.drag(768, 585, ms=650)
        self.wait(0.3)
        self.key("a")
        self.input.glide(1030, 640, ms=600)
        self.input.drag(782, 578, ms=550, arc=0.1)
        self.wait(0.3)
        self.key("t")
        self.input.click(1040, 628)
        self.wait(0.3)
        self.input.type("cap the backoff?", cps=14)
        self.wait(0.3)
        self.key("escape", label="Esc")
        self.wait(0.2)
        self.key("n")
        self.input.click(452, 398)
        self.wait(0.35)
        self.input.click(452, 511)
        self.wait(0.8)
        self.still("annotate")
        self.caption("Copy it and get back to work")
        self.key("ctrl", "c", label="Ctrl + C")
        self.wait(1.4)

    def pixelate(self):
        self.caption("Pixelate anything private")
        card = self.cards.rect(0)
        self.focus((card[0] - 200, card[1] - 150, 1920, card[3] + 30), zoom=1.7)
        self.input.glide(*self.cards.buttons(0)["edit"], ms=1000)
        self.wait(0.4)
        self.input.click()
        self.cards.remove(0)
        self.wait(1.0)
        # The logs capture, centred: 600x136 at (660, 422).
        x0, y0 = 960 - 300, 422
        self.focus((x0 - 40, y0 - 40, x0 + 640, y0 + 240), zoom=1.8)
        self.key("b")
        for line in (1, 3, 5):
            y = y0 + (511 - 478) + (line - 1) * 22.5 - 11
            self.input.glide(x0 + 384, y, ms=450)
            self.input.drag(x0 + 548, y + 20, ms=420)
            self.wait(0.15)
        self.wait(0.7)
        self.still("pixelate")
        self.key("ctrl", "c", label="Ctrl + C")
        self.wait(0.9)
        self.focus(None)

    def record(self, win):
        self.caption("Record a region, with a tiny timer pill")
        self.key("alt", "print", label="Alt + PrtSc")
        self.wait(1.0)
        btop = win["term-btop"]
        self.input.glide(btop[0] + 30, btop[1] + 24, ms=800)
        self.input.drag(btop[2] - 12, btop[3] - 14, ms=1100)
        self.wait(0.5)
        self.input.glide(1035, 55, ms=900)  # Record
        self.wait(0.3)
        self.input.click()
        self.focus((btop[0] - 20, btop[1] - 60, btop[2] + 20, btop[3] + 20), zoom=1.4)
        self.input.glide(1300, 560, ms=1000)
        self.wait(3.4)  # countdown
        self.caption("…and its status in your bar")
        self.focus((1400, 0, 1920, 300), zoom=2.2)
        self.wait(2.2)
        self.still("recording")
        self.focus(None)
        rx = (btop[0] + 30 + btop[2] - 12) / 2
        stop = (rx + 42, btop[1] + 24 - 32)
        self.wait(0.8)
        self.input.glide(*stop, ms=900)
        self.wait(0.4)
        self.input.click()
        self.cards.add(2 * (btop[2] - 12 - btop[0] - 30), 2 * (btop[3] - 14 - btop[1] - 24))
        self.wait(1.2)

    def screen(self):
        self.caption("Or the whole screen, instantly")
        self.input.glide(1150, 420, ms=800)
        self.key("shift", "print", label="Shift + PrtSc")
        self.cards.add(3840, 2160)
        self.wait(1.6)

    def outro(self):
        self.caption(None)
        self.input.glide(1780, 700, ms=900)
        self.wait(0.6)
        self.still("cards")
        self.timeline.mark("outro")
        self.wait(3.5)


if __name__ == "__main__":
    os.chdir(ROOT)
    Tour().run()
