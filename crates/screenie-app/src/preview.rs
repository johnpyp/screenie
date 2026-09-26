//! The floating preview stack: a card per fresh capture at the edge of the screen
//! (`preview.position`: a corner or the middle of an edge), with quick actions on hover. Cards slide away on their own unless the pointer is on them.
//!
//! A recording's card appears the moment it's stopped, with a spinner while the file is
//! finished (which can take a moment), and gets its file actions once it's written.
//!
//! Hovering shows only what's left to do: Copy and Save buttons until the capture is
//! copied or saved. What's done shows as a "✓ Copied & Saved" pill in the corner, hovered
//! or not. Show in folder and Delete appear once there's a file.
//!
//! All cards share one transparent layer surface along the edge. Only the cards take
//! input (see [`Hover`]), and the keyboard only while the pointer is on one: then its
//! keys (Esc dismisses, Ctrl+C copies…) go to the card rather than the app beneath.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::layer_shell::Anchor;
use gpui::prelude::*;
use gpui::BorrowAppContext;
use gpui::{
    Animation, AnimationExt, App, AsyncApp, Context, Entity, FontWeight, Global, KeyDownEvent, Keystroke, ObjectFit,
    RenderImage, Window, WindowHandle, div, img, px, rgba, size,
};
use screenie_config::{Align, ScreenPosition};
use screenie_core::Image;
use screenie_ui_kit::hud::{self, color};
use screenie_ui_kit::{Hover, Icon, LayerSpec, Tip, layer_options, ui};

use crate::clipboard;
use crate::deliver::Capture;
use crate::daemon::Daemon;

/// Largest card edge; thumbnails are fit within it.
const CARD_MAX: f32 = 236.0;
/// Smallest card, so the hover actions always fit. Odd aspect ratios are letterboxed.
const CARD_MIN: (f32, f32) = (204.0, 116.0);
const EDGE_MARGIN: f32 = 18.0;
const GAP: f32 = 12.0;
const MAX_CARDS: usize = 5;

/// What can be done with a card: from its buttons, or from its keys while the pointer is
/// on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardAction {
    Copy,
    Save,
    Reveal,
    Dismiss,
    Delete,
    Annotate,
}

impl CardAction {
    const ALL: [CardAction; 6] = [Self::Copy, Self::Save, Self::Reveal, Self::Dismiss, Self::Delete, Self::Annotate];

    fn name(self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Save => "Save",
            Self::Reveal => "Show in folder",
            Self::Dismiss => "Dismiss",
            Self::Delete => "Delete",
            Self::Annotate => "Annotate",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Copy => Icon::Copy,
            Self::Save => Icon::Download,
            Self::Reveal => Icon::FolderOpen,
            Self::Dismiss => Icon::Close,
            Self::Delete => Icon::Trash,
            Self::Annotate => Icon::Pen,
        }
    }

    /// Its key, as the tooltip shows it.
    fn key(self) -> Option<&'static str> {
        match self {
            Self::Copy => Some("Ctrl+C"),
            Self::Save => Some("Ctrl+S"),
            Self::Reveal => None,
            Self::Dismiss => Some("Esc"),
            Self::Delete => Some("Delete"),
            Self::Annotate => Some("E"),
        }
    }

    fn for_key(k: &Keystroke) -> Option<Self> {
        let m = k.modifiers;
        if m.alt || m.shift || m.platform {
            return None;
        }
        Self::ALL.into_iter().find(|a| match a {
            Self::Copy => m.control && k.key == "c",
            Self::Save => m.control && k.key == "s",
            Self::Dismiss => !m.control && k.key == "escape",
            Self::Delete => !m.control && k.key == "delete",
            Self::Annotate => !m.control && k.key == "e",
            Self::Reveal => false,
        })
    }

    fn tip(self) -> Tip {
        let tip = Tip::new(self.name());
        match self.key() {
            Some(key) => tip.key(key),
            None => tip,
        }
    }

    /// Whether `item` offers it now: only what's left to do.
    fn offered(self, item: &PreviewItem, cx: &App) -> bool {
        let screenshot = matches!(item.media, Media::Screenshot { .. });
        let saved = item.path.is_some();
        match self {
            // A recording is copied as its file.
            Self::Copy => !item.is_copied() && (screenshot || saved),
            Self::Save => screenshot && !saved,
            // Nothing to show or delete until there's a file.
            Self::Reveal | Self::Delete => saved,
            Self::Dismiss => true,
            // One overlay editor at a time.
            Self::Annotate => screenshot && !screenie_editor::overlay_open(cx),
        }
    }

    fn run(self, stack: &mut PreviewStack, id: u64, cx: &mut Context<PreviewStack>) {
        match self {
            Self::Copy => stack.copy(id, cx),
            Self::Save => stack.save(id, cx),
            Self::Reveal => stack.reveal(id, cx),
            Self::Dismiss => stack.remove(id, cx),
            Self::Delete => stack.delete(id, cx),
            Self::Annotate => stack.edit(id, cx),
        }
    }
}

