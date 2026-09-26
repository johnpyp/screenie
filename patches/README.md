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
