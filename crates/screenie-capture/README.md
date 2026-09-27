# screenie-capture

The "freeze the desktop now" facade. `CaptureContext` owns a Wayland `Capturer` and the
detected `Compositor`. `snapshot(options)` returns a `Snapshot` with all outputs
captured and the window list fetched concurrently.

`focused_output()` is the output the user is on: from compositor IPC, or else from the
layer-shell probe in `screenie-wayland`.

`stream(...)` starts a live stream of an output or a region of it, and `stream_window`
one of a window by itself where `can_stream_window` says the compositor allows it. Both
block until the first frame arrives. Where the compositor doesn't paint the pointer into
a window's frames (`Compositor::paints_pointer_into_windows`), a window stream draws it
itself, told where the window is by a thread polling compositor IPC four times a second. In `auto` mode a region goes to wlr-screencopy
first where it's offered: it copies just the region, where ext-image-copy-capture copies
the whole output and leaves the crop to the consumer.

Backend selection follows `advanced.capture_backend` (`auto`, `ext`, `wlr`, `kwin`,
`mutter`, `portal`). `auto` tries what's offered in this order, starting from whatever
worked last:

| | Stills | Streams |
| --- | --- | --- |
| wlroots and the like | ext, wlr | ext, wlr (wlr first for a region) |
| KDE Plasma | `kwin` (ScreenShot2), portal | `kwin` (`zkde_screencast`), portal |
| GNOME | portal, `mutter` | `mutter`, portal |

- **`kwin`**: KWin's own screenshots and screen casts, silent and per output. KWin grants
  them only to the binary a desktop entry runs, which `screenie-desktop` keeps in place.
- **`mutter`**: Mutter's screen casts (`org.gnome.Mutter.ScreenCast`), which need no
  permission. A cast shows in GNOME's top bar, so stills only fall back to it: a still is
  marked a recording, to be over quickly, and a recording isn't, so the top bar's
  button stops it. `RecordWindow` casts the focused window alone
  (`focused_window_still`, `stream_focused_window`), with the shadow trimmed off.
- **`portal`**: xdg-desktop-portal's Screenshot and ScreenCast. Screenshots ask once,
  as an unidentified app: GNOME lets an app with an id ask only while it's the focused
  app, which a daemon never is. Screen casts ask as screenie, and keep their restore
  token (`RestoreTokens`) so the same screens aren't asked for again.

Casts arrive over PipeWire (`screenie-pipewire`).
