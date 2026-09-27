Screenie is a screenshot & light recording app for Wayland.

We want good design and good UX.

General:
- Just wayland support is fine, but it should be robust to major wayland setups, not bespoke to one. doesn't necessarily need testing for every OS, just don't make it *coupled*.
- Invoked via cli commands, single daemon spawned on demand
- Batteries included, not piping together a selector and stuff.
- xdg config, mutable in-app

Docs:
- `ai-docs/key-decisions.md`: why things are the way they are. Read it before changing direction.
- `ai-docs/key-lessons.md`: bugs that were tricky or non-obvious. Add one when a fix took real digging.
- Crate READMEs document each crate, including its user-facing behaviour.
- The top-level README is for end users only: what screenie does, using it, configuring it,
  installing it. Development notes go here, not there.

Workflow:
- Use high quality rust crates from the ecosystem.
- Modular crate architecture for fast compile times
  - Readme per crate. focus on good names and abstraction.
- Always be refactoring to make things better. don't focus on making the "minimal change", make the best change.
- Feel free to add reference repos and things to `references/`. List repos in `references/manifest.toml`
  (`mise run refs` syncs them). Copyleft ones (GPL, LGPL, …) go in `references/manifest.local.toml`
  instead: same format, gitignored, synced along with it.
- Add tools that are helpful for you to verify or troubleshoot stuff to `tools/`. python is fine.
- Non-cargo output (logs, screenshots, samples, scratch files) goes in `.cache/`, never `target/`.
- Feel free to install any dep, system or otherwise, you need.
- Mise for project tooling. `mise run fmt` formats Rust and Python (ruff) alike.
- Commit directly to `main` (no feature branches). Intermediate commits don't need to build.

## Development

The workspace is split into small crates so builds stay fast; each has a README.

| Crate | Role |
| --- | --- |
| `screenie` | The CLI binary |
| `screenie-app` | The daemon: request routing, capture/record flows, preview cards, recording controls |
| `screenie-selector` | The capture overlay and its (unit-tested) interaction model |
| `screenie-editor` | The annotation editor (overlay or window) and its (unit-tested) interaction model |
| `screenie-annotate` | Annotation documents and their tiny-skia renderer (shared by canvas and export) |
| `screenie-record` | GStreamer recording engine |
| `screenie-capture` | "Freeze the desktop" facade and backend selection |
| `screenie-wayland` | ext-image-copy-capture / wlr-screencopy, stills and streams |
| `screenie-compositor` | Window geometry via Sway / Hyprland / niri IPC |
| `screenie-ui-kit` | Shared GPUI look: fonts, icons, HUD widgets, layer-shell helpers |
| `screenie-ipc` | CLI ⇄ daemon protocol and socket |
| `screenie-config` | Config schema, XDG paths, file naming |
| `screenie-state` | What's remembered between runs: a versioned, migrated state file |
| `screenie-core` | Geometry, images, snapshots, frame sources |

Building and checking:
- `cargo build` builds only the app (`default-members`); pass `--workspace` for the rest:
  `cargo clippy --workspace --all-targets`, `cargo test --workspace`.
- `--release` is tuned for quick rebuilds while developing (incremental, no LTO). The `dist`
  profile (thin LTO, one codegen unit, `target/dist/`) is what the README tells users to install.
- The binary is stamped with its git commit (`build.rs`); a daemon from another build is
  replaced by the next command, so `cargo build --release` and run is the whole upgrade loop.
- The top-level README's config examples are parsed by a test in `screenie-config`.
- `SCREENIE_LOG=debug` for more logging; the daemon logs to `$XDG_STATE_HOME/screenie/daemon.log`.

No display needed: `tools/` has a headless sway session with two mixed-DPI outputs, a virtual
pointer and keyboard, and an image diff:

```sh
tools/session.sh start && source $XDG_RUNTIME_DIR/screenie-session.env
cargo run -p screenie -- shot
cargo run -p wlinput -- drag 100 100 800 600 15
tools/session.sh shot .cache/session.png
python3 tools/imgdiff.py a.png b.png
uv run --project tests/e2e tools/rec_stress.py screen 6   # recording frame rate at 4K
```

`tests/e2e` drives the real daemon in that session with pytest (keyboard handoff, the editor's
close/save flows, Save As, remembered state, interface scale); each test gets its own config,
state and home. `mise run test:e2e` (or `mise run test:e2e -- -k save_as -x`). See its README.

CI (`.github/workflows/ci.yml`) runs fmt, clippy and the tests, and on every push to
`main` builds the release archives for x86_64 and aarch64 with `tools/dist.py` (run it
locally for the same archive in `.cache/dist/`). To release, bump `version` in the
workspace `Cargo.toml` and push a matching `vX.Y.Z` tag: CI publishes a GitHub release
that `mise use github:johnpyp/screenie` installs. The e2e tests don't run in CI, as the
runners' sway is too old. `mise run ci -j package` runs a job locally with act, in podman
(`systemctl --user start podman.socket`), in images of GitHub's runners (`.actrc`).

The README's images come from `tools/demo` (see its README): a headless demo desktop, a scripted
tour, `stills.py` for the hero composite and close-ups, and `render.py` for the video.
