//! What's shown of a fresh capture, and what can be done with it from there: copy it,
//! save it, annotate it, open it, show it in its folder, delete it.
//!
//! Where the compositor has layer-shell, that's a card at the edge of the screen
//! ([`card`]). Where it doesn't (GNOME), it's a desktop notification with the same
//! actions ([`notice`]), which is how GNOME apps tell of something done in the background.

mod card;
mod notice;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AsyncApp, Task};
use screenie_core::Image;
use screenie_ipc::CaptureKind;

use crate::clipboard;
use crate::daemon::Daemon;
use crate::deliver::{Actions, Capture};
use crate::last::CaptureId;

pub(crate) use card::NAMESPACE;

/// The largest thumbnail edge kept: enough for a card at twice its size.
const THUMB_MAX: f32 = 2.0 * card::CARD_MAX;

/// What can be done with a capture on show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Copy,
    Save,
    Reveal,
    Dismiss,
    Delete,
    Annotate,
}

impl Action {
    const ALL: [Action; 6] = [
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

/// A capture on show.
#[derive(Clone)]
pub(crate) struct PreviewItem {
    id: u64,
    media: Media,
    /// The capture (a recording's last frame) scaled down, keeping its aspect ratio.
    thumb: Image,
    pixel_size: (u32, u32),
    bytes: u64,
    path: Option<PathBuf>,
    /// The clipboard generation it was copied at, if it was: it's on the clipboard
    /// until we copy something else.
    copied: Option<u64>,
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
        let fit = (THUMB_MAX / iw).min(THUMB_MAX / ih).min(1.0);
        let (tw, th) = (((iw * fit) as u32).max(1), ((ih * fit) as u32).max(1));
        let pixel_size = (image.width(), image.height());
        let thumb = cx
            .background_executor()
            .spawn(async move { image.resize(tw, th) })
            .await;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            media,
            thumb,
            pixel_size,
            bytes,
            path,
            copied,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    fn is_saving(&self) -> bool {
        matches!(self.media, Media::Recording { saving: true, .. })
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

    /// A recording's file is written.
    fn saved(&mut self, finished: &screenie_record::Finished, copied: bool) {
        self.media = Media::Recording {
            duration: finished.duration,
            saving: false,
        };
        self.path = Some(finished.path.clone());
        self.bytes = finished.bytes;
        self.copied = copied.then(clipboard::generation);
    }

    /// Put it on the clipboard, in the background: the clipboard generation it's there
    /// at.
    fn copy(&self, cx: &App) -> Task<anyhow::Result<u64>> {
        let (media, path) = (self.media.clone(), self.path.clone());
        cx.background_executor().spawn(async move {
            match (media, path) {
                (Media::Screenshot { png, .. }, path) => {
                    clipboard::copy(clipboard::Content::Image {
                        png: png.to_vec(),
                        file: path.as_deref(),
                    })?
                }
                (Media::Recording { .. }, Some(path)) => {
                    clipboard::copy(clipboard::Content::File(&path))?
                }
                (Media::Recording { .. }, None) => {}
            }
            Ok(clipboard::generation())
        })
    }

    /// Save a screenshot that isn't where screenshots go.
    fn save(&mut self, cx: &mut App) {
        let Media::Screenshot {
            png,
            capture,
            noted,
            ..
        } = &self.media
        else {
            return;
        };
        if self.path.is_some() {
            return;
        }
        let path = crate::deliver::screenshot_path(&Daemon::get(cx).config, capture);
        match crate::deliver::write_atomic(&path, png) {
            Ok(()) => {
                let noted = *noted;
                Daemon::update(cx, |d, _| {
                    d.note_saved(noted, CaptureKind::Screenshot, path.clone())
                });
                self.path = Some(path);
            }
            Err(e) => tracing::error!("saving {}: {e}", path.display()),
        }
    }

    /// Open the file in its default app (image viewer, video player). An unsaved
    /// screenshot is opened from a temporary file. Whether it opened.
    fn open(&self, cx: &mut App) -> bool {
        let path = match (&self.path, &self.media) {
            (Some(path), _) => path.clone(),
            (None, Media::Screenshot { png, .. }) => {
                let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
                let path = screenie_config::Paths::get()
                    .runtime_dir()
                    .join(format!("capture-{stamp}.png"));
                if let Err(e) = crate::deliver::write_atomic(&path, png) {
                    tracing::error!("writing {}: {e}", path.display());
                    return false;
                }
                path
            }
            (None, Media::Recording { .. }) => return false,
        };
        cx.open_with_system(&path);
        true
    }

    /// Show the file in its folder. Whether there's one.
    fn reveal(&self, cx: &mut App) -> bool {
        self.path
            .as_ref()
            .inspect(|path| cx.reveal_path(path))
            .is_some()
    }

    /// Annotate it, on `output`. Whether the editor opened (the preview makes way).
    fn edit(&self, output: Option<String>, cx: &mut App) -> bool {
        let Media::Screenshot {
            mut capture,
            actions,
            ..
        } = self.media.clone()
        else {
            return false;
        };
        // It stays if another edit is under way.
        if crate::editor::ensure_free(cx).is_err() {
            return false;
        }
        capture.output = capture.output.or(output);
        // Picked up again later, it opens centred rather than where it was taken.
        capture.placement = None;
        // Done does what the capture was taken to do. And if it's on the clipboard, the
        // edit replaces it there: pasting the original after blurring something out of
        // it would defeat the point.
        let actions = Actions {
            copy: actions.copy || self.is_copied(),
            preview: false,
            edit: false,
            want_file: false,
            ..actions
        };
        if let Err(e) =
            crate::editor::open(capture, self.path.clone(), actions, Default::default(), cx)
        {
            tracing::warn!("{e:#}");
        }
        true
    }

    /// Delete the file, if there is one.
    fn delete(&self, cx: &mut App) {
        if let Some(path) = &self.path {
            match std::fs::remove_file(path) {
                Ok(()) => Daemon::update(cx, |d, _| d.note_deleted(path)),
                Err(e) => tracing::warn!("deleting {}: {e}", path.display()),
            }
        }
    }
}

/// Cards float over other windows (layer-shell, or GNOME's with screenie's extension);
/// where they can't, captures are told of in notifications.
fn cards(cx: &App) -> bool {
    screenie_ui_kit::floats(Daemon::get(cx).capture.support().layer_shell)
}

/// Show a capture, on `output` if given.
pub(crate) fn show(item: PreviewItem, output: Option<String>, cx: &mut App) {
    if cards(cx) {
        card::show(item, output, cx)
    } else {
        notice::show(item, cx)
    }
}

/// A recording shown while it was being finished now has its file. False if it's no
/// longer shown (its card went with its output, say).
pub(crate) fn saved(
    id: u64,
    finished: &screenie_record::Finished,
    copied: bool,
    cx: &mut App,
) -> bool {
    if cards(cx) {
        card::saved(id, finished, copied, cx)
    } else {
        notice::saved(id, finished, copied, cx)
    }
}

/// Take a capture away, e.g. a recording that couldn't be finished.
pub(crate) fn discard(id: u64, cx: &mut App) {
    if cards(cx) {
        card::discard(id, cx)
    } else {
        notice::discard(id, cx)
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
