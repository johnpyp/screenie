# screenie-wayland

Screen capture over raw Wayland protocols with `wayland-client`. It does not shell out
to grim.

- `Support::probe()` reports which capture, layer-shell and data-control globals exist.
- `Capturer` discovers outputs (with `xdg-output` logical geometry) and captures them:
  - `capture_outputs(names, cursor)` grabs stills of several outputs concurrently.
  - `into_stream(output, region, cursor)` returns a `FrameStream` of damage-driven
    frames for recording. `set_max_rate(fps)` paces it: the next frame is only asked
    for when it's due (a little ahead, by how long the compositor takes), so the
    compositor doesn't copy frames that would be dropped. Frames carry the compositor's
    presentation time.
  - Frames of a stream come in shared memory (two buffers, so the compositor copies one
    while the other is read) or, after `use_gpu(format)`, in GPU buffers: `gpu_offer()`
    says which GPU and formats the compositor renders into, and `dmabuf` allocates a
    pool on that GPU with GBM, leasing each buffer out until the consumer lets go of its
    frame. ext says the GPU and formats itself; wlr names a format, and the GPU and
    its layouts come from linux-dmabuf's default feedback (`state::Feedback`), which
    also answers `gpu()` for streams in memory. `snapshot()` is the stream's current
    picture in memory, e.g. for a thumbnail of GPU frames. A region of an output
    captured whole (ext) comes in whole GPU buffers with a crop, which `gpu_offer()`
    marks `cropped`.
  - `set_paused(true)` closes the capture on the compositor's side (keeping the
    buffers), so a paused recording copies nothing and a captured output can scan out
    a fullscreen app directly again. `set_paused(false)` starts it over, and its first
    frame comes right away.
  - `into_window_stream(window, cursor)` streams one window by itself
    (`ext-foreign-toplevel-image-capture-source-v1`), found through
    `ext-foreign-toplevel-list-v1` by identifier, or by app id and title. Its frames
    change size with the window, and the stream ends when it closes.
- `focused_output()` finds the output the user is on without compositor IPC: it maps a
  transparent 1×1 layer surface with no output (the compositor places it on the one the
  user last interacted with), reads `wl_surface.enter`, and removes it.
- Protocols: `ext-image-copy-capture-v1` and `wlr-screencopy-unstable-v1`, chosen
  automatically or forced with `Backend`.
- Handles 8-bit and 10-bit shm formats, y-invert, and all output transforms. Results are
  pixel-identical to grim.

```sh
cargo run -p screenie-wayland --example snap -- [dir] [ext|wlr] [--stream N]
RUST_LOG=debug cargo run --release -p screenie-wayland --example stream -- OUTPUT [SECONDS] [ext|wlr] [shm|gpu|offer] [FPS]  # capture rate alone
```
