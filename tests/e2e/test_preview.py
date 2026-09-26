"""The preview cards never take the keyboard.

A card floats over whatever you were doing (a game, a terminal). The pointer can use it,
but typing always stays with the app: keyboard focus that moved to a card mid-press
would split the press, and a game that missed the release keeps the key held (Tab stuck
down, so Shift opened Steam's overlay as Shift+Tab).
"""

from harness import wait_for

# A 400x300 capture on HEADLESS-1 (1920x1080) makes a 236x177 card in the bottom right
# corner, 18 from the edges.
SHOT = ("shot", "-r", "100,100 400x300")
CARD = (1920 - 18 - 118, 1080 - 18 - 88)


class Card:
    """Watches the card's spot on screen, against how it looked before the capture."""

    def __init__(self, session, home):
        self.session = session
        self.probe = home / "probe.png"
        self.empty = self.pixel()

    def pixel(self):
        return self.session.screenshot(self.probe).pixel(*CARD)

    def wait_shown(self):
        wait_for(lambda: self.pixel() != self.empty, "the card to show")

    def wait_gone(self):
        wait_for(lambda: self.pixel() == self.empty, "the card to go")


def test_a_key_held_onto_a_card_is_released_to_the_app(session, daemon, input, keylog, home):
    keylog.focus()
    card = Card(session, home)
    assert daemon.cli(*SHOT).returncode == 0
    card.wait_shown()

    mark = keylog.mark()
    input.do("hold tab")
    input.move(*CARD)
    input.keys("x")
    input.do("release tab")
    keylog.wait(r"sym: x", after=mark)  # typing on the card goes to the app
    wait_for(lambda: sum("sym: Tab" in line for line in keylog.since(mark)) >= 2, "Tab's release")
    tab = [line for line in keylog.since(mark) if "sym: Tab" in line]
    assert len(tab) == 2, f"the app should see Tab go down and up: {keylog.since(mark)}"
    assert not any("leave" in line for line in keylog.since(mark)), "the card took the keyboard"


def test_a_card_is_used_with_the_pointer(session, daemon, input, keylog, home):
    keylog.focus()
    card = Card(session, home)
    assert daemon.cli(*SHOT).returncode == 0
    card.wait_shown()
    mark = keylog.mark()
    input.move(*CARD)
    input.keys("escape")  # the app's, not the card's
    keylog.wait(r"sym: Escape", after=mark)
    # Dismiss: the × in the card's top-left corner shows on hover.
    input.click(CARD[0] - 118 + 20, CARD[1] - 88 + 20)
    card.wait_gone()
    assert not any("leave" in line for line in keylog.since(mark))
    daemon.assert_healthy()
