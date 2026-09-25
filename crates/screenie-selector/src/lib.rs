//! The capture overlay.
//!
//! [`select`] covers every output with a layer-shell surface showing either the frozen
//! desktop (screenshots) or the live one (recordings), and lets the user pick a region,
//! window, or screen:
//!
//! * **Drag** selects a region (Shift: square, Alt: from center, Space: move).
//! * **Click** picks the window under the cursor, or the screen if there's none.
//! * **Enter** confirms, or picks the screen under the cursor; **Esc** cancels.
//! * **1 / 2 / 3**, **Tab**, or **Space** (while idle) switch between area, window and
//!   screen modes; **M** toggles the magnifier; arrows nudge an edited selection.
//!
//! The interaction logic lives in [`model`] and is UI-toolkit-free.

pub mod model;
mod view;

use std::sync::Arc;

use gpui::{AnyWindowHandle, AppContext, AsyncApp, px, size};
use screenie_core::{OutputInfo, Snapshot, WindowInfo};
use screenie_ui_kit::layer::{LayerSpec, fallback_options, layer_options, wait_for_displays};

pub use model::{Mode, Purpose, Selection};
use view::{OutputView, Session, frozen_parts};

/// What the selector shows underneath its chrome.
pub enum Backdrop {
    /// A frozen desktop: pixel-exact, with magnifier.
    Frozen(Arc<Snapshot>),
    /// The live desktop (for recordings): only the layout is known.
    Live { outputs: Vec<OutputInfo>, windows: Vec<WindowInfo> },
}

/// Recording toggles offered in the toolbar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecordOptions {
    pub system_audio: bool,
    pub microphone: bool,
}

#[derive(Debug, Clone)]
pub struct SelectorConfig {
    pub purpose: Purpose,
    pub mode: Mode,
    pub capture_on_release: bool,
    pub magnifier: bool,
    pub window_snapping: bool,
    pub toolbar: bool,
    /// Opacity of the dimming outside the selection.
    pub dim: f64,
    /// Start with this selection ready to adjust.
    pub initial: Option<Selection>,
    /// Output to show the toolbar on before the pointer moves.
    pub focused_output: Option<String>,
    pub record: RecordOptions,
}

impl Default for SelectorConfig {
    fn default() -> Self {
        Self {
            purpose: Purpose::Screenshot,
            mode: Mode::Area,
            capture_on_release: true,
            magnifier: true,
            window_snapping: true,
            toolbar: true,
            dim: 0.45,
            initial: None,
            focused_output: None,
            record: RecordOptions::default(),
        }
    }
}

/// What the user picked.
#[derive(Debug, Clone)]
pub struct Choice {
    pub selection: Selection,
    pub record: RecordOptions,
}

/// Show the selector and wait for the user. `None` if they cancelled.
pub async fn select(cx: &mut AsyncApp, backdrop: Backdrop, config: SelectorConfig) -> Option<Choice> {
    let (outputs, windows, snapshot) = match backdrop {
        Backdrop::Frozen(snapshot) => {
            // The measured scale makes snapping match the captured pixels exactly.
            let outputs: Vec<OutputInfo> = snapshot
                .outputs
                .iter()
                .map(|c| OutputInfo { scale: c.scale(), ..c.output.clone() })
                .collect();
            (outputs, snapshot.windows.clone(), Some(snapshot))
        }
        Backdrop::Live { outputs, windows } => (outputs, windows, None),
    };
    wait_for_displays(cx, outputs.len()).await;

    let mut model = model::Model::new(outputs.clone(), windows, config.purpose, config.mode)
        .with_capture_on_release(config.capture_on_release)
        .with_window_snapping(config.window_snapping);
    if let Some(initial) = config.initial.clone() {
        model = model.with_selection(initial);
    }
    let (tx, rx) = async_channel::bounded(1);
    let active_output = config.focused_output.clone().or_else(|| outputs.first().map(|o| o.name.clone()));
    let session = cx.new(|_| Session {
        model,
        magnifier: config.magnifier && snapshot.is_some(),
        record: config.record,
        active_output,
        snapshot: snapshot.clone(),
        done: Some(tx),
        config,
    });

    let mut handles: Vec<AnyWindowHandle> = Vec::new();
    for output in outputs {
        let frozen = snapshot.as_ref().and_then(|s| frozen_parts(s, &output.name));
        let spec = LayerSpec::fullscreen_overlay(
            "screenie-selector",
            &output.name,
            size(px(output.logical.width as f32), px(output.logical.height as f32)),
        );
        let session = session.clone();
        let opened = cx.update(|cx| {
            let build = |output: OutputInfo, frozen, session| {
                move |window: &mut gpui::Window, cx: &mut gpui::App| {
                    cx.new(|cx| OutputView::new(session, output, frozen, window, cx))
                }
            };
            let first = cx.open_window(layer_options(cx, &spec), build(output.clone(), frozen.clone(), session.clone()));
            match first {
                Ok(h) => Ok(h),
                Err(e) => {
                    tracing::debug!("layer-shell window failed ({e}); falling back to a regular window");
                    cx.open_window(fallback_options(cx, &spec), build(output.clone(), frozen, session))
                }
            }
        });
        match opened {
            Ok(handle) => handles.push(handle.into()),
            Err(e) => tracing::error!(output = output.name, "cannot open selector window: {e}"),
        }
    }
    if handles.is_empty() {
        return None;
    }

    let choice = rx.recv().await.ok().flatten();
    for handle in handles {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
    choice
}
