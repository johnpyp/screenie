# screenie-ui-kit

Screenie's GPUI look and feel, shared by every window.

- **`assets`**: an `AssetSource` serving bundled Lucide icons (`Icon` enum) and Inter,
  layered over gpui-component's assets. `load_fonts` registers the fonts.
- **`hud`**: the translucent dark HUD language: `color` tokens, `panel()`, `pill()`,
  `keycap()`, `separator()`, and `HudButton` (icon or label, tooltip, selected,
  accent/record styles).
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
- **`image`**: `render_image` converts a `screenie_core::Image` into a GPUI `RenderImage`.

Call `screenie_ui_kit::init(cx)` once at startup.
