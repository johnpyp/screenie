# Key lessons

Bugs that were tricky or non-obvious, and what they taught. Add to this when a fix took
real digging.

## Wayland input and focus

- **Keyboard focus that moves mid-press splits the key between two apps.** Xwayland
  never delivers a release that happened while an app was unfocused: a Tab held onto a
  preview card stayed down in a Proton game, and Shift then opened Steam's overlay.
  Floating overlays must never take the keyboard.
- **Closing a surface on a key press leaks the key to the app beneath** (it sees the
  release, or the press, e.g. Esc in your terminal). Close only once every key and
  modifier is up (`KeyboardGrab::when_released`).
- **sway freezes the cursor over another surface while a game holds a pointer lock.**
  Detect "relative motion but no motion", drop the input region and map a throwaway
  pixel: mapping a surface is what makes sway re-pick the surface under the cursor.
- **Compositors re-pick the pointer's surface when a surface maps, not when its input
  region grows.** An overlay appearing under a resting pointer isn't entered.
- **A release can arrive without its press.** Clicking a toolbar button that switches
  mode let the release reach the canvas as a click; count releases only after a press
  on the same target.
- **Gaps in a panel are click-through unless occluded**, so near misses drew on the
  canvas beneath.

## GPUI

- **`set_destination(0, 0)` on `wp_viewport` is a protocol error that kills the whole
  connection.** Layer surfaces anchored to opposite edges request size 0 and can get a
  scale before their first configure.
- **Exporting a layer surface through xdg-foreign is a protocol error** (the portal file
  chooser's parent), and it killed every later screenshot until a restart.
- **`MouseExited` kept the last position**, so whatever was under the pointer as it left
  a layer surface stayed hovered.
- **The default quit mode exits when the last window closes**; a daemon needs
  `QuitMode::Explicit`.
- **Surfaces on the same layer stack in the order they appear.** To put cards above an
  editor that opened later, move them to a new surface.
- "window not found" errors after a selector closes are GPUI delivering a final event to
  a removed surface: benign.

## Capture

- **A protocol can be advertised but unusable** (e.g. only a pixel format we can't
  read). Hyprland may offer only packed 24-bit `BGR888`. Fall back to the other protocol
  and remember what worked.
- **Measure each output's scale** (buffer / logical width) instead of trusting it, or
  fractional scales drift by half pixels.
- **Anything over a fullscreen app costs it direct scanout**, and capturing an output
  forces compositing for as long as it lasts. A window captured by itself keeps the game
  on scanout.
- **Hiding surfaces by unmapping animates on Hyprland**; transparency doesn't (but blur
  layer rules without `ignorezero` still show for that frame).
- **Blanking the selector at capture flashed the live screen** before the editor drew;
  keep the old surface up until the new one has drawn.

## Recording

- **x264enc's `bitrate` becomes a 2 Mbit/s VBV cap in CRF mode**, starving motion. Set
  it to its maximum.
- **x264 B-frames skewed variable-frame-rate timestamps**, and **NVENC falls back to
  Baseline without B-frames**, costing ~20% more bits; force High.
- **GL elements ignore `GstVideoCropMeta`**, squashing a whole screen into a region's
  video on NVIDIA. Regions prefer wlr-screencopy, which copies only the region.
- **VA-API elements exist whenever the plugin does, driver or not.** Probe encoders by
  encoding test frames through the real conversion chain.
- **Every redraw is damage the encoder sees.** A pulsing dot made static screens encode
  at full frame rate; chrome redraws once a second.
- **Finishing a faststart MP4 can take minutes** on a long recording and slow disk.
  Wait while the file grows, and never delete data on a failed finish.
- **wlroots window captures have no cursor.** A toplevel source is a scene-node source,
  which ignores `paint_cursors` and offers no cursor session. Only output sources do, and
  only for a hardware cursor. So we draw the pointer ourselves from the outputs' cursor
  sessions, mapped into the window through its IPC rectangle.
- **wlroots has no hardware cursor on NVIDIA.** Its cursor plane takes only linear
  buffers, which NVIDIA can't render into, so no cursor format intersects and the
  cursor is drawn in software (wlroots !4596 and !5209 are the open fixes). Cursor
  sessions stay silent, OBS gets no cursor either, and `grim` without `-c` shows the
  pointer, which is the quick check for a software cursor.
- **`gloverlaycompositor` passes through when its caps don't change**, which they never
  do, so the overlay meta was silently ignored. Turn passthrough off on each CAPS event.
  Asking for the meta's caps feature instead fails: `glupload` and `glcolorconvert`
  reject it.
- **A cursor session only reports a pointer the seat has.** In the headless session the
  seat's pointer is wlinput's virtual one, so it exists only while wlinput runs.

## GNOME and KDE

- **KWin ignores `NoDisplay` desktop entries when it authorizes a process.** It finds a
  process's entry (whose `Exec` binary is `/proc/PID/exe`, canonically) among the apps a
  menu would list, so a hidden entry grants nothing: ScreenShot2 answers `NoAuthorized`
  and the restricted Wayland globals aren't advertised. Run `kbuildsycoca6` after
  writing it.
- **KWin screenshots need OpenGL.** On a software (QPainter) session ScreenShot2 answers
  `Cancelled`. The test containers get the host's `/dev/dri`.
- **KWin can hand a screen back with alpha**, zero where nothing's drawn. Screens are
  opaque: read the alpha formats as their `x` twins.
- **GNOME lets an app with an id raise an access dialog only while it's the focused
  app.** A daemon that registered its app id with the portal could never be granted
  screenshots (the portal fails at once, and nothing is stored). Unidentified host apps
  are asked any time, and share the one permission.
