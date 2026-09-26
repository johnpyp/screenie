# After capture: clipboard, saving, preview card

Status: **done**, except drag-out and the pin button (waiting on those features).
Recordings get the same card; see [recording](recording.md).

## Actions

`screenshot.after_capture` in the config sets the defaults:

| Key | Default | Meaning |
| --- | --- | --- |
| `copy` | false | Put the image on the clipboard. The card's Copy button does it on demand. |
| `save` | false | Write a PNG into `screenshot.directory` (default `~/Pictures/Screenshots`). The card's Save button does it on demand. |
| `preview` | true | Show the floating preview card. |
| `edit` | false | Open the editor straight away. Nothing is copied or saved until it's done; then `copy` and `save` apply, and no card is shown. |

Recordings are always saved (`recording.after_capture` defaults to save + preview).

The CLI overrides them per call with `--copy/--no-copy`, `--save/--no-save`,
`--no-preview`, `--edit` and `-o PATH` (which implies saving). `--stdout` writes the
PNG bytes to stdout for scripts.

File names come from `screenshot.filename`, a strftime template (default
`Screenshot_%Y-%m-%d_%H-%M-%S_{app}`). `{app}` (the window's app, e.g. `firefox`, or
`Nautilus` for `org.gnome.Nautilus`) and `{title}` fill in when a window was captured,
and disappear along with one adjacent separator otherwise. Recordings work the same way.
Collisions get `-2`, `-3`… suffixes. Writes are atomic.

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

A small card slides in on the output where the capture happened, at
`preview.position`: a corner (`top-left`, `top-right`, `bottom-left`, `bottom-right`,
the default) or the middle of an edge (`top-middle`, `bottom-middle`, `left-middle`,
`right-middle`). It sits 18px from the edges it touches, clear of bars, and slides in
from the nearest edge. The newest card is closest to that edge.
It stays for `preview.timeout` seconds (default 10; 0 = until dismissed). Hovering pauses the
timer, and leaving gives it a short fresh lease. Up to five cards stack.

Clicking a card opens the capture in its default app (from a temporary file if it
wasn't saved). Hovering shows only what's left to do:

- **Copy** and **Save** icon buttons, until the capture is copied or saved. From then
  on a small **✓ Copied**, **✓ Saved** or **✓ Copied & Saved** pill sits in the
  bottom-right corner, hovered or not. "Copied" lasts until screenie copies something
  else. Copies made by other apps go unnoticed.
- **Show in folder**, once there's a file.
- **Pencil**: annotate in the editor (screenshots).
- **×**: dismiss.
- **Trash**: delete the file and dismiss. Only shown when there is a file.
- A caption above that pill shows the pixel size and file size.

All cards share one transparent layer surface over the output's free area. It has no
fixed size, so the compositor fits it between bars. Its input region is limited to the
cards, so the rest is click-through.

Planned: a **Pin** button, and **drag the card into another app**, which GPUI can't do
yet (see ISSUES).
