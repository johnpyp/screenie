# Issues

Known bugs, gaps and upstream limitations. Newest first within each section.

## Open

- **Editor windows tile on tiling compositors** (`editor.mode = "window"` only; the
  default overlay doesn't). Wayland has no "please float" hint for toplevels. The README
  documents a float rule for the app id.

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

- Screenie showed up in its own screenshots: the last shot's card, the recording ring
  and pill, an overlay editor, a selector. They're concealed while the screen is
  frozen, and cards on a recorded screen while it's recorded.
- A second capture shortcut while the selector was up failed (`shot area` twice), or
  photographed the selector (`shot screen`). It now closes the selector, switches its
  mode, or captures what it froze.
- `query status --watch` stuck on "selecting" after every interactive shot: status
  changes nobody broadcast. Every daemon state change is broadcast now.
- `capturing`/"selecting" lasted through the PNG encode, and the 4K encode (up to
  0.6 s) delayed the card and the clipboard. It ends when the selector closes, and PNGs
  are encoded with fast compression.
- `shot screen` captured the first output on compositors without IPC (COSMIC, river,
  Wayfire, labwc). The focused output now comes from a layer-shell probe there.
- File names carried the time of saving, not of capture. Parallel captures in the same
  second could overwrite each other. `-o DIR` failed and left a `.part` file behind.
- niri: a column scrolled partly off its monitor reached onto the next one.
- `shot last`, `record last` and `query last` forgot everything when the daemon
  restarted (an automatic upgrade, above all). The last region and captures are in
  `state.yaml` now.
- Two commands at once with no daemon could start two, one of them unreachable. The
  daemon holds a lock for life, and clients take turns deciding whether to start one.
- `daemon.log` was truncated by the next spawn, erasing a crash; it's kept as
  `daemon.log.1` now, in plain text with local times.
- The daemon kept the working directory of the shell that started it (so an unmount
  failed), and relative `directory:` settings resolved against it. It runs from `/`, and
  directories expand `~`/`$VAR` and are relative to home.
- Config reload missed symlinks swapped to a target with the same mtime (home-manager).
  It compares contents now.
- `query … --watch` exited on any socket or decode error, and panicked on a closed pipe.
- `screenie cancel` exited 1 after discarding; `stop`/`pause`/`cancel` started a daemon
  to say nothing was recording.
- `selector.freeze`, `advanced.daemon_idle_exit` and `recording.after_capture.save/edit`
  did nothing; they're gone.
- With `capture_on_release: false`, a click just off a handle captured the window under
  it, discarding the adjusted selection. It keeps the selection now.
- Clicking the selector's Window button captured immediately: the release reached the
  canvas as a click. Releases now only count after a press on the canvas.
- Clearer selector hint ("…or press Enter to capture the whole screen").
- Defaults are now copy on, save off, preview for 10 s. `--edit` defers copying and
  saving to the editor's Done (or an explicit Save).
- Window captures get the app name in the file name (`{app}`/`{title}` placeholders).
- `screenie query status|last` for bars and scripts: tab-separated fixed fields,
  `--json`, `--format waybar`, `--watch` across daemon restarts. It never spawns the
  daemon. It replaces `screenie status`.

- `screenie shot --edit` / `after_capture.edit` did nothing (the editor didn't exist).
  They open the annotation editor now.

- "no supported shm format among [Bgr888]" on Hyprland: compositors that offer only
  the output's native packed 24-bit format are now supported. `auto` also falls back
  to the other capture protocol when one fails.

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
