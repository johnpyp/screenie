# screenie

The `screenie` binary: a thin clap CLI that sends a request to the daemon (starting it
if needed) and turns the response into stdout output and an exit code. `screenie daemon`
runs the daemon in the foreground, and `screenie` alone prints help.

Exit codes are 0 for done, 1 for cancelled and 2 for errors. Commands that produce a
file print its path on stdout (`screenie record` prints the path it's recording to).

## Screenshots

| Command | What happens |
| --- | --- |
| `screenie shot` / `screenie shot pick` | Freeze all outputs, show the selector in area mode. |
| `screenie shot window` | Focused window, no UI (needs compositor IPC). |
| `screenie shot window -i` | Selector in window mode (click a window). |
| `screenie shot screen` | Focused output, no UI. `--output-name DP-1` picks one. |
| `screenie shot screen -i` | Selector in screen mode (click a screen). |
| `screenie shot all` | Every output stitched into one image at the highest scale. |
| `screenie shot last` | Same region as the previous capture, even across daemon restarts. |
| `screenie shot --region "X,Y WxH"` | That logical region, no UI. `WxH+X+Y` also parses. |

`--delay N` waits first, `--cursor` includes the pointer. `--copy/--no-copy`,
`--save/--no-save`, `--no-preview` and `--edit` override `screenshot.after_capture`.
`-o PATH` implies saving: a file (`.png` added if it has no extension, an existing file
replaced) or a directory (one that exists, or a path ending in `/`), which gets a
configured name. `--stdout` writes the PNG bytes to stdout.

`-i` (`--interactive`) only applies to `window` and `screen`, and can't be combined with
`--region` or `--output-name`.

`shot --edit` waits for the editor: it prints the saved file, if any, and exits 1 if the
editor closed without Done and nothing was copied or saved.

Pressing a shortcut again while its selector is up does what you'd expect: the same one
closes it, another selection mode switches it, and a capture without a selector
(`shot window`, `shot screen`, `shot all`, `shot last`, `--region`) is taken from the frozen desktop
the selector shows.

## Recording

| Command | What happens |
| --- | --- |
| `screenie record` | Live selector with audio toggles, then **Record**. Stops the running recording instead, if there is one. |
| `screenie record window` / `screen` / `last` | The focused window, the focused output, or the previous region. |
| `screenie record window -i` / `screen -i` | Live selector in window or screen mode. |
| `screenie record --region "X,Y WxH"` | That region, no selector. |
| `screenie stop` | Stop and save. Cancels a countdown. |
| `screenie pause` | Pause or resume. |
| `screenie cancel` | Stop and delete. |

`--audio` / `--mic` turn on system audio and the microphone, `-o FILE` picks the path,
and `--no-toggle` fails instead of stopping a running recording. `stop`, `pause` and
`cancel` exit 0 when they did what they say (`stop` during the countdown exits 1, as
nothing was recorded), and 2 with nothing recording. They never start the daemon.

## Queries

`screenie query status|last` never starts the daemon: with none running the answer is
`idle`. Text output is tab-separated with a fixed number of fields; `--json` has
everything, `--format waybar` suits a waybar custom module, and `--watch` prints a line
per change until its reader goes away, through daemon restarts and upgrades. A state
this binary doesn't know (from a newer daemon) prints as `unknown`.

Logging goes to stderr and is filtered by `SCREENIE_LOG` (e.g. `SCREENIE_LOG=debug`).
`build.rs` stamps the binary with its git commit, shown by `--version` and
`query status --json`; the daemon compares builds to upgrade itself (see `screenie-ipc`).
