"""The overlay editor: one at a time, Done, and the discard prompt."""

import subprocess

import pytest

REGION = ("-r", "200,200 600x400")


def draw_rectangle(input):
    input.do("key r , drag 300 300 500 450")


def test_extra_edit_captures_become_cards(daemon):
    editing = daemon.open_editor(*REGION)
    mark = daemon.mark()
    # Doesn't wait for an edit: delivered straight away, to a preview card.
    second = daemon.cli("shot", "-r", "900,500 300x200", "--edit", timeout=5)
    assert second.returncode == 0, second.stderr
    daemon.wait_log(r"screenshot delivered", after=mark)
    assert daemon.status() == "editing"
    daemon.cli("quit")
    editing.wait(timeout=5)


@pytest.mark.config("screenshot:\n  after_capture: { copy: true }\n")
def test_done_copies_and_closes(daemon, input):
    shot = daemon.open_editor(*REGION)
    draw_rectangle(input)
    input.keys("enter")
    daemon.wait_status("idle")
    assert shot.wait(timeout=5) == 0
    types = subprocess.run(
        ["wl-paste", "-l"], env=daemon.env, capture_output=True, text=True, timeout=5, check=True
    )
    assert "image/png" in types.stdout.split()


def test_closing_asks_before_discarding(daemon, input):
    shot = daemon.open_editor(*REGION)
    draw_rectangle(input)
    input.keys("escape", "escape")  # deselect, then close: asks instead
    input.keys("escape")  # dismisses the prompt
    assert daemon.status() == "editing"
    input.keys("escape", "enter")  # asks again; Enter picks its main button (Save)
    daemon.wait_status("idle")
    assert shot.wait(timeout=5) == 0
    assert len(daemon.screenshots) == 1
    # The command waited for the edit, and printed what it saved.
    assert shot.stdout.read().strip() == str(daemon.screenshots[0])


@pytest.mark.config("editor:\n  confirm_discard: false\n")
def test_closing_discards_without_asking_if_configured(daemon, input):
    shot = daemon.open_editor(*REGION)
    draw_rectangle(input)
    input.keys("escape", "escape")
    daemon.wait_status("idle")
    assert shot.wait(timeout=5) == 1  # discarded
