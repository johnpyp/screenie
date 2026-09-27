"""Lay the session's two outputs out like tools/session.sh's sway: a 1x output on the
left, and a 1.5x one to its right."""

import os
import subprocess

from gi.repository import Gio, GLib


def gnome() -> None:
    bus = Gio.bus_get_sync(Gio.BusType.SESSION)
    name = "org.gnome.Mutter.DisplayConfig"
    path = "/org/gnome/Mutter/DisplayConfig"
    serial, monitors, _, _ = bus.call_sync(
        name, path, name, "GetCurrentState", None, None, 0, 5000
    ).unpack()
    current = {spec[0]: next(m for m in modes if m[6].get("is-current")) for spec, modes, _ in monitors}
    # Mutter only takes scales that give a whole logical size, which 1.5 doesn't for
    # 2560x1440: the nearest it offers.
    scale = min(current["Meta-1"][5], key=lambda s: abs(s - 1.5))
    # Meta-0 is 1920x1080, Meta-1 2560x1440 (see session.sh).
    layout = [
        (0, 0, 1.0, 0, True, [("Meta-0", current["Meta-0"][0], {})]),
        (1920, 0, scale, 0, False, [("Meta-1", current["Meta-1"][0], {})]),
    ]
    bus.call_sync(
        name,
        path,
        name,
        "ApplyMonitorsConfig",
        GLib.Variant("(uua(iiduba(ssa{sv}))a{sv})", (serial, 1, layout, {})),
        None,
        0,
        5000,
    )


def kde() -> None:
    subprocess.run(
        ["kscreen-doctor", "output.Virtual-0.position.0,0", "output.Virtual-1.scale.1.5",
         "output.Virtual-1.position.1920,0"],
        check=True,
        capture_output=True,
    )


{"GNOME": gnome, "KDE": kde}[os.environ["XDG_CURRENT_DESKTOP"]]()
