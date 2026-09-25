# Issues

Known bugs, gaps and upstream limitations. Newest first within each section.

## Open

- **A killed daemon loses its running recording.** On SIGTERM/SIGKILL (logout, OOM)
  the faststart MP4 is never finalized. `screenie quit` saves properly. Options:
  handle SIGTERM by stopping, or mux with mp4mux's robust (moov-reserving) mode.
- **The recording border and pill can't be excluded from capture.** Screencopy
  includes every surface, so they are placed outside the region. A full-screen
  recording of the only output therefore has no pill.

- **Drag-out from the preview card is unsupported.** GPUI (0.3.x) has no API to
  start a Wayland drag with our own data offer. Options: upstream it, or open a tiny
  wl_data_device drag from our own Wayland connection with the card surface as origin
  (needs the surface's `wl_surface`, which GPUI doesn't expose).
- **Portal capture backend missing.** GNOME and KDE fall back to an error for now.
  Planned in `screenie-portal` via ashpd.
- **No clipboard on GNOME.** GNOME has no data-control protocol. The fallback is to
  hold a focused surface briefly, which GPUI's clipboard can do while a window is
  focused.
- **"window not found" ERROR lines in daemon.log after the selector closes.** These are
  benign. GPUI's Wayland backend delivers a final pointer-leave/input event to a surface
  we've just removed (`gpui window.rs` `on_input`/`on_hover_status_change` →
  `handle.update(..).log_err()`). They have no effect. We should fix them upstream
  rather than filter logs.
- **Hyprland stacking order is approximate.** It is derived from `focusHistoryID`, which
  is fine for tiled windows but can be wrong for overlapping floats.

## Fixed

- Clicks on gaps in HUD panels (the selector toolbar) fell through to the selector
  canvas. `hud::panel()` now occludes the pointer.
- x264's B-frames skewed variable-frame-rate timestamps (inflated container duration).
  B-frames are now off.
- A pulsing "recording" dot repainted every frame, so static screens were encoded at
  full frame rate. The chrome is static now, and identical frames are skipped.

- Clipboard offered `text/plain` with the file URI, which pasted a path into text
  fields. We now omit the implicit text types.
- Preview card overlay collided with the size caption on short cards. Cards now have a
  minimum size and letterbox the thumbnail.
- Daemon exited when the last selector window closed (GPUI's default quit mode).
  It now uses `QuitMode::Explicit`.