- **GNOME's clipboard needs focus, but a remote desktop session's doesn't.** A Mutter
  RemoteDesktop session that's never started shows no indicator and can own the
  clipboard (`EnableClipboard`, `SetSelection`, answer `SelectionTransfer`). Mutter's
  clipboard manager copies images up to 200 MB right away, so a copy outlives it.
  `Stop` refuses a session that was never started; it ends with the connection.
- **A cast stopped from GNOME's top bar only pauses the stream.** The consumer's PipeWire
  stream goes to `Paused`, not an error; the node going away (the registry's
  `global_remove`) is what says it's over.
- **GPUI asked for fullscreen without naming an output**, so GNOME put every selector
  window on one screen. Patched to pass the window's own output.
- **Each Mutter RemoteDesktop session adds virtual devices, and the seat's capabilities
  change**: GPUI re-creates its `wl_pointer` and misses the next events. Test input goes
  through one session for a whole script (`tools/desktop.sh input gnome -`).
- **kglobalacceld reads an app's entry once**, when it first makes its component: a
  rewritten entry's actions and keys are ignored, and removing its keys leaves them
  bound. Drop the component (`cleanUp`) and have it made again (`doRegister` of a
  dummy action, then `unregister`, as System Settings does). `doRegister` of an action
  the entry already has makes a duplicate.
- **Clashing default keys aren't dropped when kglobalacceld loads an entry**: two
  components can hold Print, and one wins. Take keys from their holders with
  `setForeignShortcutKeys`, then set your own the same way.
- **GNOME's custom shortcuts are run by gsd-media-keys**, not the shell; a session
  without it grabs nothing. And a built-in binding wins over a custom one with the same
  key.
- **GJS can't read pixels back.** `Cogl.Texture.get_data` and
  `Clutter.Stage.paint_to_buffer` fill a copy of the array passed in, which is dropped
  (and `Cogl.Framebuffer.read_pixels` crashes the shell). An extension's screenshots
  come out as PNG only (`Shell.Screenshot`), about 3.5 s for two busy 4K screens, where a
  Mutter cast's first frame takes tens of milliseconds.
- **A cast's top-bar indicator can't be told apart by its handle**
  (`MetaRemoteAccessHandle` has no sender or session). But Mutter makes the handle while
  handling the session's `Start`, and gnome-shell's indicators hear of it right then
  (`_onNewHandle`), so the extension claims the one handle made between the caller's
  "next cast is mine" and "started", and keeps it from them. A non-recording cast's
  indicator also stays five seconds after it ends: stills are marked recordings.
- **An always-on-top window keeps new windows it mostly covers from being focused**
  (Mutter 47+, `window_would_mostly_be_covered_by_always_above_window`; any overlap
  before). A dock is above normal windows anyway (its layer), so screenie's floating
  surfaces are docks that are never made always-on-top; only the selector and editor
  are, and they're focused explicitly once shown.
- **GNOME loads extensions only at login.** A new one isn't found until then (and
  `EnableExtension` fails for it), so `screenie extension install` enables it in
  `enabled-extensions` for the next login; files updated mid-session run from the next
  one too. Extensions are also turned off while the screen is locked.
- **A Mutter remote desktop session's absolute pointer positions are in the stream's
  pixels**, not logical ones: divided by the monitor's scale on the way in.
- **A Mutter remote desktop session's first press or key goes nowhere.** Its virtual
  pointer and keyboard are made on their first events, and clients bind the new
  `wl_pointer` or `wl_keyboard` only after the seat announces it, so a press gets no
  `enter` first. Selector drags and clicks in the GNOME container silently did nothing
  (and Escape needed sending twice); `input.py` now makes both devices first.
- **Mutter stacks a Wayland window only once it has a buffer.** `make_above()` (which
  raises) on `window-created` tripped `meta_window_set_stack_position_no_sync`'s
  assertion; the extension does it on `map`. The same assertion on any window opening
  fullscreen (`foot --fullscreen` too) is Mutter's own.
- **A buffer-less commit between the first configure's ack and the first buffer** makes
  Mutter warn of invalid window geometry and of content committed without acknowledging a
  configure. GPUI's `set_input_region` committed at once, during the first draw; it waits
  for the first frame now (a patch, `patches/README.md`).
- **A monitor that sleeps comes back as a new `wl_output` with the same name.** GPUI
  never handled `global_remove`, so the dead output stayed listed under that name and a
  surface opened on it was closed at once: the selector skipped a monitor after it woke,
  until the daemon restarted. It forgets removed outputs now (a patch, `patches/README.md`).

## Packaging

- **Rust bindings to a C library build against its headers, not its ABI.** `libspa` 0.10
  wraps SPA helpers that were still macros in PipeWire 0.3.48 and struct fields added
  since, so the 22.04 release builds failed where 24.04's tests passed. Newer headers
  with the old library to link against keep the binary to what the old one has.

## Daemon, files and clipboard

- **Wayland clipboards die with their owner**, so an upgrade that replaces the daemon
  empties the clipboard (still open).
- **Two commands at once could start two daemons.** A `flock` held for the daemon's life
  plus a spawn lock for deciding.
- **Config reload by mtime misses home-manager symlink swaps**; compare contents.
- **A daemon inherits the cwd of whoever spawned it**, keeping a mount busy. Run from
  `/`, resolve relative paths against home.
- **Truncating the log on spawn erased the crash that caused the respawn**; keep
  `daemon.log.1`.
- **Name files when taken and claim the name immediately**, or parallel captures in the
  same second overwrite each other.
- **Default PNG compression took 0.6 s at 4K**, delaying the card and clipboard; use fast.
- **Every state change must be broadcast**, or `--watch` sticks on a stale state.
