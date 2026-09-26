"""Closing an overlay with the keyboard must not leak the key into the app beneath.

When a surface closes on a key press, the compositor hands the still-held key to the
next focused window, which sees it held on enter and then gets its release.
"""

import time

import pytest

EDIT = ("shot", "-r", "200,200 600x400", "--edit")


@pytest.mark.parametrize(
    ("keys", "args", "exit_code"),
    [
        # `shot --edit` lasts the edit: closed without Done or keeping anything, it's cancelled.
        pytest.param("key escape", EDIT, 1, id="editor-escape"),
        pytest.param("key enter", EDIT, 0, id="editor-enter-done"),
        pytest.param("hold ctrl , key q , release ctrl", EDIT, 1, id="editor-ctrl-q"),
        pytest.param("key escape", ("shot",), 1, id="selector-escape"),
        pytest.param("key enter", ("shot", "--no-preview"), 0, id="selector-enter"),
    ],
)
def test_closing_with_keys_doesnt_leak(daemon, input, keylog, keys, args, exit_code):
    keylog.focus()
    mark = keylog.mark()
    shot = daemon.spawn(*args)
    mark = keylog.wait(r"leave", after=mark)  # the overlay has the keyboard
    input.do(keys)
    mark = keylog.wait(r"enter", after=mark)  # it closed, and focus came back
    time.sleep(0.05)  # a leaked release arrives right behind the enter
    # Keys still held are listed under the enter as `sym:` lines; their release as `key:`.
    leaked = [
        line
        for line in keylog.since(mark - 1)
        if "key:" in line or line.startswith(" ") and "sym:" in line
    ]
    assert leaked == []
    assert shot.wait(timeout=5) == exit_code
