# Recording

Status: **done** on wlroots-family compositors (native capture). Portal/PipeWire source
for GNOME/KDE, and GIF export, are planned.

## Commands

| Command | What happens |
| --- | --- |
| `screenie record` | Live selector (area / window / screen) with audio toggles. Adjust, then press **Record**. Runs `stop` if already recording. |
| `screenie record window` | Selector in window mode. |
| `screenie record screen` | The focused output, no selector. |
| `screenie record last` | The previous capture region. |
| `screenie record --region "X,Y WxH"` | That region, no selector. |
| `screenie stop` | Stop and save. Cancels a countdown. |
| `screenie pause` | Pause or resume. |
| `screenie cancel` | Stop and delete. |

`--audio` / `--mic` turn on system audio / the microphone, and `-o FILE` picks the
output path. `--no-toggle` fails instead of stopping a running recording.
`screenie query status --watch` prints a line per change, and one per second while
recording, for status bars (see the README for the formats).

## UX

1. After picking, a countdown (`recording.countdown`, default 3; 0 disables it) shows in
   the middle of the region.
2. While recording, a red ring sits just **outside** the region. A pill shows the
   elapsed time with pause, stop and discard buttons. Both are click-through except
   the pill's buttons.
3. The pill goes below the region, else above, else beside it, else on another output.
   A recording of the only output's whole area has no pill. The countdown then says to
   stop with the record shortcut or `screenie stop`.
4. Stopping shows a preview card: the last frame and a duration badge. On hover it
   offers **Copy** (the file) until it's copied, **Show in folder**, and delete. Clicking the card opens the video. `recording.after_capture`
   works like the screenshot one (default: no copy, preview on).

Nothing screenie draws ever lands inside the region, so it never appears in the video.
The chrome redraws only when the timer's second changes, because every redraw is damage
the encoder would otherwise see.

## Windows

A picked window is recorded **by itself** where the compositor offers
`ext-foreign-toplevel-image-capture-source-v1` (sway 1.11+, other wlroots 0.19+
compositors): its own pixels, not the screen where it was. Windows covering it, bars,
notifications and other workspaces never show, and the recording follows it if it moves
or you switch workspaces. It then gets no ring, since the window may move away from
it, and the pill can sit over the window, since nothing on screen is recorded.

- The window is matched by the toplevel identifier sway's IPC reports, or else by app
  id and title, which must be unique.
- The video keeps the window's size when recording started. When the window is resized,
  each frame is scaled to fit and letterboxed.
- Closing the window stops the recording and saves it.
- Where the protocol is missing (or the match fails, which is logged), the window is
  recorded as the part of the screen it covered, like an area.

## Pipeline (`screenie-record`)

```
FrameSource (paced screencopy) ─ capture thread ─▶ appsrc ─▶ scale + convert (GPU with VA-API) ─▶ H.264 ─┐
pulsesrc @DEFAULT_MONITOR@ ─┐                                                                             ├─▶ mp4mux ─▶ file
pulsesrc (default mic) ─────┴─▶ audiomixer ─▶ AAC ────────────────────────────────────────────────────────┘
```

Settings:

- `recording.framerate` (default `60`, or `native`): the most frames a second. It paces
  the capture itself: the compositor is asked for a frame only when one is due, on a
  fixed clock (`screenie_core::Pacer`), so a game drawing 280 frames a second costs 60
  copies, not 280. `native` takes every frame the screen or window shows.
- `recording.resolution` (default `1080p`, or `native`, `720p`, `1440p`, `2160p`/`4k`):
  the most lines the video may have, in a 16:9 box turned to match the recording's
  orientation. It never scales up and keeps the aspect ratio. A 4K screen at `1080p`
  is a quarter of the pixels to encode and store.

- **Encoders** are probed once by actually encoding test frames. The order is
  `vah264enc` → `vah264lpenc` → `vaapih264enc` → `x264enc` → `openh264enc`, and
  `recording.encoder` (`auto`/`hardware`/`software`) filters it. Constant quality (CQP /
  CRF) comes from `recording.quality`, with no B-frames and keyframes every 2 s.
- **Frames** are damage-driven and paced, and identical frames are skipped. A static
  screen costs almost nothing, and the output is variable frame rate. Buffers wrap the
  captured pixels without copying. Odd sizes lose their last row or column, since 4:2:0
  needs even dimensions. The video is the first frame's size, capped by the resolution.
  The appsrc's caps follow each frame, so frames of another size (a resized window) are
  scaled to fit, letterboxed.
- **Scaling and conversion** to the encoder's 4:2:0 input happen on the GPU with
  `vapostproc` when the encoder is VA-API (with the upload, so it costs almost no CPU;
  scaling 4K to 1080p on the CPU takes a whole core). Software encoders get
  `videoscale ! videoconvert`, scaling first.
- **Falling behind**: the capture loop only pushes when the pipeline has room (two raw
  frames) and otherwise holds the newest frame. Frames replaced before they could be
  pushed are counted. Each recording logs `recorded frames received=… pushed=…
  skipped=… fps=… longest_gap=…`, and warns when more than 5% were skipped.
- **Measuring**: `tools/rec_stress.py` records an uncapped fullscreen `weston-simple-egl`
  on a temporary 4K output of the headless session and reports the video's frame rate
  and gaps. On the dev machine: 60 fps at 1080p, ~57 at native 4K.
- **Timing**: buffers carry the pipeline's running time. Pausing sets the live
  pipeline to PAUSED, which stops running time for audio and video alike, so resumed
  segments join seamlessly.
- **Audio** goes through pulsesrc (PipeWire and PulseAudio both serve it) and is encoded
  as AAC (`avenc_aac` → `fdkaacenc` → `voaacenc`), or Opus if there's no AAC encoder.
- **File**: MP4 with the index up front (faststart). Its scratch file lives next to the
  output, not in `/tmp`. Quitting the daemon saves a running recording first.

Try it without the daemon:

```sh
cargo run -p screenie-record --example rec -- encoders
cargo run -p screenie-record --example rec -- HEADLESS-1 3 .cache/rec.mp4 "100,100 800x600" audio pause
```
