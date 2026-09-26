//! The configuration schema. Every field has a default, so a config file only needs the
//! settings a user actually changed, and unknown keys are ignored for forward
//! compatibility.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// How big the interface is drawn: buttons, bars, text, cards (never the captures
    /// themselves). `auto` follows the desktop's text scaling (GNOME's "Large Text", or
    /// `org.gnome.desktop.interface text-scaling-factor`, read through the settings
    /// portal), else 1.
    pub ui_scale: UiScale,
    pub screenshot: ScreenshotConfig,
    pub recording: RecordingConfig,
    pub preview: PreviewConfig,
    pub selector: SelectorConfig,
    pub editor: EditorConfig,
    pub advanced: AdvancedConfig,
}

/// `ui_scale`: `auto`, or a factor such as `1.25`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(try_from = "UiScaleRepr", into = "UiScaleRepr")]
pub enum UiScale {
    /// Follow the desktop's text scaling.
    #[default]
    Auto,
    Fixed(f64),
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum UiScaleRepr {
    Factor(f64),
    Word(String),
}

impl TryFrom<UiScaleRepr> for UiScale {
    type Error = String;

    fn try_from(repr: UiScaleRepr) -> Result<Self, String> {
        match repr {
            UiScaleRepr::Factor(f) if f.is_finite() && f > 0.0 => Ok(UiScale::Fixed(f)),
            UiScaleRepr::Word(w) if w == "auto" => Ok(UiScale::Auto),
            _ => Err("expected `auto` or a positive number such as 1.25".into()),
        }
    }
}

impl From<UiScale> for UiScaleRepr {
    fn from(scale: UiScale) -> Self {
        match scale {
            UiScale::Auto => UiScaleRepr::Word("auto".into()),
            UiScale::Fixed(f) => UiScaleRepr::Factor(f),
        }
    }
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
    /// For screenshots: just show the card, which copies, saves or annotates on demand.
    fn default() -> Self {
        Self { copy: false, save: false, preview: true, edit: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenshotConfig {
    /// Where screenshots are saved. Empty means `$XDG_PICTURES_DIR/Screenshots`.
    pub directory: PathBuf,
    /// File name template, without extension: strftime codes, plus `{app}` and
    /// `{title}` for window captures (dropped, with a separator, otherwise).
    pub filename: String,
    /// Include the mouse cursor.
    pub show_cursor: bool,
    pub after_capture: AfterCapture,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            directory: PathBuf::new(),
            filename: "Screenshot_%Y-%m-%d_%H-%M-%S_{app}".into(),
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
            filename: "Recording_%Y-%m-%d_%H-%M-%S_{app}".into(),
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

/// Where on the screen something sits: a corner, or the middle of an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenPosition {
    TopLeft,
    TopMiddle,
    TopRight,
    LeftMiddle,
    RightMiddle,
    BottomLeft,
    BottomMiddle,
    BottomRight,
}

/// Where along one axis: at the start (left/top), in the middle, or at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Start,
    Middle,
    End,
}

impl ScreenPosition {
    pub fn horizontal(self) -> Align {
        match self {
            Self::TopLeft | Self::LeftMiddle | Self::BottomLeft => Align::Start,
            Self::TopMiddle | Self::BottomMiddle => Align::Middle,
            Self::TopRight | Self::RightMiddle | Self::BottomRight => Align::End,
        }
    }

    pub fn vertical(self) -> Align {
        match self {
            Self::TopLeft | Self::TopMiddle | Self::TopRight => Align::Start,
            Self::LeftMiddle | Self::RightMiddle => Align::Middle,
            Self::BottomLeft | Self::BottomMiddle | Self::BottomRight => Align::End,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PreviewConfig {
    /// Where the cards stack: `bottom-right`, `top-middle`, `left-middle`, ...
    pub position: ScreenPosition,
    /// Seconds before the card slides away; 0 keeps it until dismissed.
    pub timeout: u32,
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self { position: ScreenPosition::BottomRight, timeout: 10 }
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
    /// `overlay`: over the screen, the capture where it was taken; `window`: a regular
    /// window.
    pub mode: EditorMode,
    /// Hex colors offered in the palette.
    pub palette: Vec<String>,
    /// The first editor's colour and size. After that the editor remembers the last ones
    /// used (in the state file, across restarts).
    pub default_color: String,
    pub stroke_width: f64,
    /// Close the editor as soon as the image is copied (Ctrl+C or the Copy button).
    pub exit_on_copy: bool,
    /// Close the editor as soon as the image is saved (Ctrl+S, Save or Save As).
    pub exit_on_save: bool,
    /// Ask before closing (Esc, Done with nothing to do, the window's close button) if
    /// the annotations were neither copied nor saved. Off: they're discarded silently.
    pub confirm_discard: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            palette: ["#ff3b30", "#ff9500", "#ffcc00", "#34c759", "#0a84ff", "#af52de", "#ffffff", "#1c1c1e"]
                .map(String::from)
                .to_vec(),
            mode: EditorMode::Overlay,
            default_color: "#ff3b30".into(),
            stroke_width: 4.0,
            exit_on_copy: false,
            exit_on_save: false,
            confirm_discard: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EditorMode {
    #[default]
    Overlay,
    Window,
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
