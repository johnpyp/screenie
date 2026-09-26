# screenie-ui-kit

Screenie's GPUI look and feel, shared by every window.

- **`assets`**: an `AssetSource` serving bundled Lucide icons (`Icon` enum) and Inter,
  layered over gpui-component's assets. `load_fonts` registers the fonts.
- **`hud`**: the translucent dark HUD language: `color` tokens, `panel()`, `pill()`,
  `keycap()`, `separator()`, `inner_radius()` (concentric with a panel's corners), and
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
- **`keys`**: `KeyboardGrab`, for every surface that takes the keyboard. When such a
  surface closes on a key press, the compositor hands the still-held key to the app
  beneath, which then sees it pressed and gets its release (an Esc leaking into your
  terminal). Attach the grab with `KeyboardGrab::track` (capture phase, so a view can't
  hide keys from it) and close through `when_released`: the action runs once, outside
  event dispatch, when every key and modifier is up (or after a timeout). While leaving,
  the surface should render as gone, and the grab swallows further key events.
- **`scale`**: the interface scale (`ui_scale`). Interface sizes are written as `ui(n)`
  (rems: n pixels at scale 1), and screen geometry stays in `px`. `ui_px` gives the same
  length in pixels for layout math and painting. `set_ui_scale` applies a scale to
  every window; root views call `track_ui_scale` when created.
- **`image`**: `render_image` converts a `screenie_core::Image` into a GPUI `RenderImage`.

Call `screenie_ui_kit::init(cx)` once at startup.
