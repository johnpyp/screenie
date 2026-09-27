# Key decisions

Why screenie is built the way it is. Change these deliberately, not by drift.

## Product

- **Feels like CleanShot X / Screendrop, not a pile of scripts.** Selector, capture,
  clipboard, encoder and UI are all built in; nothing is piped between tools.
- **Instant.** A resident daemon, spawned on demand by the CLI, so a keypress shows the
  frozen overlay with no toolkit or GPU startup on the hot path.
- **Never disturbs what's captured.** Overlays are layer-shell surfaces, not windows, so
  fullscreen apps stay fullscreen. The frame is frozen *before* any UI appears, and
  screenie's own surfaces are concealed (made transparent, not unmapped, so nothing
  animates) until the compositor has shown a frame without them.
- **One gesture for the common case.** Drag = region, click = window, Enter = screen.
- **After-capture is where the value is.** Defaults are preview only: no copy, no save.
  The card is the hub; an unsaved capture expiring with its card is intended.
- **The editor is not a paint program.** One key per tool, an object canvas, quiet
  chrome, in place over the screen, and Enter finishes. Canvas and export share one
  renderer, so the file is what you saw.
- **Protocols, not compositors.** Native capture on anything with
  `ext-image-copy-capture` or `wlr-screencopy`; compositor IPC only *enhances* (window
  snapping, active window) and degrades gracefully. GNOME/KDE via portals (planned).
- **Non-goals:** X11, being a video editor (trimming and GIF are in scope, timelines
  aren't), global hotkey registration (compositors own keybindings), cloud upload.

## Technical

- **GPUI (with gpui-component) for UI.** GPU-rendered, so the translucent HUD look is
  ours rather than a desktop theme's, with first-class layer-shell, per-surface input
  regions and fractional scaling. The GTK4 prototype was dropped because it looked like
  GTK. The cost is young platform glue: we vendor small fixes in `patches/`, and there's
  no drag-out yet.
- **Own Wayland capture code** instead of grim: both protocols, control over formats and
  transforms, and one code path for stills and recording streams.
- **GStreamer for recording.** Encoders found in the registry whatever the vendor, and
  probed by actually encoding; frames stay on the GPU (DMA-BUF) where possible.
- **The daemon owns the clipboard** through data-control, because a Wayland clipboard
  dies with its owner and data-control needs no focused surface.
- **Overlays that float over other apps never take the keyboard** (cards, the recording
  pill). Only surfaces that are the task (selector, editor) do, and they close only
  once every key is released.
- **Logical vs physical.** Everything the user sees is logical; pixels are physical.
  Scales are measured (buffer / logical width), selections snap to the physical grid.
- **YAML config** (`config.yaml`, serde-saphyr), every key optional, hot-reloaded by
  contents. Settings (`screenie-config`) are separate from remembered state
  (`screenie-state`, versioned and migrated).
- **Upgrades are replacing the binary.** Each build is stamped; the CLI restarts an idle
  daemon from another build.
- **Small crates** for compile times, each with a README. Pure interaction models
  (`selector::model`, `editor::Session`) are unit-tested without GPUI; the real daemon is
  tested end to end in a headless sway session (`tests/e2e`).
- **Release profile tuned for daily use** (incremental, no LTO); `dist` is for packaging.
- **Releases are archives built on the oldest Ubuntu runners** (glibc 2.35), linked
  against the system's GStreamer, so one build runs on most distributions. The archive
  is laid out like a prefix (`bin/`, `share/man/`), which is what mise's github backend
  and `man` both look for.
- **Docs are generated from what they document.** The man pages come from clap's
  definitions and the config schema's doc comments (via schemars), written by the binary
  itself (`screenie man`), and a test fails when a config key has no doc comment.
