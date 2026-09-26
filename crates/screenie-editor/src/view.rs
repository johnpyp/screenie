//! The editor window: a canvas with the capture, a tool bar along the top, and a style
//! bar along the bottom showing only what applies to the current tool or selection.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, BorderStyle, Bounds, BoxShadow, Context, CursorStyle, DispatchPhase, FocusHandle, FontWeight,
    Hitbox, HitboxBehavior, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    SharedString, Window, canvas, div, fill, point, px, quad, rgba, size,
};
use screenie_annotate::{Color, Handle, Kind, Redaction, Shape, Style};
use screenie_core::{Image, Point, Rect};
use screenie_ui_kit::Icon;
use screenie_ui_kit::hud::{self, ButtonStyle, HudButton, color};

use crate::raster::Raster;
use crate::session::{Cursor, Key, Modifiers, Outcome, Reach, Session};
use crate::tool::Tool;

/// What the editor hands back to its owner.
pub enum Output {
    Copy(Image),
    /// Save over the capture's file (or to a new one if it was never saved).
    Save(Image),
    SaveAs(Image, PathBuf),
    /// Save, copy, and close.
    Done(Image),
    /// The window closed; the last style, to start the next editor with.
    Closed { style: Style },
}

/// Handles an [`Output`]; a returned message is shown briefly in the editor.
pub type OutputHandler = Rc<dyn Fn(Output, &mut App) -> anyhow::Result<Option<String>>>;

/// Space kept free around the image for the bars.
const INSET_TOP: f32 = 64.0;
const INSET_BOTTOM: f32 = 68.0;
const INSET_SIDE: f32 = 28.0;

fn workspace() -> Hsla {
    rgba(0x141416ff).into()
}

/// Maps between image pixels and window pixels.
#[derive(Debug, Clone, Copy)]
struct Viewport {
    /// Window position of image pixel (0, 0).
    origin: gpui::Point<Pixels>,
    /// Window pixels per image pixel.
    zoom: f32,
}

impl Viewport {
    fn to_image(self, p: gpui::Point<Pixels>) -> Point {
        Point::new(f64::from((p.x - self.origin.x) / self.zoom), f64::from((p.y - self.origin.y) / self.zoom))
    }

    fn to_window(self, p: Point) -> gpui::Point<Pixels> {
        point(self.origin.x + px(p.x as f32 * self.zoom), self.origin.y + px(p.y as f32 * self.zoom))
    }

    fn rect(self, r: Rect) -> Bounds<Pixels> {
        Bounds::new(self.to_window(r.origin()), size(px(r.width as f32 * self.zoom), px(r.height as f32 * self.zoom)))
    }

    fn reach(self) -> Reach {
        let per_px = 1.0 / self.zoom as f64;
        Reach { tolerance: (6.0 * per_px).max(3.0), handle: 10.0 * per_px }
    }
}

pub struct Editor {
    session: Session,
    raster: Raster,
    palette: Vec<Color>,
    path: Option<PathBuf>,
    focus: FocusHandle,
    on_output: OutputHandler,
    /// Last pointer position over the canvas, in image pixels.
    pointer: Option<Point>,
    toast: Option<(SharedString, u64)>,
    /// Asking whether to save before closing.
    confirm_close: bool,
    closed: bool,
}

