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
    Status(Status),
    Error { message: String },
}

impl Response {
    pub fn error(message: impl std::fmt::Display) -> Self {
        Response::Error { message: message.to_string() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub recording: Option<RecordingStatus>,
    pub pid: u32,
    pub version: String,
    pub compositor: String,
    pub capture_backend: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingStatus {
    pub path: PathBuf,
    pub elapsed_secs: f64,
    pub paused: bool,
}
