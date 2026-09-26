# Configuration & settings

Status: **config done; settings window planned.**

The config lives at `$XDG_CONFIG_HOME/screenie/config.yaml`. Every key is optional and
missing keys take defaults, so an empty file is valid. The daemon reloads it within a
second of any change, whether it was made by hand or by the settings window
(`screenie settings`, planned). It watches the contents, not the modification time, so a
symlink swapped to another target (home-manager's Nix store links) counts too. Saves are atomic and keep a short header comment.

Sections: `screenshot`, `recording`, `preview`, `selector`, `editor`, `advanced`. See
`crates/screenie-config/src/schema.rs` for every key with its doc comment.

`screenshot.directory` and `recording.directory` expand `~`, `$VAR` and `${VAR}`
(`$XDG_PICTURES_DIR` and `$XDG_VIDEOS_DIR` work even when only `user-dirs.dirs` sets
them), and a relative path is relative to the home directory. A variable that isn't set
falls back to the default directory, with a warning in the log. The daemon runs from
`/`, so it never holds the directory it was started from busy.

## Interface scale

`ui_scale` sizes the interface: bars, buttons, text, handles, the loupe and preview
cards. It works on top of the display's own scale, so the two multiply: `ui_scale: 1.25`
on a 1.5× monitor draws the interface at 1.875 device pixels per pixel, still sharp.
Screen geometry never scales: selections, and captures shown in place, stay exactly
where they are on screen.

- `auto` (the default) follows the desktop's text scaling: GTK's `text-scaling-factor`,
  which GNOME's "Large Text" and GNOME Tweaks set (or
  `gsettings set org.gnome.desktop.interface text-scaling-factor 1.25`). It's read
  through the settings portal and followed live. Without a GNOME or GTK portal backend
  it's 1.
- A number, such as `1.25`, fixes it. It's clamped to 0.5–3.

It's implemented with GPUI rems. Interface sizes are written as `ui(n)` (n pixels at
scale 1), and each window's rem size is set from the scale. Screen geometry stays in
plain `px`.

Other paths:

- State and logs: `$XDG_STATE_HOME/screenie/`. That holds `daemon.log` (and the
  previous run's, `daemon.log.1`), and `state.yaml`, what screenie remembers between
  runs: the editor's last colour and size, and the last captures (the region
  `shot last` reuses, and what `query last` reports). That file isn't settings: screenie
  rewrites it, and it's safe to delete.
- Socket: `$XDG_RUNTIME_DIR/screenie/<WAYLAND_DISPLAY>.sock`, one daemon per session,
  kept to one by the `<WAYLAND_DISPLAY>.lock` it holds.
