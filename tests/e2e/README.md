# End-to-end tests

pytest driving the real `screenie` daemon in the headless sway session from
`tools/session.sh` (two outputs: `HEADLESS-1` 1920x1080 at 1x, `HEADLESS-2`
2560x1440 at 1.5x to its right). They cover what unit tests can't: keyboard focus
handed between overlays and apps, Wayland dialogs, config reloads, restarts.

```sh
mise run test:e2e                       # builds release binaries, starts the session if needed
mise run test:e2e -- -k keyboard -x     # anything after -- goes to pytest
SCREENIE_E2E_KEEP_SESSION=1 mise run test:e2e   # leave a session it started running
```

Needs sway, grim, wev, wl-clipboard, dbus-daemon and gsettings; uv installs the
Python side.

## Writing tests

- **`daemon`**: a running daemon with its own `HOME`, `XDG_CONFIG_HOME`,
  `XDG_STATE_HOME` and `TMPDIR` under the test's `tmp_path`, so nothing touches your
  files. Set config with `@pytest.mark.config("yaml")` (before start) or
  `daemon.config(...)` (live reload). `open_editor(...)` and `open_selector(...)`
  return once the overlay has the keyboard. `wait_log`/`mark` wait on its debug log.
- **`input`**: one virtual pointer and keyboard (`wlinput -`) for the whole test:
  `keys`, `chord`, `type`, `click`, `drag`. The daemon depends on it: a keyboard
  added while an overlay holds focus makes sway re-enter the window, which muddles
  focus tests.
- **`keylog`**: a `wev` window logging the key events it gets, for catching keys that
  leak past screenie's overlays.
- **`session`**: outputs, windows (`swaymsg`), and `screenshot()` at one pixel per
  logical pixel.

Wait on events (status, log lines, windows, key events), not sleeps; `wait_for`
polls every 20ms. The one sleep in the suite waits on GTK's asynchronous file-name
check and says so.

Settings that come from the desktop (GTK text scaling via the settings portal) are
changed on a private D-Bus (`private_bus` in `test_ui_scale.py`) with dconf writing
into the test's directory, never on your real session bus.

A failed test leaves `screen.png`, `daemon.log` and `wev.log` in
`.cache/e2e/<test id>/`.

Tests run one at a time: they share one compositor and one seat.