#[derive(Clone)]
pub(crate) enum Media {
    Screenshot { capture: Capture, png: Arc<Vec<u8>> },
    Recording {
        duration: Duration,
        /// The file is still being finished.
        saving: bool,
    },
}

pub(crate) struct PreviewItem {
    id: u64,
    media: Media,
    thumb: Arc<RenderImage>,
    /// Card size in logical pixels.
    card: (f32, f32),
    pixel_size: (u32, u32),
    bytes: u64,
    path: Option<PathBuf>,
    /// The clipboard generation it was copied at, if it was: it's on the clipboard
    /// until we copy something else.
    copied: Option<u64>,
    deadline: Option<Instant>,
    hovered: bool,
}

impl PreviewItem {
    /// `copied`: it was just put on the clipboard.
    pub async fn screenshot(
        capture: Capture,
        png: Arc<Vec<u8>>,
        path: Option<PathBuf>,
        copied: bool,
        cx: &mut AsyncApp,
    ) -> Self {
        let bytes = png.len() as u64;
        let image = capture.image.clone();
        Self::new(Media::Screenshot { capture, png }, image, bytes, path, copied, cx).await
    }

    pub async fn recording(finished: screenie_record::Finished, copied: bool, cx: &mut AsyncApp) -> Self {
        let (w, h) = finished.size;
        let frame = finished.last_frame.unwrap_or_else(|| Image::new(w, h, screenie_core::PixelFormat::Bgra));
        let media = Media::Recording { duration: finished.duration, saving: false };
        Self::new(media, frame, finished.bytes, Some(finished.path), copied, cx).await
    }

    /// A recording that's been stopped but whose file is still being finished; see
    /// [`saved`].
    pub async fn recording_saving(frame: Option<Image>, size: (u32, u32), duration: Duration, cx: &mut AsyncApp) -> Self {
        let frame = frame.unwrap_or_else(|| Image::new(size.0, size.1, screenie_core::PixelFormat::Bgra));
        let media = Media::Recording { duration, saving: true };
        Self::new(media, frame, 0, None, false, cx).await
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    fn is_saving(&self) -> bool {
        matches!(self.media, Media::Recording { saving: true, .. })
    }

    /// The thumbnail is scaled on a background thread.
    async fn new(
        media: Media,
        image: Image,
        bytes: u64,
        path: Option<PathBuf>,
        copied: bool,
        cx: &mut AsyncApp,
    ) -> Self {
        let copied = copied.then(clipboard::generation);
        let (iw, ih) = (image.width().max(1) as f32, image.height().max(1) as f32);
        let fit = (CARD_MAX / iw).min(CARD_MAX / ih).min(1.0);
        let card = ((iw * fit).clamp(CARD_MIN.0, CARD_MAX), (ih * fit).clamp(CARD_MIN.1, CARD_MAX));
        // Thumbnail at 2x for HiDPI outputs, keeping the image's aspect ratio (the card
        // letterboxes it).
        let thumb_scale = (fit * 2.0).min(1.0);
        let (tw, th) = (((iw * thumb_scale) as u32).max(1), ((ih * thumb_scale) as u32).max(1));
        let pixel_size = (image.width(), image.height());
        let thumb = cx
            .background_executor()
            .spawn(async move { screenie_ui_kit::render_image(&image.resize(tw, th)) })
            .await;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            pixel_size,
            bytes,
            media,
            thumb,
            card,
            path,
            copied,
            deadline: None,
            hovered: false,
        }
    }

    fn is_copied(&self) -> bool {
        self.copied == Some(clipboard::generation())
    }

    fn caption(&self) -> String {
        match &self.media {
            Media::Screenshot { .. } => {
                format!("{} × {} · {}", self.pixel_size.0, self.pixel_size.1, format_bytes(self.bytes))
            }
            Media::Recording { duration, saving: true } => format!("{} · Saving…", format_duration(*duration)),
            Media::Recording { duration, .. } => format!("{} · {}", format_duration(*duration), format_bytes(self.bytes)),
        }
    }
}

