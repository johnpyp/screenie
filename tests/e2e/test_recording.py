"""Recording a window records the window itself: not whatever covers it, and not the
screen it was on once you switch away."""

import subprocess

import pytest
from PIL import Image

from harness import wait_for

NO_COUNTDOWN = "recording:\n  countdown: 0\n"
RED, BLUE = "ff0000", "0000ff"


def content(window):
    """A sway window's content rectangle (inside its border), as (x, y, width, height)."""
    outer, inner = window["rect"], window["window_rect"]
    return outer["x"] + inner["x"], outer["y"] + inner["y"], inner["width"], inner["height"]


def resize(session, app_id, width, height):
    """Resize a floating window; foot rounds the size to whole character cells."""
    session.swaymsg(f"[app_id={app_id}] resize set {width} {height}")
    wait_for(
        lambda: abs(session.window(app_id)["rect"]["width"] - width) < 20, f"{app_id} to be resized"
    )


@pytest.fixture
def solid(session, home):
    """Open a floating window of one colour (a `foot` with nothing in it) on HEADLESS-1."""
    opened = []

    def open_window(app_id, color, width=600, height=400):
        session.swaymsg("focus output HEADLESS-1")
        cmd = [
            "foot",
            "-c",
            "/dev/null",
            "--app-id",
            app_id,
            "-o",
            f"colors.background={color}",
            "sleep",
            "600",
        ]
        opened.append(
            subprocess.Popen(cmd, env={**session.env, "HOME": str(home)}, stderr=subprocess.DEVNULL)
        )
        wait_for(lambda: session.window(app_id), f"the {app_id} window")
        place = f"[app_id={app_id}] floating enable, resize set {width} {height}, move position 300 200, focus"

        def placed():
            # A resize right after mapping can lose to the window's first configure.
            session.swaymsg(place)
            return abs(session.window(app_id)["rect"]["width"] - width) < 20

        wait_for(placed, f"{app_id} to settle", interval=0.2)
        return content(session.window(app_id))

    yield open_window
    for p in opened:
        p.kill()
        p.wait()


@pytest.fixture
def workspace(session):
    """Switch workspaces during the test, and back after."""
    before = next(w["name"] for w in session.swaymsg("-t", "get_workspaces") if w["focused"])
    yield lambda name: session.swaymsg(f"workspace {name}")
    session.swaymsg(f"workspace {before}")


def frames(video, into):
    """Every frame of `video`, decoded."""
    into.mkdir()
    subprocess.run(
        [
            "ffmpeg",
            "-v",
            "error",
            "-i",
            str(video),
            "-fps_mode",
            "passthrough",
            str(into / "%03d.png"),
        ],
        check=True,
    )
    return [Image.open(p).convert("RGB") for p in sorted(into.glob("*.png"))]


def about(size, expected):
    """Sizes equal but for foot's catching up with a resize (it snaps to whole cells)."""
    return all(abs(a - b) <= 16 for a, b in zip(size, expected, strict=True))


def is_red(pixel):
    r, g, b = pixel
    return r > 180 and g < 80 and b < 80


def record_window(daemon, input, window, out, *args):
    """`screenie record ARGS`, picking `window` in the selector (switched to window mode
    with its key, unless ARGS start there)."""
    x, y, w, h = window
    daemon.spawn("record", *args, "-o", str(out))
    daemon.wait_log(r"selector keyboard focus .*active=true")
    if "window" not in args:
        input.keys("2")
    input.click(x + w / 2, y + h / 2)
    input.keys("return")
    daemon.wait_status("recording", timeout=10)


def record_and_stop(daemon, input, window, out, during, *args):
    """Record `window`, run `during()`, then stop and wait for the file."""
    mark = daemon.mark()
    record_window(daemon, input, window, out, *args)
    during()
    assert daemon.cli("record").returncode == 0
    daemon.wait_log(r"recording saved", after=mark, timeout=15)
    assert out.exists()


@pytest.mark.config(NO_COUNTDOWN)
def test_a_picked_window_is_recorded_by_itself(session, daemon, input, home, solid, workspace):
    target = solid("target", RED)
    x, y, w, h = target
    out = home / "window.mp4"

    def during():
        solid("cover", BLUE)  # right on top of it
        wait_for(
            lambda: session.screenshot(home / "covered.png").pixel(x + w / 2, y + h / 2)[2] > 180,
            "the cover",
        )
        workspace("9")
        wait_for(lambda: not session.window("cover")["visible"], "the other workspace")
        input.move(1000, 900)

    record_and_stop(daemon, input, target, out, during)
    video = frames(out, home / "frames")
    assert video, "no frames"
    width, height = video[0].size
    assert about((width, height), (w, h))  # the window's size, not the screen's
    for i, frame in enumerate(video):
        assert is_red(frame.getpixel((width * 3 // 4, height * 3 // 4))), (
            f"frame {i} isn't the window"
        )
    daemon.assert_healthy()


@pytest.mark.config(NO_COUNTDOWN)
def test_a_resized_window_is_fitted_into_the_video(session, daemon, input, home, solid):
    target = solid("target", RED)
    _, _, w, h = target
    out = home / "resized.mp4"

    def during():
        resize(session, "target", 1000, 300)
        daemon.wait_log(r"recorded frames changed")

    record_and_stop(daemon, input, target, out, during, "window")
    video = frames(out, home / "frames")
    first, last = video[0], video[-1]
    width, height = first.size
    assert last.size == first.size and about(first.size, (w, h))
    # Wider now: scaled to the video's width, with bars above and below.
    assert is_red(last.getpixel((width // 2, height // 2)))
    assert max(last.getpixel((width // 2, 20))) < 40


@pytest.mark.config(NO_COUNTDOWN)
def test_closing_the_recorded_window_saves_the_recording(session, daemon, input, home, solid):
    target = solid("target", RED)
    out = home / "closed.mp4"
    mark = daemon.mark()
    record_window(daemon, input, target, out, "window")
    session.swaymsg("[app_id=target] kill")
    daemon.wait_log(r"recording saved", after=mark, timeout=15)
    daemon.wait_status("idle")
    assert frames(out, home / "frames")
    daemon.assert_healthy()