impl Editor {
    pub(crate) fn new(
        session: Session,
        palette: Vec<Color>,
        path: Option<PathBuf>,
        on_output: OutputHandler,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| this.update(cx, |e, cx| e.should_close(cx)).unwrap_or(true));
        Self {
            session,
            raster: Raster::default(),
            palette,
            path,
            focus,
            on_output,
            pointer: None,
            toast: None,
            confirm_close: false,
            closed: false,
        }
    }

    fn emit(&mut self, output: Output, cx: &mut Context<Self>) -> bool {
        let handler = self.on_output.clone();
        match handler(output, cx) {
            Ok(message) => {
                if let Some(message) = message {
                    self.show_toast(message, cx);
                }
                true
            }
            Err(e) => {
                tracing::error!("{e:#}");
                self.show_toast(format!("{e:#}"), cx);
                false
            }
        }
    }

    fn show_toast(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let generation = self.toast.as_ref().map_or(0, |(_, g)| g + 1);
        self.toast = Some((message.into(), generation));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1800)).await;
            let _ = this.update(cx, |e, cx| {
                if e.toast.as_ref().is_some_and(|(_, g)| *g == generation) {
                    e.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let image = self.session.export();
        self.emit(Output::Copy(image), cx);
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let image = self.session.export();
        if self.emit(Output::Save(image), cx) {
            self.session.mark_saved();
        }
        cx.notify();
    }

    fn save_as(&mut self, cx: &mut Context<Self>) {
        let dir = self.path.as_ref().and_then(|p| p.parent()).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        let chosen = cx.prompt_for_new_path(&dir, name.as_deref());
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = chosen.await else { return };
            let _ = this.update(cx, |e, cx| {
                let image = e.session.export();
                if e.emit(Output::SaveAs(image, path.clone()), cx) {
                    e.session.mark_saved();
                    e.path = Some(path);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn done(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let image = self.session.export();
        if self.emit(Output::Done(image), cx) {
            self.session.mark_saved();
            self.close(window, cx);
        }
    }

    /// Close, asking first if there are unsaved changes.
    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.commit_text();
        if self.session.doc().is_modified() {
            self.confirm_close = true;
            cx.notify();
        } else {
            self.close(window, cx);
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish(cx);
        window.remove_window();
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        if !std::mem::replace(&mut self.closed, true) {
            let style = self.session.style();
            self.emit(Output::Closed { style }, cx);
        }
    }

    /// The window manager's close button.
    fn should_close(&mut self, cx: &mut Context<Self>) -> bool {
        self.session.commit_text();
        if self.session.doc().is_modified() && !self.closed {
            self.confirm_close = true;
            cx.notify();
            return false;
        }
        self.finish(cx);
        true
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        let key = ks.key.as_str();
        cx.stop_propagation();

        if self.confirm_close {
            match key {
                "escape" => self.confirm_close = false,
                "enter" => self.done(window, cx),
                _ => {}
            }
            cx.notify();
            return;
        }

        if m.control || m.platform {
            match (key, m.shift) {
                ("z", false) => {
                    self.session.undo();
                }
                ("z", true) | ("y", _) => {
                    self.session.redo();
                }
                ("c", _) if self.session.text_edit().is_none() => self.copy(cx),
                ("s", false) => self.save(cx),
                ("s", true) => self.save_as(cx),
                ("d", _) => {
                    self.session.duplicate_selected();
                }
                ("v", _) if self.session.text_edit().is_some() => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        self.session.key(Key::Text(text), Modifiers::default());
                    }
                }
                ("w" | "q", _) => self.request_close(window, cx),
                ("enter", _) => self.done(window, cx),
                _ => return,
            }
            cx.notify();
            return;
        }

        let mods = Modifiers { shift: m.shift, ctrl: m.control, alt: m.alt };
        let mapped = match key {
            "escape" => Some(Key::Escape),
            "enter" => Some(Key::Enter),
            "backspace" => Some(Key::Backspace),
            "delete" => Some(Key::Delete),
            "left" => Some(Key::Left),
            "right" => Some(Key::Right),
            "up" => Some(Key::Up),
            "down" => Some(Key::Down),
            "home" => Some(Key::Home),
            "end" => Some(Key::End),
            _ if m.alt || m.function => None,
            _ => ks.key_char.clone().map(Key::Text),
        };
        let Some(mapped) = mapped else { return };
        match self.session.key(mapped, mods) {
            Outcome::Nothing => {}
            Outcome::Redraw => cx.notify(),
            Outcome::Done => self.done(window, cx),
            Outcome::Close => self.request_close(window, cx),
        }
    }

    // Canvas.

    fn viewport(&self, bounds: Bounds<Pixels>) -> Viewport {
        let doc = self.session.doc();
        // In crop mode the whole image shows, so the crop can grow again.
        let shown = if self.session.crop_edit().is_some() { doc.bounds() } else { doc.visible() };
        let avail_w = (f32::from(bounds.size.width) - INSET_SIDE * 2.0).max(40.0);
        let avail_h = (f32::from(bounds.size.height) - INSET_TOP - INSET_BOTTOM).max(40.0);
        // Never beyond the capture's own size on screen (logical 1:1), so it stays sharp.
        let zoom = (avail_w / shown.width as f32).min(avail_h / shown.height as f32).min(1.0 / doc.scale());
        let (w, h) = (shown.width as f32 * zoom, shown.height as f32 * zoom);
        let left = f32::from(bounds.origin.x) + INSET_SIDE + (avail_w - w) / 2.0;
        let top = f32::from(bounds.origin.y) + INSET_TOP + (avail_h - h) / 2.0;
        Viewport { origin: point(px(left - shown.x as f32 * zoom), px(top - shown.y as f32 * zoom)), zoom }
    }

    fn paint_canvas(&mut self, bounds: Bounds<Pixels>, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        for stale in self.raster.take_stale() {
            let _ = window.drop_image(stale);
        }
        let vp = self.viewport(bounds);
        let doc = self.session.doc();
        let crop_edit = self.session.crop_edit();
        let full = vp.rect(doc.bounds());
        let shown = if crop_edit.is_some() { full } else { vp.rect(doc.visible()) };

        window.paint_drop_shadows(shown, px(2.).into(), &[
            shadow(0x00000080, 10.0, 36.0),
            shadow(0x00000066, 1.0, 3.0),
        ]);
        let (composite, tile) = self.raster.update(&self.session);
        let tile = tile.map(|t| (t.image.clone(), t.rect));
        let _ = window.paint_image(shown, full, px(0.).into(), composite, 0, false);
        if let Some((image, rect)) = tile {
            let tile_bounds = vp.rect(rect);
            let _ = window.paint_image(shown.intersect(&tile_bounds), tile_bounds, px(0.).into(), image, 0, false);
        }
        window.paint_quad(quad(shown, px(2.), gpui::transparent_black(), px(1.), rgba(0xffffff14), BorderStyle::Solid));

        let doc = self.session.doc();
        if let Some(crop) = crop_edit {
            paint_crop(&vp, doc.bounds(), crop, window);
        } else {
            if let Some(shape) = self.session.selected().filter(|_| !self.session.is_drawing()) {
                paint_selection(&vp, shape, doc.scale(), window);
            }
            if let Some(edit) = self.session.text_edit()
                && let Some(shape) = doc.shape(edit.id)
            {
                paint_text_edit(&vp, shape, edit.caret, doc.scale(), window);
            }
        }

        let reach = vp.reach();
        let cursor = self.pointer.map_or(Cursor::Crosshair, |p| self.session.hover(p, reach));
        window.set_cursor_style(cursor_style(cursor), hitbox);

        let this = cx.entity();
        let hb = hitbox.clone();
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left || !hb.is_hovered(window) {
                return;
            }
            let p = vp.to_image(e.position);
            this.update(cx, |editor, cx| {
                window.focus(&editor.focus, cx);
                if editor.session.press(p, reach, e.click_count) {
                    cx.notify();
                }
            });
        });
        let this = cx.entity();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let p = vp.to_image(e.position);
            let mods = Modifiers { shift: e.modifiers.shift, ctrl: e.modifiers.control, alt: e.modifiers.alt };
            this.update(cx, |editor, cx| {
                editor.pointer = Some(p);
                if e.pressed_button == Some(MouseButton::Left) {
                    editor.session.drag(p, mods);
                }
                cx.notify();
            });
        });
        let this = cx.entity();
        window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                return;
            }
            this.update(cx, |editor, cx| {
                if editor.session.release() {
                    cx.notify();
                }
            });
        });
    }

    // Bars.

    fn button_for(&self, id: &'static str, cx: &mut Context<Self>, f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static) -> HudButton {
        let this = cx.entity();
        let f = Rc::new(f);
        HudButton::new(id).on_click(move |_, window, cx| {
            let f = f.clone();
            this.update(cx, |e, cx| {
                f(e, window, cx);
                cx.notify();
            });
        })
    }

    fn tool_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.session.tool();
        let mut bar = hud::panel();
        for tool in Tool::ALL {
            if tool == Tool::Crop {
                bar = bar.child(hud::separator());
            }
            let tip = format!("{}  {}", tool.name(), tool.key().to_ascii_uppercase());
            bar = bar.child(
                self.button_for(tool_id(tool), cx, move |e, _, _| e.session.set_tool(tool))
                    .icon(tool.icon())
                    .tooltip(tip)
                    .selected(current == tool),
            );
        }
        bar
    }

    fn history_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let doc = self.session.doc();
        hud::panel()
            .child(
                self.button_for("undo", cx, |e, _, _| {
                    e.session.undo();
                })
                .icon(Icon::Undo)
                .tooltip("Undo  Ctrl+Z")
                .disabled(!doc.can_undo()),
            )
            .child(
                self.button_for("redo", cx, |e, _, _| {
                    e.session.redo();
                })
                .icon(Icon::Redo)
                .tooltip("Redo  Ctrl+Shift+Z")
                .disabled(!doc.can_redo()),
            )
    }

    fn action_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        hud::panel()
            .child(self.button_for("copy", cx, |e, _, cx| e.copy(cx)).icon(Icon::Copy).tooltip("Copy  Ctrl+C"))
            .child(
                self.button_for("save", cx, |e, _, cx| e.save(cx))
                    .icon(Icon::Download)
                    .tooltip("Save  Ctrl+S · Save as  Ctrl+Shift+S"),
            )
            .child(
                div().ml_1().child(
                    self.button_for("done", cx, |e, window, cx| e.done(window, cx))
                        .icon(Icon::Check)
                        .label("Done")
                        .tooltip("Save, copy and close  Enter")
                        .style(ButtonStyle::Accent),
                ),
            )
    }

    /// Colour, size and per-tool options, for the selection or the current tool.
    fn style_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(crop) = self.session.crop_edit() {
            return Some(self.crop_bar(crop, cx).into_any_element());
        }
        let tool = match self.session.selected().or_else(|| {
            self.session.text_edit().and_then(|t| self.session.doc().shape(t.id))
        }) {
            Some(shape) => tool_for(&shape.kind),
            None => self.session.tool(),
        };
        if !(tool.uses_color() || tool.uses_size() || tool == Tool::Redact) {
            return None;
        }
        let style = self.session.style();
        let mut bar = hud::panel().gap_1();

        if tool.uses_color() {
            for (i, c) in self.palette.iter().copied().enumerate() {
                let this = cx.entity();
                bar = bar.child(swatch(("swatch", i), c, c == style.color).on_click(move |_, _, cx| {
                    this.update(cx, |e, cx| {
                        e.session.set_color(c);
                        cx.notify();
                    })
                }));
            }
        }
        if tool == Tool::Redact {
            let mode = self.session.redaction();
            for (m, label) in [(Redaction::Pixelate, "Pixelate"), (Redaction::Blur, "Blur")] {
                bar = bar.child(
                    self.button_for(if m == Redaction::Blur { "blur" } else { "pixelate" }, cx, move |e, _, _| {
                        e.session.set_redaction(m)
                    })
                    .label(label)
                    .selected(mode == m),
                );
            }
        }
        if tool.uses_size() {
            bar = bar.child(hud::separator());
            for (i, s) in Style::SIZES.into_iter().enumerate() {
                let dot = 4.0 + i as f32 * 2.5;
                let this = cx.entity();
                bar = bar.child(
                    div()
                        .id(("size", i))
                        .size(px(30.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(9.))
                        .cursor_pointer()
                        .when(style.size == s, |d| d.bg(color::selected()))
                        .hover(|d| d.bg(color::hover()))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(format!("Size {}  {}", i + 1, i + 1)).build(window, cx)
                        })
                        .child(div().size(px(dot)).rounded_full().bg(color::text()))
                        .on_click(move |_, _, cx| {
                            this.update(cx, |e, cx| {
                                e.session.set_size(s);
                                cx.notify();
                            })
                        }),
                );
            }
        }
        if tool.uses_fill() {
            let fill = style.fill;
            let tip = if tool == Tool::Text { "Label background  F" } else { "Fill  F" };
            let this = cx.entity();
            bar = bar.child(hud::separator()).child(
                div()
                    .id("fill")
                    .size(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(9.))
                    .cursor_pointer()
                    .when(fill, |d| d.bg(color::selected()))
                    .hover(|d| d.bg(color::hover()))
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx))
                    .child(
                        div()
                            .size(px(14.))
                            .rounded(px(3.))
                            .border_2()
                            .border_color(color::text())
                            .when(fill, |d| d.bg(color::text())),
                    )
                    .on_click(move |_, _, cx| {
                        this.update(cx, |e, cx| {
                            e.session.toggle_fill();
                            cx.notify();
                        })
                    }),
            );
        }
        Some(bar.into_any_element())
    }

    fn crop_bar(&self, crop: Rect, cx: &mut Context<Self>) -> impl IntoElement {
        hud::panel()
            .child(
                div()
                    .px_2()
                    .text_color(color::text_dim())
                    .text_size(px(12.5))
                    .child(format!("{} × {}", crop.width.round(), crop.height.round())),
            )
            .child(hud::separator())
            .child(self.button_for("crop-reset", cx, |e, _, _| e.session.reset_crop()).label("Reset"))
            .child(self.button_for("crop-cancel", cx, |e, _, _| e.session.cancel_crop()).label("Cancel").tooltip("Esc"))
            .child(
                div().ml_1().child(
                    self.button_for("crop-apply", cx, |e, _, _| e.session.apply_crop())
                        .icon(Icon::Crop)
                        .label("Crop")
                        .tooltip("Enter")
                        .style(ButtonStyle::Accent),
                ),
            )
    }

    fn close_prompt(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let card = div()
            .occlude()
            .w(px(340.))
            .p(px(18.))
            .flex()
            .flex_col()
            .gap_3()
            .rounded(px(14.))
            .bg(color::panel_solid())
            .border_1()
            .border_color(color::hairline())
            .shadow(hud::panel_shadow())
            .text_color(color::text())
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("Save your changes?"))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(color::text_dim())
                    .child("Your annotations will be lost if you close without saving."),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_1()
                    .child(self.button_for("discard", cx, |e, window, cx| e.close(window, cx)).label("Discard"))
                    .child(self.button_for("keep-editing", cx, |e, _, _| e.confirm_close = false).label("Cancel"))
                    .child(
                        self.button_for("save-close", cx, |e, window, cx| e.done(window, cx))
                            .label("Save")
                            .style(ButtonStyle::Accent),
                    ),
            );
        div().absolute().inset_0().bg(color::scrim(0.45)).flex().items_center().justify_center().child(card)
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_window_edited(self.session.doc().is_modified());
        let this = cx.entity();
        let canvas = canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox: Hitbox, window, cx| {
                this.update(cx, |editor, cx| editor.paint_canvas(bounds, &hitbox, window, cx));
            },
        )
        .absolute()
        .inset_0();

        let top = div()
            .absolute()
            .top(px(12.))
            .left(px(12.))
            .right(px(12.))
            .flex()
            .flex_row()
            .justify_between()
            .items_start()
            .child(self.history_bar(cx))
            .child(self.tool_bar(cx))
            .child(self.action_bar(cx));
        let bottom = self.style_bar(cx).map(|bar| {
            div().absolute().bottom(px(14.)).left_0().right_0().flex().flex_row().justify_center().child(bar)
        });
        let toast = self.toast.as_ref().map(|(message, _)| {
            div()
                .absolute()
                .bottom(px(70.))
                .left_0()
                .right_0()
                .flex()
                .flex_row()
                .justify_center()
                .child(hud::pill(message.clone()))
        });

        div()
            .id("editor")
            .size_full()
            .relative()
            .bg(workspace())
            .font_family(screenie_ui_kit::FONT)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(canvas)
            .child(top)
            .children(bottom)
            .children(toast)
            .when(self.confirm_close, |d| d.child(self.close_prompt(cx)))
    }
}

