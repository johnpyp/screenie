# screenie-compositor

Optional window geometry from compositor IPC, used for window snapping, "active window"
and placing the pointer over a recorded window on sway. Capture works without it.

`detect()` picks an implementation from the environment:

| Compositor | Transport |
| --- | --- |
| Sway | i3 IPC (`$SWAYSOCK`) |
| Hyprland | `.socket.sock` (`j/clients`, `j/monitors`) |
| niri | `$NIRI_SOCKET` JSON |
| KDE Plasma | a KWin script run over D-Bus, reporting back through a method call |
| GNOME | screenie's GNOME Shell extension (`screenie_desktop::shell`) for windows and the pointer's output; `org.gnome.Mutter.DisplayConfig` for the layout. Without the extension, no windows (`lists_windows()` is false) |
| anything else | `Generic` (no windows) |

`Compositor::windows()` returns visible windows in logical coordinates, topmost first.
`focused_output()` names the output with focus. `lists_windows()` is false where there are
never any, so callers take the focused window another way (GNOME's window casts). `paints_pointer_into_windows()` is false
where a window capture ignores `paint_cursors` (sway), so a window recording draws the
pointer itself, placed through `windows()`.

```sh
cargo run -p screenie-compositor --example windows
```

## Compositor support

Screenie depends on protocols, not compositors. IPC only adds window snapping and
"active window".

| Compositor | Capture | Overlay | Clipboard | Windows (IPC) | Tested |
| --- | --- | --- | --- | --- | --- |
| Sway | wlr, ext | layer-shell | data-control | i3 IPC | ✅ headless |
| Hyprland | wlr (ext on 0.50+) | layer-shell | data-control | hyprctl socket | unit tests |
| niri | ext, wlr | layer-shell | data-control | niri IPC | unit tests |
| river | wlr | layer-shell | data-control | — | — |
| Wayfire | wlr | layer-shell | data-control | — | — |
| labwc | wlr | layer-shell | data-control | — | — |
| COSMIC | ext | layer-shell | data-control | — | — |
| KDE Plasma 6 | KWin's own, portal | layer-shell | data-control | KWin scripting | ✅ container |
| GNOME | Mutter casts, portal | windows the extension places | Mutter remote desktop | the extension | ✅ container (48, 50) |

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
