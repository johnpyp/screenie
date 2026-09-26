# screenie-editor

The annotation editor: an overlay over the capture's screen with the capture in place
(`Mode::Overlay`), or a regular window (`Mode::Window`).

```rust
screenie_editor::open(&image, EditorOptions { mode, output, placement, scale, style, .. }, |out, cx| {
    match out {
        // A task that ends once it's done, with the file written if any: only then does
        // the editor say "Copied" / "Saved", and an error is shown instead.
        Output::Copy(image) | Output::Save(image) | Output::SaveAs(image, _) => cx.spawn(/* … */),
        Output::Done { image, copied, saved } => { /* skip what's already done */ }
        Output::Closed { style, finished } => { /* remember for next time */ }
    }
}, cx)?;

// Overlays are one at a time: refuse another while one is open (it shows the message).
if screenie_editor::overlay_busy("Finish this one first", cx) { /* … */ }
```

- **`Session`** is the interaction model, with no GPUI in it: the current tool and
  style, the selection, drag gestures (draw, move, handle), text editing with a caret,
  crop mode, and the Esc cascade. The view feeds it pointer events in image
  coordinates and `Key`s, and it drives a `screenie_annotate::Document`. It's covered
  by unit tests.
- **`Tool`** lists the tools with their names, keys, icons, and which style controls
  apply to each.
- **`raster`** keeps redraws cheap. The canvas is a grid of tiles, each a GPUI image,
  and a change re-renders (in parallel) only the tiles it reaches: settled shapes into
  the composite, and the live shape over the composite's tiles it crosses. Replaced
  tiles are evicted from GPUI's atlas.
- **`view::Editor`** is the GPUI view. It has the canvas: in place as an overlay,
  otherwise fitted and never beyond logical 1:1. It also has the selection and crop
  chrome, the bars (hanging off the capture in an overlay, along the edges in a
  window), Ctrl+scroll sizing, the toast, and the "save your changes?" prompt. For Save
  As, an overlay closes and reopens from a clone of the session so the portal dialog
  isn't hidden underneath it.

What happens to the result (clipboard, files) is the caller's job; see
`screenie-app`'s `editor` module.

## Keys

| Key | Tool | Notes |
| --- | --- | --- |
| V | Select | Click to select, drag to move, handles to resize |
| A | Arrow | Filled head that scales with width; shaft tapers toward the tail |
| L | Line | |
| R | Rectangle | F toggles filled |
| O | Ellipse | F toggles filled |
| P | Pen | Freehand, smoothed |
| H | Highlighter | Multiplied marker stroke; Shift draws it straight |
| T | Text | Click and type. Enter adds a line; Esc or a click away commits. F puts it on a coloured label |
| N | Numbered step | Numbers follow document order and renumber on delete |
| B | Pixelate or blur | Press B again to switch mode |
| S | Spotlight | Dims everything outside its rectangles |
| C | Crop | Non-destructive. Enter, another tool, or handing the image out applies it; Esc or Cancel drops it |

- Shift snaps lines to 15° and keeps boxes square.
- Ctrl+scroll, `[` `]`, or `1`–`9` `0` set the size, of the selected shape too (in
  place: its points stay put, and a run of scrolling is one undo step).
- Arrows nudge the selection (Shift ×10), Delete removes it, Ctrl+D duplicates it.
- Ctrl+Z / Ctrl+Shift+Z undo and redo. Ctrl+C copies, Ctrl+S saves, Ctrl+Shift+S is
  Save As, and Enter is Done.
- Esc backs out one level at a time: stop typing, deselect, close. Closing with
  annotations neither copied nor saved asks first (`editor.confirm_discard`).
- Held keys repeat only typing, nudging, Delete/Backspace, `[` `]` and undo/redo, so a
  held Esc can't run on from deselecting to closing.

Drawing tools stay armed after a shape, and the new shape is selected. Sizes are ten
steps (1 2 4 6 8 12 16 20 26 32 logical px) from which every tool derives its own
(text `8 + 3.5w`, step diameter `16 + 4w`, highlighter `8 + 3w`, pixel block `4 + 2w`,
blur radius `3 + 1.5w`), multiplied by the capture's scale so they look the same on any
display. `screenie edit FILE` assumes the focused output's scale.

**Done** saves only if `save` is on or the capture already has a file, copies only if
`copy` is on (or the card's unedited original is still on the clipboard), skips
whatever was already done to the image as it is now, and closes. No preview card
follows. Its tooltip says what it will do.