/// Where the open stack lives; a new capture elsewhere moves it.
#[derive(Clone, PartialEq)]
struct Placement {
    output: Option<String>,
    position: ScreenPosition,
}

struct Previews {
    window: Option<(WindowHandle<PreviewStack>, Placement)>,
}

impl Global for Previews {}

/// Show a card, on `output` if given.
pub(crate) fn show(item: PreviewItem, output: Option<String>, cx: &mut App) {
    if !cx.has_global::<Previews>() {
        cx.set_global(Previews { window: None });
    }
    let placement = Placement { output: output.clone(), position: Daemon::get(cx).config.preview.position };
    // Reuse the open stack if it's in the same place.
    if let Some((handle, current)) = cx.global::<Previews>().window.clone()
        && current == placement
    {
        let pushed = handle.update(cx, |stack, _, cx| stack.push(item, cx));
        if pushed.is_ok() {
            return;
        }
        // The window is gone; fall through and open a new one. (The item was moved into
        // the failed update, so there's nothing to show.)
        cx.global_mut::<Previews>().window = None;
        return;
    }
    if let Some((handle, _)) = cx.global_mut::<Previews>().window.take() {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }

    // One transparent surface over the output's free area (a size of 0 lets the compositor
    // stretch it between the anchors, clear of bars); input is limited to the cards, so
    // the rest is click-through. Positions are just alignments within it.
    let spec = LayerSpec {
        output: output.clone(),
        ..LayerSpec::floating(
            "screenie-preview",
            Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            size(px(0.), px(0.)),
        )
    };
    let position = placement.position;
    let opened = cx.open_window(layer_options(cx, &spec), |window, cx| {
        let stack = cx.new(|cx| PreviewStack::new(position, output, window, cx));
        stack.update(cx, |s, cx| {
            s.push(item, cx);
        });
        stack
    });
    match opened {
        Ok(handle) => cx.global_mut::<Previews>().window = Some((handle, placement)),
        Err(e) => tracing::warn!("cannot show the preview card: {e}"),
    }
}

/// A recording's card, shown while it was being finished, now has its file.
pub(crate) fn saved(id: u64, finished: &screenie_record::Finished, copied: bool, cx: &mut App) {
    let (path, bytes, duration) = (finished.path.clone(), finished.bytes, finished.duration);
    with_stack(cx, move |stack, cx| stack.saved(id, path, bytes, duration, copied, cx));
}

/// Take a card away, e.g. a recording that couldn't be finished.
pub(crate) fn discard(id: u64, cx: &mut App) {
    with_stack(cx, move |stack, cx| stack.remove(id, cx));
}

fn with_stack(cx: &mut App, f: impl FnOnce(&mut PreviewStack, &mut Context<PreviewStack>)) {
    let Some((handle, _)) = cx.try_global::<Previews>().and_then(|p| p.window.clone()) else { return };
    let _ = handle.update(cx, |stack, _, cx| f(stack, cx));
}

pub(crate) struct PreviewStack {
    position: ScreenPosition,
    /// The output the stack is on, where editors open too.
    output: Option<String>,
    items: Vec<PreviewItem>,
    /// Input: each card is an area, keyed by its id.
    hover: Entity<Hover<u64>>,
}

