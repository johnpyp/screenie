# screenie-config

The user's settings and where things live.

- `Config`: the full schema (`screenshot`, `recording`, `preview`, `selector`,
  `editor`, `advanced`). Every field has a default and every section is optional.
- `Config::load` / `save`: `$XDG_CONFIG_HOME/screenie/config.yaml`. Saves are atomic.
- `Paths`: config, state (`state.yaml` belongs to `screenie-state`), runtime socket,
  and Pictures/Videos directories.
- `expand_template` / `unique_path`: strftime file names that never overwrite.
