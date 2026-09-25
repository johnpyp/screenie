# screenie

The `screenie` binary: a thin clap CLI that sends a request to the daemon (starting it
if needed) and turns the response into stdout output and an exit code. Exit codes are 0
for done, 1 for cancelled and 2 for errors. `screenie daemon` runs the daemon in the
foreground.

Logging goes to stderr and is filtered by `SCREENIE_LOG` (e.g. `SCREENIE_LOG=debug`).
