# Capture & selector

Status: **done** for screenshots on wlroots-family compositors. Portal backend: planned.

## Commands

| Command | What happens |
| --- | --- |
| `screenie` / `screenie shot` | Freeze all outputs, show the selector in area mode. |
| `screenie shot window` | Selector in window mode (click a window). |
| `screenie shot pick-screen` | Selector in screen mode (click a screen). |
| `screenie shot screen` | Focused output, no UI. `--output-name DP-1` picks one. |
| `screenie shot all` | Every output stitched into one image at the highest scale. |
| `screenie shot active` | Focused window, no UI (needs compositor IPC). |
| `screenie shot last` | Same region as the previous capture, even across daemon restarts (clipped to the current screens). |
| `screenie shot --region "X,Y WxH"` | That logical region, no UI. `WxH+X+Y` also parses. |

`--delay N` waits first. `--cursor` includes the pointer. Exit codes are 0 for
captured, 1 for cancelled, and 2 for errors.

Pressing a shortcut again while its selector is up does what you'd expect rather than
failing:

- The same one (`shot` again) closes the selector, like `record` stops a recording.
- Another selection mode (`shot window` over an area selector) switches it to that
  mode.
- A capture without a selector (`shot screen`, `shot all`, `shot last`, `--region`) is
  taken from the frozen desktop the selector shows, and the selector closes.

A screenshot while a recording's selector is up is taken of the live screen, with the
selector hidden for it (see below).

## Freezing

`screenie-capture` takes a `Snapshot`: one image per output, captured concurrently, plus
the window list from compositor IPC. It is taken **before** any UI appears. The overlay
then paints the frozen pixels, so the result is exactly what was on screen when the key
was pressed. Menus, tooltips and fullscreen apps are left undisturbed, because the
overlay is a layer surface and not a window.

**Screenie is never in its own screenshots.** Capture copies every surface on an output,
screenie's too: the card from the last shot, the recording ring and pill, an overlay
editor, a recording's selector. Just before freezing, those that are on screen are made
fully transparent (and take no input), and the capture waits until the compositor has
shown the frame without them: one or two frames, and nothing at all when none is up.
They stay mapped, so nothing animates away and back (`screenie_ui_kit::conceal`).

Backends (`advanced.capture_backend`):

- `ext`: `ext-image-copy-capture-v1` (newer wlroots, niri, COSMIC, KDE 6.3+ partial).
- `wlr`: `wlr-screencopy-unstable-v1` (Sway, Hyprland, river, Wayfire…).
- `portal`: xdg-desktop-portal Screenshot. For GNOME and KDE. **Not implemented yet.**
- `auto`: `ext`, then `wlr`, then `portal`.

In `auto` mode, if one protocol fails at capture time, the other one is tried, and
whichever works is remembered. A protocol can be advertised yet unusable, e.g. when it
only offers a pixel format we can't read. Recording streams are checked the same way:
the first frame is pulled before recording starts.

Both native paths handle 32-bit, packed 24-bit (`RGB888`/`BGR888`, which Hyprland's
screencopy offers on some setups) and 10-bit shm formats, y-invert, and all eight
output transforms. They are verified pixel-identical to `grim` (see `tools/imgdiff.py`).

## Coordinates

Everything the user sees is in **logical** coordinates (the compositor layout). Pixels
are **physical**. Each output's scale is *measured* (buffer width / logical width) rather
than trusted, which keeps fractional scales exact. Selection edges snap to the physical
pixel grid of the output under them, so a 1.5× output never produces half-pixel
blurring. A region spanning mixed-DPI outputs is rendered at the highest scale.

## Selector interactions

| Input | Effect |
| --- | --- |
| Drag | Region. Shift: square. Alt: from center. Space (held): move while drawing. |
| Click | The window under the pointer, or the screen if there is none. |
| Enter | Confirm: the selection, else what's highlighted, else the screen under the pointer. The toolbar's Capture / Record button does the same. |
| Esc | Cancel (or leave adjust mode). |
| 1/2/3, a/w/s, Tab | Area / window / screen mode. |
| Space (idle) | Toggle area and window mode. |
| Arrows | Nudge an editable selection by one physical pixel (Shift: 10; Ctrl: resize). |
| M | Toggle the magnifier. |

With `selector.capture_on_release = false`, a drawn region stays editable. It has
handles, can be dragged, and Enter or the toolbar button confirms. Dragging elsewhere
draws a new region, but a click that misses the handles keeps the selection: it never
picks the window underneath. The loupe shows a
15×15 pixel neighbourhood, the hex colour and coordinates, or the size while drawing.

The interaction logic is a pure state machine (`screenie-selector/src/model.rs`) with
unit tests. The view only paints it and forwards events.

## Recording

Recording uses the same selector over a **live** backdrop, since there's nothing to
freeze. Regions are clamped to one output. See [recording](recording.md).
