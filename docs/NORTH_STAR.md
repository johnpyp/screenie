# Screenie — North Star

Screenie is a screenshot and light screen-recording app for Wayland that feels like the
best macOS tools (CleanShot X, Screendrop, the native ⌘⇧4/⌘⇧5): instant, beautiful, and
out of your way. It is invoked from the CLI (so any compositor keybinding can drive it)
and does everything itself. You don't pipe a selector into a grabber into an encoder.

## What "great" means here

1. **Instant.** A keypress shows a frozen, pixel-exact overlay in well under 100 ms.
   The daemon stays resident, so there's no toolkit or GPU startup on the hot path.
2. **Never disturbs what you're capturing.** The overlay is a layer-shell surface, not a
   window, so fullscreen apps stay fullscreen. The frozen frame is taken *before* any UI
   appears, so screenie never shows up in its own screenshots.
3. **One gesture for the common case.** Drag = region, click = window, Enter = screen.
   Release = captured, copied, saved. Everything else is optional polish that stays
   out of the way.
4. **After-capture is where the value is.** A floating preview card (copy, save, annotate,
   pin, delete, and eventually drag into any app) and a real annotation editor, not a paint program.
5. **Recording is as easy as screenshots.** The same selector, one button, a tiny timer
   pill, stop from the pill or the same CLI command. The output is a sane MP4 that plays
   everywhere, hardware-encoded when possible.
6. **Correct on every Wayland setup that matters.** Mixed-DPI and fractional scaling,
   rotated outputs, multi-monitor layouts with negative coordinates. Native protocols on
   wlroots-family compositors (Sway, Hyprland, niri, river, Wayfire, COSMIC); portals on
   KDE and GNOME. Nothing is coupled to one compositor. Compositor IPC is only used to
   *enhance* (window snapping) and degrades gracefully.
7. **Configurable without editing files.** XDG TOML config that the settings window edits
   live; hand edits are picked up live too.

## The editor

The annotation editor is where screenie most easily goes wrong: it could turn into a
paint program. The bar is Screendrop's and CleanShot's editors, with Flameshot's
"nothing to set up" speed:

- **One key per tool, no set-up.** Pick a tool with a letter, drag, done. The tool
  stays armed and the new shape is selected, so adjusting it is one drag away.
- **An object canvas.** Every shape stays editable (move, resize, restyle, delete)
  until export, and cropping is non-destructive.
- **Quiet chrome.** One tool bar, and a style bar showing only what applies right now.
  Selections show corner handles and nothing else.
- **What you see is the file.** The canvas and the export share one renderer, and
  sizes are in logical pixels, so HiDPI captures look the way they did on screen.
- **Fast.** Only the shape under your hands is re-rendered while it changes.
- **In place.** The editor is an overlay over the screen, with the capture right where
  it was taken, so annotating continues the selection instead of opening a new window.
- **Finishing is one key.** Enter does what the after-capture settings say (copy, by
  default) and closes. Esc backs out one level at
  a time and never throws work away without asking.

## Non-goals

- X11.
- Being a video editor. Trimming and GIF export are in scope; timelines are not.
- Global hotkey registration. Compositors own keybindings; we document the snippets.
- Cloud upload (for now).

## Architecture

```
 screenie (CLI) ──unix socket──▶ screenie daemon (GPUI app, spawned on demand)
                                   │
       ┌───────────────┬──────────┼───────────────┬───────────────┐
   capture          selector    editor          record          app shell
 (snapshot of     (layer-shell  (annotation    (GStreamer      (preview cards,
  all outputs)     overlay)      editor)        pipelines)      pin, settings,
       │                           │                            recording pill)
 ┌─────┴──────┐                annotate
 wayland    portal            (doc model +
 (ext-image-copy,             tiny-skia)
  wlr-screencopy)
```

| Crate | Role |
| --- | --- |
| `screenie-core` | Geometry (logical vs physical), pixel buffers, outputs, `Snapshot` compositing. No GUI deps. |
| `screenie-config` | Config schema, XDG load/save, defaults, filename templates. |
| `screenie-ipc` | CLI⇄daemon protocol, socket framing, spawn-on-demand client. |
| `screenie-wayland` | Raw Wayland: output discovery, `ext-image-copy-capture` and `wlr-screencopy` frame capture (stills and streams), data-control clipboard. |
| `screenie-portal` | xdg-desktop-portal Screenshot/ScreenCast fallback for KDE/GNOME. |
| `screenie-capture` | Backend selection plus the `Snapshot` facade: "freeze the desktop now". |
| `screenie-compositor` | Optional window geometry via compositor IPC (Hyprland, Sway, niri). |
| `screenie-record` | Recording engine: frame sources to GStreamer encode/mux, audio, encoder probing. |
| `screenie-annotate` | Annotation document model and tiny-skia renderer shared by the editor and export. |
| `screenie-ui-kit` | GPUI look and feel: bundled font and icons, HUD panels and buttons, layer-shell helpers. |
| `screenie-selector` | The capture overlay: frozen or live backdrop, region/window/screen picking, loupe. |
| `screenie-editor` | The annotation editor (overlay or window). |
| `screenie-app` | The daemon: request routing, after-capture pipeline, preview cards, pins, recording pill, settings. |
| `screenie` | The binary: CLI parsing and client, `screenie daemon` entry point. |

### Key decisions

- **GPUI (with gpui-component) for UI.** GPU-rendered, so the HUD look (translucent
  panels, soft shadows, smooth animation) is ours rather than a desktop theme's. It has
  first-class layer-shell support, per-surface input regions for click-through chrome, and
  fractional scaling. gpui-component supplies the heavier widgets (inputs, settings
  forms). We bundle Inter and Lucide icons so it looks the same everywhere. The cost is
  younger platform glue: no drag-and-drop *out* of our windows yet, and a few rough
  edges tracked in [ISSUES](ISSUES.md). The previous GTK4 prototype was dropped because
  it looked like GTK.
- **Own Wayland capture code** (wayland-client) instead of shelling out to grim, so we get
  both modern protocols, control over pixel formats and transforms, and a frame stream
  for recording from the same code.
- **GStreamer for recording.** VA-API/x264/openh264 encoder fallback chain, PipeWire audio
  and portal video, and robust MP4 muxing, all as system libraries.
- **The daemon owns the clipboard.** Wayland clipboards die with their owner. Copies go
  through `ext/wlr-data-control` (wl-clipboard-rs), which needs no focused surface.

## Feature docs

- [Capture & selector](features/capture.md)
- [After-capture: preview card, clipboard, saving](features/after-capture.md)
- [Annotation editor](features/editor.md)
- [Recording](features/recording.md)
- [Configuration & settings](features/config.md)
- [Compositor support matrix](features/compositors.md)
- [Issues & bugs](ISSUES.md)
