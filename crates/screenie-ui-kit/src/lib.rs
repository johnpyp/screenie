//! Shared GPUI building blocks for screenie's surfaces: bundled assets (icons, Inter),
//! the HUD look and its tooltips, layer-shell window options, overlay input (hover and
//! the keyboard), keeping surfaces out of captures, and image conversion.

pub mod assets;
pub mod conceal;
mod hover;
pub mod hud;
mod image;
mod keys;
pub mod layer;
mod scale;
mod tip;

pub use assets::{Assets, FONT, Icon};
pub use hover::Hover;
pub use image::render_image;
pub use keys::{KeyboardGrab, RELEASE_TIMEOUT};
pub use layer::{
    LayerSpec, display_for_output, fallback_options, layer_options, wait_for_displays,
};
pub use scale::{UI_SCALE_RANGE, set_ui_scale, track as track_ui_scale, ui, ui_px, ui_scale};
pub use tip::Tip;

pub use screenie_core::APP_ID;

/// One-time setup after the GPUI app starts: components, fonts, theme.
pub fn init(cx: &mut gpui::App) {
    gpui_kit::component::init(cx);
    assets::load_fonts(cx);
    let theme = gpui_kit::component::Theme::global_mut(cx);
    theme.font_family = FONT.into();
}
