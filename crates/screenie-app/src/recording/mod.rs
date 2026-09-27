//! The recording flow: pick → countdown → record → stop → deliver.
//!
//! The daemon holds at most one [`Active`] recording. It exists from the end of the
//! countdown's first tick (so `screenie record stop` can cancel a countdown) until it's stopped.

mod controls;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use gpui::{App, AsyncApp, WindowHandle};
use screenie_config::{Config, Subject, expand_template, unique_path};
use screenie_core::{OutputInfo, Rect, WindowInfo};
use screenie_ipc::{RecordRequest, RecordingStatus, Response, Target};
use screenie_record::{AudioSources, Encoder, RecordSpec, Recording};
use screenie_selector::{Backdrop, Purpose, RecordOptions, Selection, SelectorConfig};
use screenie_ui_kit::conceal::{self, Concealed, Scope};

use crate::daemon::Daemon;
use crate::deliver::{self, Actions};
use crate::preview::{self, PreviewItem};
use controls::{Chrome, Controls, Phase, Scene};

pub(crate) struct Active {
    pub path: PathBuf,
    output: String,
    /// `None` until the countdown is over and the pipeline is running.
    pub recording: Option<Recording>,
    /// Set when stopped or cancelled, so a start still in flight backs out.
    cancelled: Arc<AtomicBool>,
    /// The chrome on screen, if it shows anything.
    controls: Option<WindowHandle<Controls>>,
    scene: Scene,
    /// Whether a window fills the recorded output (a fullscreen app), so the chrome
    /// keeps off it.
    covered: bool,
    actions: Actions,
    /// Preview cards kept out of a recorded screen until its capture stops.
    previews: Option<Concealed>,
}

impl Active {
    pub fn status(&self) -> RecordingStatus {
        let (elapsed, paused) = self
            .recording
            .as_ref()
            .map_or((Duration::ZERO, false), |r| (r.elapsed(), r.is_paused()));
        RecordingStatus {
            path: self.path.clone(),
            elapsed_secs: elapsed.as_secs_f64(),
            paused,
        }
    }
}