fn tool_id(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "tool-select",
        Tool::Arrow => "tool-arrow",
        Tool::Line => "tool-line",
        Tool::Rectangle => "tool-rectangle",
        Tool::Ellipse => "tool-ellipse",
        Tool::Pen => "tool-pen",
        Tool::Highlighter => "tool-highlighter",
        Tool::Text => "tool-text",
        Tool::Step => "tool-step",
        Tool::Redact => "tool-redact",
        Tool::Spotlight => "tool-spotlight",
        Tool::Crop => "tool-crop",
    }
}

/// The tool that makes a kind of shape, for showing its options.
fn tool_for(kind: &Kind) -> Tool {
    match kind {
        Kind::Arrow { .. } => Tool::Arrow,
        Kind::Line { .. } => Tool::Line,
        Kind::Rectangle { .. } => Tool::Rectangle,
        Kind::Ellipse { .. } => Tool::Ellipse,
        Kind::Pen { .. } => Tool::Pen,
        Kind::Highlighter { .. } => Tool::Highlighter,
        Kind::Text { .. } => Tool::Text,
        Kind::Step { .. } => Tool::Step,
        Kind::Redact { .. } => Tool::Redact,
        Kind::Spotlight { .. } => Tool::Spotlight,
    }
}

fn swatch(id: impl Into<gpui::ElementId>, c: Color, selected: bool) -> gpui::Stateful<gpui::Div> {
    let fill: Hsla = gpui::Rgba { r: c.r as f32 / 255.0, g: c.g as f32 / 255.0, b: c.b as f32 / 255.0, a: 1.0 }.into();
    div()
        .id(id.into())
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .border_2()
        .border_color(if selected { color::text() } else { gpui::transparent_black() })
        .hover(|d| d.bg(color::hover()))
        .child(div().size(px(18.)).rounded_full().bg(fill).border_1().border_color(rgba(0xffffff40)))
}

