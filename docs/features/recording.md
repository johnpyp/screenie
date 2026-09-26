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
`stop`, `pause` and `cancel` exit 0 when they did what they say (`stop` during the
countdown exits 1, as nothing was recorded). With nothing recording they say so and exit
2, and they never start the daemon to find that out.
`screenie query status --watch` prints a line per change, and one per second while
recording, for status bars (see the README for the formats).

## UX

1. After picking, a countdown (`recording.countdown`, default 3; 0 disables it) shows in
   the middle of the region.
2. While recording, a red ring sits just **outside** the region. A pill shows the
   elapsed time with pause, stop and discard buttons. Both are click-through except
   the pill, which also has the keyboard while the pointer is on it. The recorded app
   keeps its typing otherwise, and a fullscreen game's pointer lock can't trap the
   pointer on the pill.
3. The pill goes below the region, else above, else beside it, on the recorded output.
   Without room for it (a whole screen) there's no pill, and the countdown says to stop
   with the record shortcut or `screenie stop`.
4. Nothing stays over a fullscreen app. An output a window fills (a game, recorded by
   itself or as part of its screen) gets no ring and no pill once the countdown is
   over. A fullscreen app is scanned out directly (its buffer goes to the display
   without compositing: the lowest latency, and tearing where allowed) only while it's
   the only thing on its output. Anything over it, however small or transparent, costs
   that. The countdown and the preview card are brief, so they still show.
5. Stopping shows a preview card at once: the last frame and a duration badge, with a
   spinner while the file is finished (writing the index can take a moment for long
   recordings). Capture stops first, so the card is never in the video. It doesn't time
   out while saving, and gets its file actions once the file is written. On hover it
   offers **Copy** (the file) until it's copied, **Show in folder**, and delete.
   Clicking the card opens the video. `recording.after_capture` has the screenshot
   one's `copy` and `preview` (default: no copy, preview on); a recording is always
   saved.