/// `screenie record`: start, or stop the running recording when toggling.
pub(crate) async fn record(req: RecordRequest, cx: &mut AsyncApp) -> Response {
    if cx.update(|cx| Daemon::get(cx).recording.is_some()) {
        return if req.toggle {
            stop(cx).await
        } else {
            Response::error("already recording")
        };
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

/// Stop and save. During a countdown, or while the recording is starting (nothing is
/// recorded yet), this cancels instead.
pub(crate) async fn stop(cx: &mut AsyncApp) -> Response {
    // Saving from the moment the recording is taken, so watchers never see a gap.
    let set_saving = |saving: bool, cx: &mut AsyncApp| {
        cx.update(|cx| {
            Daemon::update(cx, |d, _| {
                d.saving = saving;
                d.broadcast();
            })
        })
    };
    let running = cx.update(|cx| {
        Daemon::get(cx)
            .recording
            .as_ref()
            .is_some_and(|a| a.recording.is_some())
    });
    if running {
        set_saving(true, cx);
    }
    let Some(active) = take_active(cx) else {
        // Another stop may be saving: its flag isn't ours to clear.
        if running {
            set_saving(false, cx);
        }
        return Response::error("nothing is being recorded");
    };
    let Some(recording) = active.recording else {
        if running {
            set_saving(false, cx);
        }
        return Response::Cancelled;
    };
    // The video ends now, and its card shows (with a spinner) while the file is finished
    // rather than after: that can take a moment. In that order, so the card can't end up
    // in the video.
    let background = cx.background_executor().clone();
    let recording = background
        .spawn(async move {
            let mut recording = recording;
            recording.stop_capture();
            recording
        })
        .await;
    // Nothing more is captured: the cards hidden from it come back.
    drop(active.previews);
    let card = if active.actions.preview {
        let (frame, size, duration) = (
            recording.latest_frame(),
            recording.size(),
            recording.elapsed(),
        );
        let item = PreviewItem::recording_saving(frame, size, duration, cx).await;
        let id = item.id();
        let output = active.output.clone();
        cx.update(|cx| preview::show(item, Some(output), cx));
        Some(id)
    } else {
        None
    };
    let finished = background.spawn(async move { recording.stop() }).await;
    set_saving(false, cx);
    match finished {
        Ok(finished) => {
            let path = finished.path.clone();
            deliver::recording(finished, active.actions, Some(active.output), card, cx).await;
            Response::Captured {
                path: Some(path),
                temporary: false,
            }
        }
        Err(e) => {
            tracing::error!("finishing the recording failed: {e}");
            if let Some(card) = card {
                cx.update(|cx| preview::discard(card, cx));
            }
            Response::error(format!("finishing the recording failed: {e}"))
        }
    }
}

/// Stop and delete. Discarding is what was asked for, so it's a success.
pub(crate) async fn cancel(cx: &mut AsyncApp) -> Response {
    let Some(active) = take_active(cx) else {
        return Response::error("nothing is being recorded");
    };
    if let Some(recording) = active.recording {
        cx.background_executor()
            .spawn(async move { recording.cancel() })
            .await;
    }
    Response::Ok
}

pub(crate) fn toggle_pause(cx: &mut App) -> Response {
    let result = Daemon::update(cx, |d, _| {
        let recording = d
            .recording
            .as_ref()
            .and_then(|a| a.recording.as_ref())
            .ok_or("nothing is being recorded")?;
        recording
            .set_paused(!recording.is_paused())
            .map_err(|_| "cannot pause this recording")?;
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
        controls::close(active.controls, cx);
        Some(active)
    })
}

/// Where a new recording goes.
fn recording_path(config: &Config, subject: &Subject) -> PathBuf {
    let stem = expand_template(&config.recording.filename, chrono::Local::now(), subject);
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
    crate::screenshot::leave_overview(cx).await;
    let (config, capture, last_region) = cx.update(|cx| {
        let d = Daemon::get(cx);
        (d.config.clone(), d.capture.clone(), d.last_region())
    });
    let background = cx.background_executor().clone();
    // Probe the encoders while the user picks, so the recording starts as the countdown
    // ends. Only the first recording of a daemon's life has any to do.
    {
        let (capture, preference) = (capture.clone(), config.recording.encoder);
        background
            .spawn(async move { Encoder::warm_up(preference, capture.gpu().as_ref()) })
            .detach();
    }
    let compositor = capture.compositor().clone();
    let outputs = {
        let capture = capture.clone();
        background
            .spawn(async move { capture.outputs() })
            .await
            .context("listing outputs")?
    };
    let focused = {
        let capture = capture.clone();
        background
            .spawn(async move { capture.focused_output() })
            .await
    };
    let mut audio = RecordOptions {
        system_audio: req.system_audio.unwrap_or(config.recording.system_audio),
        microphone: req.microphone.unwrap_or(config.recording.microphone),
    };

    let mut window = None;
    let mut focused_window = false;
    let region = match &req.target {
        Target::Select { mode } => {
            let windows = {
                let compositor = compositor.clone();
                background
                    .spawn(async move { compositor.windows().unwrap_or_default() })
                    .await
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
                hand_off: false,
            };
            let backdrop = Backdrop::Live {
                outputs: outputs.clone(),
                windows,
            };
            let Some(choice) = screenie_selector::select(cx, backdrop, selector).await else {
                return Ok(None);
            };
            audio = choice.record;
            match choice.selection {
                Selection::Region(r) => r,
                Selection::Window(w) => window.insert(w).rect,
                Selection::Output(o) => o.logical,
            }
        }
        Target::Screen { output } => {
            let name = output.clone().or(focused);
            let chosen = match &name {
                Some(n) => outputs
                    .iter()
                    .find(|o| &o.name == n)
                    .ok_or_else(|| anyhow!("no output named {n}"))?,
                None => outputs.first().ok_or_else(|| anyhow!("no outputs"))?,
            };
            chosen.logical
        }
        // Where no windows are listed but the focused one can be recorded by itself
        // (GNOME), it's recorded without knowing which or where it is: its screen stands
        // in for its place.
        Target::ActiveWindow
            if !compositor.lists_windows()
                && capture.captures_focused_window(config.advanced.capture_backend) =>
        {
            focused_window = true;
            let chosen = outputs
                .iter()
                .find(|o| Some(&o.name) == focused.as_ref())
                .or(outputs.first())
                .ok_or_else(|| anyhow!("no outputs"))?;
            chosen.logical
        }
        Target::ActiveWindow => {
            let compositor = compositor.clone();
            let windows = background
                .spawn(async move { compositor.windows() })
                .await?;
            let focused = windows
                .into_iter()
                .find(|w| w.focused)
                .ok_or_else(|| anyhow!("no focused window"))?;
            window.insert(focused).rect
        }
        Target::Region { rect } => *rect,
        Target::LastRegion => {
            last_region.ok_or_else(|| anyhow!("there is no previous capture region yet"))?
        }
        Target::AllScreens => bail!("recordings capture one screen at a time"),
    };

    // A recording covers one output: the one showing most of the region.
    let home = home_output(region, &outputs)
        .ok_or_else(|| anyhow!("the region is off screen"))?
        .clone();
    let region = region
        .intersection(&home.logical)
        .ok_or_else(|| anyhow!("the region is off screen"))?;
    if region.width < 8.0 || region.height < 8.0 {
        bail!("the region is too small to record");
    }
    cx.update(|cx| Daemon::update(cx, |d, _| d.remember_region(region)));

    // A window is recorded by itself where the compositor allows: the recording follows
    // it and shows nothing that covers it. Opened now, so a failure shows before the
    // countdown. Elsewhere it's recorded as the part of the screen it covers.
    let (backend, cursor) = (
        config.advanced.capture_backend,
        config.recording.show_cursor,
    );
    let window_source = match &window {
        None if focused_window => {
            let capture = capture.clone();
            let stream = background
                .spawn(async move { capture.stream_focused_window(cursor) })
                .await
                .context("recording the focused window")?;
            Some(stream)
        }
        Some(w) if capture.can_stream_window(backend) => {
            let (capture, w) = (capture.clone(), w.clone());
            match background
                .spawn(async move { capture.stream_window(&w, cursor) })
                .await
            {
                Ok(source) => Some(source),
                Err(e) => {
                    tracing::warn!("recording the window's part of the screen instead: {e}");
                    None
                }
            }
        }
        _ => None,
    };

    let subject = window
        .as_ref()
        .map(|w| Subject {
            app: Some(w.app_id.clone()),
            title: Some(w.title.clone()),
        })
        .unwrap_or_default();
    let path = req
        .output
        .clone()
        .unwrap_or_else(|| recording_path(&config, &subject));
    let actions = Actions::resolve(
        &(&config.recording.after_capture).into(),
        &req.actions,
        None,
        false,
    );
    // Without layer-shell there's no chrome to count down in, so it starts at once (GNOME
    // shows it's recording in its top bar, whose button stops it).
    let countdown = if capture.support().layer_shell {
        config.recording.countdown
    } else {
        0
    };
    let cancelled = Arc::new(AtomicBool::new(false));

    let first_phase = if countdown > 0 {
        Phase::Countdown(countdown)
    } else {
        Phase::Starting
    };
    let scene = Scene {
        region,
        chrome: if window_source.is_some() {
            Chrome::Window
        } else {
            Chrome::Region
        },
        home: home.clone(),
    };
    // A fullscreen app (a window filling the output) keeps its output to itself.
    let covered = {
        let (compositor, home) = (compositor.clone(), home.clone());
        let windows = background
            .spawn(async move { compositor.windows().unwrap_or_default() })
            .await;
        windows.iter().any(|w| controls::fills(w.rect, &home))
    };
    let handles = controls::open(&scene, covered, first_phase, cx);
    cx.update(|cx| {
        Daemon::update(cx, |d, _| {
            d.recording = Some(Active {
                path: path.clone(),
                output: home.name.clone(),
                recording: None,
                cancelled: cancelled.clone(),
                controls: handles,
                scene,
                covered,
                actions,
                previews: None,
            });
            d.broadcast();
        })
    });

    for n in (1..=countdown).rev() {
        controls::set_phase(handles, Phase::Countdown(n), cx);
        background.timer(Duration::from_secs(1)).await;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
    }
    controls::set_phase(handles, Phase::Starting, cx);
    // Let the compositor present a frame without the countdown before capturing.
    background.timer(Duration::from_millis(120)).await;

    let local = region.translate(-home.logical.x, -home.logical.y);
    let whole = Rect::new(0.0, 0.0, home.logical.width, home.logical.height);
    let crop = (local != whole).then_some(local);
    let spec = RecordSpec {
        path: path.clone(),
        framerate: config.recording.framerate,
        resolution: config.recording.resolution,
        quality: config.recording.quality,
        encoder: config.recording.encoder,
        audio: AudioSources {
            system: audio.system_audio,
            microphone: audio.microphone,
        },
    };
    // Cards on the recorded screen (from the last clip, or a screenshot taken meanwhile)
    // would be in the video: they wait, hidden, until it stops. A window recorded by
    // itself can't show them.
    let previews = if window_source.is_none() {
        let cards = Scope::only(preview::NAMESPACE).on(&home.name);
        Some(conceal::conceal(cards, cx).await)
    } else {
        None
    };
    let output = home.name.clone();
    let started = background
        .spawn(async move {
            let source = match window_source {
                Some(source) => source,
                None => capture.stream(backend, &output, crop, cursor)?,
            };
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
            Some(active)
                if Arc::ptr_eq(&active.cancelled, &cancelled)
                    && !cancelled.load(Ordering::Relaxed) =>
            {
                active.recording = Some(recording);
                active.previews = previews;
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
    controls::set_phase(handles, Phase::Recording, cx);
    cx.spawn(async move |cx| monitor(cancelled, cx).await)
        .detach();
    Ok(Some(path))
}

/// While a recording runs: keep status watchers' timers fresh, keep the chrome off a
/// fullscreen app, stop when the recorded window closes, and salvage the file if the
/// pipeline breaks (a full disk).
async fn monitor(cancelled: Arc<AtomicBool>, cx: &mut AsyncApp) {
    let compositor = cx.update(|cx| Daemon::get(cx).capture.compositor().clone());
    loop {
        cx.background_executor().timer(Duration::from_secs(1)).await;
        let windows = {
            let compositor = compositor.clone();
            cx.background_executor()
                .spawn(async move { compositor.windows() })
                .await
        };
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(windows) = windows {
            follow_fullscreen(&windows, cx);
        }
        let (ended, failure) = cx.update(|cx| {
            Daemon::update(cx, |d, _| {
                d.broadcast();
                let recording = d.recording.as_ref().and_then(|a| a.recording.as_ref());
                (
                    recording.is_some_and(|r| r.ended()),
                    recording.and_then(|r| r.failure()),
                )
            })
        });
        if ended && failure.is_none() {
            tracing::info!("what was recorded went away; saving");
            stop(cx).await;
            return;
        }
        if let Some(failure) = failure {
            tracing::error!("recording broke ({failure}); saving what we have");
            stop(cx).await;
            return;
        }
    }
}

/// Hide the chrome when a window comes to fill the recorded output (a game going
/// fullscreen), and bring it back when none does: nothing may stay over a fullscreen
/// app, even for part of a recording.
fn follow_fullscreen(windows: &[WindowInfo], cx: &mut AsyncApp) {
    let running = cx.update(|cx| {
        Daemon::get(cx)
            .recording
            .as_ref()
            .filter(|a| a.recording.is_some())
            .map(|a| (a.scene.clone(), a.covered, a.controls))
    });
    let Some((scene, was, handle)) = running else {
        return;
    };
    let covered = windows.iter().any(|w| controls::fills(w.rect, &scene.home));
    if covered == was {
        return;
    }
    tracing::info!(
        covered,
        "a window {} the recorded output",
        if covered { "fills" } else { "no longer fills" }
    );
    let handle = controls::set_covered(handle, &scene, covered, Phase::Recording, cx);
    cx.update(|cx| {
        Daemon::update(cx, |d, _| {
            if let Some(active) = d.recording.as_mut() {
                active.covered = covered;
                active.controls = handle;
            }
        })
    });
}
