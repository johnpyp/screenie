//! Messages exchanged between the `screenie` CLI and the daemon.
//!
//! Each connection carries exactly one [`Request`] and one [`Response`] (except
//! [`Request::Watch`], which streams [`Status`] updates), as newline-delimited JSON. The
//! request may take as long as the user needs, e.g. while selecting a region.

use std::path::PathBuf;

use screenie_core::Rect;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Request {
    Screenshot(ScreenshotRequest),
    Record(RecordRequest),
    /// Stop the active recording (saving it).
    RecordStop,
    /// Stop and throw away the active recording.
    RecordCancel,
    RecordPause,
    /// Open the settings window.
    Settings,
    /// Open an existing image in the editor.
    Edit { path: PathBuf },
    /// Pin an image file to the screen.
    Pin { path: PathBuf },
    Status,
    /// Stream status updates, one line per change (for status bars).
    Watch,
    Ping,
    Quit,
}

/// What to capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Target {
    /// Show the selector; the user picks.
    Select { mode: SelectMode },
    /// A whole output. `None` means the focused one (or the one under the cursor).
    Screen { output: Option<String> },
    /// Every output, as one image.
    AllScreens,
    /// The focused window.
    ActiveWindow,
    /// A fixed logical region.
    Region { rect: Rect },
    /// The region used by the previous capture.
    LastRegion,
}

/// The selector's initial mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectMode {
    #[default]
    Area,
    Window,
    Screen,
}

/// Per-invocation overrides of the configured after-capture actions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionOverrides {
    pub copy: Option<bool>,
    pub save: Option<bool>,
    pub preview: Option<bool>,
    pub edit: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenshotRequest {
    pub target: Target,
    /// Seconds to wait before capturing.
    #[serde(default)]
    pub delay: u32,
    #[serde(default)]
    pub actions: ActionOverrides,
    /// Save to exactly this path instead of the configured directory.
    #[serde(default)]
    pub output: Option<PathBuf>,
    /// The client wants the PNG bytes (e.g. to write to stdout): make sure a file exists
    /// even if saving is off, and report it as temporary.
    #[serde(default)]
    pub want_file: bool,
    #[serde(default)]
    pub cursor: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordRequest {
    pub target: Target,
    #[serde(default)]
    pub system_audio: Option<bool>,
    #[serde(default)]
    pub microphone: Option<bool>,
    #[serde(default)]
    pub output: Option<PathBuf>,
    #[serde(default)]
    pub actions: ActionOverrides,
    /// If a recording is already running, stop it instead of failing.
    #[serde(default)]
    pub toggle: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Response {
    Ok,
    /// A screenshot or recording was produced.
    Captured {
        path: Option<PathBuf>,
        /// The file is a temporary the client should delete after use.
        #[serde(default)]
        temporary: bool,
    },
    RecordingStarted { path: PathBuf },
    Cancelled,
    Status(Box<Status>),
    Error { message: String },
}

impl Response {
    pub fn error(message: impl std::fmt::Display) -> Self {
        Response::Error { message: message.to_string() }
    }
}

/// What the daemon is doing, most important first when several apply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Idle,
    /// An annotation editor is open.
    Editing,
    /// A selector is on screen.
    Selecting,
    /// A recording is counting down.
    Countdown,
    Recording,
    Paused,
    /// A recording is being finalized.
    Saving,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Editing => "editing",
            State::Selecting => "selecting",
            State::Countdown => "countdown",
            State::Recording => "recording",
            State::Paused => "paused",
            State::Saving => "saving",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    Screenshot,
    Recording,
}

impl CaptureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureKind::Screenshot => "screenshot",
            CaptureKind::Recording => "recording",
        }
    }
}

/// The most recent capture of a kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastCapture {
    pub kind: CaptureKind,
    /// Where it was saved; `None` if it was only copied.
    pub path: Option<PathBuf>,
    /// When it was taken, in Unix seconds.
    pub time: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// The one-word summary, for status bars.
    #[serde(default)]
    pub state: State,
    pub recording: Option<RecordingStatus>,
    /// Open annotation editors.
    #[serde(default)]
    pub editors: u32,
    #[serde(default)]
    pub last_screenshot: Option<LastCapture>,
    #[serde(default)]
    pub last_recording: Option<LastCapture>,
    /// A selector or other capture UI is on screen.
    #[serde(default)]
    pub capturing: bool,
    pub pid: u32,
    pub version: String,
    /// Git commit and commit time the daemon was built from.
    #[serde(default)]
    pub commit: String,
    /// Identity of the daemon's executable (see [`crate::exe_stamp`]).
    #[serde(default)]
    pub build: String,
    pub compositor: String,
    pub capture_backend: String,
}

impl Status {
    /// The latest capture of `kind`, or of either kind.
    pub fn last(&self, kind: Option<CaptureKind>) -> Option<&LastCapture> {
        let (shot, rec) = (self.last_screenshot.as_ref(), self.last_recording.as_ref());
        match kind {
            Some(CaptureKind::Screenshot) => shot,
            Some(CaptureKind::Recording) => rec,
            None => [shot, rec].into_iter().flatten().max_by_key(|c| c.time),
        }
    }

    /// Whether restarting the daemon now would interrupt the user.
    pub fn busy(&self) -> bool {
        self.recording.is_some() || self.capturing || self.editors > 0 || self.state == State::Saving
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingStatus {
    pub path: PathBuf,
    pub elapsed_secs: f64,
    pub paused: bool,
}
