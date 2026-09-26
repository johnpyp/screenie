"""The preview cards take the keyboard only while the pointer is on one.

A card floats over whatever you were doing (a game, a terminal). Its keys must reach it
while the pointer is on it (Esc dismissing the card, not unpausing the game), and never
otherwise.
"""

import time

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


def test_escape_on_a_card_dismisses_it_not_the_app_beneath(session, daemon, input, keylog, home):
    keylog.focus()
    mark = keylog.mark()
    card = Card(session, home)
    assert daemon.cli(*SHOT).returncode == 0
    card.wait_shown()

    log = daemon.mark()
    input.move(*CARD)
    daemon.wait_log(r"hover keyboard taken=true", after=log)
    mark = keylog.wait(r"leave", after=mark)  # the card has the keyboard
    input.keys("escape")
    card.wait_gone()
    mark = keylog.wait(r"enter", after=mark)  # and gave it back
    time.sleep(0.05)  # a leaked release arrives right behind the enter
    leaked = [line for line in keylog.since(mark - 1) if "key:" in line or line.startswith(" ") and "sym:" in line]
    assert leaked == []
    daemon.assert_healthy()


def test_moving_off_a_card_gives_the_keyboard_back(session, daemon, input, keylog, home):
    keylog.focus()
    mark = keylog.mark()
    card = Card(session, home)
    assert daemon.cli(*SHOT).returncode == 0
    card.wait_shown()
    input.move(*CARD)
    mark = keylog.wait(r"leave", after=mark)
    input.move(900, 500)
    mark = keylog.wait(r"enter", after=mark)
    input.keys("x")
    keylog.wait(r"sym: x", after=mark)  # typing reaches the app again


def test_a_card_under_a_still_pointer_leaves_the_keyboard_alone(session, daemon, input, keylog, home):
    input.move(*CARD)
    keylog.focus()
    mark = keylog.mark()
    card = Card(session, home)
    assert daemon.cli(*SHOT).returncode == 0
    card.wait_shown()
    input.keys("x")
    keylog.wait(r"sym: x", after=mark)  # typing still goes to the app
    assert not any("leave" in line for line in keylog.since(mark))
    assert "hover keyboard" not in daemon.log()
