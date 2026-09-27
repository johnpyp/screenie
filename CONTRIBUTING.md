# Contributing

## Setting up

Project tooling (Rust, uv, act) comes from [mise](https://mise.jdx.dev):

```sh
mise install
```

screenie builds against GStreamer, PipeWire, xkbcommon, GBM and fontconfig (and needs
libclang for PipeWire's bindings), and records with GStreamer's plugins.

On Debian or Ubuntu:

```sh
sudo apt install build-essential pkg-config \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libpipewire-0.3-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libgbm-dev libfontconfig-dev libclang-dev \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-ugly gstreamer1.0-libav gstreamer1.0-pulseaudio
```

On Arch:

```sh
sudo pacman -S --needed base-devel clang gstreamer gst-plugins-base-libs \
  libpipewire libxkbcommon-x11 mesa fontconfig \
  gst-plugins-good gst-plugins-bad-libs gst-plugins-ugly gst-libav gst-plugin-va
```

Or use the Nix dev shell: `nix develop`.

## Building and checking

```sh
cargo build --release                        # the app; run it and the daemon is replaced
cargo clippy --workspace --all-targets
cargo test --workspace
mise run fmt                                 # Rust and Python
mise run test:e2e                            # end-to-end, in a headless sway session
```

`cargo build` builds only the app; pass `--workspace` for everything else. Each crate
has a README. `tools/session.sh` starts a headless sway session for trying things without
a display, and [`tests/e2e`](tests/e2e/README.md) lists what the end-to-end tests need.
