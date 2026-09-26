# screenie-compositor

Optional window geometry from compositor IPC, used for window snapping and "active
window". Capture never depends on it.

`detect()` picks an implementation from the environment:

| Compositor | Transport |
| --- | --- |
| Sway | i3 IPC (`$SWAYSOCK`) |
| Hyprland | `.socket.sock` (`j/clients`, `j/monitors`) |
| niri | `$NIRI_SOCKET` JSON |
| anything else | `Generic` (no windows) |

`Compositor::windows()` returns visible windows in logical coordinates, topmost first.
`focused_output()` names the output with focus.

```sh
cargo run -p screenie-compositor --example windows
```

## Compositor support

Screenie depends on protocols, not compositors. IPC only adds window snapping and
"active window".

| Compositor | Capture | Overlay | Clipboard | Windows (IPC) | Tested |
| --- | --- | --- | --- | --- | --- |
| Sway | wlr / ext | layer-shell | data-control | i3 IPC | ✅ headless |
| Hyprland | wlr (ext on 0.50+) | layer-shell | data-control | hyprctl socket | unit tests |
| niri | ext / wlr | layer-shell | data-control | niri IPC | unit tests |
| river, Wayfire, labwc | wlr | layer-shell | data-control | — (screens only) | — |
| COSMIC | ext | layer-shell | data-control | — | — |
| KDE Plasma 6 | portal (planned) | layer-shell | data-control | — | — |
| GNOME | portal (planned) | fullscreen window fallback | ❌ (no data-control) | — | — |

Without IPC, window mode has no windows to pick, so clicks select screens, and the
focused output comes from a layer-shell probe (`screenie_wayland::focused_output`).
Area and screen modes work everywhere.

Recording a window by itself needs `ext-foreign-toplevel-image-capture-source-v1` and
`ext-foreign-toplevel-list-v1` (sway 1.11+ and other wlroots 0.19+ compositors). Sway's
IPC reports each window's toplevel identifier, so the match is exact; elsewhere it's by
app id and title. Without the protocol, a window is recorded as its area of the screen.

Hyprland's stacking order is approximate: it comes from `focusHistoryID`, which is right
for tiled windows but can be wrong for overlapping floats.

## Adding a compositor

Implement `Compositor` (`windows`, `focused_output`) and detect it in `detect()` from an
environment variable. Window rects must be logical layout coordinates, and the list must
be ordered topmost-first.
