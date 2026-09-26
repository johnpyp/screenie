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
4. Stopping shows a preview card: the last frame, a duration badge, **Copy** (the file),
   **Show**, and delete. Clicking the card opens the video. `recording.after_capture`
   works like the screenshot one (default: no copy, preview on).

Nothing screenie draws ever lands inside the region, so it never appears in the video.
The chrome redraws only when the timer's second changes, because every redraw is damage
the encoder would otherwise see.

## Pipeline (`screenie-record`)

```
FrameSource (screencopy stream) ─ capture thread ─▶ appsrc ─▶ videoconvert ─▶ H.264 ─┐
pulsesrc @DEFAULT_MONITOR@ ─┐                                                         ├─▶ mp4mux ─▶ file
pulsesrc (default mic) ─────┴─▶ audiomixer ─▶ AAC ────────────────────────────────────┘
```

- **Encoders** are probed once by actually encoding test frames. The order is
  `vah264enc` → `vah264lpenc` → `vaapih264enc` → `x264enc` → `openh264enc`, and
  `recording.encoder` (`auto`/`hardware`/`software`) filters it. Constant quality (CQP /
  CRF) comes from `recording.quality`, with no B-frames and keyframes every 2 s.
- **Frames** are damage-driven, capped at `recording.framerate`, and identical frames are
  skipped. A static screen costs almost nothing, and the output is variable frame rate.
  Buffers wrap the captured pixels without copying. Odd sizes lose their last row or
  column, since 4:2:0 needs even dimensions.
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
cargo run -p screenie-record --example rec -- HEADLESS-1 3 target/rec.mp4 "100,100 800x600" audio pause
```
