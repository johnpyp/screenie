//! The recording flow: pick → countdown → record → stop → deliver.
//!
//! The daemon holds at most one [`Active`] recording. It exists from the end of the
//! countdown's first tick (so `screenie stop` can cancel a countdown) until it's stopped.

mod controls;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use gpui::{App, AsyncApp, WindowHandle};
use screenie_config::{Config, expand_template, unique_path};
use screenie_core::{OutputInfo, Rect};
use screenie_ipc::{RecordRequest, RecordingStatus, Response, Target};
use screenie_record::{AudioSources, RecordSpec, Recording};
use screenie_selector::{Backdrop, Purpose, RecordOptions, SelectorConfig, Selection};

use crate::daemon::Daemon;
use crate::deliver::{self, Actions};
use controls::{Controls, Phase};

pub(crate) struct Active {
    pub path: PathBuf,
    output: String,
    /// `None` until the countdown is over and the pipeline is running.
    pub recording: Option<Recording>,
    /// Set when stopped or cancelled, so a start still in flight backs out.
    cancelled: Arc<AtomicBool>,
    controls: Vec<WindowHandle<Controls>>,
    actions: Actions,
}

impl Active {
    pub fn status(&self) -> RecordingStatus {
        let (elapsed, paused) = self.recording.as_ref().map_or((Duration::ZERO, false), |r| (r.elapsed(), r.is_paused()));
        RecordingStatus { path: self.path.clone(), elapsed_secs: elapsed.as_secs_f64(), paused }
    }
}

/// `screenie record`: start, or stop the running recording when toggling.
pub(crate) async fn record(req: RecordRequest, cx: &mut AsyncApp) -> Response {
    if cx.update(|cx| Daemon::get(cx).recording.is_some()) {
        return if req.toggle { stop(cx).await } else { Response::error("already recording") };
    }
    let busy = cx.update(|cx| Daemon::update(cx, |d, _| std::mem::replace(&mut d.capturing, true)));
    if busy {
        return Response::error("a capture is already in progress");
    }
    let result = begin(req, cx).await;
    cx.update(|cx| Daemon::update(cx, |d, _| d.capturing = false));
    match result {
        Ok(Some(path)) => Response::RecordingStarted { path },
        Ok(None) => Response::Cancelled,
        Err(e) => {
            tracing::error!("recording failed to start: {e:#}");
            Response::error(format!("{e:#}"))
        }
    }
}

/// Stop and save. During a countdown this cancels instead.
pub(crate) async fn stop(cx: &mut AsyncApp) -> Response {
    let Some(active) = take_active(cx) else {
        return Response::error("nothing is being recorded");
    };
    let Some(recording) = active.recording else {
        return Response::Cancelled;
    };
    let finished = cx.background_executor().spawn(async move { recording.stop() }).await;
    match finished {
        Ok(finished) => {
            let path = finished.path.clone();
            deliver::recording(finished, active.actions, Some(active.output), cx).await;
            Response::Captured { path: Some(path), temporary: false }
        }
        Err(e) => {
            tracing::error!("finishing the recording failed: {e}");
            Response::error(format!("finishing the recording failed: {e}"))
        }
    }
}

/// Stop and delete.
pub(crate) async fn cancel(cx: &mut AsyncApp) -> Response {
    let Some(active) = take_active(cx) else {
        return Response::error("nothing is being recorded");
    };
    if let Some(recording) = active.recording {
        cx.background_executor().spawn(async move { recording.cancel() }).await;
    }
    Response::Cancelled
}

pub(crate) fn toggle_pause(cx: &mut App) -> Response {
    let result = Daemon::update(cx, |d, _| {
        let recording = d.recording.as_ref().and_then(|a| a.recording.as_ref()).ok_or("nothing is being recorded")?;
        recording.set_paused(!recording.is_paused()).map_err(|_| "cannot pause this recording")?;
        d.broadcast();
        Ok::<_, &str>(())
    });
    cx.refresh_windows();
    match result {
        Ok(()) => Response::Ok,
        Err(e) => Response::error(e),
    }
}

fn take_active(cx: &mut AsyncApp) -> Option<Active> {
    cx.update(|cx| {
        let active = Daemon::update(cx, |d, _| {
            let active = d.recording.take();
            d.broadcast();
            active
        })?;
        active.cancelled.store(true, Ordering::Relaxed);
        controls::close(&active.controls, cx);
        Some(active)
    })
}

/// Where a new recording goes.
fn recording_path(config: &Config) -> PathBuf {
    let stem = expand_template(&config.recording.filename, chrono::Local::now());
    unique_path(&config.recording_dir(), &stem, "mp4")
}

/// The output showing most of `region`.
fn home_output(region: Rect, outputs: &[OutputInfo]) -> Option<&OutputInfo> {
    outputs
        .iter()
        .filter_map(|o| o.logical.intersection(&region).map(|i| (i.area(), o)))
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, o)| o)
}

