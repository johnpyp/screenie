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
- Crate READMEs document each crate, including its user-facing behaviour; the top-level README is for users.

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
- `cargo build` builds only the app (`default-members`); use `--workspace` for clippy and tests.
- Commit directly to `main` (no feature branches). Intermediate commits don't need to build.
