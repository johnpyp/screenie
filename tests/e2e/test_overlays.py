"""Overlays over a game can't trap its locked pointer.

Overlays never take the keyboard, so a game keeps it, and its pointer lock, while the
cursor is on a card or the pill. sway enforces a lock for the surface with the keyboard
and drops every motion while the cursor is over another surface: the cursor would freeze
on the overlay. The overlay sees that the pointer is stuck and steps aside.
"""

import itertools

import pytest

from harness import wait_for

# An 800x600 region in the middle of HEADLESS-1: the pill goes 12 below it, and the
# recording's card (236x177) in the bottom right corner.
REGION = "560,240 800x600"
PILL = (960, 240 + 600 + 12 + 19)
CARD = (1920 - 18 - 118, 1080 - 18 - 88)
GAME = (700, 400)


def escapes(game, input, overlay):
    """Move onto `overlay` and have the game lock the pointer there: moving on, the
    cursor gets back to the game, which keeps the keyboard throughout."""
    game.unlock()
    input.move(*GAME)
    mark = game.mark()
    nudges = itertools.cycle([0, 1])

    def onto():
        # Again until it's there: it takes input from its second frame on.
        input.move(overlay[0] + next(nudges), overlay[1])
        return game.seen("pointer leave", mark)

    wait_for(onto, "the pointer to get onto the overlay")
    game.lock()
    game.wait("locked", mark)

    def off():
        input.move(*GAME)
        return game.seen("pointer enter", mark)

    try:
        wait_for(off, "the pointer to get back to the game")
    except TimeoutError as e:
        raise TimeoutError(f"{e}; the game got {game.since(mark)}") from None
    after = game.mark()
    input.move(GAME[0] + 40, GAME[1] + 30)
    game.wait("motion", after)
    assert game.seen("keyboard leave", mark) is None, "an overlay took the keyboard"


@pytest.mark.config("recording:\n  countdown: 0\n")
def test_a_locked_pointer_cant_be_trapped_on_the_pill_or_a_card(session, daemon, input, game, home):
    input.move(*GAME)
    daemon.spawn("record", "region", REGION, "-o", str(home / "out.mp4"))
    daemon.wait_status("recording", timeout=10)
    escapes(game, input, PILL)

    game.unlock()
    input.move(*GAME)
    assert daemon.cli("stop").returncode == 0
    escapes(game, input, CARD)
    daemon.assert_healthy()
