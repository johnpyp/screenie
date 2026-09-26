# Annotation editor

Status: **shipped** (first version).

Marks up a capture, then copies or saves it. It opens from the preview card's pencil
button, with `screenie shot --edit` (or `after_capture.edit`), or with
`screenie edit FILE.png`.

It's minimal on purpose. The references are Screendrop's editor (an object canvas with
quiet selection chrome), Flameshot (one key per tool, nothing to set up before
drawing) and CleanShot X.

## Overlay and window

`editor.mode` picks how it appears:

- **`overlay`** (default), like Flameshot. A layer-shell surface covers the capture's
  screen and dims it. The capture sits exactly where it was taken, so annotating feels
  like a continuation of selecting (`--edit`). Cropping keeps it in place too.
  Otherwise it's centred at its on-screen size: when picked up later from a preview
  card, for `screenie edit FILE`, or if it isn't wholly on one screen. A centred
  capture over 95% of the screen's width or height is shrunk to within 80% of both, so
  it can't be mistaken for the screen itself. Centred captures also get a stronger dim
  behind them (70%), a deeper shadow, and a dark outer and light inner hairline. That
  way the edge shows on light and dark captures alike, even over a screen that looks
  just like them.
  The overlay takes the keyboard until you're done. On compositors without layer-shell
  (GNOME) it's a fullscreen window.
- **`window`**: a regular, resizable window (app id `dev.johnpyp.Screenie`).

In overlay mode there is **one editor at a time**. While it's open, captures still
work, and one that would open the editor (`shot --edit`, or `after_capture.edit`) goes
to a preview card instead, preview on or not, to be picked up once this edit is done.
Cards hide their pencil meanwhile. `screenie edit FILE` is refused, and the open
editor says so.

**Save As** would open the portal file chooser underneath the overlay. So the overlay
steps aside while the chooser is up, then comes back exactly as it was.

## Layout

```
                 ┌─────────────────────────┐
                 │        the capture      │
                 └─────────────────────────┘
      [↶ ↷] [▸ ↗ ─ □ ○ ✎ ▬ T ① ▦ ◐ | ⌗] [⧉ ⤓ ✓ Done]
            [● ● ● ● ● ● ● ●  |  − • +  |  ▢]
```

- **As an overlay**, the bars hang off the capture, centred on it. They go below it if
  there's room, otherwise above, otherwise just inside its bottom edge (a full-screen
  capture), and are kept on screen. The main bar sits nearest the capture.
- **In a window**, the tool bar runs along the top, with undo/redo on the left and the
  actions on the right. The style bar is at the bottom.
