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
  snapping, active window) and degrades gracefully.
- **GNOME and KDE through their own APIs, the portal as the fallback.** KWin's
  screenshots and casts are silent and exact, granted through a desktop entry screenie
  keeps current; Mutter's casts need no dialog. The portal asks, and on GNOME flashes and
  plays a sound, so it's for GNOME's stills without the extension (a cast's top-bar
  indicator would be in them) and whatever else lacks a way.
- **On GNOME, an extension of screenie's own.** Mutter tells no client where windows
  are, has no layer-shell, and marks every cast in the top bar; only code in the shell
  gets past that, so screenie ships a GNOME Shell extension (`screenie extension
  install`) that serves it over D-Bus, and nothing else: it answers only the binary
  screenie's desktop entry runs, as KWin does. It stays small and optional. Stills are
  still Mutter casts (the extension just keeps them out of the top bar), because the
  shell's own screenshots come out only as PNG, seconds on a busy 4K screen. Screenie's
  windows stay screenie's: the extension only places and marks them, from a description
  sent before each opens, rather than spawning the daemon (which would give it the
  daemon's lifetime) or drawing anything itself. Without it, the selector is fullscreen
  windows, previews are notifications with the card's buttons, and a recording has no
  countdown or pill: it's a screen-sharing cast, stopped from the top bar like any
  other. Before GNOME 49 (when any window could be made a dock), previews and
  recordings are that way with it too.
- **Keys belong to the desktop.** Screenie registers no global hotkeys of its own.
  GNOME and KDE bind screenshot keys in their settings, so `screenie shortcuts install`
  takes those over there, in the desktop's own layout (Print, Shift+Print for the whole
  desktop, Meta+Shift+S on KDE...), where the user can change them, and gives them back
  on `remove`. Tiling compositors bind keys in their config, which screenie doesn't
  edit: it prints the lines.
- **Non-goals:** X11, being a video editor (trimming and GIF are in scope, timelines
  aren't), cloud upload.

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
  PipeWire's Rust bindings need newer headers than 22.04's PipeWire (0.3.48), so there
  they compile against 24.04's and link 22.04's library: the linker then refuses anything
  0.3.48 lacks.
- **The Nix flake builds with nixpkgs' own Rust** (unstable), so `rust-version` can't
  run ahead of it. It's for NixOS: elsewhere a Nix build can't load the system's GPU
  drivers.
- **Docs are generated from what they document.** The man pages come from clap's
  definitions and the config schema's doc comments (via schemars), written by the binary
  itself (`screenie man`), and a test fails when a config key has no doc comment.
