# screenie-capture

The "freeze the desktop now" facade. `CaptureContext` owns a Wayland `Capturer` and the
detected `Compositor`. `snapshot(options)` returns a `Snapshot` with all outputs
captured and the window list fetched concurrently.

`stream(...)` starts a live stream of an output or a region of it, and `stream_window`
one of a window by itself where `can_stream_window` says the compositor allows it. Both
block until the first frame arrives.

Backend selection follows `advanced.capture_backend` (`auto`, `ext`, `wlr`,
`portal`). The xdg-desktop-portal backend for GNOME and KDE is not implemented yet.
