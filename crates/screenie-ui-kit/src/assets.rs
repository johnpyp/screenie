//! Bundled assets: icons (Lucide plus a few of our own, in the same style) and the Inter
//! font family, so the UI looks identical regardless of the user's system themes.

use std::borrow::Cow;

use gpui::{App, AssetSource, IntoElement, SharedString, Styled, Svg, svg};

#[derive(rust_embed::RustEmbed)]
#[folder = "icons"]
#[prefix = "icons/screenie/"]
#[include = "*.svg"]
struct Icons;

#[derive(rust_embed::RustEmbed)]
#[folder = "../../assets/fonts"]
#[include = "*.otf"]
struct Fonts;

/// Our icons first, then gpui-kit's (which gpui-component's widgets use internally).
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = Icons::get(path) {
            return Ok(Some(file.data));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> =
            Icons::iter().filter(|n| n.starts_with(path)).map(|n| SharedString::from(n.to_string())).collect();
        out.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(out)
    }
}

/// Register the bundled fonts with GPUI's text system.
pub fn load_fonts(cx: &App) {
    let fonts: Vec<Cow<'static, [u8]>> = Fonts::iter().filter_map(|f| Fonts::get(&f).map(|f| f.data)).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        tracing::warn!("loading bundled fonts failed: {e}");
    }
}

/// The UI font family.
pub const FONT: &str = "Inter";

/// Every icon screenie uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Area,
    Window,
    Screen,
    Select,
    Arrow,
    Line,
    Rectangle,
    Ellipse,
    Pen,
    Highlighter,
    Text,
    Step,
    Pixelate,
    Spotlight,
    Crop,
    Pin,
    Copy,
    Download,
    Save,
    Close,
    Mic,
    MicOff,
    Volume,
    VolumeOff,
    Video,
    Camera,
    Stop,
    Pause,
    Play,
    Settings,
    Image,
    Ocr,
    Undo,
    Redo,
    Picker,
    Check,
    FolderOpen,
    ExternalLink,
    ZoomIn,
    ZoomOut,
    Eraser,
    Trash,
    Timer,
    Film,
    Drag,
    Sparkles,
    Hand,
    Droplet,
    Minus,
    Plus,
    /// A three-quarter circle, for [`crate::hud::spinner`].
    Loader,
}

impl Icon {
    pub fn path(self) -> &'static str {
        match self {
            Icon::Area => "icons/screenie/square-dashed.svg",
            Icon::Window => "icons/screenie/app-window.svg",
            Icon::Screen => "icons/screenie/monitor.svg",
            Icon::Select => "icons/screenie/mouse-pointer-2.svg",
            Icon::Arrow => "icons/screenie/arrow.svg",
            Icon::Line => "icons/screenie/minus.svg",
            Icon::Rectangle => "icons/screenie/square.svg",
            Icon::Ellipse => "icons/screenie/circle.svg",
            Icon::Pen => "icons/screenie/pen-line.svg",
            Icon::Highlighter => "icons/screenie/highlighter.svg",
            Icon::Text => "icons/screenie/type.svg",
            Icon::Step => "icons/screenie/step.svg",
            Icon::Pixelate => "icons/screenie/grid-3x3.svg",
            Icon::Spotlight => "icons/screenie/spotlight.svg",
            Icon::Crop => "icons/screenie/crop.svg",
            Icon::Pin => "icons/screenie/pin.svg",
            Icon::Copy => "icons/screenie/copy.svg",
            Icon::Download => "icons/screenie/download.svg",
            Icon::Save => "icons/screenie/save.svg",
            Icon::Close => "icons/screenie/x.svg",
            Icon::Mic => "icons/screenie/mic.svg",
            Icon::MicOff => "icons/screenie/mic-off.svg",
            Icon::Volume => "icons/screenie/volume-2.svg",
            Icon::VolumeOff => "icons/screenie/volume-x.svg",
            Icon::Video => "icons/screenie/video.svg",
            Icon::Camera => "icons/screenie/camera.svg",
            Icon::Stop => "icons/screenie/square-stop.svg",
            Icon::Pause => "icons/screenie/pause.svg",
            Icon::Play => "icons/screenie/play.svg",
            Icon::Settings => "icons/screenie/settings.svg",
            Icon::Image => "icons/screenie/image.svg",
            Icon::Ocr => "icons/screenie/scan-text.svg",
            Icon::Undo => "icons/screenie/undo-2.svg",
            Icon::Redo => "icons/screenie/redo-2.svg",
            Icon::Picker => "icons/screenie/pipette.svg",
            Icon::Check => "icons/screenie/check.svg",
            Icon::FolderOpen => "icons/screenie/folder-open.svg",
            Icon::ExternalLink => "icons/screenie/external-link.svg",
            Icon::ZoomIn => "icons/screenie/zoom-in.svg",
            Icon::ZoomOut => "icons/screenie/zoom-out.svg",
            Icon::Eraser => "icons/screenie/eraser.svg",
            Icon::Trash => "icons/screenie/trash.svg",
            Icon::Timer => "icons/screenie/timer.svg",
            Icon::Film => "icons/screenie/film.svg",
            Icon::Drag => "icons/screenie/grip-vertical.svg",
            Icon::Sparkles => "icons/screenie/sparkles.svg",
            Icon::Hand => "icons/screenie/hand.svg",
            Icon::Droplet => "icons/screenie/droplet.svg",
            Icon::Minus => "icons/screenie/minus.svg",
            Icon::Plus => "icons/screenie/plus.svg",
            Icon::Loader => "icons/screenie/loader-circle.svg",
        }
    }

    /// A 16px icon element; color it with `text_color`.
    pub fn element(self) -> Svg {
        svg().path(self.path()).size_4().flex_none()
    }
}

impl IntoElement for Icon {
    type Element = Svg;

    fn into_element(self) -> Self::Element {
        self.element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_bundled() {
        use Icon::*;
        for icon in [
            Area, Window, Screen, Select, Arrow, Line, Rectangle, Ellipse, Pen, Highlighter, Text, Step, Pixelate,
            Spotlight, Crop, Pin, Copy, Download, Save, Close, Mic, MicOff, Volume, VolumeOff, Video, Camera, Stop,
            Pause, Play, Settings, Image, Ocr, Undo, Redo, Picker, Check, FolderOpen, ExternalLink, ZoomIn, ZoomOut,
            Eraser, Trash, Timer, Film, Drag, Sparkles, Hand, Droplet, Minus, Plus,
        ] {
            assert!(Icons::get(icon.path()).is_some(), "{icon:?} missing at {}", icon.path());
        }
        assert!(Fonts::iter().count() >= 4);
    }
}
