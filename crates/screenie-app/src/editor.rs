//! Opening captures in the annotation editor, and acting on what it hands back.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context as _;
use gpui::{App, AsyncApp};
use screenie_annotate::Color;
use screenie_core::Image;
use screenie_editor::{EditorOptions, Output};

use crate::clipboard;
use crate::daemon::Daemon;
use crate::deliver::{encode_png, screenshot_path, write_atomic};
use crate::preview::{self, PreviewItem};

/// Open `image` for editing. `path` is where the capture was saved, if it was: saving
/// from the editor overwrites it.
pub(crate) fn open(image: Image, scale: f32, path: Option<PathBuf>, output: Option<String>, cx: &mut App) {
    let d = Daemon::get(cx);
    let config = &d.config.editor;
    let palette: Vec<Color> = config.palette.iter().filter_map(|c| c.parse().ok()).collect();
    let mut style = d.editor_style.unwrap_or_default();
    if d.editor_style.is_none() {
        style.color = config.default_color.parse().unwrap_or(style.color);
        style.size = config.stroke_width as f32;
    }
    let title = match path.as_deref().and_then(|p| p.file_name()) {
        Some(name) => format!("{} — Screenie", name.to_string_lossy()),
        None => "Screenshot — Screenie".to_string(),
    };
    let options = EditorOptions {
        title,
        path: path.clone(),
        scale,
        palette: if palette.is_empty() { default_palette() } else { palette },
        style,
        output: output.clone(),
    };
    let target = Rc::new(RefCell::new(path));
    let handler = move |out: Output, cx: &mut App| handle(out, &target, scale, output.clone(), cx);
    if let Err(e) = screenie_editor::open(&image, options, handler, cx) {
        tracing::error!("cannot open the editor: {e:#}");
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
    let capture = cx.update(|cx| Daemon::get(cx).capture.clone());
    let scale = cx
        .background_executor()
        .spawn(async move {
            let outputs = capture.outputs().unwrap_or_default();
            let focused = capture.compositor().focused_output().ok().flatten();
            let output = outputs.iter().find(|o| Some(&o.name) == focused.as_ref()).or(outputs.first());
            output.map_or(1.0, |o| o.scale as f32)
        })
        .await;
    cx.update(|cx| open(image, scale, Some(path), None, cx));
    Ok(())
}

fn default_palette() -> Vec<Color> {
    screenie_config::EditorConfig::default().palette.iter().filter_map(|c| c.parse().ok()).collect()
}

fn handle(
    out: Output,
    target: &Rc<RefCell<Option<PathBuf>>>,
    scale: f32,
    output: Option<String>,
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
            let path = save_target(target, cx);
            save(image, path.clone(), cx);
            Ok(Some(format!("Saved {}", display_name(&path))))
        }
        Output::SaveAs(image, path) => {
            let path = if path.extension().is_none() { path.with_extension("png") } else { path };
            *target.borrow_mut() = Some(path.clone());
            save(image, path.clone(), cx);
            Ok(Some(format!("Saved {}", display_name(&path))))
        }
        Output::Done(image) => {
            let path = save_target(target, cx);
            cx.spawn(async move |cx| {
                let work = image.clone();
                let saved = path.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let png = encode_png(&work)?;
                        write_atomic(&saved, &png).with_context(|| format!("saving {}", saved.display()))?;
                        let copied = clipboard::copy(clipboard::Content::Image { png: png.clone(), file: Some(&saved) });
                        if let Err(e) = copied {
                            tracing::warn!("{e:#}");
                        }
                        anyhow::Ok(png)
                    })
                    .await;
                match result {
                    Ok(png) => {
                        let item = PreviewItem::screenshot(image, scale, Arc::new(png), Some(path), cx).await;
                        cx.update(|cx| preview::show(item, output, cx));
                    }
                    Err(e) => tracing::error!("{e:#}"),
                }
            })
            .detach();
            Ok(None)
        }
        Output::Closed { style } => {
            Daemon::update(cx, |d, _| d.editor_style = Some(style));
            Ok(None)
        }
    }
}

/// Where Save writes: the capture's file, or a new one in the screenshot folder.
fn save_target(target: &Rc<RefCell<Option<PathBuf>>>, cx: &App) -> PathBuf {
    let mut target = target.borrow_mut();
    target.get_or_insert_with(|| screenshot_path(&Daemon::get(cx).config)).clone()
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
