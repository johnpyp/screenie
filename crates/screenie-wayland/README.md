# screenie-wayland

Screen capture over raw Wayland protocols with `wayland-client`. It does not shell out
to grim.

- `Support::probe()` reports which capture, layer-shell and data-control globals exist.
- `Capturer` discovers outputs (with `xdg-output` logical geometry) and captures them:
  - `capture_outputs(names, cursor)` grabs stills of several outputs concurrently.
  - `into_stream(output, region, cursor)` returns a `FrameStream` of damage-driven
    frames for recording. `set_max_rate(fps)` paces it: the next frame is only asked
    for when it's due, so the compositor doesn't copy frames that would be dropped.
  - `into_window_stream(window, cursor)` streams one window by itself
    (`ext-foreign-toplevel-image-capture-source-v1`), found through
    `ext-foreign-toplevel-list-v1` by identifier, or by app id and title. Its frames
    change size with the window, and the stream ends when it closes.
- Protocols: `ext-image-copy-capture-v1` and `wlr-screencopy-unstable-v1`, chosen
  automatically or forced with `Backend`.
- Handles 8-bit and 10-bit shm formats, y-invert, and all output transforms. Results are
  pixel-identical to grim.

```sh
cargo run -p screenie-wayland --example snap -- [dir] [ext|wlr] [--stream N]
cargo run --release -p screenie-wayland --example stream -- OUTPUT [SECONDS] [ext|wlr]  # capture rate alone
```
