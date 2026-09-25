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
