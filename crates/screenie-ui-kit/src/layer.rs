//! Layer-shell surfaces: overlays and floating cards that sit above normal windows without
//! taking fullscreen away from anything or disturbing tiling. Compositors without
//! layer-shell (GNOME) get a best-effort regular window instead.

use std::time::Duration;

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui::{
    App, AsyncApp, Bounds, DisplayId, Pixels, Size, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, point, px,
};

/// Where and how a layer surface appears.
#[derive(Debug, Clone)]
pub struct LayerSpec {
    pub namespace: &'static str,
    pub layer: Layer,
    pub anchor: Anchor,
    /// Surface size in logical pixels. For edges that are anchored on both sides the
    /// compositor stretches the surface; the size still seeds the first frame.
    pub size: Size<Pixels>,
    /// Top, right, bottom, left.
    pub margin: Option<(Pixels, Pixels, Pixels, Pixels)>,
    pub keyboard: KeyboardInteractivity,
    pub exclusive_zone: Option<Pixels>,
    /// Output (connector name) to show on; `None` lets the compositor choose.
    pub output: Option<String>,
}

impl LayerSpec {
    /// A surface covering an entire output, above everything, grabbing the keyboard.
    pub fn fullscreen_overlay(namespace: &'static str, output: &str, size: Size<Pixels>) -> Self {
        Self {
            namespace,
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            size,
            margin: None,
            keyboard: KeyboardInteractivity::Exclusive,
            exclusive_zone: Some(px(-1.)),
            output: Some(output.to_string()),
        }
    }

    /// Never take keyboard focus (click-through chrome, HUDs).
    pub fn passive(mut self) -> Self {
        self.keyboard = KeyboardInteractivity::None;
        self
    }

    /// A floating surface of fixed size pinned to a corner/edge, never taking focus.
    pub fn floating(namespace: &'static str, anchor: Anchor, size: Size<Pixels>) -> Self {
        Self {
            namespace,
            layer: Layer::Overlay,
            anchor,
            size,
            margin: None,
            keyboard: KeyboardInteractivity::None,
            exclusive_zone: None,
            output: None,
        }
    }
}

/// The GPUI display for an output name. GPUI identifies Wayland outputs by a UUID derived
/// from the connector name.
pub fn display_for_output(cx: &App, name: &str) -> Option<DisplayId> {
    let want = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, name.as_bytes());
    cx.displays()
        .into_iter()
        .find(|d| d.uuid().ok() == Some(want))
        .map(|d| d.id())
}

/// GPUI learns about outputs asynchronously after startup; wait (briefly) until it knows
/// about `count` of them.
pub async fn wait_for_displays(cx: &mut AsyncApp, count: usize) {
    for _ in 0..100 {
        if cx.update(|cx| cx.displays().len()) >= count {
            return;
        }
        cx.background_executor()
            .timer(Duration::from_millis(5))
            .await;
    }
    tracing::warn!("GPUI knows fewer displays than expected ({count})");
}

/// Window options for a layer surface.
pub fn layer_options(cx: &App, spec: &LayerSpec) -> WindowOptions {
    let display_id = spec
        .output
        .as_deref()
        .and_then(|name| display_for_output(cx, name));
    if spec.output.is_some() && display_id.is_none() {
        tracing::warn!(output = ?spec.output, "no GPUI display for output");
    }
    WindowOptions {
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: spec.namespace.to_string(),
            layer: spec.layer,
            anchor: spec.anchor,
            exclusive_zone: spec.exclusive_zone,
            exclusive_edge: None,
            margin: spec.margin,
            keyboard_interactivity: spec.keyboard,
        }),
        display_id,
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            spec.size,
        ))),
        window_background: WindowBackgroundAppearance::Transparent,
        focus: spec.keyboard != KeyboardInteractivity::None,
        show: true,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some(crate::APP_ID.to_string()),
        ..Default::default()
    }
}

/// Fallback for compositors without layer-shell: a regular (fullscreen, for overlays)
/// window on the same display.
pub fn fallback_options(cx: &App, spec: &LayerSpec) -> WindowOptions {
    let display_id = spec
        .output
        .as_deref()
        .and_then(|name| display_for_output(cx, name));
    let covers = spec
        .anchor
        .contains(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    let bounds = Bounds::new(point(px(0.), px(0.)), spec.size);
    WindowOptions {
        display_id,
        window_bounds: Some(if covers {
            WindowBounds::Fullscreen(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        }),
        window_background: WindowBackgroundAppearance::Transparent,
        focus: true,
        show: true,
        app_id: Some(crate::APP_ID.to_string()),
        titlebar: None,
        ..Default::default()
    }
}
