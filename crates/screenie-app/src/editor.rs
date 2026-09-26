//! Opening captures in the annotation editor, and acting on what it hands back.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context as _;
use gpui::{App, AsyncApp};
use screenie_annotate::Color;
use screenie_config::Subject;
use screenie_core::Image;
use screenie_editor::{EditorOptions, Output};
use screenie_ipc::CaptureKind;

use crate::clipboard;
use crate::daemon::Daemon;
use crate::deliver::{Actions, Capture, encode_png, screenshot_path, write_atomic};
use crate::preview::{self, PreviewItem};

/// Open a capture for editing. `path` is where it's saved, if it is: Save and Done
/// write back there. `actions` decide what Done does (copy, save, preview card); an
/// already-saved capture is always saved back.
pub(crate) fn open(capture: Capture, path: Option<PathBuf>, actions: Actions, cx: &mut App) {
    let d = Daemon::get(cx);
    let config = &d.config.editor;
    let palette: Vec<Color> = config.palette.iter().filter_map(|c| c.parse().ok()).collect();
    let mut style = d.editor_style.unwrap_or_default();
    if d.editor_style.is_none() {
        style.color = config.default_color.parse().unwrap_or(style.color);
        style.size = config.stroke_width as f32;
    }
    // A path passed in holds this very image; `-o` only says where it should go.
    let on_disk = path.is_some();
    let path = path.or_else(|| actions.output.clone());
    let title = match path.as_deref().and_then(|p| p.file_name()) {
        Some(name) => format!("{} — Screenie", name.to_string_lossy()),
        None => "Screenshot — Screenie".to_string(),
    };
    let options = EditorOptions {
        title,
        path: path.clone(),
        scale: capture.scale,
        palette: if palette.is_empty() { default_palette() } else { palette },
        style,
        output: capture.output.clone(),
        on_disk,
        exit_on_copy: config.exit_on_copy,
        exit_on_save: config.exit_on_save,
    };
    let image = capture.image.clone();
    let target = Rc::new(RefCell::new(path));
    let handler = move |out: Output, cx: &mut App| handle(out, &target, &capture, &actions, cx);
    match screenie_editor::open(&image, options, handler, cx) {
        Ok(_) => Daemon::update(cx, |d, _| {
            d.editors += 1;
            d.broadcast();
        }),
        Err(e) => tracing::error!("cannot open the editor: {e:#}"),
    }
}

/// `screenie edit FILE`.
pub(crate) async fn open_file(path: PathBuf, cx: &mut AsyncApp) -> anyhow::Result<()> {
    let load = path.clone();
    let image = cx
        .background_executor()
        .spawn(async move { Image::load_png(&load) })
        .await
        .with_context(|| format!("opening {} (only PNG images can be edited)", path.display()))?;
    // A file doesn't say what scale it was captured at; assume the focused screen's.
    let capture_ctx = cx.update(|cx| Daemon::get(cx).capture.clone());
    let scale = cx
        .background_executor()
        .spawn(async move {
            let outputs = capture_ctx.outputs().unwrap_or_default();
            let focused = capture_ctx.compositor().focused_output().ok().flatten();
            let output = outputs.iter().find(|o| Some(&o.name) == focused.as_ref()).or(outputs.first());
            output.map_or(1.0, |o| o.scale as f32)
        })
        .await;
    cx.update(|cx| {
        let config = &Daemon::get(cx).config.screenshot.after_capture;
        let actions = Actions::resolve(config, &Default::default(), None, false);
        let capture = Capture { image, scale, subject: Subject::default(), output: None };
        open(capture, Some(path), actions, cx);
    });
    Ok(())
}

fn default_palette() -> Vec<Color> {
    screenie_config::EditorConfig::default().palette.iter().filter_map(|c| c.parse().ok()).collect()
}

fn handle(
    out: Output,
    target: &Rc<RefCell<Option<PathBuf>>>,
    capture: &Capture,
    actions: &Actions,
    cx: &mut App,
) -> anyhow::Result<Option<String>> {
    match out {
        Output::Copy(image) => {
            cx.background_executor()
                .spawn(async move {
                    let result = encode_png(&image)
                        .and_then(|png| clipboard::copy(clipboard::Content::Image { png, file: None }));
                    if let Err(e) = result {
                        tracing::warn!("copying the edited image: {e:#}");
                    }
                })
                .detach();
            Ok(Some("Copied to clipboard".into()))
        }
        Output::Save(image) => {
            let path = save_target(target, capture, cx);
            save(image, path.clone(), cx);
            Daemon::update(cx, |d, _| d.note_capture(CaptureKind::Screenshot, Some(path.clone())));
            Ok(Some(format!("Saved {}", display_name(&path))))
        }
        Output::SaveAs(image, path) => {
            let path = if path.extension().is_none() { path.with_extension("png") } else { path };
            *target.borrow_mut() = Some(path.clone());
            save(image, path.clone(), cx);
            Daemon::update(cx, |d, _| d.note_capture(CaptureKind::Screenshot, Some(path.clone())));
            Ok(Some(format!("Saved {}", display_name(&path))))
        }
        Output::Done { image, copied, saved } => {
            // Whatever was already copied or saved exactly like this isn't done again.
            let save = (actions.save || target.borrow().is_some()) && !saved;
            let path = (save || saved).then(|| save_target(target, capture, cx));
            let (copy, preview) = (actions.copy && !copied, actions.preview);
            Daemon::update(cx, |d, _| d.note_capture(CaptureKind::Screenshot, path.clone()));
            let mut edited = Capture { image: image.clone(), ..capture.clone() };
            cx.spawn(async move |cx| {
                let file = path.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let png = encode_png(&image)?;
                        if save && let Some(file) = &file {
                            write_atomic(file, &png).with_context(|| format!("saving {}", file.display()))?;
                        }
                        if copy
                            && let Err(e) =
                                clipboard::copy(clipboard::Content::Image { png: png.clone(), file: file.as_deref() })
                        {
                            tracing::warn!("{e:#}");
                        }
                        anyhow::Ok(png)
                    })
                    .await;
                match result {
                    Ok(png) if preview => {
                        let output = edited.output.take();
                        let item = PreviewItem::screenshot(edited, Arc::new(png), path, cx).await;
                        cx.update(|cx| preview::show(item, output, cx));
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!("{e:#}"),
                }
            })
            .detach();
            Ok(None)
        }
        Output::Closed { style } => {
            Daemon::update(cx, |d, _| {
                d.editor_style = Some(style);
                d.editors = d.editors.saturating_sub(1);
                d.broadcast();
            });
            Ok(None)
        }
    }
}

/// Where Save writes: the capture's file, or a new one in the screenshot folder.
fn save_target(target: &Rc<RefCell<Option<PathBuf>>>, capture: &Capture, cx: &App) -> PathBuf {
    let mut target = target.borrow_mut();
    target.get_or_insert_with(|| screenshot_path(&Daemon::get(cx).config, &capture.subject)).clone()
}

fn save(image: Image, path: PathBuf, cx: &mut App) {
    cx.background_executor()
        .spawn(async move {
            let result = encode_png(&image).and_then(|png| {
                write_atomic(&path, &png).with_context(|| format!("saving {}", path.display()))
            });
            if let Err(e) = result {
                tracing::error!("{e:#}");
            }
        })
        .detach();
}

fn display_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}
