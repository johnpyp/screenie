"""ui_scale sizes the interface but never moves screen geometry, on any display scale."""

import subprocess
from pathlib import Path

import pytest

from harness import Rect


@pytest.mark.parametrize("ui_scale", [0.75, 1.5])
@pytest.mark.parametrize("output", ["HEADLESS-1", "HEADLESS-2"])  # 1x and 1.5x
def test_editor_shows_the_capture_in_place(session, daemon, input, home, output, ui_scale):
    daemon.config(f"ui_scale: {ui_scale}\n")
    daemon.wait_log(rf"interface scale scale={ui_scale}")
    o = session.outputs()[output].rect
    region = Rect(o.x + 300, o.y + 200, 600, 400)
    # Just inside and just outside two corners (the bars hang below the capture).
    inside = [(region.x + 4, region.y + 4), (region.x + region.width - 4, region.y + 4)]
    outside = [(region.x - 4, region.y - 4), (region.x + region.width + 4, region.y + 4)]
    before = session.screenshot(home / "before.png")

    shot = daemon.open_editor(
        "region", f"{region.x:.0f},{region.y:.0f} {region.width:.0f}x{region.height:.0f}"
    )
    after = session.screenshot(home / "after.png")
    for p in inside:
        assert after.pixel(*p) == before.pixel(*p), f"the capture moved: {p}"
    for p in outside:
        assert after.brightness(*p) < before.brightness(*p) * 0.7, (
            f"not dimmed around the capture: {p}"
        )
    input.keys("escape")
    # Closed without keeping anything.
    assert shot.wait(timeout=5) == 1


def test_follows_config_changes_live(daemon):
    mark = daemon.mark()
    daemon.config("ui_scale: 1.5\n")
    mark = daemon.wait_log(r"interface scale scale=1.5", after=mark)
    daemon.config("ui_scale: 1\n")
    daemon.wait_log(r"interface scale scale=1.0", after=mark)


@pytest.fixture
def private_bus(session, home):
    """A D-Bus session of our own, whose dconf writes go to the test's directory: GTK
    settings can change without touching the real desktop's."""
    env = {**session.env, "XDG_CONFIG_HOME": str(home / "desktop-config")}
    bus = subprocess.Popen(
        ["dbus-daemon", "--session", "--nofork", "--print-address=1"],
        env=env,
        stdout=subprocess.PIPE,
        text=True,
    )
    address = bus.stdout.readline().strip()
    yield {**env, "DBUS_SESSION_BUS_ADDRESS": address}
    bus.terminate()
    bus.wait(timeout=5)


def test_auto_follows_gtk_text_scaling(daemon, private_bus):
    daemon.env["DBUS_SESSION_BUS_ADDRESS"] = private_bus["DBUS_SESSION_BUS_ADDRESS"]
    mark = daemon.mark()
    daemon.restart()
    # The portal starts on demand on the new bus, which takes a moment.
    mark = daemon.wait_log(r"desktop text scaling scale=1\.0", after=mark, timeout=15)
    subprocess.run(
        ["gsettings", "set", "org.gnome.desktop.interface", "text-scaling-factor", "1.25"],
        env=private_bus,
        check=True,
    )
    daemon.wait_log(r"interface scale scale=1\.25", after=mark)
    assert (Path(private_bus["XDG_CONFIG_HOME"]) / "dconf/user").exists(), (
        "dconf wrote somewhere else"
    )
