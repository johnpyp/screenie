"""Screenshot every output of a desktop session, laid out side by side at one pixel per
logical pixel: tools/desktop.sh shot gnome|kde [FILE].

GNOME: a Mutter screen cast of each monitor (one frame from PipeWire). KDE: KWin's
ScreenShot2 (allowed by the desktop file this installs for the Python running it).
"""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

from gi.repository import Gio, GLib

BUS = Gio.bus_get_sync(Gio.BusType.SESSION)


def call(name, path, iface, method, args=None, fds=None):
    if fds is None:
        return BUS.call_sync(name, path, iface, method, args, None, 0, 10000, None).unpack()
    reply, _ = BUS.call_with_unix_fd_list_sync(name, path, iface, method, args, None, 0, 10000, fds, None)
    return reply.unpack()


def gnome_outputs():
    """(connector, logical x, logical y, scale) of each monitor."""
    name, path = "org.gnome.Mutter.DisplayConfig", "/org/gnome/Mutter/DisplayConfig"
    _, _, logical, _ = call(name, path, name, "GetCurrentState")
    return [(monitors[0][0], x, y, scale) for x, y, scale, _, _, monitors, _ in logical]


def gnome_capture(connector: str, out: Path) -> None:
    mc = "org.gnome.Mutter.ScreenCast"
    (session,) = call(mc, "/org/gnome/Mutter/ScreenCast", mc, "CreateSession", GLib.Variant("(a{sv})", ({},)))
    (stream,) = call(
        mc,
        session,
        mc + ".Session",
        "RecordMonitor",
        GLib.Variant("(sa{sv})", (connector, {"cursor-mode": GLib.Variant("u", 1)})),
    )
    loop = GLib.MainLoop()
    node = []

    def added(_conn, _sender, _path, _iface, _signal, params):
        node.append(params[0])
        loop.quit()

    BUS.signal_subscribe(None, mc + ".Stream", "PipeWireStreamAdded", stream, None, 0, added)
    call(mc, session, mc + ".Session", "Start")
    GLib.timeout_add(5000, loop.quit)
    loop.run()
    try:
        subprocess.run(
            ["gst-launch-1.0", "-q", "pipewiresrc", f"path={node[0]}", "num-buffers=1", "always-copy=true",
             "!", "videoconvert", "!", "pngenc", "!", "filesink", f"location={out}"],
            check=True,
            timeout=20,
        )
    finally:
        call(mc, session, mc + ".Session", "Stop")


def kde_outputs():
    out = subprocess.run(["kscreen-doctor", "-j"], check=True, capture_output=True, text=True).stdout
    import json

    return [
        (o["name"], o["pos"]["x"], o["pos"]["y"], o["scale"])
        for o in json.loads(out)["outputs"]
        if o["enabled"]
    ]


def kde_capture(name: str, out: Path) -> None:
    allow_kwin_screenshots()
    r, w = os.pipe()
    fds = Gio.UnixFDList.new()
    fds.append(w)
    os.close(w)
    (meta,) = call(
        "org.kde.KWin",
        "/org/kde/KWin/ScreenShot2",
        "org.kde.KWin.ScreenShot2",
        "CaptureScreen",
        GLib.Variant("(sa{sv}h)", (name, {"include-cursor": GLib.Variant("b", True),
                                          "native-resolution": GLib.Variant("b", True)}, 0)),
        fds,
    )
    del fds
    data = bytearray()
    while chunk := os.read(r, 1 << 20):
        data += chunk
    os.close(r)
    # QImage::Format_ARGB32 or _RGB32 is BGRA in memory.
    w, h, stride = meta["width"], meta["height"], meta["stride"]
    rows = b"".join(bytes(data[y * stride : y * stride + w * 4]) for y in range(h))
    subprocess.run(["magick", "-size", f"{w}x{h}", "-depth", "8", "bgra:-", str(out)], input=rows, check=True)


def allow_kwin_screenshots() -> None:
    """KWin lets a process take screenshots when a desktop file with its executable asks.
    It looks among the apps a menu would show, so the file can't be NoDisplay."""
    apps = Path.home() / ".local/share/applications"
    desktop = apps / "screenie-desktop-shot.desktop"
    entry = (
        "[Desktop Entry]\nType=Application\nName=Desktop shot\n"
        f"Exec={os.path.realpath(sys.executable)}\n"
        "X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2\n"
    )
    if desktop.exists() and desktop.read_text() == entry:
        return
    apps.mkdir(parents=True, exist_ok=True)
    desktop.write_text(entry)
    subprocess.run(["kbuildsycoca6"], capture_output=True)


def main() -> None:
    target = Path(sys.argv[1])
    desktop = os.environ["XDG_CURRENT_DESKTOP"]
    outputs, capture = {"GNOME": (gnome_outputs, gnome_capture), "KDE": (kde_outputs, kde_capture)}[desktop]
    with tempfile.TemporaryDirectory() as tmp:
        # A canvas as big as the layout, and each output composited at its place, at one
        # pixel per logical pixel.
        layers, width, height = [], 0, 0
        for name, x, y, scale in outputs():
            png = Path(tmp) / f"{name}.png"
            capture(name, png)
            size = subprocess.run(
                ["magick", "identify", "-format", "%w %h", str(png)], check=True, capture_output=True, text=True
            ).stdout.split()
            width = max(width, x + round(int(size[0]) / scale))
            height = max(height, y + round(int(size[1]) / scale))
            layers += ["(", str(png), "-resize", f"{100 / scale}%", ")", "-geometry", f"+{x}+{y}", "-composite"]
        subprocess.run(["magick", "-size", f"{width}x{height}", "xc:black", *layers, str(target)], check=True)
    print(target)


main()
