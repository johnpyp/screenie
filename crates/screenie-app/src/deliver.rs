//! After-capture actions: save, copy, preview, edit.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Context as _;
use chrono::{DateTime, Local};
use gpui::AsyncApp;
use screenie_config::{
    AfterCapture, Config, Paths, Subject, claim_unique, expand_template, unique_path,
};
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
    pub fn resolve(
        configured: &AfterCapture,
        overrides: &ActionOverrides,
        output: Option<PathBuf>,
        want_file: bool,
    ) -> Self {
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
    /// When the screen was frozen, which names its file however much later it's saved.
    pub taken: DateTime<Local>,
}

pub(crate) struct Delivered {
    pub path: Option<PathBuf>,
    pub temporary: bool,
}

/// Where a screenshot's journey stands once its after-capture actions have run.
pub(crate) enum Delivery {
    Delivered(Delivered),
    /// Open in the editor: delivered once it's done with.
    Editing {
        editing: crate::editor::Editing,
        /// The client wants a file even if the edit isn't saved.
        want_file: bool,
    },
}

impl Delivery {
    /// Wait for an edit to end. `None`: it was discarded.
    pub async fn finished(self, cx: &mut AsyncApp) -> anyhow::Result<Option<Delivered>> {
        let (editing, want_file) = match self {
            Self::Delivered(delivered) => return Ok(Some(delivered)),
            Self::Editing { editing, want_file } => (editing, want_file),
        };
        let Some(kept) = editing.finished().await? else {
            return Ok(None);
        };
        let image = match (kept.path, kept.image) {
            (Some(path), _) => {
                return Ok(Some(Delivered {
                    path: Some(path),
                    temporary: false,
                }));
            }
            (None, Some(image)) if want_file => image,
            (None, _) => {
                return Ok(Some(Delivered {
                    path: None,
                    temporary: false,
                }));
            }
        };
        let path = cx
            .background_executor()
            .spawn(async move {
                write_temporary(&encode_png(&image)?)
            })
            .await?;
        Ok(Some(Delivered {
            path: Some(path),
            temporary: true,
        }))
    }
}

/// Write `bytes` to `path` atomically, creating parent directories: readers see the old
/// file or the new one, never part of one. Nothing is left behind if it fails, not even
/// the empty file a [`claim_unique`]d name starts as.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    // Beside the file (a rename can't cross filesystems), hidden, and never shared.
    let tmp = dir.join(format!(
        ".{name}.{}-{}.part",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&tmp, bytes))
        .and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
        if std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == 0) {
            let _ = std::fs::remove_file(path);
        }
    }
    written
}

/// Write a capture to a new file for a client that wants one when it isn't saved
/// (`--stdout`); the client removes it once read.
fn write_temporary(png: &[u8]) -> anyhow::Result<PathBuf> {
    let stamp = chrono::Local::now().format("capture-%Y%m%d-%H%M%S%.3f");
    claim_unique(Paths::get().runtime_dir(), &stamp.to_string(), "png")
        .and_then(|path| write_atomic(&path, png).map(|()| path))
        .context("writing the capture to a temporary file")
}

/// A new file in the screenshot folder for `capture`, named as configured for when it
/// was taken. The name is claimed straight away, so two captures can't get the same one;
/// write it with [`write_atomic`].
pub(crate) fn screenshot_path(config: &Config, capture: &Capture) -> PathBuf {
    let dir = config.screenshot_dir();
    let stem = file_stem(config, capture);
    claim_unique(&dir, &stem, "png").unwrap_or_else(|e| {
        // Saving will fail too, and say why.
        tracing::debug!("claiming a name in {}: {e}", dir.display());
        unique_path(&dir, &stem, "png")
    })
}

/// Where a new screenshot of `capture` would go, without claiming the name: a starting
/// point to offer (Save As), not a file about to be written.
pub(crate) fn suggested_path(config: &Config, capture: &Capture) -> PathBuf {
    unique_path(&config.screenshot_dir(), &file_stem(config, capture), "png")
}

fn file_stem(config: &Config, capture: &Capture) -> String {
    expand_template(&config.screenshot.filename, capture.taken, &capture.subject)
}

/// Where `-o` puts a screenshot: in a directory (one that exists, or a path ending in
/// `/`), a new file named as configured; otherwise that file, with `.png` added if it has
/// no extension. An existing file is replaced.
pub(crate) fn output_path(output: &Path, config: &Config, capture: &Capture) -> PathBuf {
    if output.is_dir() || output.as_os_str().as_encoded_bytes().ends_with(b"/") {
        unique_path(output, &file_stem(config, capture), "png")
    } else if output.extension().is_none() {
        output.with_extension("png")
    } else {
        output.to_path_buf()
    }
}

/// PNG-encode a capture. Fast compression: a 4K screen takes a few tens of
/// milliseconds rather than half a second, for files a little larger.
pub(crate) fn encode_png(image: &Image) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    image.write_png(&mut out, png::Compression::Fast)?;
    Ok(out)
}

