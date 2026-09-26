# screenie-core

The vocabulary every other crate shares. It has no Wayland or GUI dependencies.

- **`geom`**: `Point`, `Size`, `Rect` (logical, `f64`) and `PixelRect` (physical, integer).
  `Rect::to_pixels(origin, scale)` is the one place logical becomes physical. Rects parse
  from `"X,Y WxH"` and `"WxH+X+Y"`. `Transform` models `wl_output` transforms.
- **`image`**: `Image`, a cheap-to-clone (Arc'd) 8-bit pixel buffer in one of four
  byte orders. It supports crop, blit, bilinear resize, format conversion and PNG
  encode/decode. Opaque images are written as RGB.
- **`desktop`**: `OutputInfo`, `WindowInfo`, and `Snapshot` (every output's pixels plus
  the window list at one instant). `Snapshot::render_region` turns any logical rect into
  an image. It crops natively when the rect is on one output and composites at the
  highest scale when it spans several. `WindowInfo::toplevel` carries the window's
  `ext-foreign-toplevel-list` identifier where the compositor's IPC reports it.
- **`stream`**: the `FrameSource` trait, a live view of the screen or of one window
  that yields frames as it changes (`Next::Frame`), nothing (`Unchanged`), or `Ended`.
  The recorder consumes it; screencopy (and later PipeWire) implement it. `pace(fps)`
  asks a source for fewer frames, and `Pacer` is the fixed clock both sides tick on. A
  `Frame` is `Pixels` (an `Image`, or a `Dmabuf` on the GPU) and when it was presented.
  `gpu()` says which GPU frames are rendered on, `gpu_offer()` / `use_gpu(format)`
  switch a source to GPU buffers, and `snapshot()` gets its picture in memory.
- **`gpu`**: `GpuDevice` (which GPU a device number is, from sysfs: render node,
  vendor, driver, bus), so frames and encoders are matched by device, not vendor.
  `DmabufFormat` (a DRM fourcc and its modifiers), `GpuOffer`, and `Dmabuf`, a frame in
  GPU memory whose buffer goes back to its source when the last clone is dropped.
