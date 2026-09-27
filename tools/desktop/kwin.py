"""Letting a test tool use KWin's restricted interfaces."""

import os
import subprocess
from pathlib import Path


def allow(name: str, exe: str, dbus: tuple[str, ...] = (), wayland: tuple[str, ...] = ()) -> None:
    """KWin lets a process use its restricted D-Bus and Wayland interfaces when the desktop
    entry whose Exec is the process's binary asks for them. It looks among the apps a menu
    would show, so the entry can't be NoDisplay."""
    apps = Path.home() / ".local/share/applications"
    desktop = apps / f"screenie-desktop-{name}.desktop"
    entry = f"[Desktop Entry]\nType=Application\nName={name}\nExec={os.path.realpath(exe)}\n"
    if dbus:
        entry += f"X-KDE-DBUS-Restricted-Interfaces={','.join(dbus)}\n"
    if wayland:
        entry += f"X-KDE-Wayland-Interfaces={','.join(wayland)}\n"
    if desktop.exists() and desktop.read_text() == entry:
        return
    apps.mkdir(parents=True, exist_ok=True)
    desktop.write_text(entry)
    subprocess.run(["kbuildsycoca6"], capture_output=True)
