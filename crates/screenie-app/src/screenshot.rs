//! The screenshot flow: freeze, pick, render, deliver.
//!
//! The screen is frozen with screenie's own surfaces concealed (see
//! `screenie_ui_kit::conceal`), so a card from the last shot, the recording chrome, an
//! overlay editor or a selector never ends up in the image.
//!
//! A screenshot requested while a screenshot selector is up goes to it: the same
//! shortcut again closes it (like `record` stops a recording), another selection mode
//! switches it to that mode, and a capture without a selector (`shot screen`) is taken
//! from the moment the selector froze, which then closes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, anyhow, bail};
use gpui::AsyncApp;
use screenie_capture::{CaptureContext, SnapshotOptions};
use screenie_config::{EditorMode, Subject};
use screenie_core::{Rect, Snapshot, WindowInfo};
use screenie_ipc::{Response, ScreenshotRequest, SelectMode, Target};
use screenie_selector::{Backdrop, Mode, Purpose, Remote, Selection, SelectorConfig};
use screenie_ui_kit::conceal::{self, Scope};

use crate::daemon::Daemon;
use crate::deliver::{self, Actions, Capture};
use crate::editor::Handover;

/// The longest a selector stays up for the editor to take over (see
/// [`SelectorConfig::hand_off`]).
const HANDOFF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// A screenshot selector on screen.
pub(crate) struct Selecting {
    id: u64,
    /// The mode it was last asked for.
    mode: SelectMode,
    remote: Remote,
    /// The desktop it froze.
    snapshot: Arc<Snapshot>,
}

pub(crate) async fn take(req: ScreenshotRequest, cx: &mut AsyncApp) -> Response {
    if let Target::Select { mode } = req.target
        && let Some(response) = cx.update(|cx| Daemon::update(cx, |d, _| steer(d, mode)))
    {
        return response;
    }
    // An edit goes on after the capture: wait for it without holding up other captures.
    let result = match run(req, cx).await {
        Ok(Some(delivery)) => delivery.finished(cx).await,
        other => other.map(|_| None),
    };
    match result {
        Ok(Some(delivered)) => Response::Captured {
            path: delivered.path,
            temporary: delivered.temporary,
        },
        Ok(None) => Response::Cancelled,
        Err(e) => {
            tracing::error!("screenshot failed: {e:#}");
            Response::error(format!("{e:#}"))
        }
    }
}

/// A selection shortcut pressed while a screenshot selector is up: the same one closes it,
/// another switches it to its mode. `None` if there's no selector.
fn steer(d: &mut Daemon, mode: SelectMode) -> Option<Response> {
    let open = d.selector.as_mut()?;
    if open.mode == mode {
        open.remote.cancel();
        Some(Response::Cancelled)
    } else {
        open.mode = mode;
        open.remote.set_mode(selector_mode(mode));
        Some(Response::Ok)
    }
}

pub(crate) fn selector_mode(mode: SelectMode) -> Mode {
    match mode {
        SelectMode::Area => Mode::Area,
        SelectMode::Window => Mode::Window,
        SelectMode::Screen => Mode::Screen,
    }
}

