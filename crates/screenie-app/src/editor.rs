//! Opening captures in the annotation editor, and acting on what it hands back.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context as _;
use gpui::{App, AsyncApp, Task};
use screenie_annotate::{Color, Style};
use screenie_config::{EditorMode, Subject};
use screenie_core::Image;
use screenie_editor::{EditorOptions, Mode, OnDone, Output};
use screenie_ipc::CaptureKind;
use screenie_state::EditorState;

use crate::clipboard;
use crate::daemon::Daemon;
use crate::deliver::{
    Actions, Capture, encode_png, screenshot_path, suggested_path, write_atomic,
};

/// Why a second editor can't open.
const ALREADY_EDITING: &str = "Already editing a capture: finish this one first";

/// Overlay mode allows one editor at a time; if one is open, it says so and this is an
/// error.
pub(crate) fn ensure_free(cx: &mut App) -> anyhow::Result<()> {
    if screenie_editor::overlay_busy(ALREADY_EDITING, cx) {
        anyhow::bail!("already editing a capture; finish that edit first");
    }
    Ok(())
}

/// What an edit kept: the image as last copied, saved or handed over by Done (none if
/// Done had nothing to do), and the file it's saved in, if it is.
pub(crate) struct Kept {
    pub image: Option<Image>,
    pub path: Option<PathBuf>,
}

/// An open editor, to wait on (`screenie shot --edit`).
pub(crate) struct Editing(async_channel::Receiver<anyhow::Result<Option<Kept>>>);

impl Editing {
    /// Once the editor has closed: what it kept, nothing if the edit was abandoned
    /// (closed without Done, and neither copied nor saved), or why Done failed.
    pub async fn finished(self) -> anyhow::Result<Option<Kept>> {
        self.0.recv().await.unwrap_or(Ok(None))
    }
}

/// One open editor's side of things.
struct Edit {
    capture: Capture,
    actions: Actions,
    /// Where Save writes: the capture's file, once it has one (or `-o`).
    target: Option<PathBuf>,
    /// The file last written, and the image last kept.
    saved: Option<PathBuf>,
    kept: Option<Image>,
    /// `screenie query last` knows the capture already (it came with a file, or was
    /// saved here), so Done records it again only if it saves.
    noted: bool,
    /// Done is handing the image out, and says how the edit ended once it has.
    finishing: bool,
    ended: async_channel::Sender<anyhow::Result<Option<Kept>>>,
}

impl Edit {
    /// Where Save writes: the capture's file, or a new one in the screenshot folder.
    fn save_target(&self, cx: &App) -> PathBuf {
        self.target
            .clone()
            .unwrap_or_else(|| screenshot_path(&Daemon::get(cx).config, &self.capture))
    }

    /// `image` was kept (copied, or saved to `saved`).
    fn keep(&mut self, image: Image, saved: Option<PathBuf>) {
        if let Some(path) = saved {
            self.target = Some(path.clone());
            self.saved = Some(path);
        }
        self.kept = Some(image);
    }

    /// How the edit ended, if it ended with Done (`finished`) or kept something.
    fn outcome(&self, finished: bool) -> Option<Kept> {
        (finished || self.kept.is_some()).then(|| Kept {
            image: self.kept.clone(),
            path: self.saved.clone(),
        })
    }

    fn end(&self, outcome: anyhow::Result<Option<Kept>>) {
        let _ = self.ended.try_send(outcome);
    }
}

/// Open a capture for editing. `path` is where it's saved, if it is: Save and Done
/// write back there. `actions` decide what Done does (copy, save, hand the image over);
/// an already-saved capture is always saved back. Done ends the capture's journey: no
/// preview card follows.
pub(crate) fn open(
    capture: Capture,
    path: Option<PathBuf>,
    actions: Actions,
    cx: &mut App,
) -> anyhow::Result<Editing> {
    ensure_free(cx)?;
    let d = Daemon::get(cx);
    let config = &d.config.editor;
    let palette: Vec<Color> = config
        .palette
        .iter()
        .filter_map(|c| c.parse().ok())
        .collect();
    let style = remembered_style(d);
    // A path passed in holds this very image; `-o` only says where it should go.
    let on_disk = path.is_some();
    let saved = path.clone();
    let path = path.or_else(|| actions.output.clone());
    let title = match path.as_deref().and_then(|p| p.file_name()) {
        Some(name) => format!("{} — Screenie", name.to_string_lossy()),
        None => "Screenshot — Screenie".to_string(),
    };
    let options = EditorOptions {
        title,
        path: path.clone(),
        suggested_path: suggested_path(&d.config, &capture),
        scale: capture.scale,
        palette: if palette.is_empty() {
            default_palette()
        } else {
            palette
        },
        style,
        mode: match config.mode {
            EditorMode::Overlay => Mode::Overlay,
            EditorMode::Window => Mode::Window,
        },
        output: capture.output.as_deref().and_then(|name| {
            d.capture
                .outputs()
                .ok()?
                .into_iter()
                .find(|o| o.name == name)
        }),
        placement: capture.placement,
        on_disk,
        on_done: OnDone {
            copy: actions.copy,
            save: actions.save,
            hand_over: actions.want_file,
        },
        exit_on_copy: config.exit_on_copy,
        exit_on_save: config.exit_on_save,
        confirm_discard: config.confirm_discard,
    };
    let image = capture.image.clone();
    let (ended, waiting) = async_channel::bounded(1);
    let edit = Rc::new(RefCell::new(Edit {
        kept: on_disk.then(|| image.clone()),
        capture,
        actions,
        target: path,
        saved,
        noted: on_disk,
        finishing: false,
        ended,
    }));
    let handler = move |out: Output, cx: &mut App| handle(out, &edit, cx);
    screenie_editor::open(&image, options, handler, cx).context("opening the editor")?;
    Daemon::update(cx, |d, _| {
        d.editors += 1;
        d.broadcast();
    });
    Ok(Editing(waiting))
}

