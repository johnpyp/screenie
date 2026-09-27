"""Print what's on a desktop session's clipboard: tools/desktop.sh run gnome|kde python3
tools/desktop/paste.py [MIME] > FILE. Without a MIME type, lists the types on offer.

GNOME: the clipboard of a Mutter remote desktop session (wl-paste needs data-control,
which GNOME doesn't have). Elsewhere: wl-paste.
"""

import os
import sys

from gi.repository import Gio, GLib


def gnome(mime: str | None) -> None:
    bus = Gio.bus_get_sync(Gio.BusType.SESSION)
    rd = "org.gnome.Mutter.RemoteDesktop"
    (session,) = bus.call_sync(
        rd, "/org/gnome/Mutter/RemoteDesktop", rd, "CreateSession", None, None, 0, 5000, None
    ).unpack()
    types = []
    loop = GLib.MainLoop()

    def owner_changed(_conn, _sender, _path, _iface, _signal, params):
        (options,) = params.unpack()
        types[:] = [
            t
            for group in options.get("mime-types", ())
            for t in (group if isinstance(group, list) else [group])
        ]
        loop.quit()

    bus.signal_subscribe(
        rd, rd + ".Session", "SelectionOwnerChanged", session, None, 0, owner_changed
    )
    bus.call_sync(
        rd,
        session,
        rd + ".Session",
        "EnableClipboard",
        GLib.Variant("(a{sv})", ({},)),
        None,
        0,
        5000,
        None,
    )
    GLib.timeout_add(1000, loop.quit)
    loop.run()
    if mime is None:
        print("\n".join(types))
    else:
        reply, fds = bus.call_with_unix_fd_list_sync(
            rd,
            session,
            rd + ".Session",
            "SelectionRead",
            GLib.Variant("(s)", (mime,)),
            None,
            0,
            5000,
            None,
            None,
        )
        fd = fds.get(reply.unpack()[0])
        os.set_blocking(fd, True)
        with os.fdopen(fd, "rb") as f:
            sys.stdout.buffer.write(f.read())


def main() -> None:
    mime = sys.argv[1] if len(sys.argv) > 1 else None
    if os.environ.get("XDG_CURRENT_DESKTOP") == "GNOME":
        gnome(mime)
    else:
        os.execvp(
            "wl-paste",
            ["wl-paste", "--list-types"] if mime is None else ["wl-paste", "--type", mime],
        )


main()
