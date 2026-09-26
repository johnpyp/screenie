//! The floating preview stack: a card per fresh capture at the edge of the screen
//! (`preview.position`: a corner or the middle of an edge), with quick actions on hover.
//! Cards slide away on their own unless the pointer is on them, or an overlay editor is
//! open: then they wait for it to close.
//!
//! A recording's card appears the moment it's stopped, with a spinner while the file is
//! finished (which can take a moment), and gets its file actions once it's written.
//!
//! Hovering shows only what's left to do: Copy and Save buttons until the capture is
//! copied or saved. What's done shows as a "✓ Copied & Saved" pill in the corner, hovered
//! or not. Show in folder and Delete appear once there's a file.
//!
//! The cards on an output share one transparent layer surface along the edge (a stack;
//! each output has its own). Only the cards take input, and only the pointer's: the
//! keyboard stays with the app you're in (see [`Hover`]).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::layer_shell::Anchor;
use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt, AnyElement, AnyWindowHandle, App, AsyncApp, Context, Entity,
    FontWeight, Global, ObjectFit, RenderImage, Window, WindowHandle, div, img, px, rgba, size,
};
use screenie_config::{Align, ScreenPosition};
use screenie_core::Image;
use screenie_ipc::CaptureKind;
use screenie_ui_kit::hud::{self, color};
use screenie_ui_kit::{Hover, Icon, LayerSpec, Tip, layer_options, ui};

use crate::clipboard;
use crate::daemon::Daemon;
use crate::deliver::{Actions, Capture};
use crate::last::CaptureId;

/// Largest card edge; thumbnails are fit within it.
const CARD_MAX: f32 = 236.0;
/// Smallest card, so the hover actions always fit. Odd aspect ratios are letterboxed.
const CARD_MIN: (f32, f32) = (204.0, 116.0);
const EDGE_MARGIN: f32 = 18.0;
const GAP: f32 = 12.0;
/// At most this many cards stack, fewer if they don't fit the output (see [`overflow`]).
const MAX_CARDS: usize = 5;
/// The stack's layer namespace, which also names it for [`conceal`](screenie_ui_kit::conceal).
pub(crate) const NAMESPACE: &str = "screenie-preview";

/// What can be done with a card, from its buttons.
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
    const ALL: [CardAction; 6] = [
        Self::Copy,
        Self::Save,
        Self::Reveal,
        Self::Dismiss,
        Self::Delete,
        Self::Annotate,
    ];

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

    fn tip(self) -> Tip {
        Tip::new(self.name())
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
    Screenshot {
        capture: Capture,
        png: Arc<Vec<u8>>,
        /// What it was taken to do (`--copy`, `--no-save`…): the editor does it when
        /// it's done.
        actions: Actions,
        /// Its entry in `screenie query last`, which learns where it's saved.
        noted: CaptureId,
    },
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
    /// It's been on screen (rather than concealed from the moment it came).
    seen: bool,
    /// It's been on screen already (and moved to a new surface): it doesn't slide in
    /// again.
    arrived: bool,
}

impl PreviewItem {
    /// `actions`: the ones it was taken with. `noted`: its entry in `screenie query
    /// last`. `copied`: it was just put on the clipboard.
    pub async fn screenshot(
        capture: Capture,
        png: Arc<Vec<u8>>,
        actions: Actions,
        noted: CaptureId,
        path: Option<PathBuf>,
        copied: bool,
        cx: &mut AsyncApp,
    ) -> Self {
        let bytes = png.len() as u64;
        let image = capture.image.clone();
        Self::new(
            Media::Screenshot {
                capture,
                png,
                actions,
                noted,
            },
            image,
            bytes,
            path,
            copied,
            cx,
        )
        .await
    }

    pub async fn recording(
        finished: screenie_record::Finished,
        copied: bool,
        cx: &mut AsyncApp,
    ) -> Self {
        let (w, h) = finished.size;
        let frame = finished
            .last_frame
            .unwrap_or_else(|| Image::new(w, h, screenie_core::PixelFormat::Bgra));
        let media = Media::Recording {
            duration: finished.duration,
            saving: false,
        };
        Self::new(
            media,
            frame,
            finished.bytes,
            Some(finished.path),
            copied,
            cx,
        )
        .await
    }

