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
| Annotation editor, pinning, settings window | planned |
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

What happens after a capture (copy, save, preview) is configurable, and can be
overridden per call with `--copy/--no-copy`, `--save/--no-save`, `--no-preview` and
`-o FILE`.

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

### Status bars

```sh
screenie status                  # idle, or the recording and its elapsed time
screenie status --watch --json   # one JSON line per change (every second while recording)
```

Exit codes: `0` done, `1` cancelled, `2` error. Captured file paths are printed on stdout.

## Configuration

The config lives in `~/.config/screenie/config.toml`. Every key is optional, and changes
apply within a second, without a restart.

```toml
[screenshot]
directory = "~/Pictures/Screenshots"
filename = "Screenshot_%Y-%m-%d_%H-%M-%S"
after_capture = { copy = true, save = true, preview = true }

[recording]
framerate = 60
quality = "high"          # low | medium | high | lossless
encoder = "auto"          # auto | hardware | software
countdown = 3
system_audio = false
microphone = false

[preview]
corner = "bottom-right"
timeout = 6               # seconds; 0 keeps cards until dismissed

[selector]
capture_on_release = true # false: adjust the selection, then press Enter
dim = 0.45
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

To upgrade, replace the binary. The next `screenie` command notices that the daemon
is running a different build and restarts it, unless a recording or selector is active,
in which case it waits for a later command. `screenie --version` and `screenie status`
show the git commit each side was built from.

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
| `screenie-record` | GStreamer recording engine |
| `screenie-capture` | "Freeze the desktop" facade and backend selection |
| `screenie-wayland` | ext-image-copy-capture / wlr-screencopy, stills and streams |
| `screenie-compositor` | Window geometry via Sway / Hyprland / niri IPC |
| `screenie-ui-kit` | Shared GPUI look: fonts, icons, HUD widgets, layer-shell helpers |
| `screenie-ipc` | CLI ⇄ daemon protocol and socket |
| `screenie-config` | Config schema, XDG paths, file naming |
| `screenie-core` | Geometry, images, snapshots, frame sources |

You don't need a display to develop. `tools/` has a headless sway session with two
mixed-DPI outputs, a virtual pointer and keyboard, and an image diff:

```sh
tools/session.sh start && source $XDG_RUNTIME_DIR/screenie-session.env
cargo run -p screenie -- shot
cargo run -p wlinput -- drag 100 100 800 600 15
tools/session.sh shot target/session.png
python3 tools/imgdiff.py a.png b.png
```

Reference projects (Screendrop, gpui-component…) are
cloned into `references/` with `mise run refs`.

## License

MIT. The bundled Inter font is under the SIL Open Font License, and the Lucide icons
are under ISC.