- The **style bar** shows only what applies to the selected shape or the current tool.
  That means the palette, the size stepper, fill (rectangles, ellipses, and the label
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
| C | Crop | Non-destructive. Enter applies it, and so does another tool or handing the image out (copy, save, Done). Esc or Cancel drops it |

Other keys:

- Shift snaps lines to 15° and keeps boxes square.
- **Ctrl+scroll** changes the size, like the wheel over the size stepper. With a shape
  selected, or while drawing one, that shape gets thicker or thinner in place: its
  points stay put. A run of scrolling is one undo step. `[` and `]` step too, and `1`–`9`, `0` pick the ten
  sizes directly.
- Arrow keys nudge the selection (Shift ×10). Delete removes it, and Ctrl+D duplicates it.
- Ctrl+Z / Ctrl+Shift+Z undo and redo.
- Ctrl+C copies (while typing too), Ctrl+S saves and Ctrl+Shift+S is Save As. Enter is
  Done (below).
- Esc backs out one level at a time: stop typing, then deselect, then close. Copying
  or saving (including Save As) also commits typing and deselects, so after Ctrl+C one
  Esc closes. Closing with unsaved changes asks first, and so does a window's close
  button.
- Holding a key down repeats typing, nudging, Delete and Backspace, `[` `]`, and undo
  and redo. Everything else acts once per press, so a held Esc can't go on from
  deselecting to closing, or open and dismiss the close prompt over and over.

Drawing tools stay active after a shape, and the new shape is selected so it can be
adjusted straight away. Pressing on the selected shape moves it, and pressing anywhere
else draws a new one. A click without a drag draws nothing.

## Sizes and scale

Shapes are stored in image pixels. Widths are in logical pixels, multiplied by the
capture's output scale, so a size-4 arrow looks the same on a 2× capture as on a 1×
one. There are ten sizes: 1, 2, 4, 6, 8, 12, 16, 20, 26 and 32. One size control
drives everything:

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
- **Save As** uses the portal file chooser. An overlay steps aside for it. It starts
  in the capture's folder, or for a capture never saved, in the screenshot folder
  with a new screenshot's name. It writes PNG: a name without an extension gets
  `.png`, and another image format's (`.jpg`) is replaced. It needs a portal with a
  FileChooser (xdg-desktop-portal-gtk, -kde or -gnome; -wlr and -hyprland have none),
  and says so when there isn't one.
- The editor says "Copied" or "Saved" once that has actually happened. If it fails
  (no clipboard protocol on GNOME, a full disk), it shows why, and the annotations
  don't count as kept, so closing still asks first.
- **Done** applies the after-capture settings and closes. It saves only if `save` is
  on or the capture already has a file (opened from one, given `-o`, or saved in the
  editor), and copies only if `copy` is on. Anything already copied or saved exactly as
  it is now isn't done again. Done's tooltip says what it will do: "Copy and close",
  "Save, copy and close", or just "Close". Editing ends there: no preview card follows.
- `editor.exit_on_copy` / `editor.exit_on_save` close the editor once a copy or save
  has gone through (unless more was drawn meanwhile).
- Closing asks "Keep your annotations?" only if the current annotations were neither
  copied nor saved. Its main button does what Done would (Copy, Save, or Save & copy).
  When Done would only close (the defaults), Done asks too, and the prompt offers Copy
  and Save instead. `editor.confirm_discard: false` turns the prompt off everywhere
  (Esc, Done, a window's close button): unkept annotations are then discarded.
- `screenie shot --edit` finishes when the editor closes, like any capture. It prints
  the saved file, if the edit was saved (`--stdout` writes the edited PNG instead:
  Done then always hands it over). It exits 1 if the editor was closed without Done
  and nothing was copied or saved. Other captures don't wait for it.
- The capture shows up in `screenie query last` once: when saved, or at Done. A later
  save records the file again, but Done doesn't repeat a capture already recorded.

The last colour, size and fill are remembered for the next editor, across restarts, in
`$XDG_STATE_HOME/screenie/state.yaml` (see `screenie-state`). Until then the editor
starts from `editor.default_color` and `editor.stroke_width`.

## Architecture

- `screenie-annotate` has no UI. It holds the `Document` (base image, shapes, crop,
  and undo history as whole-state snapshots, capped at 200), hit testing, handles, and
  the **tiny-skia** renderer. The editor and export share that renderer, so the file
  matches the canvas (up to tiny-skia antialiasing a path a hair differently where a
  tile cuts it off). Text uses cosmic-text.
- `screenie-editor` holds the `Session`, which is the interaction model: tools,
  gestures, text editing, crop and the Esc cascade. It's plain Rust with unit tests.
  The GPUI view around it translates events into image coordinates.
- The canvas redraws cheaply. It's a grid of 256px tiles, each its own GPU image, and a
  change re-renders only the tiles the shape crosses (`Shape::touches`), in parallel:
  dragging a corner-to-corner arrow on a 4K capture takes about a millisecond per
  move, not its whole bounding box. Settled shapes are rendered into the composite
  as they change. The shape being drawn, dragged or typed into is drawn over the
  composite's tiles it crosses; a stroke being drawn redraws only the tiles around its
  end. Shadows are blurred only near what casts them. `cargo run --release -p
  screenie-annotate --example bench` times it.

## Later

- Zoom and pan (fit/100%, and a zoom gesture that isn't Ctrl+scroll, which sizes).
- Multi-select and marquee selection.
- Curved arrows, rotation, arrowhead styles.
- Keeping annotations editable after saving (a sidecar file).
- A colour picker beyond the palette.
- Smart redaction via OCR.
- Backgrounds and padding for sharing.