impl PreviewStack {
    fn new(position: ScreenPosition, output: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        screenie_ui_kit::track_ui_scale(window, cx);
        let hover = Hover::new(window, cx);
        cx.observe(&hover, |stack, hover, cx| {
            let card = hover.read(cx).hovered().copied();
            stack.pointer_on(card, cx);
        })
        .detach();
        screenie_editor::observe_overlays(cx, |_, cx| cx.notify()).detach();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                let alive = this.update_in(cx, |stack, window, cx| {
                    stack.expire(cx);
                    // Not while it has the keyboard: a held key would go to the app beneath.
                    if stack.items.is_empty() && !stack.hover.read(cx).has_keyboard() {
                        window.remove_window();
                        cx.update_global::<Previews, _>(|p, _| p.window = None);
                        false
                    } else {
                        true
                    }
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        Self { position, output, items: Vec::new(), hover }
    }

    fn timeout(cx: &App) -> Option<Duration> {
        match Daemon::get(cx).config.preview.timeout {
            0 => None,
            s => Some(Duration::from_secs(s as u64)),
        }
    }

    fn push(&mut self, mut item: PreviewItem, cx: &mut Context<Self>) {
        item.deadline = Self::timeout(cx).map(|t| Instant::now() + t);
        self.items.push(item);
        if self.items.len() > MAX_CARDS {
            self.items.remove(0);
        }
        cx.notify();
    }

    fn expire(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let before = self.items.len();
        self.items.retain(|i| i.hovered || i.is_saving() || i.deadline.is_none_or(|d| d > now));
        if self.items.len() != before {
            cx.notify();
        }
    }

    fn remove(&mut self, id: u64, cx: &mut Context<Self>) {
        self.items.retain(|i| i.id != id);
        cx.notify();
    }

    fn saved(&mut self, id: u64, path: PathBuf, bytes: u64, duration: Duration, copied: bool, cx: &mut Context<Self>) {
        let timeout = Self::timeout(cx);
        let Some(item) = self.item(id) else { return };
        item.media = Media::Recording { duration, saving: false };
        item.path = Some(path);
        item.bytes = bytes;
        item.copied = copied.then(clipboard::generation);
        // Its time starts now that there's something to do with it.
        item.deadline = timeout.map(|t| Instant::now() + t);
        cx.notify();
    }

    fn item(&mut self, id: u64) -> Option<&mut PreviewItem> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = CardAction::for_key(&event.keystroke) else { return };
        let Some(item) = self.items.iter().find(|i| i.hovered) else { return };
        if action.offered(item, cx) {
            let id = item.id;
            cx.stop_propagation();
            action.run(self, id, cx);
        }
    }

    /// The pointer is on this card (or none).
    fn pointer_on(&mut self, card: Option<u64>, cx: &mut Context<Self>) {
        let changed: Vec<(u64, bool)> = self
            .items
            .iter()
            .filter(|i| i.hovered != (Some(i.id) == card))
            .map(|i| (i.id, Some(i.id) == card))
            .collect();
        for (id, hovered) in changed {
            self.set_hovered(id, hovered, cx);
        }
    }

    fn set_hovered(&mut self, id: u64, hovered: bool, cx: &mut Context<Self>) {
        let timeout = Self::timeout(cx);
        if let Some(item) = self.item(id) {
            item.hovered = hovered;
            // Leaving a card gives it a fresh (shorter) lease so it doesn't vanish instantly.
            if !hovered {
                item.deadline = timeout.map(|t| Instant::now() + t.min(Duration::from_secs(3)));
            }
        }
        cx.notify();
    }

    fn copy(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(item) = self.item(id) else { return };
        let (media, path) = (item.media.clone(), item.path.clone());
        let copying = cx.background_executor().spawn(async move {
            let result = match (media, path) {
                (Media::Screenshot { png, .. }, path) => {
                    clipboard::copy(clipboard::Content::Image { png: png.to_vec(), file: path.as_deref() })
                }
                (Media::Recording { .. }, Some(path)) => clipboard::copy(clipboard::Content::File(&path)),
                (Media::Recording { .. }, None) => Ok(()),
            };
            result.map(|()| clipboard::generation())
        });
        cx.spawn(async move |this, cx| match copying.await {
            Ok(generation) => {
                let _ = this.update(cx, |stack, cx| {
                    if let Some(item) = stack.item(id) {
                        item.copied = Some(generation);
                    }
                    cx.notify();
                });
            }
            Err(e) => tracing::warn!("{e}"),
        })
        .detach();
    }

    fn reveal(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(path) = self.item(id).and_then(|i| i.path.clone()) {
            cx.reveal_path(&path);
            self.remove(id, cx);
        }
    }

    fn save(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(item) = self.item(id) else { return };
        if item.path.is_some() {
            return;
        }
        let Media::Screenshot { png, capture } = item.media.clone() else { return };
        let path = crate::deliver::screenshot_path(&Daemon::get(cx).config, &capture.subject);
        match crate::deliver::write_atomic(&path, &png) {
            Ok(()) => {
                if let Some(item) = self.item(id) {
                    item.path = Some(path);
                }
                cx.notify();
            }
            Err(e) => tracing::error!("saving {}: {e}", path.display()),
        }
    }

    /// Open the file in its default app (image viewer, video player). An unsaved
    /// screenshot is opened from a temporary file.
    fn open(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(item) = self.item(id) else { return };
        let path = match (&item.path, &item.media) {
            (Some(path), _) => path.clone(),
            (None, Media::Screenshot { png, .. }) => {
                let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
                let path = screenie_config::Paths::get().runtime_dir().join(format!("capture-{stamp}.png"));
                if let Err(e) = crate::deliver::write_atomic(&path, png) {
                    tracing::error!("writing {}: {e}", path.display());
                    return;
                }
                path
            }
            (None, Media::Recording { .. }) => return,
        };
        cx.open_with_system(&path);
        self.remove(id, cx);
    }

    /// Annotate the capture; the card makes way for the editor.
    fn edit(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(item) = self.item(id) else { return };
        let Media::Screenshot { mut capture, .. } = item.media.clone() else { return };
        // The card stays if another edit is under way.
        if crate::editor::ensure_free(cx).is_err() {
            return;
        }
        let path = item.path.clone();
        capture.output = capture.output.or_else(|| self.output.clone());
        // Picked up again later, it opens centred rather than where it was taken.
        capture.placement = None;
        self.remove(id, cx);
        let config = &Daemon::get(cx).config.screenshot.after_capture;
        let actions = crate::deliver::Actions::resolve(config, &Default::default(), None, false);
        if let Err(e) = crate::editor::open(capture, path, actions, cx) {
            tracing::warn!("{e:#}");
        }
    }

    fn delete(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(path) = self.item(id).and_then(|i| i.path.clone())
            && let Err(e) = std::fs::remove_file(&path)
        {
            tracing::warn!("deleting {}: {e}", path.display());
        }
        self.remove(id, cx);
    }

    fn card(&self, item: &PreviewItem, cx: &mut Context<Self>) -> impl IntoElement {
        let id = item.id;
        // The card's size in pixels at the interface scale (exact, so the thumbnail can
        // fill its inside precisely); everything else in it is in `ui` lengths.
        let k = screenie_ui_kit::ui_scale(cx);
        let (w, h) = (item.card.0 * k, item.card.1 * k);
        let hovered = item.hovered;
        let saved = item.path.is_some();
        // Cards slide in from the edge they sit at (the side one, for corners).
        let slide = match (self.position.horizontal(), self.position.vertical()) {
            (Align::Start, _) => (-48.0 * k, 0.0),
            (Align::End, _) => (48.0 * k, 0.0),
            (Align::Middle, Align::Start) => (0.0, -48.0 * k),
            (Align::Middle, _) => (0.0, 48.0 * k),
        };

        let button = |action: CardAction, cx: &mut Context<Self>| {
            div()
                .id(action.name())
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .child(action.icon().element().text_color(gpui::white()))
                .tooltip(action.tip().builder())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    action.run(this, id, cx)
                }))
        };
        // Small round buttons in the corners, and larger ones in the middle for what's
        // left to do with the capture.
        let corner = |action, cx: &mut Context<Self>| {
            button(action, cx).size(ui(30.)).bg(rgba(0x000000a6)).hover(|s| s.bg(rgba(0x000000d9)))
        };
        let middle = |action, cx: &mut Context<Self>| {
            button(action, cx)
                .size(ui(42.))
                .bg(rgba(0xffffff2e))
                .hover(|s| s.bg(rgba(0xffffff4d)))
                .active(|s| s.opacity(0.85))
        };
        let copied = item.is_copied();
        let done: Vec<&str> = [(copied, "Copied"), (saved, "Saved")].into_iter().filter(|d| d.0).map(|d| d.1).collect();
        // Bottom right, hovered or not: what's already been done with it (the buttons for
        // those are gone), with the size caption above it on hover.
        let status = (!done.is_empty()).then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px(ui(6.))
                .py(ui(2.))
                .rounded(ui(6.))
                .bg(rgba(0x000000b3))
                .text_color(gpui::white())
                .text_size(ui(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(Icon::Check.element().size_3().text_color(gpui::white()))
                .child(done.join(" & "))
        });
        let caption = hovered.then(|| div().text_size(ui(11.)).text_color(rgba(0xffffffb3)).child(item.caption()));
        let corner_info = (status.is_some() || caption.is_some()).then(|| {
            div()
                .absolute()
                .bottom(ui(8.))
                .right(ui(8.))
                .flex()
                .flex_col()
                .items_end()
                .gap_1()
                .children(caption)
                .children(status)
        });

        // Videos are marked so they aren't mistaken for screenshots.
        let badge = match (&item.media, hovered) {
            (Media::Recording { duration, .. }, false) => Some(
                div()
                    .absolute()
                    .bottom(ui(8.))
                    .left(ui(8.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px(ui(6.))
                    .py(ui(2.))
                    .rounded(ui(6.))
                    .bg(rgba(0x000000b3))
                    .text_color(gpui::white())
                    .text_size(ui(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Icon::Video.element().size_3().text_color(gpui::white()))
                    .child(format_duration(*duration)),
            ),
            _ => None,
        };

        let saving = item.is_saving();
        let spinner = || hud::spinner(("saving", id), ui(20.), gpui::white());
        // While the file is finished: the spinner in the middle, over a light tint (the
        // hover overlay has it instead of the buttons).
        let pending = (saving && !hovered).then(|| {
            div()
                .absolute()
                .inset_0()
                .rounded(ui(11.))
                .bg(rgba(0x00000059))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size(ui(36.))
                        .rounded_full()
                        .bg(rgba(0x000000a6))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(spinner()),
                )
        });

        let overlay = hovered.then(|| {
            let offers: Vec<CardAction> = CardAction::ALL.into_iter().filter(|a| a.offered(item, cx)).collect();
            let offered = |action| offers.contains(&action);
            let actions = div()
                .flex()
                .flex_row()
                .gap_3()
                .when(saving, |d| d.child(spinner()))
                .when(offered(CardAction::Copy), |d| d.child(middle(CardAction::Copy, cx)))
                .when(offered(CardAction::Save), |d| d.child(middle(CardAction::Save, cx)))
                .when(offered(CardAction::Reveal), |d| d.child(middle(CardAction::Reveal, cx)));
            div()
                .absolute()
                .inset_0()
                .rounded(ui(11.))
                .bg(rgba(0x00000099))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(actions)
                .child(div().absolute().top(ui(6.)).left(ui(6.)).child(corner(CardAction::Dismiss, cx)))
                .when(offered(CardAction::Delete), |d| {
                    d.child(div().absolute().bottom(ui(6.)).left(ui(6.)).child(corner(CardAction::Delete, cx)))
                })
                .when(offered(CardAction::Annotate), |d| {
                    d.child(div().absolute().top(ui(6.)).right(ui(6.)).child(corner(CardAction::Annotate, cx)))
                })
        });

        div()
            .id(("card", id))
            .relative()
            .w(px(w))
            .h(px(h))
            .rounded(ui(12.))
            .bg(color::panel_solid())
            .border_1()
            .border_color(rgba(0xffffff26))
            .shadow(hud::panel_shadow())
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.open(id, cx)))
            // Sized explicitly to the inside of the card: GPUI's `img` imposes the image's
            // own aspect ratio on relative sizes, so `size_full` made a tall capture spill
            // out of the bottom of the card. `object_fit` then letterboxes it within.
            .child(
                img(item.thumb.clone())
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(w - 2.))
                    .h(px(h - 2.))
                    .object_fit(ObjectFit::Contain)
                    .rounded(ui(11.)),
            )
            .children(badge)
            .children(pending)
            .children(overlay)
            .children(corner_info)
            .child(Hover::area(&self.hover, id))
            .with_animation(
                ("card-in", id),
                Animation::new(Duration::from_millis(260)).with_easing(gpui::ease_out_quint()),
                move |el, t| el.left(px(slide.0 * (1.0 - t))).top(px(slide.1 * (1.0 - t))).opacity(t),
            )
    }
}

fn format_duration(d: Duration) -> String {
    let s = d.as_secs_f64().round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn format_bytes(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1} MB", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.0} KB", n as f64 / 1e3),
        n => format!("{n} B"),
    }
}

impl Render for PreviewStack {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The newest card sits nearest the edge (at the bottom, for the side middles).
        let vertical = self.position.vertical();
        let mut cards: Vec<_> = self.items.iter().map(|item| self.card(item, cx).into_any_element()).collect();
        if vertical == Align::Start {
            cards.reverse();
        }
        let stack = div()
            .id("preview")
            .size_full()
            .font_family(screenie_ui_kit::FONT)
            .flex()
            .flex_col()
            .on_key_down(cx.listener(Self::on_key_down));
        let stack = match vertical {
            Align::Start => stack.justify_start(),
            Align::Middle => stack.justify_center(),
            Align::End => stack.justify_end(),
        };
        let stack = match self.position.horizontal() {
            Align::Start => stack.items_start(),
            Align::Middle => stack.items_center(),
            Align::End => stack.items_end(),
        };
        let stack = stack.gap(ui(GAP)).p(ui(EDGE_MARGIN)).children(cards);
        Hover::root(&self.hover, stack, cx)
    }
}
