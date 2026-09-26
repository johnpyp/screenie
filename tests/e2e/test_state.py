"""What the editor remembers survives a daemon restart (state.yaml)."""

import re


def remembered_size(daemon) -> str | None:
    text = daemon.state_file.read_text() if daemon.state_file.exists() else ""
    match = re.search(r"size: ([\d.]+)", text)
    return match and match.group(1)


def test_style_survives_a_restart(daemon, input):
    daemon.open_editor("region", "200,200 600x400")
    input.keys("6")  # the sixth size: 12
    input.keys("escape")  # nothing drawn: closes, remembering the style
    daemon.wait_status("idle")
    assert remembered_size(daemon) == "12.0"

    daemon.restart()
    daemon.open_editor("region", "200,200 600x400")
    input.keys("escape")
    daemon.wait_status("idle")
    # It opened with 12: closing would have written the default back otherwise.
    assert remembered_size(daemon) == "12.0"
