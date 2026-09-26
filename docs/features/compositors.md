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

The output you're on (for `shot screen`, where the selector's toolbar starts, and where
`screenie edit` opens) comes from IPC where there is one. Elsewhere screenie asks
layer-shell: a surface opened without an output goes where the compositor thinks you
are, so a transparent pixel is mapped for a moment and `wl_surface.enter` names the
output (`screenie_wayland::focused_output`).

Recording a window by itself needs `ext-foreign-toplevel-image-capture-source-v1` and
`ext-foreign-toplevel-list-v1` (sway 1.11+ and other wlroots 0.19+ compositors). Sway's
IPC reports each window's toplevel identifier, so the match is exact. Elsewhere it's by
app id and title. Without the protocol, a window is recorded as its area of the screen.

## Adding a compositor

Implement `screenie_compositor::Compositor` (`windows`, `focused_output`) and detect it
in `detect()` from an environment variable. Window rects must be logical layout
coordinates, and the list must be ordered topmost-first.
