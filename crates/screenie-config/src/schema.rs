//! The configuration schema. Every field has a default, so a config file only needs the
//! settings a user actually changed, and unknown keys are ignored for forward
//! compatibility.
//!
//! The doc comments are the config's reference: `screenie(5)` is made from them (see
//! [`crate::reference`]), so every key needs one, on the field or on its type.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
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

/// `auto`, or a factor from 0.5 to 3, such as `1.25`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "UiScaleRepr", into = "UiScaleRepr")]
pub enum UiScale {
    /// Follow the desktop's text scaling.
    #[default]
    Auto,
    Fixed(f64),
}

#[derive(Serialize, Deserialize, JsonSchema)]
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

/// What happens automatically once a screenshot is taken.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
        Self {
            copy: false,
            save: false,
            preview: true,
            edit: false,
        }
    }
}

/// Screenshots: where they're saved, what they're called, and what happens once one is
/// taken.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ScreenshotConfig {
    /// Where screenshots are saved. Empty means `$XDG_PICTURES_DIR/Screenshots`. `~` and
    /// `$VAR` expand, and a relative path is relative to the home directory.
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

/// `native`, or at most this many frames a second, from 1 to 240. `native` records every
/// frame shown: a game drawing 280 a second makes the compositor copy 280, which can
/// starve the recording and the game alike. A number asks the compositor for only as
/// many.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "WordOr<u32>", into = "WordOr<u32>")]
pub enum Framerate {
    /// Every frame the screen (or window) shows.
    Native,
    /// At most this many a second.
    Fps(u32),
}

impl Default for Framerate {
    fn default() -> Self {
        Framerate::Fps(60)
    }
}

impl Framerate {
    /// The cap, if any.
    pub fn fps(self) -> Option<u32> {
        match self {
            Framerate::Native => None,
            Framerate::Fps(n) => Some(n),
        }
    }
}

impl TryFrom<WordOr<u32>> for Framerate {
    type Error = String;

    fn try_from(repr: WordOr<u32>) -> Result<Self, String> {
        match repr {
            WordOr::Value(n @ 1..=240) => Ok(Framerate::Fps(n)),
            WordOr::Word(w) if w == "native" => Ok(Framerate::Native),
            _ => Err("expected `native` or frames per second from 1 to 240".into()),
        }
    }
}

impl From<Framerate> for WordOr<u32> {
    fn from(rate: Framerate) -> Self {
        match rate {
            Framerate::Native => WordOr::Word("native".into()),
            Framerate::Fps(n) => WordOr::Value(n),
        }
    }
}

/// `native`, or the most lines the video may have, such as `720p`, `1080p`, `1440p` or
/// `2160p` (also `4k`). It caps the size, keeping the aspect ratio and never scaling up:
/// `1080p` fits a recording into 1920×1080, or 1080×1920 when it's taller than wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub enum Resolution {
    /// The captured pixels, one for one.
    Native,
    Lines(u32),
}

impl Default for Resolution {
    fn default() -> Self {
        Resolution::Lines(1080)
    }
}

impl Resolution {
    /// The largest (width, height) a `width`×`height` capture may be recorded at: a 16:9
    /// box, turned to match the capture's orientation.
    pub fn bounds(self, width: u32, height: u32) -> Option<(u32, u32)> {
        let Resolution::Lines(lines) = self else {
            return None;
        };
        let long = (lines as u64 * 16).div_ceil(9) as u32;
        Some(if width >= height {
            (long, lines)
        } else {
            (lines, long)
        })
    }
}

impl TryFrom<String> for Resolution {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let lines = text.strip_suffix('p').and_then(|n| n.parse::<u32>().ok());
        match (text.as_str(), lines) {
            ("native", _) => Ok(Resolution::Native),
            ("4k", _) => Ok(Resolution::Lines(2160)),
            (_, Some(n @ 144..=4320)) => Ok(Resolution::Lines(n)),
            _ => Err(
                "expected `native` or a size such as `720p`, `1080p`, `1440p` or `2160p`".into(),
            ),
        }
    }
}

impl From<Resolution> for String {
    fn from(resolution: Resolution) -> Self {
        match resolution {
            Resolution::Native => "native".into(),
            Resolution::Lines(n) => format!("{n}p"),
        }
    }
}

/// A setting that's a keyword or a value, as written in the file.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
enum WordOr<T> {
    Value(T),
    Word(String),
}

/// How good a recording looks against how big it is. `lossless` is visually lossless,
/// in far bigger files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Quality {
    Low,
    Medium,
    High,
    Lossless,
}

/// Which video encoder records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EncoderPreference {
    /// Hardware if it works, software otherwise.
    Auto,
    /// The GPU's (VA-API or NVENC), or none.
    Hardware,
    /// x264 or OpenH264, on the CPU.
    Software,
}

