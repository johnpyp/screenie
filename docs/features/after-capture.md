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
| `edit` | false | Open the editor straight away. Nothing is copied or saved until it's done; then `copy` and `save` apply, and no card is shown. The command waits for the edit, prints the saved file (or writes the PNG with `--stdout`), and exits 1 if the edit was abandoned. If an overlay editor is already open, the capture goes to a card instead (as if `edit` were off, with `preview` on). |

Recordings are always saved, and there's no video editor, so `recording.after_capture`
has only `copy` (the file, default false) and `preview` (default true).

The CLI overrides them per call with `--copy/--no-copy`, `--save/--no-save`,
`--no-preview`, `--edit` and `-o PATH` (which implies saving). `-o` takes a file (`.png`
is added if it has no extension; an existing file is replaced) or a directory (one that
exists, or a path ending in `/`), which gets a file named as below. `--stdout` writes
the PNG bytes to stdout for scripts.

File names come from `screenshot.filename`, a strftime template (default
`Screenshot_%Y-%m-%d_%H-%M-%S_{app}`), filled in with the moment the screen was frozen,
however much later the capture is saved (from its card, or the editor). `{app}` (the
window's app, e.g. `firefox`, or `Nautilus` for `org.gnome.Nautilus`) and `{title}` fill
in when a window was captured, and disappear along with one adjacent separator
otherwise. Recordings work the same way. Collisions get `-2`, `-3`… suffixes; a name is
claimed the moment it's chosen, so captures in the same second (a script capturing two
screens at once) never share one. Writes are atomic, and a failed one leaves nothing
behind. A capture whose file can't be written is still copied and previewed, so it
isn't lost; the CLI reports the error.

PNGs are written with fast compression: a 4K screen encodes in a few tens of
milliseconds instead of half a second, so the card and the clipboard aren't kept
waiting, for files somewhat larger than the smallest possible.

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
timer, and leaving gives it a short fresh lease. Up to five cards stack, fewer if they
wouldn't fit on the screen: a card past that pushes out the oldest one that isn't under
the pointer or still being saved. Each output keeps its own stack, so a capture on
another screen (or after `preview.position` changed) leaves the cards already up where
they are. If the compositor closes a stack's surface (its output unplugged or turned
off), the next card opens a new one. With copy and save
off, an unsaved capture exists only in its card, so it's gone when the card goes.
That's intended: turning on copy or save is how you keep captures.

Clicking a card opens the capture in its default app (from a temporary file if it
wasn't saved). Hovering shows only what's left to do:

- **Copy** (Ctrl+C) and **Save** (Ctrl+S) icon buttons, until the capture is copied or
  saved. From then on a small **✓ Copied**, **✓ Saved** or **✓ Copied & Saved** pill
  sits in the bottom-right corner, hovered or not. "Copied" lasts until screenie copies
  something else. Copies made by other apps go unnoticed.
- **Show in folder**, once there's a file.
- **Pencil** (E): annotate in the editor (screenshots).
- **×** (Esc): dismiss.
- **Trash** (Delete): delete the file and dismiss. Only shown when there is a file.
- Along the top, between × and the pencil, the pixel size and file size.

The hover layout is in rows (corner buttons and caption, the main buttons, then Delete),
so nothing overlaps, whatever the card's shape.

The cards never take the keyboard from the app you're in, except while the pointer is
on one: then those keys reach the card (Esc dismisses it rather than, say, unpausing the
game beneath, whose pointer lock would also trap the pointer on the card). Moving off
gives the keyboard back once no key is held, so nothing pressed on the card is released
into the app. Cards that appear under a resting pointer aren't entered until it moves,
so typing elsewhere carries on.

Cards are never in a capture: they're hidden for the moment a screenshot is taken, and
while their screen is recorded (see [recording](recording.md)).

All cards share one transparent layer surface over the output's free area. It has no
fixed size, so the compositor fits it between bars. Only the cards take input; the rest
is click-through. The recording pill works the same way (see `screenie_ui_kit::Hover`).

Planned: a **Pin** button, and **drag the card into another app**, which GPUI can't do
yet (see ISSUES).
