# screenie-capture

The "freeze the desktop now" facade. `CaptureContext` owns a Wayland `Capturer` and the
detected `Compositor`. `snapshot(options)` returns a `Snapshot` with all outputs
captured and the window list fetched concurrently.

`focused_output()` is the output the user is on: from compositor IPC, or else from the
layer-shell probe in `screenie-wayland`.

`stream(...)` starts a live stream of an output or a region of it, and `stream_window`
one of a window by itself where `can_stream_window` says the compositor allows it. Both
block until the first frame arrives. A window stream that draws the pointer is told
where the window is by a thread polling compositor IPC four times a second. In `auto` mode a region goes to wlr-screencopy
first where it's offered: it copies just the region, where ext-image-copy-capture copies
the whole output and leaves the crop to the consumer.

Backend selection follows `advanced.capture_backend` (`auto`, `ext`, `wlr`,
`portal`). The xdg-desktop-portal backend for GNOME and KDE is not implemented yet.
