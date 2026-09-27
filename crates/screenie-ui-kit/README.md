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
  panel) targeted to an output by connector name. `open_layer` opens one: through
  layer-shell, or on GNOME as a window screenie's GNOME Shell extension makes into it
  (described first, by a title only it has), or, for one that takes the keyboard, as a
  plain fullscreen window. `floats` says whether surfaces that don't (cards, a
  recording's chrome) can be had at all. `wait_for_displays` covers GPUI's late output
  discovery.
- **`keys`** and **`hover`**: input for overlays; see below.
- **`conceal`**: keeping screenie out of its own captures. Surfaces that must never be
  captured `track` themselves when they open and finish their root with `root`, which
  makes them fully transparent while concealed (`Hover` drops their input region
  too). `conceal(scope, cx)` conceals what a `Scope` covers (everything, or one layer
  namespace on one output) and resolves once the compositor has shown the frame
  without them. The returned guard shows them again when dropped.
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

**Surfaces that float over other apps** (preview cards, the recording pill) never take
the keyboard, and use `Hover` for the pointer:

- `Hover::new(window, cx)` when the surface opens (with no keyboard interactivity).
- `Hover::area(&hover, key)` as the last child of each interactive element.
- `Hover::root(&hover, root, cx)` around the finished root element.
- Observe the entity for `hovered()`.

What it guarantees:

- **No keyboard, ever.** Keyboard focus that moves while a key is held splits the press
  between two apps, and one that doesn't catch up keeps the key held. Xwayland never
  delivers a release that happened while an app was unfocused, so in a Proton game a
  Tab held onto a card stayed down, and Shift then opened Steam's overlay as Shift+Tab.
  Notifications, bars and docks never take it for the same reason.
- **The rest of the surface is click-through.** The input region is the areas as last
  painted.
- **A stuck pointer gets out.** sway enforces a game's pointer lock for the surface with
  the keyboard (the game, since overlays never take it), and drops every motion while the
  cursor is over another surface: a game that locks with the cursor on a card freezes it
  there. GPUI tells `Hover` when the pointer is stuck (relative motion but no motion;
  see `patches/README.md`). It then empties its input region for a moment and maps a
  throwaway pixel, since a surface mapping is what makes sway pick the surface under the
  cursor again, and that's the game now. Hyprland, KWin, niri and mutter hand a locked
  pointer to the game themselves.
- **It doesn't grab a resting pointer, at first.** A surface gets its input region only
  once it's on screen. Compositors re-pick the pointer's surface when a surface maps,
  not when a region grows, so an overlay appearing under a resting pointer isn't entered
  when it appears. sway also re-picks it on a button release and after any layout
  change, and then a parked pointer is entered without moving. Being entered takes
  nothing from the app beneath, though: it keeps the keyboard.
- **Hover is tracked from the raw pointer events**, capture phase, so no element can hide
  a move from it.

Nothing else in an overlay should set the input region.