fn shadow(color: u32, y: f32, blur: f32) -> BoxShadow {
    BoxShadow { color: rgba(color).into(), offset: point(px(0.), px(y)), blur_radius: px(blur), spread_radius: px(0.), inset: false }
}

fn cursor_style(cursor: Cursor) -> CursorStyle {
    match cursor {
        Cursor::Arrow => CursorStyle::Arrow,
        Cursor::Crosshair => CursorStyle::Crosshair,
        Cursor::Text => CursorStyle::IBeam,
        Cursor::Move => CursorStyle::OpenHand,
        Cursor::Grabbing => CursorStyle::ClosedHand,
        Cursor::Resize(Handle::TopLeft | Handle::BottomRight) => CursorStyle::ResizeUpLeftDownRight,
        Cursor::Resize(Handle::TopRight | Handle::BottomLeft) => CursorStyle::ResizeUpRightDownLeft,
        Cursor::Resize(Handle::Top | Handle::Bottom) => CursorStyle::ResizeUpDown,
        Cursor::Resize(Handle::Left | Handle::Right) => CursorStyle::ResizeLeftRight,
        Cursor::Resize(Handle::Start | Handle::End) => CursorStyle::ClosedHand,
    }
}

fn handle_square(center: gpui::Point<Pixels>, window: &mut Window) {
    let b = Bounds::new(point(center.x - px(4.5), center.y - px(4.5)), size(px(9.), px(9.)));
    window.paint_drop_shadows(b, px(2.).into(), &[shadow(0x00000059, 1.0, 3.0)]);
    window.paint_quad(quad(b, px(2.), gpui::white(), px(1.5), color::accent(), BorderStyle::Solid));
}

