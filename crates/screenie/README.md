# screenie

The `screenie` binary: a thin clap CLI that sends a request to the daemon (starting it
if needed) and turns the response into stdout output and an exit code. `screenie daemon`
runs the daemon in the foreground, and `screenie` alone prints help.

Exit codes are 0 for done, 1 for cancelled and 2 for errors. Commands that produce a
file print its path on stdout (`screenie record` prints the path it's recording to).

## Screenshots

`screenie shot [TARGET] [OPTIONS]`. Each target takes only the arguments that fit it,
and the options can go before or after it.

| Target | What happens |
| --- | --- |
| (none) | Same as `pick`. |
| `pick` | Freeze all outputs, show the selector in area mode. |
| `window` | The focused window, no UI. Needs compositor IPC. |
| `window -i` | The selector in window mode. |
| `screen` | The focused output, no UI. |
| `screen DP-1` | That output. |
| `screen -i` | The selector in screen mode. |
| `all` | Every output stitched into one image, at the highest scale. |
| `last` | The previous capture's region, even across daemon restarts. |
| `region "X,Y WxH"` | That logical region, no UI. `WxH+X+Y` also parses. |

| Option | |
| --- | --- |
| `-d N`, `--delay N` | Wait N seconds first. |
| `--cursor` | Include the pointer. |
| `--copy`, `--no-copy` | Override `screenshot.after_capture.copy`. |
| `--save`, `--no-save` | Override `screenshot.after_capture.save`. |
| `--no-preview` | Override `screenshot.after_capture.preview`. |
| `--edit` | Override `screenshot.after_capture.edit`. |
| `-o FILE` | Save to FILE. `.png` is added if it has no extension, and an existing file is replaced. |
| `-o DIR/` | Save into DIR, with the configured name. Also used when the path is an existing directory. |
| `--stdout` | Write the PNG to stdout. |

`shot --edit` waits for the editor. It prints the saved file, if any, and exits 1 if the
editor closed without Done and nothing was copied or saved.

Pressing a shortcut again while its selector is up:

- The same shortcut closes it.
- Another selection mode switches it.
- A capture without a selector (`window`, `screen`, `all`, `last`, `region`) is taken
  from the frozen desktop the selector shows.

## Recording

`screenie record [TARGET] [OPTIONS]`, with the same targets as `shot` except `all`. A
recording covers one output. While one is running, `screenie record` stops it.

| Target | What happens |
| --- | --- |
| (none) | Same as `pick`. |
| `pick` | The live selector with audio toggles, then **Record**. |
| `window` | The focused window. |
| `window -i` | The live selector in window mode. |
| `screen` | The focused output. |
| `screen DP-1` | That output. |
| `screen -i` | The live selector in screen mode. |
| `last` | The previous region. |
| `region "X,Y WxH"` | That region, no selector. |

| Option | |
| --- | --- |
| `--audio` | Record system audio. |
| `--mic` | Record the microphone. |
| `-o FILE` | Save to FILE. |
| `--no-toggle` | Fail instead of stopping a running recording. |

| Command | What happens |
| --- | --- |
| `screenie stop` | Stop and save. Cancels a countdown. |
| `screenie pause` | Pause or resume. |
| `screenie cancel` | Stop and delete. |

`stop`, `pause` and `cancel` never start the daemon. They exit 0 when they did what they
say, and 2 with nothing recording. `stop` during the countdown exits 1, as nothing was
recorded.

## Queries

`screenie query status|last` never starts the daemon: with none running the answer is
`idle`. Text output is tab-separated with a fixed number of fields; `--json` has
everything, `--format waybar` suits a waybar custom module, and `--watch` prints a line
per change until its reader goes away, through daemon restarts and upgrades. A state
this binary doesn't know (from a newer daemon) prints as `unknown`.

Logging goes to stderr and is filtered by `SCREENIE_LOG` (e.g. `SCREENIE_LOG=debug`).
`build.rs` stamps the binary with its git commit, shown by `--version` and
`query status --json`; the daemon compares builds to upgrade itself (see `screenie-ipc`).
