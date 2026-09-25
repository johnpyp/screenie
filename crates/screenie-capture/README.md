# screenie-capture

The "freeze the desktop now" facade. `CaptureContext` owns a Wayland `Capturer` and the
detected `Compositor`. `snapshot(options)` returns a `Snapshot` with all outputs
captured and the window list fetched concurrently.

Backend selection follows `advanced.capture_backend` (`auto`, `ext`, `wlr`,
`portal`). The xdg-desktop-portal backend for GNOME and KDE is not implemented yet.
