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
| `screenie-capture` | "Freeze the desktop" facade and backend selection (Wayland, KWin, Mutter, portal) |
| `screenie-wayland` | ext-image-copy-capture / wlr-screencopy, stills and streams; KWin's screen casts |
| `screenie-pipewire` | Screen casts (Mutter, KWin, portal) read over PipeWire |
| `screenie-compositor` | Window geometry via Sway / Hyprland / niri IPC, KWin scripts, Mutter's display config |
| `screenie-desktop` | GNOME and KDE: desktop entry, shortcuts, notifications, GNOME's clipboard and overview, and screenie's GNOME Shell extension (`extension/`) |
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

GNOME and KDE Plasma run headless in podman containers (Fedora images, real gnome-shell and
kwin_wayland with two virtual outputs, D-Bus, PipeWire, the portals), with the repository
and target directory mounted at the same paths, so a host build runs inside as is:

```sh
tools/desktop.sh build                       # once
tools/desktop.sh start gnome                 # or kde, or gnome48; home is .cache/desktop/<name>/home
tools/desktop.sh run gnome target/release/screenie shot all
printf 'key Print\nsleep 2\n' | tools/desktop.sh input gnome -   # one input session per script
tools/desktop.sh shot gnome .cache/gnome.png
```

On GNOME, each `input` call is a remote desktop session, shown in the top bar while it
runs and for a few seconds after, as are `shot`'s screen casts: wait them out before a
capture that shows the top bar. `screenie extension install` there, then `start` again, is a login
with the extension; `start gnome --unsafe-mode` allows `org.gnome.Shell.Eval` for poking
at the shell.

`tests/e2e` drives the real daemon in the sway session with pytest (keyboard handoff, the editor's
close/save flows, Save As, remembered state, interface scale); each test gets its own config,
state and home. `mise run test:e2e` (or `mise run test:e2e -- -k save_as -x`). See its README.

CI (`.github/workflows/ci.yml`) runs fmt, clippy and the tests on every push. For a tag
or a manual run it also builds the release archives for x86_64 and aarch64 with
`tools/dist.py` (run it locally for the same archive in `.cache/dist/`) and the flake. To release, run the Release workflow
(`gh workflow run release.yml -f bump=patch|minor|major`, or `-f version=X.Y.Z`,
`-f dry_run=true` to look first): it bumps the version (`tools/bump_version.py`),
commits, tags `vX.Y.Z` and starts CI on the tag, which publishes a GitHub release that
`mise use github:johnpyp/screenie` installs once lint and tests pass. Pushing a tag by
hand does the same. The e2e tests don't run in CI, as the
runners' sway is too old. `mise run ci -j package` runs a job locally with act, in podman
(`systemctl --user start podman.socket`), in images of GitHub's runners (`.actrc`).

`flake.nix` packages screenie (`nix/package.nix`: the dist profile, man pages, the desktop
entry, and GStreamer's plugin directories built in as `SCREENIE_GST_PLUGIN_PATH`, not
set by a wrapper, since KWin checks the binary an entry runs) and has a dev shell; CI's `nix` job builds it. Its
source is a fileset, so a file embedded from outside `crates/`, `patches/` or `assets/`
needs adding there. A Nix build only opens windows with NixOS's GPU drivers (or nixGL).

The README's images come from `tools/demo` (see its README): a headless demo desktop, a scripted
tour, `stills.py` for the hero composite and close-ups, and `render.py` for the video.
