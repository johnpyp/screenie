//! The configuration schema. Every field has a default, so a config file only needs the
//! settings a user actually changed, and unknown keys are ignored for forward
//! compatibility.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub screenshot: ScreenshotConfig,
    pub recording: RecordingConfig,
    pub preview: PreviewConfig,
    pub selector: SelectorConfig,
    pub editor: EditorConfig,
    pub advanced: AdvancedConfig,
}

/// What happens automatically once a capture is taken.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AfterCapture {
    /// Put the capture on the clipboard.
    pub copy: bool,
    /// Write the capture to the save directory.
    pub save: bool,
    /// Show the floating preview card.
    pub preview: bool,
    /// Open the capture straight in the editor.
    pub edit: bool,
}

impl Default for AfterCapture {
    fn default() -> Self {
        Self { copy: true, save: true, preview: true, edit: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenshotConfig {
    /// Where screenshots are saved. Empty means `$XDG_PICTURES_DIR/Screenshots`.
    pub directory: PathBuf,
    /// strftime template for file names, without extension.
    pub filename: String,
    /// Include the mouse cursor.
    pub show_cursor: bool,
    pub after_capture: AfterCapture,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            directory: PathBuf::new(),
            filename: "Screenshot_%Y-%m-%d_%H-%M-%S".into(),
            show_cursor: false,
            after_capture: AfterCapture::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Quality {
    Low,
    Medium,
    High,
    Lossless,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EncoderPreference {
    /// Hardware if it works, software otherwise.
    Auto,
    Hardware,
    Software,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingConfig {
    /// Where recordings are saved. Empty means `$XDG_VIDEOS_DIR/Screencasts`.
    pub directory: PathBuf,
    pub filename: String,
    pub framerate: u32,
    pub quality: Quality,
    pub encoder: EncoderPreference,
    pub show_cursor: bool,
    /// Record what the speakers play.
    pub system_audio: bool,
    /// Record the default microphone.
    pub microphone: bool,
    /// Seconds of countdown before recording starts; 0 disables it.
    pub countdown: u32,
    pub after_capture: AfterCapture,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            directory: PathBuf::new(),
            filename: "Recording_%Y-%m-%d_%H-%M-%S".into(),
            framerate: 60,
            quality: Quality::High,
            encoder: EncoderPreference::Auto,
            show_cursor: true,
            system_audio: false,
            microphone: false,
            countdown: 3,
            after_capture: AfterCapture { copy: false, save: true, preview: true, edit: false },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub fn is_left(self) -> bool {
        matches!(self, Self::TopLeft | Self::BottomLeft)
    }

    pub fn is_top(self) -> bool {
        matches!(self, Self::TopLeft | Self::TopRight)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PreviewConfig {
    pub corner: Corner,
    /// Seconds before the card slides away; 0 keeps it until dismissed.
    pub timeout: u32,
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self { corner: Corner::BottomRight, timeout: 6 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SelectorConfig {
    /// Freeze the screen while selecting a screenshot region.
    pub freeze: bool,
    /// Show the pixel magnifier next to the cursor.
    pub magnifier: bool,
    /// Take the screenshot as soon as the drag ends. When off, the selection stays
    /// adjustable until confirmed with Enter or the capture button.
    pub capture_on_release: bool,
    /// Highlight and snap to windows under the cursor (needs compositor IPC).
    pub window_snapping: bool,
    /// Show the mode toolbar at the bottom of the screen.
    pub toolbar: bool,
    /// Strength of the dimming outside the selection, 0.0–1.0.
    pub dim: f64,
}

impl Default for SelectorConfig {
    fn default() -> Self {
        Self {
            freeze: true,
            magnifier: true,
            capture_on_release: true,
            window_snapping: true,
            toolbar: true,
            dim: 0.45,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    /// Hex colors offered in the palette.
    pub palette: Vec<String>,
    pub default_color: String,
    pub stroke_width: f64,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            palette: ["#ff3b30", "#ff9500", "#ffcc00", "#34c759", "#0a84ff", "#af52de", "#ffffff", "#1c1c1e"]
                .map(String::from)
                .to_vec(),
            default_color: "#ff3b30".into(),
            stroke_width: 4.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureBackend {
    Auto,
    /// `ext-image-copy-capture-v1`
    Ext,
    /// `wlr-screencopy-unstable-v1`
    Wlr,
    /// xdg-desktop-portal
    Portal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvancedConfig {
    pub capture_backend: CaptureBackend,
    /// Seconds without requests after which the daemon exits; 0 keeps it resident.
    pub daemon_idle_exit: u32,
}

impl Default for AdvancedConfig {
    fn default() -> Self {
        Self { capture_backend: CaptureBackend::Auto, daemon_idle_exit: 0 }
    }
}
