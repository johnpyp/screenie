# screenie-ipc

How the CLI talks to the daemon.

- **Protocol** (`protocol.rs`): `Request` / `Response` as serde enums, sent as
  newline-delimited JSON, one request per connection. `Watch` keeps the connection open
  and streams `Status` updates.
- **Transport**: a Unix socket at `$XDG_RUNTIME_DIR/screenie/<WAYLAND_DISPLAY>.sock`, so
  each Wayland session gets its own daemon.
- `Client::connect_or_spawn()` starts `screenie daemon` (detached with setsid, in `/`,
  with stdio on `/dev/null`) when none is running, then waits for it.
- **Upgrades**: `connect_or_spawn()` compares `exe_stamp()` (the identity of the
  running executable) with the daemon's. If they differ and the daemon is idle, it asks
  the daemon to quit and starts a fresh one, so replacing the binary is all an upgrade
  takes. Daemons from before stamps report none, so they count as outdated.
- **One daemon per session**, however many commands start at once. The daemon holds
  `<display>.lock` (a `flock`, so a crash releases it) for as long as it runs:
  `bind_listener()` takes it, replaces a stale socket file and returns a `SocketClaim`
  that removes the socket when dropped. A client holds `<display>.spawn.lock` while it
  decides whether to start a daemon, so concurrent commands start one between them.
