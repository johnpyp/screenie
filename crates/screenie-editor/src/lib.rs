//! The annotation editor: a window for marking up a capture with arrows, boxes, text,
//! numbered steps, redactions and more, then copying or saving the result.
//!
//! The document model and renderer live in `screenie-annotate`; this crate is the
//! interaction ([`Session`], UI-free and unit-tested) and the GPUI window around it.

mod raster;
mod session;
mod tool;
mod view;

use std::path::PathBuf;
use std::rc::Rc;

use gpui::{App, AppContext as _, Bounds, Size, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowHandle, WindowOptions, px, size};
use screenie_annotate::{Color, Document, Style};
use screenie_core::Image;

pub use session::{Cursor, Key, Modifiers, Outcome, Reach, Session, TextEdit};
pub use tool::Tool;
pub use view::{Editor, Output, OutputHandler};

pub struct EditorOptions {
    pub title: String,
    /// The file the capture was saved to, if any (the starting point for Save As).
    pub path: Option<PathBuf>,
    /// Image pixels per logical pixel (the capture's output scale).
    pub scale: f32,
    pub palette: Vec<Color>,
    pub style: Style,
    /// Output (connector name) to open on.
    pub output: Option<String>,
}

/// Open an editor window for `image`. `on_output` receives copies, saves and the final
/// result.
pub fn open(
    image: &Image,
    options: EditorOptions,
    on_output: impl Fn(Output, &mut App) -> anyhow::Result<Option<String>> + 'static,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<Editor>> {
    screenie_annotate::text::warm_up();
    let doc = Document::new(image, options.scale);
    let display = options.output.as_deref().and_then(|name| screenie_ui_kit::display_for_output(cx, name));
    let window_size = initial_size(&doc, display.and_then(|d| cx.find_display(d)).map(|d| d.bounds().size));
    let window_options = WindowOptions {
        titlebar: Some(gpui::TitlebarOptions { title: Some(options.title.clone().into()), ..Default::default() }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(display, window_size, cx))),
        window_min_size: Some(size(px(780.), px(480.))),
        window_decorations: Some(WindowDecorations::Server),
        window_background: WindowBackgroundAppearance::Opaque,
        display_id: display,
        app_id: Some(screenie_ui_kit::APP_ID.to_string()),
        focus: true,
        show: true,
        ..Default::default()
    };
    let session = Session::new(doc, options.style);
    let on_output: OutputHandler = Rc::new(on_output);
    let handle = cx.open_window(window_options, move |window, cx| {
        cx.new(|cx| Editor::new(session, options.palette, options.path, on_output, window, cx))
    })?;
    handle.update(cx, |_, window, _| window.activate_window())?;
    Ok(handle)
}

/// Big enough to show the capture at its on-screen size plus the bars, within 85% of the
/// display.
fn initial_size(doc: &Document, display: Option<Size<gpui::Pixels>>) -> Size<gpui::Pixels> {
    let (w, h) = (doc.width() as f32 / doc.scale(), doc.height() as f32 / doc.scale());
    let (max_w, max_h) = display.map_or((1600.0, 1000.0), |d| (f32::from(d.width) * 0.85, f32::from(d.height) * 0.85));
    size(px((w + 56.0).clamp(780.0, max_w.max(780.0))), px((h + 132.0).clamp(480.0, max_h.max(480.0))))
}
