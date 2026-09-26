//! Daemon-wide state and request routing.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui::{App, AsyncApp, BorrowAppContext, Global};
use screenie_capture::CaptureContext;
use screenie_config::{Config, Paths};
use screenie_core::Rect;
use screenie_ipc::{CaptureKind, LastCapture, Request, Response, State, Status};

use crate::server::Incoming;

pub(crate) struct Daemon {
    pub config: Config,
    pub capture: Arc<CaptureContext>,
    /// Region of the last capture, for `screenie shot last`.
    pub last_region: Option<Rect>,
    /// A selector (or other capture UI) is on screen.
    pub capturing: bool,
    pub recording: Option<crate::recording::Active>,
    /// A stopped recording is being finalized.
    pub saving: bool,
    /// Open editor windows.
    pub editors: u32,
    last_screenshot: Option<LastCapture>,
    last_recording: Option<LastCapture>,
    /// What's remembered between runs (the editor's last style).
    pub state: screenie_state::StateFile,
    commit: &'static str,
    watchers: Vec<async_channel::Sender<Status>>,
    /// What watchers were last told.
    told: Option<Status>,
}

impl Global for Daemon {}

impl Daemon {
    pub fn new(config: Config, capture: Arc<CaptureContext>, commit: &'static str) -> Self {
        Self {
            config,
            capture,
            last_region: None,
            capturing: false,
            recording: None,
            saving: false,
            editors: 0,
            last_screenshot: None,
            last_recording: None,
            state: screenie_state::StateFile::open(),
            commit,
            watchers: Vec::new(),
            told: None,
        }
    }

    pub fn get(cx: &App) -> &Daemon {
        cx.global::<Daemon>()
    }

    /// Change the daemon's state. Status watchers hear of whatever that changes.
    pub fn update<R>(cx: &mut App, f: impl FnOnce(&mut Daemon, &mut App) -> R) -> R {
        cx.update_global(|d: &mut Daemon, cx| {
            let result = f(d, cx);
            d.broadcast();
            result
        })
    }

    pub fn status(&self) -> Status {
        let recording = self.recording.as_ref().map(|a| a.status());
        let state = match (&self.recording, &recording) {
            _ if self.saving => State::Saving,
            (Some(active), _) if active.recording.is_none() => State::Countdown,
            (_, Some(r)) if r.paused => State::Paused,
            (_, Some(_)) => State::Recording,
            _ if self.capturing => State::Selecting,
            _ if self.editors > 0 => State::Editing,
            _ => State::Idle,
        };
        Status {
            state,
            recording,
            editors: self.editors,
            last_screenshot: self.last_screenshot.clone(),
            last_recording: self.last_recording.clone(),
            capturing: self.capturing,
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: self.commit.to_string(),
            build: screenie_ipc::exe_stamp(),
            compositor: self.capture.compositor().name().to_string(),
            capture_backend: self
                .capture
                .backend_name(self.config.advanced.capture_backend)
                .to_string(),
        }
    }

    /// Remember a finished capture (for `screenie last` and status watchers).
    pub fn note_capture(&mut self, kind: CaptureKind, path: Option<PathBuf>) {
        let time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let last = Some(LastCapture { kind, path, time });
        match kind {
            CaptureKind::Screenshot => self.last_screenshot = last,
            CaptureKind::Recording => self.last_recording = last,
        }
        self.broadcast();
    }

    /// Tell status watchers the status, if it changed since they were last told.
    /// [`Daemon::update`] does after every change; state kept elsewhere (a recording's
    /// clock or pause) needs a call when it changes.
    pub fn broadcast(&mut self) {
        self.watchers.retain(|w| !w.is_closed());
        if self.watchers.is_empty() {
            self.told = None;
            return;
        }
        let status = self.status();
        if self.told.as_ref().is_some_and(|told| same(told, &status)) {
            return;
        }
        self.watchers
            .retain(|w| w.try_send(status.clone()).is_ok() || !w.is_closed());
        self.told = Some(status);
    }

