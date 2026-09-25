# screenie-app

The daemon. `screenie_app::run()` binds the socket, starts GPUI with
`QuitMode::Explicit`, and serves requests until `quit`.

| Module | Role |
| --- | --- |
| `server` | Socket accept loop (threads) → `Incoming` messages on a channel. |
| `daemon` | The `Daemon` global (config, capture context, last region, status watchers), request routing, config hot-reload. |
| `screenshot` | Freeze → select or resolve target → render → deliver. |
| `deliver` | After-capture actions for screenshots and recordings: encode, save, copy, preview. |
| `clipboard` | Data-control clipboard offers (PNG + file URIs). |
| `recording` | Record flow (live selector → countdown → `screenie-record` → deliver), stop/pause/cancel, and the on-screen controls (border, countdown, pill). |
| `preview` | The floating preview card stack (screenshots and recordings). |
