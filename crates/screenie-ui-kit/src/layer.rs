//! Layer-shell surfaces: overlays and floating cards that sit above normal windows without
//! taking fullscreen away from anything or disturbing tiling. GNOME has no layer-shell:
//! there, screenie's GNOME Shell extension makes ordinary windows into them
//! (`screenie_desktop::shell::place`), and without it an overlay is a plain fullscreen
//! window, and nothing floats.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui::{
    App, AsyncApp, Bounds, DisplayId, Entity, Pixels, Render, Size, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, point, px,
    size,
};
use screenie_desktop::shell::{self, Feature};

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

/// Open `spec` as a layer surface: through layer-shell where the compositor has it, else
/// as a window screenie's GNOME Shell extension makes one, else, for one that takes the
/// keyboard, as a plain (fullscreen) window. Fails where a surface that doesn't take the
/// keyboard can't float over other windows (see [`floats`]).
pub fn open_layer<V: 'static + Render>(
    cx: &mut App,
    spec: &LayerSpec,
    build: impl Fn(&mut gpui::Window, &mut App) -> Entity<V>,
) -> anyhow::Result<WindowHandle<V>> {
    match cx.open_window(layer_options(cx, spec), &build) {
        Ok(handle) => return Ok(handle),
        Err(e) => tracing::debug!(namespace = spec.namespace, "no layer-shell surface: {e}"),
    }
    let keyboard = spec.keyboard != KeyboardInteractivity::None;
    let feature = if keyboard {
        Feature::Overlays
    } else {
        Feature::Floating
    };
    if shell::offers(feature) {
        let title = unique_title(spec.namespace);
        match shell::place(&title, &gnome_layer(spec)) {
            Ok(()) => return cx.open_window(placed_options(cx, spec, title), &build),
            Err(e) => tracing::warn!("screenie's GNOME Shell extension didn't take it: {e}"),
        }
    }
    anyhow::ensure!(
        keyboard,
        "nothing floats over other windows here (no layer-shell, and no GNOME Shell extension)"
    );
    cx.open_window(fallback_options(cx, spec), &build)
}

/// Whether surfaces that don't take the keyboard (cards, a recording's controls) float
/// over other windows here: with layer-shell (`layer_shell`), or on GNOME with screenie's
/// extension. Blocking there, but seldom.
pub fn floats(layer_shell: bool) -> bool {
    layer_shell || shell::offers(Feature::Floating)
}

/// A title no other window of this process has, to tell the extension which it is.
fn unique_title(namespace: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("{namespace} {}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// `spec` as screenie's GNOME Shell extension takes it.
fn gnome_layer(spec: &LayerSpec) -> shell::Layer<'_> {
    let (top, right, bottom, left) = spec.margin.unwrap_or_default();
    let px = |p: Pixels| f32::from(p).round() as i32;
    shell::Layer {
        output: spec.output.as_deref().unwrap_or_default(),
        anchor: spec.anchor.bits(),
        margin: (px(top), px(right), px(bottom), px(left)),
        keyboard: spec.keyboard != KeyboardInteractivity::None,
        exclusive: spec.exclusive_zone.map_or(0, px),
    }
}

/// Options for a window the extension makes into `spec`, titled `title`. One that takes
/// the keyboard is fullscreen on its output; one that doesn't is placed and sized by the
/// extension, and starts at its own size (or its output's, stretched between anchors).
fn placed_options(cx: &App, spec: &LayerSpec, title: String) -> WindowOptions {
    let titlebar = Some(TitlebarOptions {
        title: Some(title.into()),
        ..Default::default()
    });
    if spec.keyboard != KeyboardInteractivity::None {
        return WindowOptions {
            titlebar,
            ..fallback_options(cx, spec)
        };
    }
    let display = spec
        .output
        .as_deref()
        .and_then(|name| display_for_output(cx, name));
    let output_size = display
        .and_then(|id| cx.find_display(id))
        .map(|d| d.bounds().size)
        .unwrap_or(size(px(800.), px(600.)));
    let size = size(
        if spec.size.width > px(0.) {
            spec.size.width
        } else {
            output_size.width
        },
        if spec.size.height > px(0.) {
            spec.size.height
        } else {
            output_size.height
        },
    );
    WindowOptions {
        titlebar,
        display_id: display,
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size,
        ))),
        window_background: WindowBackgroundAppearance::Transparent,
        focus: false,
        show: true,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some(crate::APP_ID.to_string()),
        ..Default::default()
    }
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