    /// Start telling `watcher` the status: now, and whenever it changes.
    fn watch(&mut self, watcher: async_channel::Sender<Status>) {
        // Anyone already watching hears of a change first, so all agree from here on.
        self.broadcast();
        let status = self.status();
        if watcher.try_send(status.clone()).is_ok() {
            self.watchers.push(watcher);
            self.told = Some(status);
        }
    }
}

/// Whether watchers would see nothing new going from `a` to `b`. A recording's clock
/// counts in whole seconds, as they show it.
fn same(a: &Status, b: &Status) -> bool {
    let seconds = |s: &Status| {
        let mut s = s.clone();
        if let Some(r) = &mut s.recording {
            r.elapsed_secs = r.elapsed_secs.floor();
        }
        s
    };
    seconds(a) == seconds(b)
}

/// Route requests from the socket to their handlers. Each runs as its own task so a
/// long-running one (a selector waiting for the user) never blocks `status` or `stop`.
pub(crate) async fn serve(incoming: async_channel::Receiver<Incoming>, cx: &mut AsyncApp) {
    while let Ok(message) = incoming.recv().await {
        match message {
            Incoming::Watch { updates } => {
                cx.update(|cx| Daemon::update(cx, |d, _| d.watch(updates)));
            }
            Incoming::Request { request, reply } => {
                cx.spawn(async move |cx| {
                    let response = handle(request, cx).await;
                    let _ = reply.send(response).await;
                })
                .detach();
            }
        }
    }
}

async fn handle(request: Request, cx: &mut AsyncApp) -> Response {
    match request {
        Request::Ping => Response::Ok,
        Request::Status => Response::Status(Box::new(cx.update(|cx| Daemon::get(cx).status()))),
        Request::Quit => {
            // Never lose a recording to a quit.
            if cx.update(|cx| Daemon::get(cx).recording.is_some()) {
                crate::recording::stop(cx).await;
            }
            cx.update(|cx| cx.quit());
            Response::Ok
        }
        Request::Screenshot(req) => crate::screenshot::take(req, cx).await,
        Request::Record(req) => crate::recording::record(req, cx).await,
        Request::RecordStop => crate::recording::stop(cx).await,
        Request::RecordCancel => crate::recording::cancel(cx).await,
        Request::RecordPause => cx.update(crate::recording::toggle_pause),
        Request::Settings => Response::error("the settings window is not implemented yet"),
        Request::Edit { path } => match crate::editor::open_file(path, cx).await {
            Ok(()) => Response::Ok,
            Err(e) => Response::error(format!("{e:#}")),
        },
        Request::Pin { .. } => Response::error("pinning is not implemented yet"),
        Request::Watch => Response::error("watch is a streaming request"),
    }
}

/// Reload the config when the file changes (from the settings window or by hand).
pub(crate) async fn watch_config(cx: &mut AsyncApp) {
    let path: PathBuf = Paths::get().config_file();
    let mtime = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut last: Option<SystemTime> = mtime(&path);
    loop {
        cx.background_executor().timer(Duration::from_secs(1)).await;
        let now = mtime(&path);
        if now == last {
            continue;
        }
        last = now;
        match Config::load() {
            Ok(config) => {
                tracing::info!("config reloaded");
                cx.update(|cx| {
                    Daemon::update(cx, |d, _| d.config = config);
                    crate::appearance::apply(cx);
                });
            }
            Err(e) => tracing::warn!("{e}; keeping the previous settings"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenie_ipc::RecordingStatus;

    #[test]
    fn watchers_hear_of_changes_they_can_see() {
        let recording = |elapsed_secs| Status {
            state: State::Recording,
            recording: Some(RecordingStatus {
                path: "a.mp4".into(),
                elapsed_secs,
                paused: false,
            }),
            ..Default::default()
        };
        assert!(same(&recording(1.2), &recording(1.8)));
        assert!(!same(&recording(1.9), &recording(2.0)));
        let selecting = Status {
            state: State::Selecting,
            capturing: true,
            ..Default::default()
        };
        assert!(!same(&Status::default(), &selecting));
    }
}
