//! After-capture actions: save, copy, preview, edit.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::AsyncApp;
use screenie_config::{AfterCapture, Config, Paths, Subject, expand_template, unique_path};
use screenie_core::{Image, Rect};
use screenie_ipc::{ActionOverrides, CaptureKind};

use crate::clipboard;
use crate::daemon::Daemon;
use crate::preview::{self, PreviewItem};

/// The after-capture actions in effect for one capture.
#[derive(Debug, Clone)]
pub(crate) struct Actions {
    pub copy: bool,
    pub save: bool,
    pub preview: bool,
    pub edit: bool,
    /// Save to exactly this path.
    pub output: Option<PathBuf>,
    /// The client wants a file even if saving is off.
    pub want_file: bool,
}

impl Actions {
    pub fn resolve(configured: &AfterCapture, overrides: &ActionOverrides, output: Option<PathBuf>, want_file: bool) -> Self {
        Self {
            copy: overrides.copy.unwrap_or(configured.copy),
            // An explicit output path implies saving.
            save: output.is_some() || overrides.save.unwrap_or(configured.save),
            preview: overrides.preview.unwrap_or(configured.preview),
            edit: overrides.edit.unwrap_or(configured.edit),
            output,
            want_file,
        }
    }
}

/// A finished screenshot and what it shows.
#[derive(Clone)]
pub(crate) struct Capture {
    pub image: Image,
    /// Image pixels per logical pixel.
    pub scale: f32,
    /// The window captured, if any (for the file name).
    pub subject: Subject,
    /// The output showing most of it, where cards and editors appear.
    pub output: Option<String>,
    /// Where it was on that output (logical, relative to the output), if entirely on it:
    /// the overlay editor shows it right there.
    pub placement: Option<Rect>,
}

pub(crate) struct Delivered {
    pub path: Option<PathBuf>,
    pub temporary: bool,
}

/// Write `bytes` to `path` atomically, creating parent directories.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!(
        "{}.part",
        path.extension().and_then(|e| e.to_str()).unwrap_or("tmp")
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Where a new screenshot would be saved.
pub(crate) fn screenshot_path(config: &Config, subject: &Subject) -> PathBuf {
    let dir = config.screenshot_dir();
    let stem = expand_template(&config.screenshot.filename, chrono::Local::now(), subject);
    unique_path(&dir, &stem, "png")
}

pub(crate) fn encode_png(image: &Image) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    image.write_png(&mut out, png::Compression::Balanced)?;
    Ok(out)
}

/// Run the after-capture actions for a screenshot. When editing, nothing is saved or
/// copied yet: the editor does that when it's done.
pub(crate) async fn screenshot(
    capture: Capture,
    mut actions: Actions,
    config: Config,
    cx: &mut AsyncApp,
) -> anyhow::Result<Delivered> {
    if actions.edit && !actions.want_file {
        // Overlay mode edits one capture at a time. Another one waits as a preview card,
        // to be picked up once this edit is done.
        if cx.update(|cx| screenie_editor::overlay_open(cx)) {
            actions.edit = false;
            actions.preview = true;
        } else {
            cx.update(|cx| crate::editor::open(capture, None, actions, cx))?;
            return Ok(Delivered { path: None, temporary: false });
        }
    }
    let started = std::time::Instant::now();
    let work_image = capture.image.clone();
    let work_actions = actions.clone();
    let subject = capture.subject.clone();
    let (path, temporary, png, copied) = cx
        .background_executor()
        .spawn(async move {
            let png = encode_png(&work_image)?;
            let saved = if work_actions.save {
                let path = work_actions.output.clone().unwrap_or_else(|| screenshot_path(&config, &subject));
                write_atomic(&path, &png)?;
                Some(path)
            } else {
                None
            };
            let temp = if saved.is_none() && work_actions.want_file {
                let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
                let path = Paths::get().runtime_dir().join(format!("capture-{stamp}.png"));
                write_atomic(&path, &png)?;
                Some(path)
            } else {
                None
            };
            let copied = work_actions.copy
                && clipboard::copy(clipboard::Content::Image { png: png.clone(), file: saved.as_deref() })
                    .inspect_err(|e| tracing::warn!("{e}"))
                    .is_ok();
            let temporary = temp.is_some();
            anyhow::Ok((saved.or(temp), temporary, png, copied))
        })
        .await?;
    tracing::info!(elapsed = ?started.elapsed(), path = ?path, "screenshot delivered");
    let kept = if temporary { None } else { path.clone() };
    cx.update(|cx| Daemon::update(cx, |d, _| d.note_capture(CaptureKind::Screenshot, kept)));

    if actions.preview {
        let saved = if temporary { None } else { path.clone() };
        let output = capture.output.clone();
        let item = PreviewItem::screenshot(capture, Arc::new(png), saved, copied, cx).await;
        cx.update(|cx| preview::show(item, output, cx));
    }
    Ok(Delivered { path, temporary })
}

/// Run the after-capture actions for a finished recording (it's already saved).
pub(crate) async fn recording(
    finished: screenie_record::Finished,
    actions: Actions,
    output_name: Option<String>,
    cx: &mut AsyncApp,
) {
    let path = finished.path.clone();
    cx.update(|cx| Daemon::update(cx, |d, _| d.note_capture(CaptureKind::Recording, Some(path))));
    let copied = actions.copy && {
        let path = finished.path.clone();
        cx.background_executor()
            .spawn(async move { clipboard::copy(clipboard::Content::File(&path)).inspect_err(|e| tracing::warn!("{e}")) })
            .await
            .is_ok()
    };
    if actions.preview {
        let item = PreviewItem::recording(finished, copied, cx).await;
        cx.update(|cx| preview::show(item, output_name, cx));
    }
}