fn handle_dot(center: gpui::Point<Pixels>, window: &mut Window) {
    let b = Bounds::new(point(center.x - px(5.), center.y - px(5.)), size(px(10.), px(10.)));
    window.paint_drop_shadows(b, px(5.).into(), &[shadow(0x00000059, 1.0, 3.0)]);
    window.paint_quad(quad(b, px(5.), gpui::white(), px(1.5), color::accent(), BorderStyle::Solid));
}

fn paint_selection(vp: &Viewport, shape: &Shape, scale: f32, window: &mut Window) {
    let accent = color::accent();
    match &shape.kind {
        Kind::Arrow { .. } | Kind::Line { .. } => {
            for (_, p) in shape.handles() {
                handle_dot(vp.to_window(p), window);
            }
        }
        Kind::Rectangle { rect } | Kind::Ellipse { rect } | Kind::Redact { rect, .. } | Kind::Spotlight { rect } => {
            window.paint_quad(quad(vp.rect(*rect), px(0.), gpui::transparent_black(), px(1.), accent, BorderStyle::Solid));
            for (handle, p) in shape.handles() {
                if handle.is_visible() {
                    handle_square(vp.to_window(p), window);
                }
            }
        }
        _ => {
            let pad = shape.stroke(scale) / 2.0 + 4.0 / vp.zoom as f64;
            let outline = vp.rect(shape.bounds(scale).inset(-pad));
            window.paint_quad(quad(outline, px(3.), gpui::transparent_black(), px(1.), accent, BorderStyle::Dashed));
        }
    }
}

