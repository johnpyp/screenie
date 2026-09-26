# screenie

Screenshots and screen recordings for Wayland, meant to feel like CleanShot X rather
than a pile of scripts. Press a key and the desktop freezes under a pixel-exact overlay.
Drag, click a window, or press Enter, and the capture is copied, saved and shown in a
little preview card. Recording uses the same selector, one button, and a tiny timer pill.

Everything is built in: selector, capture, clipboard, encoder and UI. You don't pipe
`slurp` into `grim` into `wl-copy`. screenie is written in Rust with a GPU-rendered
[GPUI](https://www.gpui.rs/) interface.

## Status

| | |
| --- | --- |
| Screenshots: area / window / screen / all / active window / last region | ✅ |
| Frozen overlay with loupe, window snapping, pixel-grid snapping on fractional scales | ✅ |
| Clipboard (image + file), saving, floating preview cards | ✅ |
| Screen recording: MP4 (H.264/AAC), VA-API or x264, system audio + mic, pause | ✅ |
| Annotation editor: arrows, shapes, text, steps, pixelate/blur, spotlight, crop | ✅ |
| Pinning, settings window | planned |
| GNOME / KDE (xdg-desktop-portal capture) | planned |
| OCR, GIF export | later |

Native capture works on wlroots-family compositors and anything else with
`ext-image-copy-capture-v1` or `wlr-screencopy`: Sway, Hyprland, niri, river, Wayfire,
COSMIC… Window picking uses compositor IPC where it's available (Sway, Hyprland, niri).
See [the compositor matrix](docs/features/compositors.md).

## Usage

Every command talks to a small daemon, which starts automatically on first use. Bind
the commands to keys in your compositor:

```sh
# Hyprland
bind = , Print, exec, screenie                    # pick an area / window / screen
bind = SHIFT, Print, exec, screenie shot screen   # the focused screen, instantly
bind = ALT, Print, exec, screenie record          # start, or stop, a recording

# Sway
bindsym Print exec screenie
bindsym Shift+Print exec screenie shot screen
bindsym Alt+Print exec screenie record
```

### Screenshots

```sh
screenie                         # same as `screenie shot area`
screenie shot window             # click a window
screenie shot screen             # focused output, no UI (--output-name DP-1 for another)
screenie shot all                # every output stitched together
screenie shot active             # focused window, no UI
screenie shot last               # same region as last time
screenie shot --region "100,100 800x600"
screenie shot --delay 3 --cursor
screenie shot --no-save --stdout > shot.png
```

In the selector:

| | |
| --- | --- |
| Drag | Select a region. Shift keeps it square, Alt draws from the center, and holding Space moves it. |
| Click | The window under the pointer, or the screen |
| Enter / Esc | Confirm / cancel |
| 1 2 3, Tab | Area / window / screen mode |
| Arrows | Nudge an editable selection (Shift ×10, Ctrl resizes) |
| M | Toggle the magnifier |

By default a screenshot is just shown in a preview card, whose buttons copy, save or
annotate it. What happens after a capture (copy, save, preview, edit) is configurable, and can be
overridden per call with `--copy/--no-copy`, `--save/--no-save`, `--no-preview`,
`--edit` and `-o FILE`.

### Annotating

```sh
screenie shot --edit             # capture, then annotate
screenie edit shot.png           # annotate an existing PNG
```

Or hover a preview card and click the pencil. With `--edit`, nothing is copied or saved
until you're done. Enter (Done) applies the after-capture `copy` / `save` settings and
closes, with no preview card afterwards. Ctrl+C and Ctrl+S copy and save explicitly. Each tool has one key: **A**rrow,
**L**ine, **R**ectangle, **O** ellipse, **P**en, **H**ighlighter, **T**ext,
**N**umbered step, **B** pixelate/blur, **S**potlight, **C**rop, **V** select.
Ctrl+scroll (or `[` `]`) changes the size, of the selected shape too, and **F** toggles
fill. Esc backs out. See
[the editor doc](docs/features/editor.md) for the rest.

The editor opens as an overlay on the capture's screen, with the capture right where
it was taken, and only one at a time. For a regular window, set `editor.mode =
"window"`. On tiling compositors, float that window by matching its app id
`dev.johnpyp.Screenie`, e.g. on Hyprland:
`windowrule = float, class:dev.johnpyp.Screenie`.

### Recording

```sh
screenie record                  # pick an area, toggle audio, press Record
screenie record screen           # the focused output
screenie record --audio --mic    # with system audio and microphone
screenie stop                    # save (running `screenie record` again also stops)
screenie pause                   # pause / resume
screenie cancel                  # discard
```

A countdown runs first. While recording, a thin ring marks the region and a pill shows
the time with pause/stop/discard. Both are drawn outside the region, so they never end
up in the video.

### Status bars and scripts

`screenie query` reports what screenie is doing or has done. It never starts the
daemon; if it isn't running, the answer is `idle`. Text output is tab-separated with a
fixed number of fields (empty when they don't apply), so `cut -f1` and friends always
work.

```sh
screenie query status            # state, elapsed, path: "recording\t1:23\t/…/Recording.mp4", "idle\t\t"
screenie status                  # the same, shorter
screenie query status --watch    # a line per change, every second while recording; survives daemon restarts
screenie query status --json     # everything, including the last captures
screenie query status --format waybar --watch   # a waybar custom module (empty when idle)
screenie query last              # kind, Unix time, path of the latest capture ("screenshot\t1790381588\t/…png")
screenie query last recording    # or: screenshot. Exits 1 if there's none yet
screenie query last --watch      # a line whenever a new capture lands
```

States: `idle`, `selecting`, `editing`, `countdown`, `recording`, `paused`, `saving`.

```sh
# swaybar / i3blocks: show a recording indicator
screenie query status --watch | while IFS=$'\t' read -r state elapsed path; do
  case $state in recording) echo "● $elapsed" ;; paused) echo "⏸ $elapsed" ;; saving) echo "saving…" ;; *) echo ;; esac
done
```

```jsonc
// waybar
"custom/screenie": {
  "exec": "screenie query status --format waybar --watch",
  "return-type": "json",
  "on-click": "screenie stop"
}
```

Other commands print the file they produced (if saved) on stdout: captures, and
`screenie record` prints the path it's recording to. Exit codes: `0` done, `1`
cancelled, `2` error.

## Configuration

The config lives in `~/.config/screenie/config.yaml`. Every key is optional, and changes
apply within a second, without a restart.

```yaml
ui_scale: auto            # interface size (bars, buttons, text, cards), e.g. 1.25; auto follows GTK text scaling

screenshot:
  directory: ~/Pictures/Screenshots
  filename: "Screenshot_%Y-%m-%d_%H-%M-%S_{app}"  # {app}/{title}: the captured window, if any
  after_capture: { copy: false, save: false, preview: true, edit: false }

recording:
  framerate: 60
  quality: high           # low | medium | high | lossless
  encoder: auto           # auto | hardware | software
  countdown: 3
  system_audio: false
  microphone: false

preview:
  position: bottom-right   # top-left | top-middle | top-right | left-middle | right-middle | bottom-left | bottom-middle | bottom-right
  timeout: 10             # seconds; 0 keeps cards until dismissed

selector:
  capture_on_release: true  # false: adjust the selection, then press Enter
  dim: 0.45

editor:
  mode: overlay           # over the screen, the capture in place; or window
  palette: ["#ff3b30", "#ff9500", "#ffcc00", "#34c759", "#0a84ff", "#af52de", "#ffffff", "#1c1c1e"]
  default_color: "#ff3b30"
  stroke_width: 4.0       # logical pixels; sizes are 1 2 4 6 8 12 16 20 26 32
  exit_on_copy: false     # close the editor once the image is copied (Ctrl+C / Copy)
  exit_on_save: false     # … or saved (Ctrl+S / Save / Save As)
  confirm_discard: true   # ask before closing with annotations neither copied nor saved
```

All keys are listed in [`crates/screenie-config/src/schema.rs`](crates/screenie-config/src/schema.rs).
The daemon logs to `~/.local/state/screenie/daemon.log`. Set `SCREENIE_LOG=debug` for
more detail.

## Building

You need Rust (stable, via [mise](https://mise.jdx.dev) or rustup) and, on Debian/Ubuntu:

```sh
sudo apt install clang mold pkg-config \
  libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev libfontconfig-dev libfreetype-dev \
  libwayland-dev libvulkan-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly \
  gstreamer1.0-libav gstreamer1.0-pulseaudio
```

Then:

```sh
cargo build --release
install -Dm755 target/release/screenie ~/.local/bin/screenie
```

`--release` is tuned for quick rebuilds while developing. For a packaged build, use
`cargo build --profile dist` (thin LTO, one codegen unit), which lands in `target/dist/`.

To upgrade, replace the binary. The next `screenie` command notices that the daemon
is running a different build and restarts it, unless it's in use (recording, selecting,
editing), in which case it waits for a later command. `screenie daemon` does the same
and says what it did. `screenie --version` and
`screenie query status --json` show the git commit each side was built from.

At runtime, recording uses VA-API when a driver is present (`mesa-va-drivers`,
`intel-media-va-driver`), and falls back to x264 otherwise.

## Development

The workspace is split into small crates so builds stay fast. Each one has its own
README. The design and its reasoning are in [`docs/NORTH_STAR.md`](docs/NORTH_STAR.md),
per-feature docs are in [`docs/features/`](docs/features), and known problems are in
[`docs/ISSUES.md`](docs/ISSUES.md).

| Crate | Role |
| --- | --- |
| `screenie` | The CLI binary |
| `screenie-app` | The daemon: request routing, capture/record flows, preview cards, recording controls |
| `screenie-selector` | The capture overlay and its (unit-tested) interaction model |
| `screenie-editor` | The annotation editor (overlay or window) and its (unit-tested) interaction model |
| `screenie-annotate` | Annotation documents and their tiny-skia renderer (shared by canvas and export) |
| `screenie-record` | GStreamer recording engine |
| `screenie-capture` | "Freeze the desktop" facade and backend selection |
| `screenie-wayland` | ext-image-copy-capture / wlr-screencopy, stills and streams |
| `screenie-compositor` | Window geometry via Sway / Hyprland / niri IPC |
| `screenie-ui-kit` | Shared GPUI look: fonts, icons, HUD widgets, layer-shell helpers |
| `screenie-ipc` | CLI ⇄ daemon protocol and socket |
| `screenie-config` | Config schema, XDG paths, file naming |
| `screenie-state` | What's remembered between runs: a versioned, migrated state file |
| `screenie-core` | Geometry, images, snapshots, frame sources |

You don't need a display to develop. `tools/` has a headless sway session with two
mixed-DPI outputs, a virtual pointer and keyboard, and an image diff:

```sh
tools/session.sh start && source $XDG_RUNTIME_DIR/screenie-session.env
cargo run -p screenie -- shot
cargo run -p wlinput -- drag 100 100 800 600 15
tools/session.sh shot .cache/session.png
python3 tools/imgdiff.py a.png b.png
```

`tests/e2e` drives the real daemon in that session with pytest: keyboard handoff,
the editor's close/save flows, Save As, remembered state, interface scale. Each test
gets its own config, state and home, so nothing touches yours. See its
[README](tests/e2e/README.md).

```sh
mise run test:e2e                 # or: mise run test:e2e -- -k save_as -x
```

Reference projects (Screendrop, gpui-component…) are
cloned into `references/` with `mise run refs`.

## License

MIT. The bundled Inter font (in `assets/fonts`) is under the SIL Open Font License, and the Lucide icons
are under ISC.
