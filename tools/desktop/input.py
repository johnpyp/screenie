"""Pointer and keyboard for a desktop session: tools/desktop.sh input gnome|kde CMD...

Commands (several at once separated by `;`, or `-` to read them from stdin, one per
line, keeping the devices between them):

    move X Y                 pointer to X,Y (logical, in the whole layout)
    click [BUTTON]           left (default), right or middle
    down [BUTTON] / up [BUTTON]
    drag X1 Y1 X2 Y2 [STEPS]
    key KEY...               press and release each, e.g. `key Escape`, `key ctrl+c`
    type TEXT
    sleep SECONDS

GNOME: Mutter's RemoteDesktop D-Bus API, with a screen cast of each monitor for absolute
positions. KDE: KWin's fake input protocol, through tools/wlinput (built on the host).
"""

import ctypes
import os
import shlex
import subprocess
import sys
import time
from pathlib import Path

import kwin
from gi.repository import Gio, GLib

BUS = Gio.bus_get_sync(Gio.BusType.SESSION)
BUTTONS = {"left": 0x110, "right": 0x111, "middle": 0x112}
MODIFIERS = {
    "ctrl": "Control_L",
    "shift": "Shift_L",
    "alt": "Alt_L",
    "super": "Super_L",
    "meta": "Super_L",
}


def call(name, path, iface, method, args=None):
    return BUS.call_sync(name, path, iface, method, args, None, 0, 10000, None).unpack()


XKB = ctypes.CDLL("libxkbcommon.so.0")


def keysym(name: str) -> int:
    value = XKB.xkb_keysym_from_name(name.encode(), 0)
    if value == 0:
        sys.exit(f"unknown key: {name}")
    return value


class Gnome:
    RD = "org.gnome.Mutter.RemoteDesktop"
    SC = "org.gnome.Mutter.ScreenCast"

    def __init__(self):
        (self.session,) = call(self.RD, "/org/gnome/Mutter/RemoteDesktop", self.RD, "CreateSession")
        session_id = BUS.call_sync(
            self.RD,
            self.session,
            "org.freedesktop.DBus.Properties",
            "Get",
            GLib.Variant("(ss)", (self.RD + ".Session", "SessionId")),
            None,
            0,
            5000,
            None,
        ).unpack()[0]
        (cast,) = call(
            self.SC,
            "/org/gnome/Mutter/ScreenCast",
            self.SC,
            "CreateSession",
            GLib.Variant(
                "(a{sv})", ({"remote-desktop-session-id": GLib.Variant("s", session_id)},)
            ),
        )
        name = "org.gnome.Mutter.DisplayConfig"
        _, _, logical, _ = call(name, "/org/gnome/Mutter/DisplayConfig", name, "GetCurrentState")
        self.monitors = []
        for x, y, scale, _, _, monitors, _ in logical:
            connector = monitors[0][0]
            (stream,) = call(
                self.SC,
                cast,
                self.SC + ".Session",
                "RecordMonitor",
                GLib.Variant("(sa{sv})", (connector, {})),
            )
            self.monitors.append((x, y, scale, stream))
        call(self.RD, self.session, self.RD + ".Session", "Start")
        # Streams take a moment to be ready for pointer events.
        time.sleep(0.6)

    def _rd(self, method, signature, *args):
        call(self.RD, self.session, self.RD + ".Session", method, GLib.Variant(signature, args))

    def move(self, x, y):
        # The monitor holding the point; positions are relative to it, in its pixels.
        for mx, my, scale, stream in self.monitors:
            if mx <= x and my <= y:
                best = (mx, my, scale, stream)
        mx, my, scale, stream = best
        sx, sy = (x - mx) * scale, (y - my) * scale
        self._rd("NotifyPointerMotionAbsolute", "(sdd)", stream, float(sx), float(sy))

    def button(self, button, pressed):
        self._rd("NotifyPointerButton", "(ib)", BUTTONS[button], pressed)

    def _key(self, sym, pressed):
        self._rd("NotifyKeyboardKeysym", "(ub)", sym, pressed)

    def chord(self, chord):
        syms = [keysym(MODIFIERS.get(k.lower(), k)) for k in chord.split("+")]
        for s in syms:
            self._key(s, True)
        time.sleep(0.02)
        for s in reversed(syms):
            self._key(s, False)

    def type(self, text):
        for ch in text:
            s = keysym({" ": "space", "\n": "Return"}.get(ch, ch))
            self._key(s, True)
            self._key(s, False)
            time.sleep(0.01)

    def close(self):
        call(self.RD, self.session, self.RD + ".Session", "Stop")


class Kde:
    """wlinput, kept running, on KWin's fake input."""

    WLINPUT = Path(__file__).resolve().parents[2] / "target/debug/wlinput"

    def __init__(self):
        exe = os.path.realpath(self.WLINPUT)
        kwin.allow("input", exe, wayland=("org_kde_kwin_fake_input",))
        self.proc = subprocess.Popen(
            [exe, "-"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
        )
        if self.proc.stdout.readline().strip() != "ready":
            sys.exit("wlinput didn't start")

    def _send(self, *commands):
        self.proc.stdin.write(" , ".join(commands) + "\n")
        self.proc.stdin.flush()
        answer = self.proc.stdout.readline().strip()
        if answer != "ok":
            sys.exit(f"wlinput: {answer}")

    def move(self, x, y):
        self._send(f"move {x} {y}")

    def button(self, button, pressed):
        self._send(f"{'down' if pressed else 'up'} {button}")

    def chord(self, chord):
        *mods, key = [k.lower() for k in chord.split("+")]
        mods = ["super" if m == "meta" else m for m in mods]
        self._send(
            *[f"hold {m}" for m in mods], f"key {key}", *[f"release {m}" for m in reversed(mods)]
        )

    def type(self, text):
        self._send(f"type {text}")

    def close(self):
        self.proc.stdin.close()
        self.proc.wait()


def run(device, words):
    cmd, args = words[0], words[1:]
    if cmd == "move":
        device.move(float(args[0]), float(args[1]))
    elif cmd in ("click", "down", "up"):
        button = args[0] if args else "left"
        if cmd in ("click", "down"):
            device.button(button, True)
        if cmd in ("click", "up"):
            time.sleep(0.02)
            device.button(button, False)
    elif cmd == "drag":
        x1, y1, x2, y2 = map(float, args[:4])
        steps = int(args[4]) if len(args) > 4 else 10
        device.move(x1, y1)
        time.sleep(0.03)
        device.button("left", True)
        for i in range(1, steps + 1):
            time.sleep(0.016)
            device.move(x1 + (x2 - x1) * i / steps, y1 + (y2 - y1) * i / steps)
        time.sleep(0.03)
        device.button("left", False)
    elif cmd == "key":
        for chord in args:
            device.chord(chord)
            time.sleep(0.02)
    elif cmd == "type":
        device.type(" ".join(args))
    elif cmd == "sleep":
        time.sleep(float(args[0]))
    else:
        sys.exit(f"unknown command: {cmd}")
    # Give the compositor a moment to act on it.
    time.sleep(0.02)


def main():
    desktop = os.environ["XDG_CURRENT_DESKTOP"]
    device = {"GNOME": Gnome, "KDE": Kde}[desktop]()
    try:
        if sys.argv[1:] == ["-"]:
            for line in sys.stdin:
                if line.strip():
                    run(device, shlex.split(line))
                    print("ok", flush=True)
        else:
            command = []
            for word in [*sys.argv[1:], ";"]:
                if word != ";":
                    command.append(word)
                elif command:
                    run(device, command)
                    command = []
    finally:
        device.close()


main()