/// `screenie edit FILE`.
pub(crate) async fn open_file(path: PathBuf, cx: &mut AsyncApp) -> anyhow::Result<()> {
    cx.update(ensure_free)?;
    let load = path.clone();
    let image = cx
        .background_executor()
        .spawn(async move { Image::load_png(&load) })
        .await
        .with_context(|| format!("opening {} (only PNG images can be edited)", path.display()))?;
    // A file doesn't say what scale it was captured at; open it on the focused screen
    // and assume that one's.
    let capture_ctx = cx.update(|cx| Daemon::get(cx).capture.clone());
    let (output, scale) = cx
        .background_executor()
        .spawn(async move {
            let outputs = capture_ctx.outputs().unwrap_or_default();
            let focused = capture_ctx.focused_output();
            let output = outputs
                .iter()
                .find(|o| Some(&o.name) == focused.as_ref())
                .or(outputs.first());
            (
                output.map(|o| o.name.clone()),
                output.map_or(1.0, |o| o.scale as f32),
            )
        })
        .await;
    // Taken when it was last written, as far as anyone can tell.
    let taken = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .map_or_else(|_| chrono::Local::now(), chrono::DateTime::from);
    cx.update(|cx| {
        let config = &Daemon::get(cx).config.screenshot.after_capture;
        let actions = Actions::resolve(config, &Default::default(), None, false);
        let capture = Capture {
            image,
            scale,
            subject: Subject::default(),
            output,
            placement: None,
            taken,
        };
        open(capture, Some(path), actions, cx).map(drop)
    })
}

/// The style the last editor closed with (even in an earlier run), or the configured
/// defaults.
fn remembered_style(d: &Daemon) -> Style {
    let (config, last) = (&d.config.editor, &d.state.state().editor);
    let color = last
        .color
        .as_deref()
        .and_then(|c| c.parse().ok())
        .or_else(|| config.default_color.parse().ok());
    Style {
        color: color.unwrap_or(Style::default().color),
        size: last.size.unwrap_or(config.stroke_width as f32),
        fill: last.fill.unwrap_or_default(),
    }
}

fn remember_style(style: Style, d: &mut Daemon) {
    let result = d.state.update(|s| {
        s.editor = EditorState {
            color: Some(style.color.to_string()),
            size: Some(style.size),
            fill: Some(style.fill),
        };
    });
    if let Err(e) = result {
        tracing::warn!("{e}");
    }
}

fn default_palette() -> Vec<Color> {
    screenie_config::EditorConfig::default()
        .palette
        .iter()
        .filter_map(|c| c.parse().ok())
        .collect()
}

/// Act on what the editor hands back. The tasks end once the copy or save has happened
/// (see [`screenie_editor::OutputHandler`]).
fn handle(
    out: Output,
    edit: &Rc<RefCell<Edit>>,
    cx: &mut App,
) -> Task<anyhow::Result<Option<PathBuf>>> {
    match out {
        Output::Copy(image) => {
            let edit = edit.clone();
            cx.spawn(async move |cx| {
                let png = encode(image.clone(), cx).await?;
                copy(png, None, cx).await?;
                edit.borrow_mut().keep(image, None);
                Ok(None)
            })
        }
        Output::Save(image) => {
            let path = edit.borrow().save_target(cx);
            save(image, path, edit.clone(), cx)
        }
        Output::SaveAs(image, path) => save(image, png_path(path), edit.clone(), cx),
        Output::Done {
            image,
            copied,
            saved,
        } => finish(image, copied, saved, edit.clone(), cx),
        Output::Closed { style, finished } => {
            Daemon::update(cx, |d, _| {
                remember_style(style, d);
                d.editors = d.editors.saturating_sub(1);
                d.broadcast();
            });
            // Unless Done is handing the image out (and will say how that went): it
            // kept what was copied or saved along the way.
            let edit = edit.borrow();
            if !edit.finishing {
                edit.end(Ok(edit.outcome(finished)));
            }
            Task::ready(Ok(None))
        }
    }
}