/// Recordings: where they're saved, what they're called, and how they're encoded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RecordingConfig {
    /// Where recordings are saved. Empty means `$XDG_VIDEOS_DIR/Screencasts`. Written
    /// like `screenshot.directory`.
    pub directory: PathBuf,
    /// File name template, without extension. Written like `screenshot.filename`.
    pub filename: String,
    pub framerate: Framerate,
    /// The most the video's size may be: recording a 4K screen at `1080p` makes a
    /// quarter of the pixels to encode and store.
    pub resolution: Resolution,
    pub quality: Quality,
    pub encoder: EncoderPreference,
    /// Include the mouse cursor.
    pub show_cursor: bool,
    /// Record what the speakers play.
    pub system_audio: bool,
    /// Record the default microphone.
    pub microphone: bool,
    /// Seconds of countdown before recording starts; 0 disables it.
    pub countdown: u32,
    pub after_capture: RecordingAfterCapture,
}

/// What happens automatically once a recording is finished. It's always saved (it's
/// written as it's recorded), and there's no video editor, so only these two apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RecordingAfterCapture {
    /// Put the file on the clipboard.
    pub copy: bool,
    /// Show the floating preview card.
    pub preview: bool,
}

impl Default for RecordingAfterCapture {
    fn default() -> Self {
        Self {
            copy: false,
            preview: true,
        }
    }
}

impl From<&RecordingAfterCapture> for AfterCapture {
    fn from(recording: &RecordingAfterCapture) -> Self {
        Self {
            copy: recording.copy,
            save: true,
            preview: recording.preview,
            edit: false,
        }
    }
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            directory: PathBuf::new(),
            filename: "Recording_%Y-%m-%d_%H-%M-%S_{app}".into(),
            framerate: Framerate::default(),
            resolution: Resolution::default(),
            quality: Quality::High,
            encoder: EncoderPreference::Auto,
            show_cursor: true,
            system_audio: false,
            microphone: false,
            countdown: 3,
            after_capture: RecordingAfterCapture::default(),
        }
    }
}

/// Where on the screen something sits: a corner, or the middle of an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

/// The preview cards that appear after a capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct PreviewConfig {
    /// Where the cards stack.
    pub position: ScreenPosition,
    /// Seconds before the card slides away; 0 keeps it until dismissed.
    pub timeout: u32,
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self {
            position: ScreenPosition::BottomRight,
            timeout: 10,
        }
    }
}

/// The overlay that picks an area, window or screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SelectorConfig {
    /// Show the pixel magnifier next to the cursor.
    pub magnifier: bool,
    /// Take the screenshot as soon as the drag ends. When off, the selection stays
    /// adjustable until confirmed with Enter or the capture button.
    pub capture_on_release: bool,
    /// In area mode, highlight the window under the cursor and pick it with a click
    /// (needs compositor IPC). Window mode always does.
    pub window_snapping: bool,
    /// Show the mode toolbar at the bottom of the screen.
    pub toolbar: bool,
    /// Strength of the dimming outside the selection, 0.0–1.0.
    pub dim: f64,
}

impl Default for SelectorConfig {
    fn default() -> Self {
        Self {
            magnifier: true,
            capture_on_release: true,
            window_snapping: true,
            toolbar: true,
            dim: 0.45,
        }
    }
}

/// The annotation editor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct EditorConfig {
    /// Where the editor opens.
    pub mode: EditorMode,
    /// Hex colors offered in the palette.
    pub palette: Vec<String>,
    /// The first editor's colour. After that the editor remembers the last one used (in
    /// the state file, across restarts).
    pub default_color: String,
    /// The first editor's stroke width, remembered like `default_color`.
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
            palette: [
                "#ff3b30", "#ff9500", "#ffcc00", "#34c759", "#0a84ff", "#af52de", "#ffffff",
                "#1c1c1e",
            ]
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EditorMode {
    /// Over the screen, with the capture where it was taken.
    #[default]
    Overlay,
    /// In a window of its own.
    Window,
}

/// The Wayland protocol that captures the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureBackend {
    /// Whichever the compositor offers, trying the other when one fails.
    Auto,
    /// `ext-image-copy-capture-v1`
    Ext,
    /// `wlr-screencopy-unstable-v1`
    Wlr,
    /// xdg-desktop-portal. Planned, and left out of the docs until it captures.
    #[schemars(skip)]
    Portal,
}

/// For troubleshooting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AdvancedConfig {
    pub capture_backend: CaptureBackend,
}

impl Default for AdvancedConfig {
    fn default() -> Self {
        Self {
            capture_backend: CaptureBackend::Auto,
        }
    }
}
