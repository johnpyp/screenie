"""Overlays over a game can't trap its locked pointer.

On sway, the app with the keyboard gets its pointer lock back (a game, as soon as it's
focused), and while the cursor is on another surface every motion is dropped. An overlay that had the pointer but not the keyboard would hold the cursor
for good. So an overlay holds the keyboard while the pointer is on it.
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
    """Move onto `overlay`, have the game lock the pointer, and move off: the game gets
    the pointer back, and its lock."""
    game.unlock()
    input.move(*GAME)
    mark = game.mark()
    nudges = itertools.cycle([0, 1])

    def onto():
        # Again until it's there: it takes input from its second frame on.
        input.move(overlay[0] + next(nudges), overlay[1])
        return game.seen("pointer leave", mark)

    wait_for(onto, "the pointer to get onto the overlay")
    game.wait("keyboard leave", mark)
    game.lock()
    mark = game.mark()
    input.move(*GAME)
    game.wait("pointer enter", mark)
    game.wait("locked", mark)
    input.move(GAME[0] + 40, GAME[1] + 30)
    game.wait("motion", mark)


@pytest.mark.config("recording:\n  countdown: 0\n")
def test_a_locked_pointer_cant_be_trapped_on_the_pill_or_a_card(session, daemon, input, game, home):
    input.move(*GAME)
    daemon.spawn("record", "--region", REGION, "-o", str(home / "out.mp4"))
    daemon.wait_status("recording", timeout=10)
    escapes(game, input, PILL)

    game.unlock()
    input.move(*GAME)
    assert daemon.cli("stop").returncode == 0
    escapes(game, input, CARD)
    daemon.assert_healthy()