/// Save (or Save As) `image` to `path`.
fn save(
    image: Image,
    path: PathBuf,
    edit: Rc<RefCell<Edit>>,
    cx: &mut App,
) -> Task<anyhow::Result<Option<PathBuf>>> {
    cx.spawn(async move |cx| {
        let png = encode(image.clone(), cx).await?;
        write(png, path.clone(), cx).await?;
        {
            let mut edit = edit.borrow_mut();
            edit.keep(image, Some(path.clone()));
            edit.noted = true;
        }
        let noted = path.clone();
        cx.update(|cx| {
            Daemon::update(cx, |d, _| {
                d.note_capture(CaptureKind::Screenshot, Some(noted))
            })
        });
        Ok(Some(path))
    })
}

/// Done: copy and save what isn't already (`copied`, `saved`), then say how the edit
/// ended to whoever waits for it.
fn finish(
    image: Image,
    copied: bool,
    saved: bool,
    edit: Rc<RefCell<Edit>>,
    cx: &mut App,
) -> Task<anyhow::Result<Option<PathBuf>>> {
    let (save_to, copy_too, file) = {
        let mut e = edit.borrow_mut();
        e.finishing = true;
        let save = (e.actions.save || e.target.is_some()) && !saved;
        let file = (save || saved).then(|| e.save_target(cx));
        (
            save.then(|| file.clone()).flatten(),
            e.actions.copy && !copied,
            file,
        )
    };
    cx.spawn(async move |cx| {
        let mut wrote = None;
        let result = async {
            if save_to.is_none() && !copy_too {
                return Ok(());
            }
            let png = encode(image.clone(), cx).await?;
            if let Some(path) = save_to {
                write(png.clone(), path.clone(), cx).await?;
                edit.borrow_mut().keep(image.clone(), Some(path.clone()));
                wrote = Some(path);
            }
            if copy_too {
                copy(png, file, cx).await?;
            }
            anyhow::Ok(())
        }
        .await;

        let mut e = edit.borrow_mut();
        // Recorded once per capture (once delivered), and again for each new version
        // of its file.
        if wrote.is_some() || (!e.noted && result.is_ok()) {
            e.noted = true;
            let path = e.saved.clone();
            cx.update(|cx| {
                Daemon::update(cx, |d, _| d.note_capture(CaptureKind::Screenshot, path))
            });
        }
        match result {
            Ok(()) => {
                e.kept = Some(image);
                e.end(Ok(e.outcome(true)));
                Ok(wrote)
            }
            Err(err) => {
                e.end(Err(anyhow::anyhow!("{err:#}")));
                Err(err)
            }
        }
    })
}

async fn encode(image: Image, cx: &AsyncApp) -> anyhow::Result<Arc<Vec<u8>>> {
    cx.background_executor()
        .spawn(async move { encode_png(&image).map(Arc::new) })
        .await
}

async fn write(png: Arc<Vec<u8>>, path: PathBuf, cx: &AsyncApp) -> anyhow::Result<()> {
    cx.background_executor()
        .spawn(async move {
            write_atomic(&path, &png).with_context(|| format!("saving {}", path.display()))
        })
        .await
}

/// Put the image on the clipboard (as a file too, if `file` holds it).
async fn copy(png: Arc<Vec<u8>>, file: Option<PathBuf>, cx: &AsyncApp) -> anyhow::Result<()> {
    cx.background_executor()
        .spawn(async move {
            clipboard::copy(clipboard::Content::Image {
                png: Arc::unwrap_or_clone(png),
                file: file.as_deref(),
            })
        })
        .await
}

/// Save As writes PNG, so the file's name says so: a name without an extension gets
/// `.png`, and another image format's (`shot.jpg`) is swapped for it.
fn png_path(path: PathBuf) -> PathBuf {
    const OTHER_IMAGES: &[&str] = &[
        "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "avif", "heic", "jxl", "qoi",
    ];
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match extension.as_deref() {
        Some("png") => path,
        Some(e) if OTHER_IMAGES.contains(&e) => path.with_extension("png"),
        _ => {
            let mut name = path.file_name().unwrap_or_default().to_os_string();
            name.push(".png");
            path.with_file_name(name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_as_names_say_png() {
        for (chosen, written) in [
            ("/p/bug.png", "/p/bug.png"),
            ("/p/bug.PNG", "/p/bug.PNG"),
            ("/p/bug", "/p/bug.png"),
            ("/p/bug.jpg", "/p/bug.png"),
            ("/p/bug.JPEG", "/p/bug.png"),
            ("/p/v1.2", "/p/v1.2.png"),
            ("/p/.hidden", "/p/.hidden.png"),
        ] {
            assert_eq!(png_path(chosen.into()), PathBuf::from(written), "{chosen}");
        }
    }
}
