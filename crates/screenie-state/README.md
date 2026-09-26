# screenie-state

What screenie remembers between runs, as opposed to what the user sets
(`screenie-config`): the editor's last colour, size and fill, and the last captures (the
region `shot last` reuses, and the latest screenshot and recording `query last`
reports), so they survive the daemon restarting.

- **`StateFile`** is the only reader and writer of `$XDG_STATE_HOME/screenie/state.yaml`.
  `open()` never fails: a missing file is empty state, and a broken or unversioned one
  is renamed to `state.yaml.bad` and replaced. `update(|state| …)` writes atomically,
  and only when something changed.
- **`State`** is the current layout. Every field is optional: absent means "nothing
  remembered yet", and the caller falls back to the configured default. It's its own
  on-disk type, not the app's internal ones, so internal refactors don't change the file.
- **Versions.** The file carries `version:`. Older files are migrated as untyped YAML
  through a chain of `if from < N { … }` steps (`src/migrate.rs`) before being read. A
  file from a *newer* screenie is left untouched: this run remembers in memory only.
- **Fixtures.** `tests/fixtures/vN.yaml` holds a file as each version writes it. The
  tests migrate every one and check nothing is lost on the way. To change the layout,
  follow the steps at the top of `src/migrate.rs`.
