# Annotation editor

Status: **shipped** (first version).

A window for marking up a capture, then copying or saving it. It's opened from the
preview card's pencil button, with `screenie shot --edit` (or `after_capture.edit`), or
with `screenie edit FILE.png`.

It's minimal on purpose. The references are Screendrop's editor (an object canvas with
quiet selection chrome), Flameshot (one key per tool, nothing to set up before
drawing) and CleanShot X.

## Layout

```
 [↶ ↷]      [▸ ↗ ─ □ ○ ✎ ▬ T ① ▦ ◐ | ⌗]      [⧉ ⤓ ✓ Done]
                 ┌─────────────────────────┐
                 │        the capture      │
                 └─────────────────────────┘
            [● ● ● ● ● ● ● ●  |  · • • ● ●  |  ▢]
```

- The **tool bar** is at the top centre, with undo/redo on the left and the actions
  (Copy, Save, Done) on the right.
- The **style bar** is at the bottom. It shows only what applies to the selected shape
  or the current tool: palette, five sizes, fill (rectangles, ellipses, and the label
  background for text), and Pixelate/Blur for redactions. In crop mode it's replaced by
  the crop size, Reset, Cancel and Crop.
- The capture is shown at most at its on-screen size (logical 1:1), so it stays sharp,
  and is scaled down to fit otherwise. Zoom and pan are later work.

## Tools

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
| N | Numbered step | Click to place; numbers follow document order and renumber on delete |
| B | Pixelate / blur | Press B again to switch mode |
| S | Spotlight | Dims everything outside its rectangles |
| C | Crop | Non-destructive; Enter applies, Esc cancels |

Other keys:

- Shift snaps lines to 15° and keeps boxes square.
- `1`–`5` pick a size, `[` and `]` step through them.
- Arrow keys nudge the selection (Shift ×10). Delete removes it, and Ctrl+D duplicates it.
- Ctrl+Z / Ctrl+Shift+Z undo and redo.
- Ctrl+C copies, Ctrl+S saves and Ctrl+Shift+S is Save As. Enter is Done: save, copy
  and close.
- Esc backs out one level at a time: stop typing, then deselect, then close. Closing
  with unsaved changes asks first, and so does the window's close button.

Drawing tools stay active after a shape, and the new shape is selected so it can be
adjusted straight away. Pressing on the selected shape moves it, and pressing anywhere
else draws a new one. A click without a drag draws nothing.

## Sizes and scale

Shapes are stored in image pixels. Widths are in logical pixels, multiplied by the
capture's output scale, so a size-4 arrow looks the same on a 2× capture as on a 1×
one. One size control drives everything:

| | from width `w` (logical px) |
| --- | --- |
| Text | `8 + 3.5w` |
| Step diameter | `16 + 4w` |
| Highlighter | `8 + 3w` |
| Pixel block | `4 + 2w` |
| Blur radius | `3 + 1.5w` |

`screenie edit FILE` doesn't know the capture's scale, so it assumes the focused
output's scale.

## Rendering

Strokes, text and steps get a soft shadow (black at 35%, 2px blur, 1px down) so they
read on busy backgrounds. Draw order is redactions, then spotlights (one dimmed layer
with a hole per spotlight), then everything else in document order. Text is Inter
Bold, with system fonts as fallback for emoji and CJK.

## Saving

- **Copy** copies the rendered PNG.
- **Save** overwrites the capture's file, or picks a new screenshot name if the
  capture was never saved.
- **Save As** uses the portal file chooser.
- **Done** saves, copies, closes, and shows a preview card for the result.

The last colour and size are remembered for the next editor while the daemon runs.

## Architecture

- `screenie-annotate` has no UI. It holds the `Document` (base image, shapes, crop,
  and undo history as whole-state snapshots, capped at 200), hit testing, handles, and
  the **tiny-skia** renderer. The editor and export share that renderer, so the file
  matches the canvas exactly. Text uses cosmic-text.
- `screenie-editor` holds the `Session`, which is the interaction model: tools,
  gestures, text editing, crop and the Esc cascade. It's plain Rust with unit tests.
  The GPUI view around it translates events into image coordinates.
- The canvas redraws cheaply. Settled shapes are rendered once into a full-size
  composite. The shape being drawn, dragged or typed into is re-rendered alone into a
  small tile over it (all spotlights together, when one of them moves).

## Later

- Zoom and pan (Ctrl+scroll, fit/100%).
- Multi-select and marquee selection.
- Curved arrows, rotation, arrowhead styles.
- Keeping annotations editable after saving (a sidecar file).
- A colour picker beyond the palette.
- Smart redaction via OCR.
- Backgrounds and padding for sharing.
