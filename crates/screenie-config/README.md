# screenie-config

The user's settings and where things live.

- `Config`: the full schema (`screenshot`, `recording`, `preview`, `selector`,
  `editor`, `advanced`, `ui_scale`). Every field has a default and every section is
  optional, so an empty file is valid. `schema.rs` documents every key.
- `Config::load` / `save`: `$XDG_CONFIG_HOME/screenie/config.yaml`. Saves are atomic.
  The daemon reloads on a change of *contents*, not mtime, so a symlink swapped to
  another target (home-manager's Nix store links) counts too.
- `Paths`: config, state (`state.yaml` belongs to `screenie-state`), runtime socket,
  and Pictures/Videos directories.
- `expand_user_path`: `screenshot.directory` and `recording.directory` expand `~`,
  `$VAR` and `${VAR}` (`$XDG_PICTURES_DIR` and `$XDG_VIDEOS_DIR` work even when only
  `user-dirs.dirs` sets them). A relative path is relative to home, never the daemon's
  working directory (it runs from `/`). An unset variable falls back to the default
  directory, with a warning.
- `expand_template` / `unique_path` / `claim_unique`: strftime file names that never
  overwrite. The time is when the screen was frozen, however much later the capture is
  saved. `{app}` (e.g. `firefox`, or `Nautilus` for `org.gnome.Nautilus`) and `{title}`
  fill in when a window was captured, and otherwise disappear along with one adjacent
  separator. Collisions get `-2`, `-3`… `claim_unique` takes the name on the spot (an
  empty file to replace), so concurrent captures never pick the same one.

## Where things live

| Path | What |
| --- | --- |
| `$XDG_CONFIG_HOME/screenie/config.yaml` | Settings. |
| `$XDG_STATE_HOME/screenie/state.yaml` | What's remembered between runs (`screenie-state`). Safe to delete. |
| `$XDG_STATE_HOME/screenie/daemon.log` | The daemon's log; the previous run's is `daemon.log.1`. |
| `$XDG_RUNTIME_DIR/screenie/<WAYLAND_DISPLAY>.sock` | The daemon's socket, one per session, kept to one by `<WAYLAND_DISPLAY>.lock`. |

## Interface scale

`ui_scale` sizes the interface (bars, buttons, text, handles, the loupe, cards) on top
of the display's own scale: `1.25` on a 1.5× monitor draws at 1.875 device pixels per
pixel, still sharp. Screen geometry never scales. `auto` follows GTK's
`text-scaling-factor` (GNOME's "Large Text") live through the settings portal, and is 1
without a GNOME or GTK portal backend. A number is clamped to 0.5–3. See
`screenie_ui_kit::scale` for how it's applied.

The top-level README's config examples are parsed by a test here, so they can't drift
from the schema.
