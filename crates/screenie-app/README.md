# screenie-app

The daemon. `screenie_app::run()` binds the socket, starts GPUI with
`QuitMode::Explicit`, and serves requests until `quit`.

| Module | Role |
| --- | --- |
| `server` | Socket accept loop (threads) → `Incoming` messages on a channel. |
| `daemon` | The `Daemon` global (config, capture context, last region, status watchers), request routing, config hot-reload. Every `Daemon::update` tells status watchers of whatever it changed. |
| `screenshot` | Freeze → select or resolve target → render → deliver. |
| `deliver` | After-capture actions for screenshots and recordings: encode, save, copy, preview or edit. |
| `editor` | Opens `screenie-editor` for a capture or `screenie edit FILE`, and acts on its output (copy, save over the capture, Save As, Done → preview card). |
| `clipboard` | Data-control clipboard offers (PNG + file URIs). |
| `recording` | Record flow (live selector → countdown → `screenie-record` → deliver), stop/pause/cancel, and the on-screen controls (border, countdown, pill). |
| `preview` | The floating preview card stack (screenshots and recordings), with an Annotate button on screenshots. |