    /// A recording that's been stopped but whose file is still being finished; see
    /// [`saved`].
    pub async fn recording_saving(
        frame: Option<Image>,
        size: (u32, u32),
        duration: Duration,
        cx: &mut AsyncApp,
    ) -> Self {
        let frame =
            frame.unwrap_or_else(|| Image::new(size.0, size.1, screenie_core::PixelFormat::Bgra));
        let media = Media::Recording {
            duration,
            saving: true,
        };
        Self::new(media, frame, 0, None, false, cx).await
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    fn is_saving(&self) -> bool {
        matches!(self.media, Media::Recording { saving: true, .. })
    }

    /// In use, so it mustn't go: under the pointer, or a recording still being finished.
    fn pinned(&self) -> bool {
        self.hovered || self.is_saving()
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
        let card = (
            (iw * fit).clamp(CARD_MIN.0, CARD_MAX),
            (ih * fit).clamp(CARD_MIN.1, CARD_MAX),
        );
        // Thumbnail at 2x for HiDPI outputs, keeping the image's aspect ratio (the card
        // letterboxes it).
        let thumb_scale = (fit * 2.0).min(1.0);
        let (tw, th) = (
            ((iw * thumb_scale) as u32).max(1),
            ((ih * thumb_scale) as u32).max(1),
        );
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
            seen: false,
            arrived: false,
        }
    }

    fn is_copied(&self) -> bool {
        self.copied == Some(clipboard::generation())
    }

    fn caption(&self) -> String {
        match &self.media {
            Media::Screenshot { .. } => {
                format!(
                    "{} × {} · {}",
                    self.pixel_size.0,
                    self.pixel_size.1,
                    format_bytes(self.bytes)
                )
            }
            Media::Recording {
                duration,
                saving: true,
            } => format!("{} · Saving…", format_duration(*duration)),
            Media::Recording { duration, .. } => format!(
                "{} · {}",
                format_duration(*duration),
                format_bytes(self.bytes)
            ),
        }
    }
}

/// Where a stack lives.
#[derive(Clone, PartialEq)]
struct Placement {
    output: Option<String>,
    position: ScreenPosition,
}

/// The open stacks, one per placement. A capture on another output (or after
/// `preview.position` changed) opens a stack there, and the cards already up stay where
/// they are.
#[derive(Default)]
struct Previews {
    stacks: Vec<(WindowHandle<PreviewStack>, Placement)>,
}

impl Global for Previews {}

impl Previews {
    /// Forget stacks whose surface is gone: closed by the compositor (an output
    /// unplugged or turned off) rather than by us.
    fn prune(cx: &mut App) {
        let stacks = std::mem::take(&mut cx.default_global::<Previews>().stacks);
        let live = stacks
            .into_iter()
            .filter(|(handle, _)| handle.update(cx, |_, _, _| ()).is_ok())
            .collect();
        cx.global_mut::<Previews>().stacks = live;
    }

    fn forget(handle: WindowHandle<PreviewStack>, cx: &mut App) {
        cx.default_global::<Previews>()
            .stacks
            .retain(|(h, _)| *h != handle);
    }

    fn handles(cx: &App) -> Vec<WindowHandle<PreviewStack>> {
        cx.try_global::<Previews>()
            .map(|p| p.stacks.iter().map(|(h, _)| *h).collect())
            .unwrap_or_default()
    }
}

/// Show a card, on `output` if given.
pub(crate) fn show(item: PreviewItem, output: Option<String>, cx: &mut App) {
    Previews::prune(cx);
    let placement = Placement {
        output,
        position: Daemon::get(cx).config.preview.position,
    };
    let existing = cx
        .global::<Previews>()
        .stacks
        .iter()
        .find(|(_, p)| *p == placement)
        .map(|(h, _)| *h);
    let mut item = Some(item);
    if let Some(handle) = existing {
        // If the surface is gone after all, the closure never runs and the item is still
        // here for a new one.
        let _ = handle.update(cx, |stack, _, cx| {
            if let Some(item) = item.take() {
                stack.push(item, cx);
            }
        });
        if item.is_none() {
            return;
        }
        Previews::forget(handle, cx);
    }
    open_stack(placement, item.into_iter().collect(), cx);
}

