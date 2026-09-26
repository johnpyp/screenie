//! The annotation editor: marking up a capture with arrows, boxes, text, numbered
//! steps, redactions and more, then copying or saving the result.
//!
//! It opens as an overlay over the screen, with the capture where it was taken
//! ([`Mode::Overlay`]), or as a regular window ([`Mode::Window`]).
//!
//! The document model and renderer live in `screenie-annotate`; this crate is the
//! interaction ([`Session`], UI-free and unit-tested) and the GPUI view around it.

mod raster;
mod session;
mod tool;
mod view;

use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Bounds, Context, Size, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowHandle,
    WindowOptions, px, size,
};
use screenie_annotate::{Color, Document, Style};
use screenie_core::{Image, OutputInfo, Rect};
use screenie_ui_kit::layer::{LayerSpec, fallback_options, layer_options};

pub use session::{Cursor, Key, Modifiers, Outcome, Reach, Session, TextEdit};
pub use tool::Tool;
pub use view::{Editor, Output, OutputHandler};

/// How the editor appears.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// Over everything on the capture's screen, the capture shown where it was taken and
    /// the rest dimmed (like the selector it follows).
    #[default]
    Overlay,
    /// A regular, resizable window.
    Window,
}

pub struct EditorOptions {
    pub title: String,
    /// The file the capture was saved to, if any (the starting point for Save As).
    pub path: Option<PathBuf>,
    /// Image pixels per logical pixel (the capture's output scale).
    pub scale: f32,
    pub palette: Vec<Color>,
    pub style: Style,
    pub mode: Mode,
    /// The screen to open on.
    pub output: Option<OutputInfo>,
    /// Where the capture was on that screen (logical pixels, relative to it), so the
    /// overlay can show it in place.
    pub placement: Option<Rect>,
    /// `path` holds exactly this image already, so Done needn't save it again.
    pub on_disk: bool,
    /// What Done does besides closing, to say so on the button.
    pub on_done: OnDone,
    /// Close as soon as the image is copied / saved.
    pub exit_on_copy: bool,
    pub exit_on_save: bool,
    /// Ask before closing with annotations neither copied nor saved.
    pub confirm_discard: bool,
}

/// What the owner does with the image on Done (see [`Output::Done`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OnDone {
    pub copy: bool,
    /// Save even if the image has no file yet. One that has (opened from a file,
    /// given `-o`, or saved in the editor) is always saved back.
    pub save: bool,
}

/// What an editor needs besides its session; kept to reopen the overlay after stepping
/// aside for a file dialog.
pub(crate) struct Setup {
    pub title: String,
    pub palette: Vec<Color>,
    pub mode: Mode,
    pub output: Option<OutputInfo>,
    pub placement: Option<Rect>,
    pub on_done: OnDone,
    pub exit_on_copy: bool,
    pub exit_on_save: bool,
    pub confirm_discard: bool,
    pub on_output: OutputHandler,
}

/// The overlay editors open: one at most, though Save As briefly closes it for the
/// file dialog ("parked").
#[derive(Default)]
pub(crate) struct Overlays {
    pub open: Vec<WindowHandle<Editor>>,
    pub parked: usize,
}

impl gpui::Global for Overlays {}

/// Whether an overlay editor is open (or stepped aside for Save As). Overlay mode
/// allows one at a time.
pub fn overlay_open(cx: &App) -> bool {
    cx.try_global::<Overlays>().is_some_and(|o| !o.open.is_empty() || o.parked > 0)
}

/// Call `f` whenever an overlay editor opens or closes.
pub fn observe_overlays<V: 'static>(
    cx: &mut Context<V>,
    f: impl FnMut(&mut V, &mut Context<V>) + 'static,
) -> gpui::Subscription {
    cx.observe_global::<Overlays>(f)
}

/// If an overlay editor is open, show it `message` and return true.
pub fn overlay_busy(message: &str, cx: &mut App) -> bool {
    if let Some(handle) = cx.try_global::<Overlays>().and_then(|o| o.open.last().copied()) {
        let message = message.to_string();
        let _ = handle.update(cx, |editor, window, cx| {
            editor.show_toast(message, cx);
            window.activate_window();
        });
    }
    overlay_open(cx)
}

/// Open an editor for `image`. `on_output` receives copies, saves and the final result.
pub fn open(
    image: &Image,
    options: EditorOptions,
    on_output: impl Fn(Output, &mut App) -> anyhow::Result<Option<String>> + 'static,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<Editor>> {
    screenie_annotate::text::warm_up();
    let session = Session::new(Document::new(image, options.scale), options.style, options.on_disk);
    let setup = Rc::new(Setup {
        title: options.title,
        palette: options.palette,
        mode: options.mode,
        output: options.output,
        placement: options.placement,
        on_done: options.on_done,
        exit_on_copy: options.exit_on_copy,
        exit_on_save: options.exit_on_save,
        confirm_discard: options.confirm_discard,
        on_output: Rc::new(on_output),
    });
    open_session(session, options.path, setup, cx)
}

/// Open a window editing `session`.
pub(crate) fn open_session(
    session: Session,
    path: Option<PathBuf>,
    setup: Rc<Setup>,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<Editor>> {
    let name = setup.output.as_ref().map(|o| o.name.as_str());
    let display = name.and_then(|name| screenie_ui_kit::display_for_output(cx, name));
    let screen = setup.output.as_ref().map(|o| size(px(o.logical.width as f32), px(o.logical.height as f32)));
    let build = {
        let setup = setup.clone();
        move |session: Session, path| {
            let setup = setup.clone();
            move |window: &mut gpui::Window, cx: &mut App| cx.new(|cx| Editor::new(session, path, setup, window, cx))
        }
    };
    let handle = match setup.mode {
        Mode::Overlay => {
            // The surface stretches over the whole output; the size only seeds the first
            // frame, and must be the output's own or the compositor centres it instead.
            let spec = LayerSpec::fullscreen_overlay(
                "screenie-editor",
                name.unwrap_or_default(),
                screen.unwrap_or(size(px(1280.), px(800.))),
            );
            let spec = LayerSpec { output: name.map(String::from), ..spec };
            match cx.open_window(layer_options(cx, &spec), build(session.clone(), path.clone())) {
                Ok(handle) => handle,
                Err(e) => {
                    tracing::debug!("layer-shell editor failed ({e}); falling back to a fullscreen window");
                    cx.open_window(fallback_options(cx, &spec), build(session, path))?
                }
            }
        }
        Mode::Window => {
            let window_size = initial_size(session.doc(), screen);
            let options = WindowOptions {
                titlebar: Some(gpui::TitlebarOptions { title: Some(setup.title.clone().into()), ..Default::default() }),
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
            cx.open_window(options, build(session, path))?
        }
    };
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
