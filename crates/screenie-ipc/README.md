# screenie-ipc

How the CLI talks to the daemon.

- **Protocol** (`protocol.rs`): `Request` / `Response` as serde enums, sent as
  newline-delimited JSON, one request per connection. `Watch` keeps the connection open
  and streams `Status` updates.
- **Transport**: a Unix socket at `$XDG_RUNTIME_DIR/screenie/<WAYLAND_DISPLAY>.sock`, so
  each Wayland session gets its own daemon.
- `Client::connect_or_spawn()` starts `screenie daemon` (detached with setsid, logging to
  `$XDG_STATE_HOME/screenie/daemon.log`) when none is running, then waits for it.
- `bind_listener()` refuses to take over a live socket and replaces a stale one.