fn paint_text_edit(vp: &Viewport, shape: &Shape, caret: usize, scale: f32, window: &mut Window) {
    let (Kind::Text { origin, .. }, Some(block)) = (&shape.kind, shape.text_block(scale)) else { return };
    // A box a little larger than the text, so an empty one is still visible.
    let min_w = block.line_height() as f64 * 0.6;
    let mut bounds = shape.bounds(scale);
    bounds.width = bounds.width.max(min_w);
    let outline = vp.rect(bounds.inset(-4.0 / vp.zoom as f64));
    window.paint_quad(quad(outline, px(3.), gpui::transparent_black(), px(1.), rgba(0xffffffb3), BorderStyle::Dashed));
    let (x, y) = block.caret(caret);
    let top = vp.to_window(Point::new(origin.x + x as f64, origin.y + y as f64));
    let height = px(block.line_height() * vp.zoom);
    let color = if shape.style.fill { shape.style.color.contrasting() } else { shape.style.color };
    let c: Hsla = gpui::Rgba { r: color.r as f32 / 255.0, g: color.g as f32 / 255.0, b: color.b as f32 / 255.0, a: 1.0 }.into();
    window.paint_quad(fill(Bounds::new(point(top.x - px(1.), top.y), size(px(2.), height)), c));
}