/// Open a stack surface at `placement`, holding `items`.
fn open_stack(placement: Placement, items: Vec<PreviewItem>, cx: &mut App) {
    // One transparent surface over the output's free area (a size of 0 lets the compositor
    // stretch it between the anchors, clear of bars); input is limited to the cards, so
    // the rest is click-through. Positions are just alignments within it.
    let spec = LayerSpec {
        output: placement.output.clone(),
        ..LayerSpec::floating(
            NAMESPACE,
            Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            size(px(0.), px(0.)),
        )
    };
    let (position, output) = (placement.position, placement.output.clone());
    let opened = cx.open_window(layer_options(cx, &spec), |window, cx| {
        screenie_ui_kit::conceal::track(window, &spec, cx);
        let stack = cx.new(|cx| PreviewStack::new(position, output, window, cx));
        stack.update(cx, |s, cx| {
            for item in items {
                s.push(item, cx);
            }
        });
        stack
    });
    match opened {
        Ok(handle) => cx
            .default_global::<Previews>()
            .stacks
            .push((handle, placement)),
        Err(e) => tracing::warn!("cannot show the preview card: {e}"),
    }
}

/// Move a stack to a new surface over the editor that just opened (see
/// [`PreviewStack::editors_changed`]). The old surface closes once it's empty.
fn raise(handle: WindowHandle<PreviewStack>, cx: &mut App) {
    let Some(placement) = cx.try_global::<Previews>().and_then(|p| {
        p.stacks
            .iter()
            .find(|(h, _)| *h == handle)
            .map(|(_, p)| p.clone())
    }) else {
        return;
    };
    let Ok(items) = handle.update(cx, |stack, _, cx| stack.hand_over(cx)) else {
        return;
    };
    Previews::forget(handle, cx);
    if !items.is_empty() {
        open_stack(placement, items, cx);
    }
}

/// A recording's card, shown while it was being finished, now has its file. False if
/// the card is gone (its surface closed with its output, say).
pub(crate) fn saved(
    id: u64,
    finished: &screenie_record::Finished,
    copied: bool,
    cx: &mut App,
) -> bool {
    let (path, bytes, duration) = (finished.path.clone(), finished.bytes, finished.duration);
    with_card(id, cx, move |stack, cx| {
        stack.saved(id, path, bytes, duration, copied, cx)
    })
}

/// Take a card away, e.g. a recording that couldn't be finished.
pub(crate) fn discard(id: u64, cx: &mut App) {
    with_card(id, cx, move |stack, cx| stack.remove(id, cx));
}

/// Run `f` on the stack holding card `id`. False if none does.
fn with_card(
    id: u64,
    cx: &mut App,
    f: impl FnOnce(&mut PreviewStack, &mut Context<PreviewStack>),
) -> bool {
    let mut f = Some(f);
    for handle in Previews::handles(cx) {
        let _ = handle.update(cx, |stack, _, cx| {
            if stack.items.iter().any(|i| i.id == id)
                && let Some(f) = f.take()
            {
                f(stack, cx);
            }
        });
        if f.is_none() {
            return true;
        }
    }
    false
}

pub(crate) struct PreviewStack {
    position: ScreenPosition,
    /// The output the stack is on, where editors open too.
    output: Option<String>,
    items: Vec<PreviewItem>,
    /// Input: each card is an area, keyed by its id.
    hover: Entity<Hover<u64>>,
    /// When the cards' time was last counted.
    ticked: Instant,
    handle: Option<WindowHandle<Self>>,
    /// An overlay editor is open: the cards wait for it.
    editing: bool,
    /// The overlay editors that were open when this surface opened, so it's above them.
    above: Vec<AnyWindowHandle>,
}

