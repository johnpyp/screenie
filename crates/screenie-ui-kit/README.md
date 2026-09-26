# screenie-ui-kit

Screenie's GPUI look and feel, shared by every window.

- **`assets`**: an `AssetSource` serving bundled Lucide icons (`Icon` enum) and Inter,
  layered over gpui-component's assets. `load_fonts` registers the fonts.
- **`hud`**: the translucent dark HUD language: `color` tokens, `panel()`, `pill()`,
  `keycap()`, `separator()`, `spinner()`, `inner_radius()` (concentric with a panel's corners), and
  `HudButton` (icon or label, tooltip, selected, accent/record styles).
- **`tip`**: `Tip`, the one tooltip look: a title, its shortcut as keycaps
  (`.key("Ctrl+Shift+Z")`, more than one are alternatives), related actions under it
  (`.also("Save as").key(...)`), and a short `.note(...)` only where the control doesn't
  explain itself. `HudButton::tooltip` takes one (or a plain title); other elements use
  `.tooltip(tip.builder())`.
- **`layer`**: `LayerSpec` describes a layer-shell surface (fullscreen overlay or floating
  panel) targeted to an output by connector name. `layer_options` turns it into GPUI
  window options, and `fallback_options` gives a plain window where layer-shell is
  missing. `wait_for_displays` covers GPUI's late output discovery.
- **`keys`** and **`hover`**: input for overlays; see below.
- **`scale`**: the interface scale (`ui_scale`). Interface sizes are written as `ui(n)`
  (rems: n pixels at scale 1), and screen geometry stays in `px`. `ui_px` gives the same
  length in pixels for layout math and painting. `set_ui_scale` applies a scale to
  every window; root views call `track_ui_scale` when created.
- **`image`**: `render_image` converts a `screenie_core::Image` into a GPUI `RenderImage`.

Call `screenie_ui_kit::init(cx)` once at startup.

## Overlays

Every screenie surface is a layer-shell overlay, of one of two kinds.

**Surfaces that own the keyboard** (the selector, the editor) open with exclusive
keyboard interactivity and take input everywhere. They use `KeyboardGrab`. When such a
surface closes on a key press, the compositor hands the still-held key to the app
beneath, which then sees it pressed and gets its release (an Esc leaking into your
terminal). Attach the grab with `KeyboardGrab::track` (capture phase, so a view can't
hide keys from it) and close through `when_released`: the action runs once, outside
event dispatch, when every key and modifier is up (or after a timeout). While leaving,
the surface should render as gone, and the grab swallows further key events.

**Surfaces that float over other apps** (preview cards, the recording pill) open with no
keyboard interactivity and use `Hover`:

- `Hover::new(window, cx)` when the surface opens.
- `Hover::area(&hover, key)` as the last child of each interactive element.
- `Hover::root(&hover, root, cx)` around the finished root element.
- `when_released` to close.
- Observe the entity for `hovered()`.

`Hover` keeps one rule: the surface holds the keyboard exactly while the pointer is on
one of its areas, and until the keys pressed there are let go.

- **The rest of the surface is click-through.** The input region is the areas as last
  painted.
- **No pointer-lock trap.** Pointer focus without keyboard focus is a trap on sway. The
  focused app's pointer lock (a fullscreen game) drops every motion while the cursor is
  on another surface, so the cursor would freeze on the overlay. Holding the keyboard
  lifts the lock.
- **Keys go where the pointer is.** Esc on a card dismisses the card, rather than going
  to the game beneath.
- **It doesn't grab a resting pointer.** A surface gets its input region only once it's
  on screen. Compositors re-pick the pointer's surface when a surface maps, not when a
  region grows, so an overlay appearing under a resting pointer isn't entered until the
  pointer moves.
- **Hover is tracked from the raw pointer events**, capture phase, so no element can hide
  a move from it.

Nothing else in an overlay should set the input region or keyboard interactivity.
