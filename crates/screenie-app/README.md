# screenie-app

The daemon. `screenie_app::run()` binds the socket, starts GPUI with
`QuitMode::Explicit`, and serves requests until `quit`.

| Module | Role |
| --- | --- |
| `server` | Socket accept loop (threads) → `Incoming` messages on a channel. |
| `daemon` | The `Daemon` global (config, capture context, last region, status watchers), request routing, config hot-reload. Every `Daemon::update` tells status watchers of whatever it changed. |
| `last` | The latest capture of each kind (`screenie query last`), kept up to date as cards save or delete them, and remembered in `state.yaml`. |
| `screenshot` | Freeze → select or resolve target → render → deliver. |
| `deliver` | After-capture actions for screenshots and recordings: encode, save, copy, preview or edit. |
| `editor` | Opens `screenie-editor` for a capture or `screenie edit FILE`, and acts on its output (copy, save over the capture, Save As, Done). |
| `clipboard` | Data-control clipboard offers: `image/png`, plus `text/uri-list` and `x-special/gnome-copied-files` once there's a file, so pasting into a file manager or chat attaches it. No `text/plain`, so a text field doesn't get a URI. The daemon owns the offers, since a Wayland clipboard dies with its owner. |
| `recording` | Record flow (live selector → countdown → `screenie-record` → deliver), stop/pause/cancel, and the on-screen controls (border, countdown, pill). |
| `preview` | The floating preview card stacks (screenshots and recordings), one per output, on one click-through layer surface fitted between bars. Cards never take the keyboard (`screenie_ui_kit::Hover`), pause while hovered or while an overlay editor is open, and are hidden from captures of their screen. |