async fn run(
    req: ScreenshotRequest,
    cx: &mut AsyncApp,
) -> anyhow::Result<Option<deliver::Delivery>> {
    let (config, capture, last_region) = cx.update(|cx| {
        let d = Daemon::get(cx);
        (d.config.clone(), d.capture.clone(), d.last_region())
    });
    let mut actions = Actions::resolve(
        &config.screenshot.after_capture,
        &req.actions,
        req.output.clone(),
        req.want_file,
    );
    if req.delay > 0 {
        cx.background_executor()
            .timer(std::time::Duration::from_secs(req.delay as u64))
            .await;
    }

    let interactive = matches!(req.target, Target::Select { .. });
    if interactive {
        // From here until the selector closes (not through the delay before, nor the
        // delivery after).
        let busy =
            cx.update(|cx| Daemon::update(cx, |d, _| std::mem::replace(&mut d.capturing, true)));
        if busy {
            bail!("a capture is already in progress");
        }
    }
    let options = SnapshotOptions {
        cursor: req.cursor.unwrap_or(config.screenshot.show_cursor),
        backend: config.advanced.capture_backend,
        windows: interactive || req.target == Target::ActiveWindow,
    };
    // What a selector on screen froze is what the user sees: take it from there.
    let frozen = (!interactive)
        .then(|| {
            cx.update(|cx| {
                Daemon::update(cx, |d, _| {
                    let open = d.selector.take()?;
                    open.remote.cancel();
                    Some(open.snapshot)
                })
            })
        })
        .flatten();
    let snapshot = match frozen {
        Some(snapshot) => snapshot,
        None => match freeze(&capture, options, cx).await {
            Ok(snapshot) => Arc::new(snapshot),
            Err(e) => {
                if interactive {
                    cx.update(|cx| Daemon::update(cx, |d, _| d.capturing = false));
                }
                return Err(e);
            }
        },
    };

    // The window being captured, if any, names the file.
    let mut window: Option<WindowInfo> = None;
    // A selector left up for the editor to take over from.
    let mut lingering: Option<screenie_selector::Handoff> = None;
    let region: Rect = match &req.target {
        Target::Select { mode } => {
            // Steerable from the moment it's decided on.
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let (remote, steering) = Remote::new();
            let open = Selecting {
                id,
                mode: *mode,
                remote,
                snapshot: snapshot.clone(),
            };
            cx.update(|cx| Daemon::update(cx, |d, _| d.selector = Some(open)));
            let focused = focused_output(&capture, cx).await;
            let selector = SelectorConfig {
                purpose: Purpose::Screenshot,
                mode: selector_mode(*mode),
                capture_on_release: config.selector.capture_on_release,
                magnifier: config.selector.magnifier,
                window_snapping: config.selector.window_snapping,
                toolbar: config.selector.toolbar,
                dim: config.selector.dim,
                initial: None,
                focused_output: focused,
                record: Default::default(),
                // The overlay editor takes over from the frozen screen.
                hand_off: actions.edit && config.editor.mode == EditorMode::Overlay,
            };
            let backdrop = Backdrop::Frozen(snapshot.clone());
            let choice = screenie_selector::select_steered(cx, backdrop, selector, steering).await;
            cx.update(|cx| {
                Daemon::update(cx, |d, _| {
                    d.capturing = false;
                    if d.selector.as_ref().is_some_and(|s| s.id == id) {
                        d.selector = None;
                    }
                })
            });
            let Some(choice) = choice else {
                return Ok(None);
            };
            if let Some(handoff) = choice.handoff.clone() {
                lingering = Some(handoff.clone());
                // Whatever happens next, the selector doesn't outstay it.
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(HANDOFF_TIMEOUT).await;
                    cx.update(|cx| handoff.close(cx));
                })
                .detach();
            }
            match choice.selection {
                Selection::Region(r) => r,
                Selection::Window(w) => {
                    let rect = w.rect;
                    window = Some(w);
                    rect
                }
                Selection::Output(o) => o.logical,
            }
        }
        Target::Screen { output } => {
            let name = match output {
                Some(name) => Some(name.clone()),
                None => focused_output(&capture, cx).await,
            };
            let chosen = match &name {
                Some(n) => snapshot
                    .output_named(n)
                    .ok_or_else(|| anyhow!("no output named {n}"))?,
                None => snapshot
                    .outputs
                    .first()
                    .ok_or_else(|| anyhow!("no outputs"))?,
            };
            chosen.output.logical
        }
        Target::AllScreens => snapshot.layout_bounds(),
        Target::ActiveWindow => {
            let focused = snapshot
                .windows
                .iter()
                .find(|w| w.focused)
                .cloned()
                .ok_or_else(|| {
                    anyhow!(
                        "no focused window (screenie finds windows on Hyprland, Sway, niri \
                         and KDE Plasma)"
                    )
                })?;
            let rect = focused.rect;
            window = Some(focused);
            rect
        }
        Target::Region { rect } => *rect,
        Target::LastRegion => {
            last_region.ok_or_else(|| anyhow!("there is no previous capture region yet"))?
        }
    };

    let close_selector = |cx: &mut AsyncApp| {
        if let Some(lingering) = &lingering {
            cx.update(|cx| lingering.close(cx));
        }
    };
    let Some(region) = region.intersection(&snapshot.layout_bounds()) else {
        close_selector(cx);
        bail!("the region is off screen");
    };
    if region.width < 1.0 || region.height < 1.0 {
        close_selector(cx);
        bail!("the region is empty");
    }
    let output_name = output_for(&snapshot, region);
    let placement = output_name
        .as_deref()
        .and_then(|name| snapshot.output_named(name))
        .and_then(|o| {
            let screen = o.output.logical;
            (screen.intersection(&region) == Some(region)).then(|| {
                Rect::new(
                    region.x - screen.x,
                    region.y - screen.y,
                    region.width,
                    region.height,
                )
            })
        });
    let render_snapshot = snapshot.clone();
    let image = cx
        .background_executor()
        .spawn(async move { render_snapshot.render_region(region) })
        .await;
    let taken = snapshot.taken_at.into();
    // An edit goes on over the screen as it was frozen.
    let screen = actions
        .edit
        .then(|| {
            output_name
                .as_deref()
                .and_then(|n| snapshot.output_named(n))
        })
        .flatten()
        .map(|o| o.image.clone());
    drop(snapshot);
    cx.update(|cx| Daemon::update(cx, |d, _| d.remember_region(region)));

    let capture = Capture {
        scale: (image.width() as f64 / region.width) as f32,
        image,
        subject: window
            .map(|w| Subject {
                app: Some(w.app_id),
                title: Some(w.title),
            })
            .unwrap_or_default(),
        output: output_name,
        placement,
        taken,
    };
    actions.output = actions
        .output
        .map(|output| deliver::output_path(&output, &config, &capture));
    let handover = Handover {
        screen,
        selector: lingering,
    };
    deliver::screenshot(capture, actions, config, handover, cx)
        .await
        .map(Some)
}

/// Capture every output, with screenie's own surfaces out of the way.
async fn freeze(
    capture: &Arc<CaptureContext>,
    options: SnapshotOptions,
    cx: &mut AsyncApp,
) -> anyhow::Result<Snapshot> {
    let concealed = conceal::conceal(Scope::everything(), cx).await;
    let capture = capture.clone();
    let snapshot = cx
        .background_executor()
        .spawn(async move { capture.snapshot(options) })
        .await
        .context("capturing the screen");
    drop(concealed);
    snapshot
}

/// The output the user is on (see [`CaptureContext::focused_output`]).
async fn focused_output(capture: &Arc<CaptureContext>, cx: &mut AsyncApp) -> Option<String> {
    let capture = capture.clone();
    cx.background_executor()
        .spawn(async move { capture.focused_output() })
        .await
}

/// The output showing most of `region`.
fn output_for(snapshot: &Snapshot, region: Rect) -> Option<String> {
    snapshot
        .outputs
        .iter()
        .filter_map(|o| {
            o.output
                .logical
                .intersection(&region)
                .map(|i| (i.area(), &o.output.name))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, name)| name.clone())
}