async fn begin(req: RecordRequest, cx: &mut AsyncApp) -> anyhow::Result<Option<PathBuf>> {
    let (config, capture, last_region) = cx.update(|cx| {
        let d = Daemon::get(cx);
        (d.config.clone(), d.capture.clone(), d.last_region)
    });
    let background = cx.background_executor().clone();
    let compositor = capture.compositor().clone();
    let outputs = {
        let capture = capture.clone();
        background.spawn(async move { capture.outputs() }).await.context("listing outputs")?
    };
    let focused = {
        let compositor = compositor.clone();
        background.spawn(async move { compositor.focused_output().ok().flatten() }).await
    };
    let mut audio = RecordOptions {
        system_audio: req.system_audio.unwrap_or(config.recording.system_audio),
        microphone: req.microphone.unwrap_or(config.recording.microphone),
    };

    let region = match &req.target {
        Target::Select { mode } => {
            let windows = if config.selector.window_snapping {
                let compositor = compositor.clone();
                background.spawn(async move { compositor.windows().unwrap_or_default() }).await
            } else {
                Vec::new()
            };
            let selector = SelectorConfig {
                purpose: Purpose::Recording,
                mode: crate::screenshot::selector_mode(*mode),
                // Recording starts from the toolbar, after choosing audio.
                capture_on_release: false,
                magnifier: false,
                window_snapping: config.selector.window_snapping,
                toolbar: true,
                dim: config.selector.dim,
                initial: None,
                focused_output: focused,
                record: audio,
            };
            let backdrop = Backdrop::Live { outputs: outputs.clone(), windows };
            let Some(choice) = screenie_selector::select(cx, backdrop, selector).await else {
                return Ok(None);
            };
            audio = choice.record;
            match choice.selection {
                Selection::Region(r) => r,
                Selection::Window(w) => w.rect,
                Selection::Output(o) => o.logical,
            }
        }
        Target::Screen { output } => {
            let name = output.clone().or(focused);
            let chosen = match &name {
                Some(n) => outputs.iter().find(|o| &o.name == n).ok_or_else(|| anyhow!("no output named {n}"))?,
                None => outputs.first().ok_or_else(|| anyhow!("no outputs"))?,
            };
            chosen.logical
        }
        Target::ActiveWindow => {
            let windows = background.spawn(async move { compositor.windows() }).await?;
            windows.into_iter().find(|w| w.focused).map(|w| w.rect).ok_or_else(|| anyhow!("no focused window"))?
        }
        Target::Region { rect } => *rect,
        Target::LastRegion => last_region.ok_or_else(|| anyhow!("there is no previous capture region yet"))?,
        Target::AllScreens => bail!("recordings capture one screen at a time"),
    };

    // A recording covers one output: the one showing most of the region.
    let home = home_output(region, &outputs).ok_or_else(|| anyhow!("the region is off screen"))?.clone();
    let region = region.intersection(&home.logical).ok_or_else(|| anyhow!("the region is off screen"))?;
    if region.width < 8.0 || region.height < 8.0 {
        bail!("the region is too small to record");
    }
    cx.update(|cx| Daemon::update(cx, |d, _| d.last_region = Some(region)));

    let path = req.output.clone().unwrap_or_else(|| recording_path(&config));
    let actions = Actions::resolve(&config.recording.after_capture, &req.actions, None, false);
    let countdown = config.recording.countdown;
    let cancelled = Arc::new(AtomicBool::new(false));

    let first_phase = if countdown > 0 { Phase::Countdown(countdown) } else { Phase::Recording };
    let handles = controls::open(region, &home, &outputs, first_phase, cx);
    cx.update(|cx| {
        Daemon::update(cx, |d, _| {
            d.recording = Some(Active {
                path: path.clone(),
                output: home.name.clone(),
                recording: None,
                cancelled: cancelled.clone(),
                controls: handles.clone(),
                actions,
            });
            d.broadcast();
        })
    });

    for n in (1..=countdown).rev() {
        controls::set_phase(&handles, Phase::Countdown(n), cx);
        background.timer(Duration::from_secs(1)).await;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
    }
    controls::set_phase(&handles, Phase::Recording, cx);
    // Let the compositor present a frame without the countdown before capturing.
    background.timer(Duration::from_millis(120)).await;

    let local = region.translate(-home.logical.x, -home.logical.y);
    let whole = Rect::new(0.0, 0.0, home.logical.width, home.logical.height);
    let crop = (local != whole).then_some(local);
    let spec = RecordSpec {
        path: path.clone(),
        framerate: config.recording.framerate,
        quality: config.recording.quality,
        encoder: config.recording.encoder,
        audio: AudioSources { system: audio.system_audio, microphone: audio.microphone },
    };
    let (backend, cursor, output) = (config.advanced.capture_backend, config.recording.show_cursor, home.name.clone());
    let started = background
        .spawn(async move {
            let source = capture.stream(backend, &output, crop, cursor)?;
            anyhow::Ok(Recording::start(source, spec)?)
        })
        .await;

    let recording = match started {
        Ok(recording) => recording,
        Err(e) => {
            if !cancelled.load(Ordering::Relaxed) {
                take_active(cx);
            }
            return Err(e);
        }
    };
    // Hand the pipeline over, unless the user gave up while it was starting.
    let leftover = cx.update(|cx| {
        Daemon::update(cx, |d, _| match &mut d.recording {
            Some(active) if Arc::ptr_eq(&active.cancelled, &cancelled) && !cancelled.load(Ordering::Relaxed) => {
                active.recording = Some(recording);
                d.broadcast();
                None
            }
            _ => Some(recording),
        })
    });
    if let Some(recording) = leftover {
        background.spawn(async move { recording.cancel() }).await;
        return Ok(None);
    }
    cx.spawn(async move |cx| monitor(cancelled, cx).await).detach();
    Ok(Some(path))
}

/// While a recording runs: keep status watchers' timers fresh, and salvage the file if
/// the pipeline breaks (an output unplugged, a full disk).
async fn monitor(cancelled: Arc<AtomicBool>, cx: &mut AsyncApp) {
    loop {
        cx.background_executor().timer(Duration::from_secs(1)).await;
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        let failure = cx.update(|cx| {
            Daemon::update(cx, |d, _| {
                d.broadcast();
                d.recording.as_ref().and_then(|a| a.recording.as_ref()).and_then(|r| r.failure())
            })
        });
        if let Some(failure) = failure {
            tracing::error!("recording broke ({failure}); saving what we have");
            stop(cx).await;
            return;
        }
    }
}
