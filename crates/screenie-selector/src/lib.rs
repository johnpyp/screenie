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
//! While it's open, a [`Remote`] steers it from outside: the capture shortcut pressed
//! again cancels it or switches its mode.
//!
//! A capture that goes on to the overlay editor ([`SelectorConfig::hand_off`]) keeps the
//! frozen screen up until the editor covers it, so the live screen never flashes in
//! between: the caller closes it with [`Handoff::close`].
//!
//! The interaction logic lives in [`model`] and is UI-toolkit-free.

pub mod model;
mod view;

use std::sync::Arc;

use gpui::{AnyWindowHandle, App, AppContext, AsyncApp, px, size};
use screenie_core::{OutputInfo, Snapshot, WindowInfo};
use screenie_ui_kit::KeyboardGrab;
use screenie_ui_kit::layer::{LayerSpec, open_layer, wait_for_displays};

pub use model::{Mode, Purpose, Selection};
use view::{OutputView, Session, frozen_parts};

/// What the selector shows underneath its chrome.
pub enum Backdrop {
    /// A frozen desktop: pixel-exact, with magnifier.
    Frozen(Arc<Snapshot>),
    /// The live desktop (for recordings): only the layout is known.
    Live {
        outputs: Vec<OutputInfo>,
        windows: Vec<WindowInfo>,
    },
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
    /// A capture goes on to something that will cover the screen (the overlay editor):
    /// once captured, keep showing the frozen screen, dimmed around the capture, until
    /// the [`Choice`]'s [`Handoff`] is closed.
    pub hand_off: bool,
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
            hand_off: false,
        }
    }
}

/// What the user picked.
#[derive(Debug, Clone)]
pub struct Choice {
    pub selection: Selection,
    pub record: RecordOptions,
    /// With [`SelectorConfig::hand_off`], the selector still on screen.
    pub handoff: Option<Handoff>,
}

/// A selector that stayed on screen after a capture (see [`SelectorConfig::hand_off`]).
/// It takes no more input; close it once whatever follows covers the screen.
#[derive(Debug, Clone)]
pub struct Handoff(Vec<AnyWindowHandle>);

impl Handoff {
    /// Take the selector off the screen. Closing it again does nothing.
    pub fn close(&self, cx: &mut App) {
        close(&self.0, cx);
    }
}

fn close(handles: &[AnyWindowHandle], cx: &mut App) {
    for handle in handles {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

/// Steers an open selector from outside, e.g. when the capture shortcut is pressed
/// again while it's up. Does nothing once it's closed.
#[derive(Clone)]
pub struct Remote(async_channel::Sender<Command>);

/// What a [`Remote`] sends, for [`select_steered`] to follow.
pub struct Steering(async_channel::Receiver<Command>);

enum Command {
    Cancel,
    Mode(Mode),
}

impl Remote {
    /// A remote, and the steering to open a selector with.
    pub fn new() -> (Remote, Steering) {
        let (tx, rx) = async_channel::unbounded();
        (Remote(tx), Steering(rx))
    }

    /// Close the selector as if the user cancelled.
    pub fn cancel(&self) {
        let _ = self.0.try_send(Command::Cancel);
    }

    /// Switch to `mode`, as its key would.
    pub fn set_mode(&self, mode: Mode) {
        let _ = self.0.try_send(Command::Mode(mode));
    }
}

/// Show the selector and wait for the user. `None` if they cancelled.
pub async fn select(
    cx: &mut AsyncApp,
    backdrop: Backdrop,
    config: SelectorConfig,
) -> Option<Choice> {
    let (_, steering) = Remote::new();
    select_steered(cx, backdrop, config, steering).await
}

/// [`select`], following the [`Remote`] that `steering` came with.
pub async fn select_steered(
    cx: &mut AsyncApp,
    backdrop: Backdrop,
    config: SelectorConfig,
    steering: Steering,
) -> Option<Choice> {
    let (outputs, windows, snapshot) = match backdrop {
        Backdrop::Frozen(snapshot) => {
            // The measured scale makes snapping match the captured pixels exactly.
            let outputs: Vec<OutputInfo> = snapshot
                .outputs
                .iter()
                .map(|c| OutputInfo {
                    scale: c.scale(),
                    ..c.output.clone()
                })
                .collect();
            (outputs, snapshot.windows.clone(), Some(snapshot))
        }
        Backdrop::Live { outputs, windows } => (outputs, windows, None),
    };
    wait_for_displays(cx, outputs.len()).await;

    let hand_off = config.hand_off;
    let mut model = model::Model::new(outputs.clone(), windows, config.purpose, config.mode)
        .with_capture_on_release(config.capture_on_release)
        .with_ui_scale(f64::from(cx.update(|cx| screenie_ui_kit::ui_scale(cx))))
        .with_window_snapping(config.window_snapping);
    if let Some(initial) = config.initial.clone() {
        model = model.with_selection(initial);
    }
    let (tx, rx) = async_channel::bounded(1);
    let active_output = config
        .focused_output
        .clone()
        .or_else(|| outputs.first().map(|o| o.name.clone()));
    let grab = cx.update(KeyboardGrab::new);
    let session = cx.new(|_| Session {
        model,
        grab,
        magnifier: config.magnifier && snapshot.is_some(),
        record: config.record,
        active_output,
        snapshot: snapshot.clone(),
        done: Some(tx),
        settled: None,
        config,
    });

    let mut handles: Vec<AnyWindowHandle> = Vec::new();
    for output in outputs {
        let frozen = snapshot
            .as_ref()
            .and_then(|s| frozen_parts(s, &output.name));
        let spec = LayerSpec::fullscreen_overlay(
            "screenie-selector",
            &output.name,
            size(
                px(output.logical.width as f32),
                px(output.logical.height as f32),
            ),
        );
        let session = session.clone();
        let opened = cx.update(|cx| {
            open_layer(cx, &spec, |window, cx| {
                // Kept out of captures taken while it's open (a screenshot while a
                // recording's area is picked).
                screenie_ui_kit::conceal::track(window, &spec, cx);
                let (session, output, frozen) = (session.clone(), output.clone(), frozen.clone());
                cx.new(|cx| OutputView::new(session, output, frozen, window, cx))
            })
        });
        match opened {
            Ok(handle) => handles.push(handle.into()),
            Err(e) => tracing::error!(output = output.name, "cannot open selector window: {e}"),
        }
    }
    if handles.is_empty() {
        return None;
    }

    let steer = cx.spawn({
        let session = session.clone();
        async move |cx| {
            while let Ok(command) = steering.0.recv().await {
                session.update(cx, |s, cx| match command {
                    Command::Cancel => s.finish(None, cx),
                    Command::Mode(mode) => {
                        let outcome = s.model.set_mode(mode);
                        s.apply(outcome, cx);
                    }
                });
            }
        }
    });
    let mut choice = rx.recv().await.ok().flatten();
    drop(steer);
    match &mut choice {
        Some(choice) if hand_off => choice.handoff = Some(Handoff(handles)),
        _ => cx.update(|cx| close(&handles, cx)),
    }
    choice
}
