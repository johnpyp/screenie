//! Interface scale (`ui_scale` in the config).
//!
//! It scales the interface (buttons, bars, text, cards) but never screen geometry: a
//! selection, or a capture shown in place, must stay exactly where it is on screen. So
//! interface sizes are written in [`ui`] lengths, which are rems (16px at scale 1). Each
//! window's rem size is set from the scale, and screen geometry stays in plain `px`.
//!
//! Root views call [`track`] when they're created; [`set_ui_scale`] then keeps every
//! window up to date.

use gpui::{App, Global, Pixels, Rems, Window, px, rems};

/// Pixels per rem at scale 1.
const BASE_REM: f32 = 16.0;

/// The range a scale is clamped to, so a typo can't make the interface unusable.
pub const UI_SCALE_RANGE: (f32, f32) = (0.5, 3.0);

struct UiScale(f32);

impl Global for UiScale {}

/// An interface length: `n` pixels at scale 1, scaled with the interface.
pub fn ui(n: f32) -> Rems {
    rems(n / BASE_REM)
}

/// [`ui`] as pixels in `window`, for layout math and painting.
pub fn ui_px(window: &Window, n: f32) -> Pixels {
    window.rem_size() * (n / BASE_REM)
}

/// The current interface scale.
pub fn ui_scale(cx: &App) -> f32 {
    cx.try_global::<UiScale>().map_or(1.0, |s| s.0)
}

/// Set the interface scale and apply it to every open window.
pub fn set_ui_scale(scale: f32, cx: &mut App) {
    let scale = scale.clamp(UI_SCALE_RANGE.0, UI_SCALE_RANGE.1);
    if (ui_scale(cx) - scale).abs() < f32::EPSILON {
        return;
    }
    tracing::info!(scale, "interface scale");
    cx.set_global(UiScale(scale));
    for handle in cx.windows() {
        let _ = handle.update(cx, |_, window, _| {
            window.set_rem_size(px(BASE_REM * scale));
            window.refresh();
        });
    }
}

/// Give a new window the current scale. Call from the root view's constructor.
pub fn track(window: &mut Window, cx: &App) {
    window.set_rem_size(px(BASE_REM * ui_scale(cx)));
}
