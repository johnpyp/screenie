//! The screenshot flow: freeze, pick, render, deliver.

use std::sync::Arc;

use anyhow::{Context as _, anyhow, bail};
use gpui::AsyncApp;
use screenie_capture::SnapshotOptions;
use screenie_config::Subject;
use screenie_core::{Rect, Snapshot, WindowInfo};
use screenie_ipc::{Response, ScreenshotRequest, SelectMode, Target};
use screenie_selector::{Backdrop, Mode, Purpose, Selection, SelectorConfig};

use crate::daemon::Daemon;
use crate::deliver::{self, Actions, Capture};

pub(crate) async fn take(req: ScreenshotRequest, cx: &mut AsyncApp) -> Response {
    let interactive = matches!(req.target, Target::Select { .. });
    if interactive {
        let busy =
            cx.update(|cx| Daemon::update(cx, |d, _| std::mem::replace(&mut d.capturing, true)));
        if busy {
            return Response::error("a capture is already in progress");
        }
    }
    let result = run(req, cx).await;
    if interactive {
        cx.update(|cx| Daemon::update(cx, |d, _| d.capturing = false));
    }
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
) -> anyhow::Result<Option<deliver::Delivered>> {
    let (config, capture, last_region) = cx.update(|cx| {
        let d = Daemon::get(cx);
        (d.config.clone(), d.capture.clone(), d.last_region)
    });
    let actions = Actions::resolve(
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
    let options = SnapshotOptions {
        cursor: req.cursor.unwrap_or(config.screenshot.show_cursor),
        backend: config.advanced.capture_backend,
        windows: interactive || req.target == Target::ActiveWindow,
    };
    let snapshot_capture = capture.clone();
    let snapshot = cx
        .background_executor()
        .spawn(async move { snapshot_capture.snapshot(options) })
        .await
        .context("capturing the screen")?;
    let snapshot = Arc::new(snapshot);

    // The window being captured, if any, names the file.
    let mut window: Option<WindowInfo> = None;
    let region: Rect = match &req.target {
        Target::Select { mode } => {
            let compositor = capture.compositor().clone();
            let focused = cx
                .background_executor()
                .spawn(async move { compositor.focused_output().ok().flatten() })
                .await;
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
            };
            let Some(choice) =
                screenie_selector::select(cx, Backdrop::Frozen(snapshot.clone()), selector).await
            else {
                return Ok(None);
            };
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
                None => {
                    let compositor = capture.compositor().clone();
                    cx.background_executor()
                        .spawn(async move { compositor.focused_output().ok().flatten() })
                        .await
                }
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
                    anyhow!("no focused window (needs compositor IPC: Hyprland, Sway or niri)")
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

    let region = region
        .intersection(&snapshot.layout_bounds())
        .ok_or_else(|| anyhow!("the region is off screen"))?;
    if region.width < 1.0 || region.height < 1.0 {
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
    drop(snapshot);
    cx.update(|cx| Daemon::update(cx, |d, _| d.last_region = Some(region)));

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
    };
    deliver::screenshot(capture, actions, config, cx)
        .await
        .map(Some)
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
