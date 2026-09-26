//! Shared GPUI building blocks for screenie's surfaces: bundled assets (icons, Inter),
//! the HUD look, layer-shell window options, releasing the keyboard cleanly, and image
//! conversion.

pub mod assets;
pub mod hud;
mod image;
mod keys;
pub mod layer;
mod scale;

pub use assets::{Assets, FONT, Icon};
pub use image::render_image;
pub use keys::{KeyboardGrab, RELEASE_TIMEOUT};
pub use scale::{UI_SCALE_RANGE, set_ui_scale, track as track_ui_scale, ui, ui_px, ui_scale};
pub use layer::{LayerSpec, display_for_output, fallback_options, layer_options, wait_for_displays};

/// Application id used for windows and the desktop entry.
pub const APP_ID: &str = "dev.johnpyp.Screenie";

/// One-time setup after the GPUI app starts: components, fonts, theme.
pub fn init(cx: &mut gpui::App) {
    gpui_kit::component::init(cx);
    assets::load_fonts(cx);
    let theme = gpui_kit::component::Theme::global_mut(cx);
    theme.font_family = FONT.into();
}
