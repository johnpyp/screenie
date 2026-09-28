# screenie-pipewire

Screen casts over PipeWire, as `FrameSource`s. Desktops without Wayland capture
protocols cast the screen to a PipeWire node instead: Mutter's ScreenCast D-Bus API,
KWin's `zkde_screencast`, and the ScreenCast portal all hand out a node id (the portal
also a restricted remote to reach it through, `Remote::Fd`).

- `Stream::connect(remote, node, pointer, keep)` runs its own PipeWire loop on a thread
  of its own (`thread`). It negotiates a format screenie reads, 8-bit RGB in any byte
  order (`format`), in shared memory, and yields each new picture as a `Frame`, cropped
  to what the compositor marks as content. Frames reach the consumer through a slot that
  holds only the newest, so a slow consumer skips frames rather than holding up the
  compositor. `first_picture(timeout)` waits for one: a still is the first picture of a
  short cast.
- `Keep` is what's kept of each picture: all of it, a region (`Crop`, logical, scaled by
  what each frame measures), or the opaque part (`Opaque`: a window cast without the
  shadow it draws around itself, measured again whenever its size changes).
- `Pointer::Metadata` takes the pointer from the cast's metadata, beside the frames, and
  each `Frame` carries it for the consumer to draw.
- The stream is over when PipeWire says so, or when its node goes away: a cast stopped
  from GNOME's top bar only pauses the consumer's stream, so the registry's
  `global_remove` of the node is what ends it.

`examples/first_picture.rs` reads a node's first picture from the session's daemon, as a
still is read: `cargo run -p screenie-pipewire --example first_picture -- NODE`.

The bindings (`pipewire` 0.10) need PipeWire 1.0's headers or newer to build, but the
binary runs against 0.3.x: the release archives, built on Ubuntu 22.04 (0.3.48), compile
against 24.04's headers and link 22.04's library
(`.github/actions/setup/pipewire-headers.sh`).
