# After capture: clipboard, saving, preview card

Status: **done**, except drag-out and editor/pin buttons (waiting on those features).
Recordings get the same card; see [recording](recording.md).

## Actions

`screenshot.after_capture` in the config sets the defaults:

| Key | Default | Meaning |
| --- | --- | --- |
| `copy` | true | Put the image on the clipboard. |
| `save` | true | Write a PNG into `screenshot.directory` (default `~/Pictures/Screenshots`). |
| `preview` | true | Show the floating preview card. |
| `edit` | false | Open the editor straight away. |

The CLI overrides them per call with `--copy/--no-copy`, `--save/--no-save`,
`--no-preview`, `--edit` and `-o PATH` (which implies saving). `--stdout` writes the
PNG bytes to stdout for scripts.

File names come from `screenshot.filename`, a strftime template (default
`Screenshot_%Y-%m-%d_%H-%M-%S`). Collisions get `-2`, `-3`… suffixes. Writes are
atomic.

## Clipboard

The daemon owns the clipboard, because Wayland clipboards die with their owner. It
offers:

- `image/png`: the pixels.
- `text/uri-list` and `x-special/gnome-copied-files`: the saved file, when there is
  one, so pasting into a file manager or chat app attaches the file.

No `text/plain` is offered, so pasting into a text field doesn't insert a file URI. This
needs `ext-data-control-v1` or `wlr-data-control-unstable-v1`. GNOME has neither, which
is tracked in [ISSUES](../ISSUES.md).

## Preview card

A small card slides in at the bottom-right of the output where the capture happened.
It stays for `preview.timeout` seconds (0 = until dismissed). Hovering pauses the
timer, and leaving gives it a short fresh lease. Up to five cards stack.

Clicking a card opens the file in its default app. On hover:

- **Copy**: copy again and dismiss.
- **Save / Show**: save an unsaved capture, or reveal the saved file in the file manager.
- **×**: dismiss.
- **Trash**: delete the saved file and dismiss.
- A caption shows the pixel size and file size.

All cards share one layer surface along the right edge. Its input region is limited to
the cards, so the empty part of the column is click-through.

Planned: an **Edit** button (the editor), a **Pin** button, and **drag the card into
another app**, which GPUI can't do yet (see ISSUES).
