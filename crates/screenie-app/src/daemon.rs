//! Daemon-wide state and request routing.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui::{App, AsyncApp, BorrowAppContext, Global};
use screenie_capture::CaptureContext;
use screenie_config::{Config, Paths};
use screenie_core::Rect;
use screenie_ipc::{CaptureKind, Request, Response, State, Status};

use crate::last::{CaptureId, LastCaptures};
use crate::server::Incoming;

pub(crate) struct Daemon {
    pub config: Config,
    pub capture: Arc<CaptureContext>,
    /// A selector (or other capture UI) is on screen.
    pub capturing: bool,
    /// The screenshot selector on screen, to hand a second request to.
    pub selector: Option<crate::screenshot::Selecting>,
    pub recording: Option<crate::recording::Active>,
    /// A stopped recording is being finalized.
    pub saving: bool,
    /// Open editor windows.
    pub editors: u32,
    /// The latest captures, kept in `state` too.
    last: LastCaptures,
    /// What's remembered between runs: the editor's last style, and the last captures
    /// (so `shot last` and `query last` survive a restart, above all an automatic one).
    pub state: screenie_state::StateFile,
    commit: &'static str,
    watchers: Vec<async_channel::Sender<Status>>,
    /// What watchers were last told.
    told: Option<Status>,
}

impl Global for Daemon {}

impl Daemon {
    pub fn new(config: Config, capture: Arc<CaptureContext>, commit: &'static str) -> Self {
        let state = screenie_state::StateFile::open();
        Self {
            config,
            capture,
            capturing: false,
            selector: None,
            recording: None,
            saving: false,
            editors: 0,
            last: LastCaptures::restore(&state.state().last),
            state,
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
            last_screenshot: self.last.get(CaptureKind::Screenshot).cloned(),
            last_recording: self.last.get(CaptureKind::Recording).cloned(),
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

    /// Remember a finished capture (for `screenie query last` and status watchers).
    pub fn note_capture(&mut self, kind: CaptureKind, path: Option<PathBuf>) -> CaptureId {
        let id = self.last.note(kind, path, unix_now());
        self.captures_changed();
        id
    }

    /// Capture `id` was saved to `path` later, from its preview card.
    pub fn note_saved(&mut self, id: CaptureId, kind: CaptureKind, path: PathBuf) {
        self.last.saved(id, kind, path, unix_now());
        self.captures_changed();
    }

    /// The file at `path` was deleted, from its preview card.
    pub fn note_deleted(&mut self, path: &Path) {
        if self.last.deleted(path) {
            self.captures_changed();
        }
    }

    /// Remember the latest captures across runs, and tell watchers.
    fn captures_changed(&mut self) {
        if let Err(e) = self.state.update(|s| self.last.persist(&mut s.last)) {
            tracing::warn!("{e}");
        }
        self.broadcast();
    }

    /// The region of the latest capture, for `screenie shot last` / `record last`.
    pub fn last_region(&self) -> Option<Rect> {
        let r = self.state.state().last.region?;
        Some(Rect::new(r.x, r.y, r.width, r.height))
    }

    pub fn remember_region(&mut self, rect: Rect) {
        let region = screenie_state::Region {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        };
        self.remember(|last| last.region = Some(region));
    }

    fn remember(&mut self, change: impl FnOnce(&mut screenie_state::LastState)) {
        if let Err(e) = self.state.update(|s| change(&mut s.last)) {
            tracing::warn!("{e}");
        }
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

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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
        Request::Edit { path } => match crate::editor::open_file(path, cx).await {
            Ok(()) => Response::Ok,
            Err(e) => Response::error(format!("{e:#}")),
        },
        Request::Watch => Response::error("watch is a streaming request"),
    }
}

/// Reload the config when the file changes (from the settings window or by hand).
///
/// It compares what the file says, not when it was modified: a config managed by
/// home-manager is a symlink into the Nix store, where every file's mtime is the epoch,
/// so switching generations changes the link's target and nothing else. The file is
/// small enough to read every second.
pub(crate) async fn watch_config(cx: &mut AsyncApp) {
    let path: PathBuf = Paths::get().config_file();
    let background = cx.background_executor().clone();
    let read = || {
        let path = path.clone();
        background.spawn(async move { std::fs::read(path).ok() })
    };
    let mut last = read().await;
    loop {
        background.timer(Duration::from_secs(1)).await;
        let now = read().await;
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