Nothing screenie draws ever lands inside the region, so it never appears in the video.
Preview cards on the recorded screen are hidden once capture starts (a window recorded
by itself can't show them). Cards already seen run out as usual; one that comes while
recording (a screenshot) waits, hidden, until the recording stops. Cards on other
screens show as usual.
The chrome redraws only when the timer's second changes, because every redraw is damage
the encoder would otherwise see.

## Windows

A picked window is recorded **by itself** where the compositor offers
`ext-foreign-toplevel-image-capture-source-v1` (sway 1.11+, other wlroots 0.19+
compositors): its own pixels, not the screen where it was. Windows covering it, bars,
notifications and other workspaces never show, and the recording follows it if it moves
or you switch workspaces. It then gets no ring, since the window may move away from
it, and the pill can sit over the window (unless it's fullscreen), since nothing on
screen is recorded. It's also the light way to record a fullscreen game: on wlroots,
capturing an output composites it for as long as the capture lasts, while a window is
rendered for the capture separately and the game stays on direct scanout
(`tools/scanout.py` shows both).

- The window is matched by the toplevel identifier sway's IPC reports, or else by app
  id and title, which must be unique.
- The video keeps the window's size when recording started. When the window is resized,
  each frame is scaled to fit and letterboxed.
- Closing the window stops the recording and saves it.
- Where the protocol is missing (or the match fails, which is logged), the window is
  recorded as the part of the screen it covered, like an area.

## Pipeline (`screenie-record`)

```
FrameSource (paced capture) ─ capture thread ─▶ appsrc ─▶ scale + convert (GPU) ─▶ H.264 ─┐
  frames in GPU buffers, or memory                                                          ├─▶ mp4mux ─▶ file
pulsesrc @DEFAULT_MONITOR@ ─┐                                                               │
pulsesrc (default mic) ─────┴─▶ audiomixer ─▶ AAC ──────────────────────────────────────────┘
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

- **Frames stay on the GPU** where the compositor takes GPU buffers:
  `ext-image-copy-capture-v1` with a DMA-BUF device (sway 1.10+), or
  `wlr-screencopy-unstable-v1` v3, whose buffer format gets its GPU and layouts from
  linux-dmabuf's feedback (v4+). screenie allocates them with GBM on the compositor's
  GPU, in a format and layout (modifier) both the compositor and the encoder's
  converter take, and the compositor renders each frame into one: no read-back for the compositor and no copy for us, since the
  converter imports the buffer directly. A buffer goes back to the capture pool once
  the converter is done with it. Otherwise frames come through shared memory,
  double-buffered so the compositor copies the next frame while this one is read: on
  rotated or flipped outputs, once a recorded window is resized, on compositors without
  GPU buffers, and with GStreamer before 1.24.
- **Encoders** are found in the GStreamer registry, whatever the vendor: VA-API (AMD,
  Intel, and any GPU with a VA driver, one element set per GPU), NVENC (NVIDIA),
  gstreamer-vaapi, V4L2 (SoCs), then x264 and OpenH264. Hardware on the GPU the
  compositor renders with comes first (known from the capture protocol, else from
  linux-dmabuf's feedback, so frames in memory go there too, not to an iGPU that
  happens to be listed first), then other hardware, then software, and `recording.encoder`
  (`auto`/`hardware`/`software`) filters the list. Each is probed once by encoding test
  frames through the same conversion a recording uses: VA-API elements exist whenever
  the plugin does, driver or not. Encoders whose size limits (read from their pad
  templates; VA-API takes 128 to 4096 pixels a side) don't fit the video are skipped,
  so a tiny region or a native ultrawide falls back to x264. Constant quality (CQP /
  CRF) comes from `recording.quality`, with no B-frames, keyframes every 2 s, and the
  High profile (left to itself, NVENC picks Baseline, which costs a fifth more bits).
  x264enc's `bitrate` is set to its maximum: in CRF mode GStreamer turns it into a VBV
  ceiling, 2 Mbit/s by default, which starves anything that moves.
  `SCREENIE_ENCODER=factory[:va|gl|cpu]` forces one, for troubleshooting; the `rec`
  example below lists them.
- **Scaling and conversion** to the encoder's 4:2:0 input happen on the encoder's GPU:
  `vapostproc` for VA-API, GL (`glupload ! glcolorscale ! glcolorconvert`) for NVENC,
  which takes GL memory as is. Both take GPU buffers and memory alike. Software encoders
  get `videoscale ! videoconvert`, scaling first (4K to 1080p on the CPU takes a core).
- **Frames** are damage-driven and paced, and identical frames in memory are skipped. A
  static screen costs almost nothing, and the output is variable frame rate. Each frame
  is asked for a little ahead of when it's due, by how long the compositor has been
  taking, so it's there on time. Buffers wrap the captured pixels without copying. Odd sizes lose their last row or column, since 4:2:0
  needs even dimensions. The video is the first frame's size, capped by the resolution.
  The appsrc's caps follow each frame, so frames of another size (a resized window) are
  scaled to fit, letterboxed.
- **Falling behind**: the capture loop only pushes when the pipeline has room (two raw
  frames) and otherwise holds the newest frame. Frames replaced before they could be
  pushed are counted. Each recording logs `recording encoder=… gpu_frames=…` as it
  starts, then `capture stream frames=… gpu_frames=… latency_avg=… latency_max=…` (how
  long the compositor takes to deliver a frame) and `recorded frames received=…
  pushed=… gpu=… skipped=… fps=… longest_gap=…`, and warns when more than 5% were
  skipped.
- **Measuring**: `tools/rec_stress.py` records an uncapped fullscreen `weston-simple-egl`
  (the whole 4K output, an odd-sized region of it, or the window) on a temporary output
  of the headless session and reports the video's frame rate and gaps, the encoder and
  the bitrate. On the dev machine (a 2-CU integrated Radeon), 4K to 1080p: 60 fps while
  the GPU has headroom, about half when the demo saturates it, since VA-API's scaler
  shares it. On an RTX 5090 recording a game drawing ~290 fps: a steady 60 fps with
  NVENC, and the game within a few percent of its usual rate. `tools/scanout.py` shows
  whether a fullscreen app stays on direct scanout while it's recorded.
- **Timing**: each frame is stamped with when the compositor presented it, on the
  pipeline's clock (CLOCK_MONOTONIC), so the video's timing is the screen's however
  unevenly frames reach us. Pausing sets the live pipeline to PAUSED, which stops
  running time for audio and video alike, so resumed segments join seamlessly.
- **Audio** goes through pulsesrc (PipeWire and PulseAudio both serve it) and is encoded
  as AAC (`avenc_aac` → `fdkaacenc` → `voaacenc`), or Opus if there's no AAC encoder.
- **File**: MP4 with the index up front (faststart). Its scratch file lives next to the
  output, not in `/tmp`. Quitting the daemon saves a running recording first.

Try it without the daemon:

```sh
cargo run -p screenie-record --example rec -- encoders
cargo run -p screenie-record --example rec -- HEADLESS-1 3 .cache/rec.mp4 "100,100 800x600" audio pause
```
