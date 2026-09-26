# Patched dependencies

Vendored copies of crates with local fixes, wired up through `[patch.crates-io]` in the
workspace `Cargo.toml`. Each fix is marked `screenie patch` in the source. Drop a patch
once an upstream release fixes it.

## gpui-pre-linux 0.3.6

`src/linux/wayland/window.rs`, `set_size_and_scale`: don't set the `wp_viewport`
destination while the window size is 0x0.

A layer surface anchored to opposite edges must request size 0 for the compositor to
stretch it (clear of bars). On a scaled output the preferred scale can arrive before the
first configure, and GPUI then sent `set_destination(0, 0)`: a protocol error that kills
the whole Wayland connection (the daemon's UI goes dead). Upstream: Zed's `gpui_linux`.

`src/linux/wayland/client.rs`, `window_identifier`: only export the focused window as a
file dialog's parent when it's an `xdg_toplevel`.

GPUI exported whichever window had keyboard focus through xdg-foreign. For a layer
surface (screenie's overlays: the editor's Save As) that's a protocol error
(`zxdg_exporter_v2` `invalid_surface`), which kills the whole Wayland connection, so
no screenshot works until the daemon restarts. The portal simply opens the dialog
without a parent instead.

`src/linux/wayland/client.rs` (`zwp_relative_pointer_v1`) and `window.rs`
(`set_pointer_stuck`): the Wayland side of `Window::observe_pointer_stuck` below. It binds
the relative pointer, and a window under the pointer that gets a few relative moves in a
row with no motion (pushing against its edge doesn't count) is told the pointer is stuck.

## gpui-pre 0.3.6

`src/platform.rs` (`PlatformWindow::on_pointer_stuck`) and `src/window.rs`
(`Window::observe_pointer_stuck`): the pointer is over the window and being moved, but
doesn't move. sway enforces a pointer lock for the surface with the keyboard and drops
every motion while the cursor is over another one, so a game that locks its pointer
while the cursor is on a card (which never takes the keyboard) freezes the cursor there.
The relative motion still reaches the card, and `screenie_ui_kit::Hover` steps aside
when told.

HACK: it exists for a sway bug (1.12 and master as of 2026-09): sway activates a pointer
constraint without giving its surface pointer focus, which the protocol guarantees.
Drop it, and the step-aside in `Hover`, once sway fixes that.

The examples aren't vendored (their `[[example]]` entries are removed from
`Cargo.toml`).

`src/window.rs` (`Window::dispatch_event`, `mouse_hit_test_in`, `TooltipId::is_hovered`)
and `src/elements/div.rs` (the tooltip's prepaint hover check): forget the hover when the
pointer leaves the window.

`MouseExited` kept the last `mouse_position`, and the hit test was re-derived from it on
every frame, so whatever was under the pointer as it left stayed hovered (a button's
highlight, a tooltip still coming up) until it came back. A layer surface is left from
its edge often: the pointer goes from a button straight onto the app beneath. Upstream:
Zed's `gpui`.
