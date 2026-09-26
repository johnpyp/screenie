"""Whether a recording keeps a fullscreen app on direct scanout.

A fullscreen app (a game) is normally scanned out directly: its buffer goes to the
display without the compositor compositing it, which is also what allows tearing. Any
surface over it, even a transparent one, and some kinds of capture (a software cursor
for recording the pointer) turn that off, adding latency that the app's own frame rate
doesn't show.

Starts a private headless sway 1.10+ with debug logging and two outputs, runs a
fullscreen, uncapped `weston-simple-egl` on the first, records it (the whole output, or
the window by itself) with a throwaway daemon, and prints what wlroots logged about
direct scanout at each step. Doesn't touch the session from tools/session.sh. Usage:

    uv run --project tests/e2e tools/scanout.py [screen|window] [seconds] [recording settings]

e.g. `... screen 3 "cursor: true"` to see what recording the pointer costs.

On wlroots, capturing an output holds direct scanout off for as long as it lasts
("Disabling direct scan-out … (locks: 1)"); capturing a window renders it separately
and holds nothing. What screenie shows on screen shouldn't change either.
"""

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
mode = sys.argv[1] if len(sys.argv) > 1 else "screen"
seconds = float(sys.argv[2]) if len(sys.argv) > 2 else 3
settings = sys.argv[3] if len(sys.argv) > 3 else ""

home = Path(tempfile.mkdtemp(prefix="scanout-", dir=ROOT / ".cache"))
env_file = home / "sway.env"
sway_log = home / "sway.log"
(home / "sway.conf").write_text(f"""\
output HEADLESS-1 resolution 1920x1080 position 0 0 scale 1
output HEADLESS-2 resolution 1920x1080 position 1920 0 scale 1
default_border none
exec sh -c 'echo "WAYLAND_DISPLAY=$WAYLAND_DISPLAY SWAYSOCK=$SWAYSOCK" > {env_file}'
""")

sway = subprocess.Popen(
    ["sway", "-d", "-c", str(home / "sway.conf")],
    env={k: v for k, v in os.environ.items() if k not in ("WAYLAND_DISPLAY", "DISPLAY", "SWAYSOCK")}
    | {
        "WLR_BACKENDS": "headless",
        "WLR_HEADLESS_OUTPUTS": "2",
        "WLR_LIBINPUT_NO_DEVICES": "1",
        "WLR_RENDERER": os.environ.get("WLR_RENDERER", "gles2"),
        "XDG_CURRENT_DESKTOP": "sway",
        "XDG_SESSION_TYPE": "wayland",
    },
    stdout=subprocess.DEVNULL,
    stderr=sway_log.open("w"),
    start_new_session=True,
)
events: list[tuple[int, str]] = []


def mark(what: str) -> None:
    """Note that `what` happened at this point in the compositor's log."""
    time.sleep(0.5)  # let the compositor's reaction land in the log first
    events.append((len(sway_log.read_text(errors="replace")), what))


try:
    wait_for(lambda: env_file.exists() and env_file.read_text().strip(), "sway to start", timeout=10)
    session = Session(
        {k: v for k, v in os.environ.items() if k != "DISPLAY"}
        | dict(pair.split("=", 1) for pair in env_file.read_text().split())
    )
    session.swaymsg("focus output HEADLESS-1")
    player = subprocess.Popen(["weston-simple-egl", "-f", "-b"], env=session.env, stdout=subprocess.DEVNULL)
    daemon = Daemon(session, home)
    daemon.config(f"recording: {{ countdown: 0, {settings} }}\n")
    try:
        wait_for(lambda: session.window("simple-egl"), "the demo", timeout=10)
        time.sleep(1)
        mark("demo fullscreen")
        daemon.start()
        mark("daemon started")
        if mode == "window":
            input = Input(session)
            daemon.spawn("record", "window", "-o", str(home / "out.mp4"))
            daemon.wait_log(r"selector keyboard focus .*active=true")
            input.click(960, 540)
            input.keys("return")
            input.close()
        else:
            daemon.spawn("record", "screen", "-o", str(home / "out.mp4"))
        daemon.wait_status("recording", timeout=10)
        mark("recording started")
        time.sleep(seconds)
        mark(f"recording for {seconds:g}s")
        stop = daemon.mark()
        daemon.cli("record")
        daemon.wait_log(r"recording saved", after=stop, timeout=30)
        time.sleep(2)  # the preview card comes and goes on the other output
        mark("recording stopped")
    finally:
        daemon.stop()
        player.kill()
finally:
    sway.terminate()
    sway.wait()

log = sway_log.read_text(errors="replace")
pattern = re.compile(r"[Dd]irect scan-out.*|software cursors? .*")
start = 0
for end, what in events:
    lines = [m.group(0) for m in pattern.finditer(log[start:end])]
    print(f"{what}:")
    for line in lines[-6:]:
        print(f"    {line}")
    if not lines:
        print("    (no change)")
    start = end
print("sway log:", sway_log)
