//! After-capture actions: save, copy, preview, edit.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::AsyncApp;
use screenie_config::{AfterCapture, Config, Paths, expand_template, unique_path};
use screenie_core::Image;
use screenie_ipc::ActionOverrides;

use crate::clipboard;
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
pub(crate) fn screenshot_path(config: &Config) -> PathBuf {
    let dir = config.screenshot_dir();
    let stem = expand_template(&config.screenshot.filename, chrono::Local::now());
    unique_path(&dir, &stem, "png")
}

pub(crate) fn encode_png(image: &Image) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    image.write_png(&mut out, png::Compression::Balanced)?;
    Ok(out)
}

/// Run the after-capture actions for a screenshot.
pub(crate) async fn screenshot(
    image: Image,
    scale: f32,
    actions: Actions,
    config: Config,
    output_name: Option<String>,
    cx: &mut AsyncApp,
) -> anyhow::Result<Delivered> {
    let started = std::time::Instant::now();
    let work_image = image.clone();
    let work_actions = actions.clone();
    let (path, temporary, png) = cx
        .background_executor()
        .spawn(async move {
            let png = encode_png(&work_image)?;
            let saved = if work_actions.save {
                let path = work_actions.output.clone().unwrap_or_else(|| screenshot_path(&config));
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
            if work_actions.copy
                && let Err(e) = clipboard::copy(clipboard::Content::Image { png: png.clone(), file: saved.as_deref() })
            {
                tracing::warn!("{e}");
            }
            let temporary = temp.is_some();
            anyhow::Ok((saved.or(temp), temporary, png))
        })
        .await?;
    tracing::info!(elapsed = ?started.elapsed(), path = ?path, "screenshot delivered");

    let saved = if temporary { None } else { path.clone() };
    if actions.edit {
        // The editor takes the preview card's place.
        cx.update(|cx| crate::editor::open(image, scale, saved, output_name, cx));
    } else if actions.preview {
        let item = PreviewItem::screenshot(image, scale, Arc::new(png), saved, cx).await;
        cx.update(|cx| preview::show(item, output_name, cx));
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
    if actions.copy {
        let path = finished.path.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = clipboard::copy(clipboard::Content::File(&path)) {
                    tracing::warn!("{e}");
                }
            })
            .detach();
    }
    if actions.preview {
        let item = PreviewItem::recording(finished, cx).await;
        cx.update(|cx| preview::show(item, output_name, cx));
    }
}
