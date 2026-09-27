# screenie

The `screenie` binary: a thin clap CLI that sends a request to the daemon (starting it
if needed) and turns the response into stdout output and an exit code. `screenie daemon`
runs the daemon in the foreground, and `screenie` alone prints help.

The daemon always logs to `$XDG_STATE_HOME/screenie/daemon.log`, starting it afresh
once it holds the socket (the previous run's is kept as `daemon.log.1`). It also logs to
stderr when that leads somewhere, like a terminal or the journal. With stderr on
`/dev/null`, as when the CLI starts it, stdout and stderr go to the log, so panics and
what native libraries print land there too.

Exit codes are 0 for done, 1 for cancelled and 2 for errors. Commands that produce a
file print its path on stdout (`screenie record` prints the path it's recording to).

## Screenshots

`screenie shot [TARGET] [OPTIONS]`. Each target takes only the arguments that fit it,
and its options follow it. Without a target, they follow `shot`.

| Target | What happens |
| --- | --- |
| (none) | Same as `pick`. |
| `pick` | Freeze all outputs, show the selector in area mode. |
| `window` | The focused window, no UI. Needs compositor IPC, or GNOME. |
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
| `screenie record stop` | Stop and save. Cancels a countdown. |
| `screenie record pause` | Pause or resume. |
| `screenie record cancel` | Stop and delete. |

`stop`, `pause` and `cancel` take no options and never start the daemon. They exit 0 when they did what they
say, and 2 with nothing recording. `stop` during the countdown exits 1, as nothing was
recorded.

## Shortcuts

`screenie shortcuts [show|install|remove]` asks the daemon, which keeps the record. On
GNOME and KDE Plasma, `show` (the default) lists the desktop's screenshot keys, what each
runs and what holds it now; `install` takes them for screenie in the desktop's shortcut
settings and `remove` gives them back (see `screenie-desktop`). The keys run the
`screenie` found in `$PATH`, or the path it was run by. Elsewhere, `show` prints the
lines for the compositor's configuration, and `install` fails with them.

## Queries

`screenie query status|last` never starts the daemon: with none running the answer is
`idle`. Text output is tab-separated with a fixed number of fields; `--json` has
everything, `--format waybar` suits a waybar custom module, and `--watch` prints a line
per change until its reader goes away, through daemon restarts and upgrades. A state
this binary doesn't know (from a newer daemon) prints as `unknown`.

Logging goes to stderr and is filtered by `SCREENIE_LOG` (e.g. `SCREENIE_LOG=debug`).
`build.rs` stamps the binary with its git commit (or the one the environment names, for
Nix builds), shown by `--version` and `query status --json`; the daemon compares builds to upgrade itself (see `screenie-ipc`).

## Man pages

`screenie man DIR` (hidden, for packaging) writes them into `DIR/man1` and `DIR/man5`
(`man.rs`): `screenie(1)` and a page per command and target (`screenie-shot-region(1)`)
from the clap definitions, with exit status, environment and files added to the first,
and `screenie(5)`, the config file key by key, from `screenie_config::reference`. They're
dated by the commit. `tools/dist.py` puts them in the release archives.

`screenie share DIR` (hidden too) writes what a package whose binary stays put (Nix)
installs into `share`: the desktop entry, running this binary, and the icon
(`DIR/applications`, `DIR/icons`), which the daemon then needs none of its own of, and
the GNOME Shell extension (`DIR/gnome-shell/extensions`).
