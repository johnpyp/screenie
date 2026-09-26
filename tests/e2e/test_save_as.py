"""Save As from the overlay editor: the overlay steps aside for the portal's file
chooser and comes back, whether the dialog is cancelled or saves.

Regression: GPUI exported the overlay (a layer surface) as the dialog's parent, a
protocol error that killed the daemon's Wayland connection.
"""

import time

from harness import wait_for

DIALOG = "Save File"


def open_save_as(daemon, input, session):
    shot = daemon.open_editor("-r", "200,200 600x400")
    input.chord("ctrl", "shift", key="s")
    wait_for(
        lambda: (w := session.window(DIALOG)) and w["focused"],
        "the file chooser to take focus",
        timeout=10,
    )
    return shot


def dialog_gone(session):
    wait_for(lambda: session.window(DIALOG) is None, "the file chooser to close")


def test_cancelling_brings_the_editor_back(session, daemon, input):
    shot = open_save_as(daemon, input, session)
    mark = daemon.mark()
    input.keys("escape")
    dialog_gone(session)
    daemon.wait_log(r"editor keyboard focus active=true", after=mark)  # back, with the keyboard
    input.keys("escape")
    daemon.wait_status("idle")
    assert shot.wait(timeout=5) == 1  # nothing kept
    # And the daemon still works.
    next_shot = daemon.tmp / "next.png"
    assert (
        daemon.cli("shot", "-r", "100,100 300x200", "--no-preview", "-o", str(next_shot)).returncode
        == 0
    )
    assert next_shot.read_bytes().startswith(b"\x89PNG")
    daemon.assert_healthy()


def test_saving_writes_the_file_and_brings_the_editor_back(session, daemon, input):
    shot = open_save_as(daemon, input, session)
    input.chord("ctrl", key="a")
    input.type("e2e-saved")
    # GTK checks the typed name asynchronously and ignores Enter until it has, with
    # nothing to observe: the one fixed wait in the suite.
    time.sleep(0.5)
    mark = daemon.mark()
    input.keys("enter")
    dialog_gone(session)
    # The dialog starts in the screenshot folder for a capture with no file yet, and
    # `.png` is added.
    saved = daemon.home / "Pictures/Screenshots/e2e-saved.png"
    wait_for(saved.exists, f"{saved} to be written")
    assert saved.read_bytes().startswith(b"\x89PNG")
    daemon.wait_log(r"editor keyboard focus active=true", after=mark)
    input.keys("escape")
    daemon.wait_status("idle")
    assert shot.wait(timeout=5) == 0
    assert shot.stdout.read().strip() == str(saved)
    daemon.assert_healthy()