impl PreviewStack {
    fn new(
        position: ScreenPosition,
        output: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        screenie_ui_kit::track_ui_scale(window, cx);
        let hover = Hover::new(window, cx);
        cx.observe(&hover, |stack, hover, cx| {
            let card = hover.read(cx).hovered().copied();
            stack.pointer_on(card, cx);
        })
        .detach();
        screenie_editor::observe_overlays(cx, Self::editors_changed).detach();
        let handle = window.window_handle().downcast::<Self>();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let alive = this.update_in(cx, |stack, window, cx| {
                    stack.expire(window, cx);
                    let done = stack.items.is_empty();
                    if done {
                        window.remove_window();
                    }
                    !done
                });
                if !matches!(alive, Ok(true)) {
                    // Emptied, or closed by the compositor along with its output.
                    if let Some(handle) = handle {
                        AsyncApp::update(cx, |cx| Previews::forget(handle, cx));
                    }
                    break;
                }
            }
        })
        .detach();
        Self {
            position,
            output,
            items: Vec::new(),
            hover,
            ticked: Instant::now(),
            handle,
            editing: screenie_editor::overlay_open(cx),
            above: screenie_editor::overlay_windows(cx),
        }
    }

    /// An overlay editor opened or closed. Cards taken while editing wait for the edit
    /// to be done, and stay within reach meanwhile: over the editor, rather than dimmed
    /// under its backdrop where they can't be pointed at.
    fn editors_changed(&mut self, cx: &mut Context<Self>) {
        let editing = screenie_editor::overlay_open(cx);
        if self.editing && !editing {
            // Done editing: the cards get their time from now.
            let deadline = Self::timeout(cx).map(|t| Instant::now() + t);
            for item in &mut self.items {
                item.deadline = deadline;
            }
        }
        self.editing = editing;
        let editors = screenie_editor::overlay_windows(cx);
        if editors.iter().any(|e| !self.above.contains(e)) {
            // Surfaces on the same layer stack in the order they're mapped, so the
            // stack moves to a new one above the editor.
            self.above = editors;
            if let Some(handle) = self.handle {
                cx.defer(move |cx| raise(handle, cx));
            }
        }
        cx.notify();
    }

    /// Give up the cards to a new surface.
    fn hand_over(&mut self, cx: &mut Context<Self>) -> Vec<PreviewItem> {
        cx.notify();
        let timeout = Self::timeout(cx);
        let mut items = std::mem::take(&mut self.items);
        for item in &mut items {
            item.arrived = true;
            if item.hovered {
                item.hovered = false;
                item.deadline = Self::lease(timeout);
            }
        }
        items
    }

    fn timeout(cx: &App) -> Option<Duration> {
        match Daemon::get(cx).config.preview.timeout {
            0 => None,
            s => Some(Duration::from_secs(s as u64)),
        }
    }

    fn push(&mut self, mut item: PreviewItem, cx: &mut Context<Self>) {
        // A card moved from another surface keeps its time.
        if !item.arrived {
            item.deadline = Self::timeout(cx).map(|t| Instant::now() + t);
        }
        self.items.push(item);
        if self.items.len() > MAX_CARDS {
            // The oldest that isn't in use: never the one under the pointer, or a
            // recording still being finished (its file would get no card).
            if let Some(i) = self.items.iter().position(|i| !i.pinned()) {
                self.items.remove(i);
            }
        }
        cx.notify();
    }

    /// Let the oldest cards go while the stack is taller than the surface (a small
    /// screen, tall captures, a large interface scale), rather than spill off it.
    fn fit(&mut self, window: &Window, cx: &App) {
        let height = f32::from(window.viewport_size().height);
        // Not laid out yet.
        if height <= 0. {
            return;
        }
        let k = screenie_ui_kit::ui_scale(cx);
        let cards: Vec<_> = self
            .items
            .iter()
            .map(|i| (i.card.1 * k, i.pinned()))
            .collect();
        for i in overflow(&cards, height - 2. * EDGE_MARGIN * k, GAP * k)
            .into_iter()
            .rev()
        {
            self.items.remove(i);
        }
    }

    fn expire(&mut self, window: &Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        // While their screen is recorded, cards are concealed. One that came meanwhile (a
        // screenshot) waits to be seen; one already seen runs out as usual, so a hidden
        // surface doesn't stay over a fullscreen game for the whole recording.
        if screenie_ui_kit::conceal::hidden(window, cx) {
            let hidden_for = now - self.ticked;
            let waiting = self.items.iter_mut().filter(|i| !i.seen);
            for deadline in waiting.filter_map(|i| i.deadline.as_mut()) {
                *deadline += hidden_for;
            }
        } else {
            self.items.iter_mut().for_each(|i| i.seen = true);
        }
        self.ticked = now;
        // Captures taken while editing wait for the edit to be done (see
        // `editors_changed`).
        if self.editing {
            return;
        }
        let before = self.items.len();
        self.items
            .retain(|i| i.pinned() || i.deadline.is_none_or(|d| d > now));
        if self.items.len() != before {
            cx.notify();
        }
    }

    fn remove(&mut self, id: u64, cx: &mut Context<Self>) {
        self.items.retain(|i| i.id != id);
        cx.notify();
    }

    fn saved(
        &mut self,
        id: u64,
        path: PathBuf,
        bytes: u64,
        duration: Duration,
        copied: bool,
        cx: &mut Context<Self>,
    ) {
        let timeout = Self::timeout(cx);
        let Some(item) = self.item(id) else { return };
        item.media = Media::Recording {
            duration,
            saving: false,
        };
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
            if !hovered {
                item.deadline = Self::lease(timeout);
            }
        }
        cx.notify();
    }

    /// Leaving a card gives it a fresh (shorter) lease so it doesn't vanish instantly.
    fn lease(timeout: Option<Duration>) -> Option<Instant> {
        timeout.map(|t| Instant::now() + t.min(Duration::from_secs(3)))
    }

    fn copy(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(item) = self.item(id) else { return };
        let (media, path) = (item.media.clone(), item.path.clone());
        let copying = cx.background_executor().spawn(async move {
            let result = match (media, path) {
                (Media::Screenshot { png, .. }, path) => {
                    clipboard::copy(clipboard::Content::Image {
                        png: png.to_vec(),
                        file: path.as_deref(),
                    })
                }
                (Media::Recording { .. }, Some(path)) => {
                    clipboard::copy(clipboard::Content::File(&path))
                }
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
        let Media::Screenshot {
            png,
            capture,
            noted,
            ..
        } = item.media.clone()
        else {
            return;
        };
        let path = crate::deliver::screenshot_path(&Daemon::get(cx).config, &capture);
        match crate::deliver::write_atomic(&path, &png) {
            Ok(()) => {
                Daemon::update(cx, |d, _| {
                    d.note_saved(noted, CaptureKind::Screenshot, path.clone())
                });
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
                let path = screenie_config::Paths::get()
                    .runtime_dir()
                    .join(format!("capture-{stamp}.png"));
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
        let Media::Screenshot {
            mut capture,
            actions,
            ..
        } = item.media.clone()
        else {
            return;
        };
        // The card stays if another edit is under way.
        if crate::editor::ensure_free(cx).is_err() {
            return;
        }
        let (path, copied) = (item.path.clone(), item.is_copied());
        capture.output = capture.output.or_else(|| self.output.clone());
        // Picked up again later, it opens centred rather than where it was taken.
        capture.placement = None;
        self.remove(id, cx);
        // Done does what the capture was taken to do. And if it's on the clipboard, the
        // edit replaces it there: pasting the original after blurring something out of
        // it would defeat the point.
        let actions = Actions {
            copy: actions.copy || copied,
            preview: false,
            edit: false,
            want_file: false,
            ..actions
        };
        if let Err(e) = crate::editor::open(capture, path, actions, Default::default(), cx) {
            tracing::warn!("{e:#}");
        }
    }

    fn delete(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(path) = self.item(id).and_then(|i| i.path.clone()) {
            match std::fs::remove_file(&path) {
                Ok(()) => Daemon::update(cx, |d, _| d.note_deleted(&path)),
                Err(e) => tracing::warn!("deleting {}: {e}", path.display()),
            }
        }
        self.remove(id, cx);
    }

    fn card(&self, item: &PreviewItem, cx: &mut Context<Self>) -> AnyElement {
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
            button(action, cx)
                .size(ui(30.))
                .bg(rgba(0x000000a6))
                .hover(|s| s.bg(rgba(0x000000d9)))
        };
        let middle = |action, cx: &mut Context<Self>| {
            button(action, cx)
                .size(ui(42.))
                .bg(rgba(0xffffff2e))
                .hover(|s| s.bg(rgba(0xffffff4d)))
                .active(|s| s.opacity(0.85))
        };
        let copied = item.is_copied();
        let done: Vec<&str> = [(copied, "Copied"), (saved, "Saved")]
            .into_iter()
            .filter(|d| d.0)
            .map(|d| d.1)
            .collect();
        // Bottom right, hovered or not: what's already been done with it (the buttons for
        // those are gone).
        let status = (!done.is_empty()).then(|| {
            div()
                .absolute()
                .bottom(ui(8.))
                .right(ui(8.))
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
            let offers: Vec<CardAction> = CardAction::ALL
                .into_iter()
                .filter(|a| a.offered(item, cx))
                .collect();
            let offered = |action| offers.contains(&action);
            let actions = div()
                .flex()
                .flex_row()
                .gap_3()
                .when(saving, |d| d.child(spinner()))
                .when(offered(CardAction::Copy), |d| {
                    d.child(middle(CardAction::Copy, cx))
                })
                .when(offered(CardAction::Save), |d| {
                    d.child(middle(CardAction::Save, cx))
                })
                .when(offered(CardAction::Reveal), |d| {
                    d.child(middle(CardAction::Reveal, cx))
                });
            // Rows, so nothing can overlap at any card size: the corner buttons with the
            // size caption between them, what's left to do in the middle, and Delete
            // along the bottom, clear of the pill in the corner.
            let slot = || div().flex_none().size(ui(30.));
            let top = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(slot().child(corner(CardAction::Dismiss, cx)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_center()
                        .text_size(ui(11.))
                        .text_color(rgba(0xffffffb3))
                        .child(item.caption()),
                )
                .child(slot().when(offered(CardAction::Annotate), |d| {
                    d.child(corner(CardAction::Annotate, cx))
                }));
            let center = div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(actions);
            let bottom =
                div()
                    .flex()
                    .flex_row()
                    .child(slot().when(offered(CardAction::Delete), |d| {
                        d.child(corner(CardAction::Delete, cx))
                    }));
            div()
                .absolute()
                .inset_0()
                .p(ui(6.))
                .rounded(ui(11.))
                .bg(rgba(0x00000099))
                .flex()
                .flex_col()
                .child(top)
                .child(center)
                .child(bottom)
        });

        let card = div()
            .id(("card", id))
            .relative()
            .w(px(w))
            .h(px(h))
            // Never squeezed: a stack that doesn't fit drops cards instead.
            .flex_none()
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
            .children(status)
            .child(Hover::area(&self.hover, id));
        if item.arrived {
            return card.into_any_element();
        }
        card.with_animation(
            ("card-in", id),
            Animation::new(Duration::from_millis(260)).with_easing(gpui::ease_out_quint()),
            move |el, t| {
                el.left(px(slide.0 * (1.0 - t)))
                    .top(px(slide.1 * (1.0 - t)))
                    .opacity(t)
            },
        )
        .into_any_element()
    }
}

/// Which of a stack's cards (oldest first: their heights, and whether each is pinned)
/// to let go so the rest fit in `room`: the oldest first, never a pinned one, and never
/// the newest.
fn overflow(cards: &[(f32, bool)], room: f32, gap: f32) -> Vec<usize> {
    let Some(newest) = cards.len().checked_sub(1) else {
        return Vec::new();
    };
    let mut height = cards.iter().map(|c| c.0).sum::<f32>() + gap * newest as f32;
    let mut drop = Vec::new();
    for (i, &(h, pinned)) in cards[..newest].iter().enumerate() {
        if height <= room {
            break;
        }
        if !pinned {
            drop.push(i);
            height -= h + gap;
        }
    }
    drop
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.fit(window, cx);
        // The newest card sits nearest the edge (at the bottom, for the side middles).
        let vertical = self.position.vertical();
        let mut cards: Vec<_> = self.items.iter().map(|item| self.card(item, cx)).collect();
        if vertical == Align::Start {
            cards.reverse();
        }
        let stack = div()
            .id("preview")
            .size_full()
            .font_family(screenie_ui_kit::FONT)
            .flex()
            .flex_col();
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
        let stack = screenie_ui_kit::conceal::root(stack, window, cx);
        Hover::root(&self.hover, stack, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stack_that_fits_keeps_every_card() {
        assert!(overflow(&[(100., false); 3], 324., 12.).is_empty());
        assert!(overflow(&[], 0., 12.).is_empty());
    }

    #[test]
    fn the_oldest_cards_go_first() {
        // 3 × 100 + 2 × 12 = 324: one too many for 300.
        assert_eq!(overflow(&[(100., false); 3], 300., 12.), [0]);
        assert_eq!(overflow(&[(100., false); 3], 150., 12.), [0, 1]);
    }

    #[test]
    fn pinned_cards_and_the_newest_stay() {
        let cards = [(100., true), (100., false), (100., false)];
        assert_eq!(overflow(&cards, 150., 12.), [1]);
        // Nothing else can go: it overflows rather than lose them.
        assert_eq!(
            overflow(&[(100., true), (100., false)], 50., 12.),
            Vec::<usize>::new()
        );
    }
}