/// Run the after-capture actions for a screenshot. When editing, nothing is saved or
/// copied yet: the editor does that when it's done (see [`Delivery::finished`]).
pub(crate) async fn screenshot(
    capture: Capture,
    mut actions: Actions,
    config: Config,
    cx: &mut AsyncApp,
) -> anyhow::Result<Delivery> {
    if actions.edit {
        // Overlay mode edits one capture at a time. Another one waits as a preview card,
        // to be picked up once this edit is done.
        if cx.update(|cx| screenie_editor::overlay_open(cx)) {
            actions.edit = false;
            actions.preview = true;
        } else {
            let want_file = actions.want_file;
            let editing = cx.update(|cx| crate::editor::open(capture, None, actions, cx))?;
            return Ok(Delivery::Editing { editing, want_file });
        }
    }
    let started = std::time::Instant::now();
    let work_capture = capture.clone();
    let work_actions = actions.clone();
    let (path, temporary, png, copied, failed) = cx
        .background_executor()
        .spawn(async move {
            let png = encode_png(&work_capture.image)?;
            // A file that can't be written doesn't lose the capture: it's still copied
            // and previewed (where it can be saved again), and the error comes after.
            let saved = work_actions
                .save
                .then(|| {
                    let path = match &work_actions.output {
                        Some(output) => output.clone(),
                        None => screenshot_path(&config, &work_capture),
                    };
                    write_atomic(&path, &png)
                        .with_context(|| format!("saving {}", path.display()))
                        .map(|()| path)
                })
                .transpose();
            let (saved, failed) = match saved {
                Ok(saved) => (saved, None),
                Err(e) => (None, Some(e)),
            };
            let temp = if saved.is_none() && failed.is_none() && work_actions.want_file {
                let path = write_temporary(&png)?;
                Some(path)
            } else {
                None
            };
            let copied = work_actions.copy
                && clipboard::copy(clipboard::Content::Image {
                    png: png.clone(),
                    file: saved.as_deref(),
                })
                .inspect_err(|e| tracing::warn!("{e}"))
                .is_ok();
            let temporary = temp.is_some();
            anyhow::Ok((saved.or(temp), temporary, png, copied, failed))
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
    if let Some(e) = failed {
        return Err(e);
    }
    Ok(Delivery::Delivered(Delivered { path, temporary }))
}

/// Run the after-capture actions for a finished recording (it's already saved). `card`:
/// its preview card, shown while it was being finished.
pub(crate) async fn recording(
    finished: screenie_record::Finished,
    actions: Actions,
    output_name: Option<String>,
    card: Option<u64>,
    cx: &mut AsyncApp,
) {
    let path = finished.path.clone();
    cx.update(|cx| {
        Daemon::update(cx, |d, _| {
            d.note_capture(CaptureKind::Recording, Some(path))
        })
    });
    let copied = actions.copy && {
        let path = finished.path.clone();
        cx.background_executor()
            .spawn(async move {
                clipboard::copy(clipboard::Content::File(&path))
                    .inspect_err(|e| tracing::warn!("{e}"))
            })
            .await
            .is_ok()
    };
    if !actions.preview {
        return;
    }
    // Its card, or a new one if that's gone (its output unplugged, say).
    let finished_card =
        card.is_some_and(|card| cx.update(|cx| preview::saved(card, &finished, copied, cx)));
    if !finished_card {
        let item = PreviewItem::recording(finished, copied, cx).await;
        cx.update(|cx| preview::show(item, output_name, cx));
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use screenie_core::PixelFormat;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("screenie-deliver-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn capture() -> Capture {
        Capture {
            image: Image::new(1, 1, PixelFormat::Bgra),
            scale: 1.0,
            subject: Subject::default(),
            output: None,
            placement: None,
            taken: Local.with_ymd_and_hms(2026, 9, 26, 14, 2, 0).unwrap(),
        }
    }

    #[test]
    fn files_are_named_for_when_they_were_taken() {
        let dir = scratch("named");
        let mut config = Config::default();
        config.screenshot.directory = dir.clone();
        let first = screenshot_path(&config, &capture());
        let second = screenshot_path(&config, &capture());
        assert_eq!(
            first.file_name().unwrap(),
            "Screenshot_2026-09-26_14-02-00.png"
        );
        assert_eq!(
            second.file_name().unwrap(),
            "Screenshot_2026-09-26_14-02-00-2.png"
        );
        write_atomic(&first, b"png").unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"png");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_write_leaves_nothing_behind() {
        let dir = scratch("failed");
        // A directory where the file should go (`-o ~/Desktop` before directories were
        // understood): the rename fails.
        let target = dir.join("shot.png");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep"), b"").unwrap();
        assert!(write_atomic(&target, b"png").is_err());
        // No temporary file is left beside it.
        assert_eq!(files(&dir), ["shot.png"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn output_to_a_directory_or_without_an_extension() {
        let dir = scratch("output");
        let config = Config::default();
        let named = dir.join("Screenshot_2026-09-26_14-02-00.png");
        assert_eq!(output_path(&dir, &config, &capture()), named);
        let fresh = dir.join("new/");
        assert_eq!(
            output_path(&fresh, &config, &capture()),
            dir.join("new").join("Screenshot_2026-09-26_14-02-00.png")
        );
        assert_eq!(
            output_path(&dir.join("shot"), &config, &capture()),
            dir.join("shot.png")
        );
        assert_eq!(
            output_path(&dir.join("shot.jpeg"), &config, &capture()),
            dir.join("shot.jpeg")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
