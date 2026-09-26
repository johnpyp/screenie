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

`src/linux/wayland/window.rs`, `set_keyboard_interactivity`: the Wayland side of the
`gpui-pre` patch below.

## gpui-pre 0.3.6

`src/platform.rs` (`PlatformWindow`) and `src/window.rs` (`Window`):
`set_keyboard_interactivity`, to change a layer surface's keyboard interactivity after
it's mapped, like the existing `set_exclusive_zone` and `set_input_region`.

The preview cards take the keyboard only while the pointer is on them
(`screenie_ui_kit::HoverKeyboard`). Layer-shell allows that
(`zwlr_layer_surface_v1.set_keyboard_interactivity` any time), but GPUI only set it at
creation. The examples aren't vendored (their `[[example]]` entries are removed from
`Cargo.toml`). Upstream: Zed's `gpui`.
