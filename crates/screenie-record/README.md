# screenie-record

The recording engine. It turns a `screenie_core::FrameSource` (frames as the screen
changes) and optional audio into an H.264/AAC MP4 with GStreamer.

```rust
let source = capture.stream(backend, "DP-1", Some(region), cursor)?;
let recording = Recording::start(source, RecordSpec { path, framerate: 60, quality, encoder, audio })?;
recording.set_paused(true)?;
let finished = recording.stop()?; // path, duration, size, bytes, last frame
```

- `Encoder::select(preference)` finds a working H.264 encoder (VA-API first, then x264,
  then openh264) by test-encoding, and caches the result.
- The frame pump runs on its own thread. It caps the frame rate, holds early frames
  rather than dropping them, skips identical frames, and hands pixels to GStreamer
  without copying.
- The video is the first frame's size. Frames of another size (a recorded window being
  resized) are scaled to fit, letterboxed.
- `stop()` blocks until the file is finalized. `cancel()` or dropping the recording
  deletes it. `failure()` reports a broken pipeline so the caller can salvage the file,
  and `ended()` a source that went away (a recorded window closed).

See [docs/features/recording.md](../../docs/features/recording.md) for the design.

```sh
cargo run -p screenie-record --example rec -- encoders
cargo run -p screenie-record --example rec -- OUTPUT SECONDS FILE [region] [audio] [mic] [pause]
```