fn paint_crop(vp: &Viewport, image: Rect, crop: Rect, window: &mut Window) {
    let dim = color::scrim(0.55);
    let outer = vp.rect(image);
    let hole = vp.rect(crop);
    for b in [
        Bounds::new(outer.origin, size(outer.size.width, hole.origin.y - outer.origin.y)),
        Bounds::new(point(outer.origin.x, hole.bottom()), size(outer.size.width, outer.bottom() - hole.bottom())),
        Bounds::new(point(outer.origin.x, hole.origin.y), size(hole.origin.x - outer.origin.x, hole.size.height)),
        Bounds::new(point(hole.right(), hole.origin.y), size(outer.right() - hole.right(), hole.size.height)),
    ] {
        if b.size.width > px(0.) && b.size.height > px(0.) {
            window.paint_quad(fill(b, dim));
        }
    }
    let grid: Hsla = rgba(0xffffff59).into();
    for k in [1.0 / 3.0, 2.0 / 3.0] {
        let x = hole.origin.x + hole.size.width * k;
        let y = hole.origin.y + hole.size.height * k;
        window.paint_quad(fill(Bounds::new(point(x, hole.origin.y), size(px(1.), hole.size.height)), grid));
        window.paint_quad(fill(Bounds::new(point(hole.origin.x, y), size(hole.size.width, px(1.))), grid));
    }
    window.paint_quad(quad(hole, px(0.), gpui::transparent_black(), px(1.5), gpui::white(), BorderStyle::Solid));
    for handle in Handle::BOX {
        let c = vp.to_window(handle.position(&crop));
        let (w, h) = match handle {
            Handle::Top | Handle::Bottom => (px(26.), px(6.)),
            Handle::Left | Handle::Right => (px(6.), px(26.)),
            _ => (px(13.), px(13.)),
        };
        let b = Bounds::new(point(c.x - w / 2., c.y - h / 2.), size(w, h));
        window.paint_drop_shadows(b, px(2.5).into(), &[shadow(0x00000073, 1.0, 4.0)]);
        window.paint_quad(quad(b, px(2.5), gpui::white(), px(0.), gpui::transparent_black(), BorderStyle::Solid));
    }
}
