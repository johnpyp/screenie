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
- **`image`**: `render_image` converts a `screenie_core::Image` into a GPUI `RenderImage`.

Call `screenie_ui_kit::init(cx)` once at startup.
