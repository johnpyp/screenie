# screenie-record

The recording engine. It turns a `screenie_core::FrameSource` (frames as the screen
changes) and optional audio into an H.264/AAC MP4 with GStreamer.

```rust
let source = capture.stream(backend, "DP-1", Some(region), cursor)?;
let recording = Recording::start(source, RecordSpec { path, framerate: 60, quality, encoder, audio })?;
recording.set_paused(true)?;
let finished = recording.stop()?; // path, duration, size, bytes, last frame
```

- `Encoder::candidates(preference, size, gpu)` lists the working H.264 encoders that take
  the video's size, best first: hardware on the GPU holding the frames, other hardware
  (VA-API, NVENC, gstreamer-vaapi, V4L2), then software (x264, OpenH264). Each is
  test-encoded once. An encoder's `Chain` is how frames reach it (VA-API's converter,
  GL, or the CPU); `Encoder::gpu_format(offer)` says whether it can take the source's
  GPU buffers: on its GPU, in a format its chain imports, and, for a region of a
  buffer (`GpuOffer::cropped`), only if the chain crops (`Chain::crops`: VA-API's does,
  GL's doesn't). `SCREENIE_ENCODER` forces one.
- The frame pump runs on its own thread. It caps the frame rate, holds early frames
  rather than dropping them, skips identical frames, and hands frames to GStreamer
  without copying (`feed`): memory as is, GPU buffers (DMA-BUF) as DMA-BUF memory the
  converter imports, released to the source once it's done with them. Frames are stamped
  with when the compositor presented them.
- `RecordSpec::framerate` paces the source (`FrameSource::pace`), so the compositor
  only copies frames that get recorded. `resolution` caps the video's size.
- The video is the first frame's size, capped. Frames of another size (a recorded
  window being resized) are scaled to fit, letterboxed, and come through memory from
  then on. Scaling and colour conversion run on the encoder's GPU (`vapostproc`, or GL
  for NVENC).
- Each recording logs its encoder and whether frames stay on the GPU, then how many
  frames it received, pushed (on the GPU) and skipped: skips mean the pipeline couldn't
  keep up.
- `stop_capture()` ends the video there (its length and last frame are final, see
  `latest_frame()`), so the caller can show a preview before `stop()` finishes the
  file. `stop()` blocks until the file is finalized, however long that takes while it's
  still being written; a finish that fails or stalls deletes nothing, and
  `Error::Unfinished` says where the recording was kept. `cancel()` or dropping the
  recording deletes it. `failure()` reports a broken pipeline so the caller can salvage the file,
  and `ended()` a source that went away (a recorded window closed).

See [docs/features/recording.md](../../docs/features/recording.md) for the design.

```sh
cargo run -p screenie-record --example rec -- encoders
cargo run -p screenie-record --example rec -- OUTPUT SECONDS FILE [region] [audio] [mic] [pause]
```
