"""Record fast-changing 4K content in the headless session and report dropped frames.

Adds a 3840x2160 output with a fullscreen, uncapped `weston-simple-egl` on
it (needs the weston package), records it with a throwaway daemon, and prints the frame
count and every gap in the video's timestamps. Usage (the session from tools/session.sh
must be running):

    uv run --project tests/e2e tools/rec_stress.py [window|screen|region] [seconds] [recording settings] [GST_DEBUG]

`region` records an odd-sized part of the output around the demo, which exercises cropping.

e.g. `... screen 6 "resolution: native, framerate: native"`, or `... screen 4 "" appsrc:5`
to see GStreamer's view in the daemon log. `SCREENIE_CONFIG` adds to the daemon's
config, e.g. `SCREENIE_CONFIG="advanced: { capture_backend: wlr }"` to capture with
wlr-screencopy.
"""

import itertools
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tests/e2e"))
from harness import Daemon, Input, Session, wait_for

ROOT = Path(__file__).resolve().parents[1]
mode = sys.argv[1] if len(sys.argv) > 1 else "window"
seconds = float(sys.argv[2]) if len(sys.argv) > 2 else 8
settings = sys.argv[3] if len(sys.argv) > 3 else ""

session = Session.load()
home = Path(tempfile.mkdtemp(prefix="rec-stress-", dir=ROOT / ".cache"))
out = home / "stress.mp4"


def record(big: str) -> Daemon:
    """Record `big` (an output) for `seconds`, with the demo fullscreen on it."""
    session.swaymsg(f"output {big} resolution 3840x2160 position 3700 0 scale 1")
    session.swaymsg(f"focus output {big}")
    # weston's EGL demo, fullscreen and not waiting for frame callbacks (like a game with
    # an uncapped frame rate): new content as fast as the GPU draws it.
    player = subprocess.Popen(["weston-simple-egl", "-f", "-b", "-o"], env=session.env, stdout=subprocess.DEVNULL)
    input = Input(session)
    daemon = Daemon(session, home)
    if len(sys.argv) > 4:
        daemon.env["GST_DEBUG"] = sys.argv[4]
    daemon.config(f"recording: {{ countdown: 0, {settings} }}\n{os.environ.get('SCREENIE_CONFIG', '')}\n")
    daemon.start()
    try:
        wait_for(lambda: session.window("simple-egl"), "the demo", timeout=10)
        time.sleep(1)
        if mode == "window":
            daemon.spawn("record", "window", "-o", str(out))
            daemon.wait_log(r"selector keyboard focus .*active=true")
            o = session.outputs()[big].rect
            input.click(o.x + o.width / 2, o.y + o.height / 2)
            input.keys("return")
        elif mode == "region":
            o = session.outputs()[big].rect
            region = f"{o.x + o.width // 4 + 1},{o.y + o.height // 4 + 1} {o.width // 2 + 1}x{o.height // 2 + 1}"
            daemon.spawn("record", "--region", region, "-o", str(out))
        else:
            daemon.spawn("record", "screen", "-o", str(out))
        daemon.wait_status("recording", timeout=10)
        time.sleep(seconds)
        mark = daemon.mark()
        daemon.cli("record")
        daemon.wait_log(r"recording saved", after=mark, timeout=60)
        return daemon
    finally:
        daemon.stop()
        player.kill()
        input.close()


before = set(session.outputs())
session.swaymsg("create_output")  # named HEADLESS-<n>, never reused
big = wait_for(lambda: next(iter(set(session.outputs()) - before), None), "the new output")
try:
    daemon = record(big)
finally:
    # Leave the session as the e2e tests expect it.
    session.swaymsg(f"output {big} unplug")
    session.swaymsg("focus output HEADLESS-1")

pts = subprocess.run(
    [
        "ffprobe",
        "-v",
        "error",
        "-select_streams",
        "v",
        "-show_entries",
        "frame=pts_time",
        "-of",
        "csv=p=0",
        str(out),
    ],
    capture_output=True,
    text=True,
    check=True,
).stdout.split()
pts = [float(p.split(",")[0]) for p in pts if p.split(",")[0]]
gaps = [(a, b) for a, b in itertools.pairwise(pts) if b - a > 0.1]
print(f"{len(pts)} frames over {pts[-1]:.2f}s ({len(pts) / pts[-1]:.1f} fps), {len(gaps)} gaps > 100ms")
for a, b in gaps[:20]:
    print(f"  {a:7.3f} -> {b:7.3f}  ({b - a:.3f}s)")
log = daemon.log()
stats = [line for line in log.splitlines() if "recorded frames" in line]
print(stats[-1].split("screenie_record: ")[-1] if stats else "no frame stats logged")
encoder = re.search(r'encoder="([^"]+)"', log)
rate = subprocess.run(
    ["ffprobe", "-v", "error", "-show_entries", "format=bit_rate", "-of", "csv=p=0", str(out)],
    capture_output=True,
    text=True,
    check=False,
).stdout.strip()
print(f"encoder {encoder[1] if encoder else '?'}, {int(rate or 0) / 1e6:.1f} Mbit/s")
print("log:", daemon.log_path)
