# Compositor support

Screenie depends on protocols, not compositors. Compositor IPC is only an enhancement
for window snapping and "active window".

| Compositor | Capture | Overlay | Clipboard | Windows (IPC) | Tested |
| --- | --- | --- | --- | --- | --- |
| Sway | wlr / ext | layer-shell | data-control | i3 IPC | ✅ headless |
| Hyprland | wlr (ext on 0.50+) | layer-shell | data-control | hyprctl socket | unit tests |
| niri | ext / wlr | layer-shell | data-control | niri IPC | unit tests |
| river, Wayfire, labwc | wlr | layer-shell | data-control | — (screens only) | — |
| COSMIC | ext | layer-shell | data-control | — | — |
| KDE Plasma 6 | portal (planned) | layer-shell | data-control | — | — |
| GNOME | portal (planned) | fullscreen window fallback | ❌ (no data-control) | — | — |

Without IPC, window mode has no windows to pick, so clicks select screens. Area and screen
modes work everywhere.

## Adding a compositor

Implement `screenie_compositor::Compositor` (`windows`, `focused_output`) and detect it
in `detect()` from an environment variable. Window rects must be logical layout
coordinates, and the list must be ordered topmost-first.
