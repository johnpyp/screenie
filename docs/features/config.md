# Configuration & settings

Status: **config done; settings window planned.**

The config lives at `$XDG_CONFIG_HOME/screenie/config.yaml`. Every key is optional and
missing keys take defaults, so an empty file is valid. The daemon reloads it within a
second of any change, whether it was made by hand or by the settings window
(`screenie settings`, planned). Saves are atomic and keep a short header comment.

Sections: `screenshot`, `recording`, `preview`, `selector`, `editor`, `advanced`. See
`crates/screenie-config/src/schema.rs` for every key with its doc comment.

Other paths:

- State and logs: `$XDG_STATE_HOME/screenie/` (`daemon.log`).
- Socket: `$XDG_RUNTIME_DIR/screenie/<WAYLAND_DISPLAY>.sock`, one daemon per session.
