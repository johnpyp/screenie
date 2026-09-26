//! The editor view: a canvas with the capture, the tool bar, and a style bar showing
//! only what applies to the current tool or selection. As an overlay the bars hang off
//! the capture; in a window they run along the top and bottom.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, BorderStyle, Bounds, BoxShadow, Context, CursorStyle, DispatchPhase, FocusHandle, FontWeight,
    Hitbox, HitboxBehavior, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    ScrollDelta, ScrollWheelEvent, SharedString, Size, Window, canvas, div, fill, point, px, quad, rgba, size,
};
use screenie_annotate::{Color, Handle, Kind, Redaction, Shape, Style};
use screenie_core::{Image, Point, Rect};
use screenie_ui_kit::Icon;
use screenie_ui_kit::hud::{self, ButtonStyle, HudButton, color};

use crate::raster::Raster;
use crate::session::{Cursor, Key, Modifiers, Outcome, Reach, Session};
use crate::tool::Tool;
use crate::{Mode, Overlays, Setup};

/// What the editor hands back to its owner.
pub enum Output {
    Copy(Image),
    /// Save over the capture's file (or to a new one if it was never saved).
    Save(Image),
    SaveAs(Image, PathBuf),
    /// Finished: apply the after-capture actions, then the window closes. `copied` and
    /// `saved` say the image is already on the clipboard / saved exactly like this, so
    /// there's no need to do it again.
    Done { image: Image, copied: bool, saved: bool },
    /// The window closed; the last style, to start the next editor with.
    Closed { style: Style },
}

/// Handles an [`Output`]; a returned message is shown briefly in the editor.
pub type OutputHandler = Rc<dyn Fn(Output, &mut App) -> anyhow::Result<Option<String>>>;

/// Space kept free around the image for the bars: top, bottom, sides.
const WINDOW_INSETS: (f32, f32, f32) = (64.0, 68.0, 28.0);
/// As an overlay, a capture not shown in place is centred at its on-screen size, unless
/// it nearly fills the screen (over this share of its width or height): then it's
/// shrunk to within `CENTRED_FIT` of both, so it can't be mistaken for the screen itself.
const CENTRED_LIMIT: f32 = 0.95;
const CENTRED_FIT: f32 = 0.8;
/// Distance between the capture and the bars hanging off it, between bars, and from
/// the screen edges.
const BAR_GAP: f32 = 10.0;
const SCREEN_MARGIN: f32 = 8.0;
/// Each bar's row (a HUD panel of 30px buttons). The overlay lays out as if both rows
/// were always there, so switching tools (which shows or hides the style bar) never
/// moves the capture or the main bar.
const BAR_HEIGHT: f32 = 40.0;
const BARS_HEIGHT: f32 = BAR_HEIGHT * 2.0 + BAR_GAP - 2.0;
/// How much the overlay dims the screen around the capture: in place, and centred
/// (darker, so a capture of the screen doesn't blend into the screen behind it).
const OVERLAY_DIM: f32 = 0.5;
const CENTRED_DIM: f32 = 0.7;

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
    setup: Rc<Setup>,
    /// Where Save As starts (and what the last one chose).
    path: Option<PathBuf>,
    focus: FocusHandle,
    /// Last pointer position over the canvas, in image pixels.
    pointer: Option<Point>,
    /// Scroll-wheel travel not yet turned into size steps.
    scrolled: f32,
    /// The overlay's main bar width as last laid out, to keep it on screen next frame.
    bar_width: Rc<Cell<Option<Pixels>>>,
    toast: Option<(SharedString, u64)>,
    /// Asking whether to save before closing.
    confirm_close: bool,
    closed: bool,
}

impl Editor {
    pub(crate) fn new(
        session: Session,
        path: Option<PathBuf>,
        setup: Rc<Setup>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| this.update(cx, |e, cx| e.should_close(cx)).unwrap_or(true));
        if setup.mode == Mode::Overlay
            && let Some(handle) = window.window_handle().downcast::<Editor>()
        {
            cx.default_global::<Overlays>().open.push(handle);
            cx.on_release(move |_, cx| cx.default_global::<Overlays>().open.retain(|h| *h != handle)).detach();
        }
        Self {
            session,
            raster: Raster::default(),
            setup,
            path,
            focus,
            pointer: None,
            scrolled: 0.0,
            bar_width: Rc::default(),
            toast: None,
            confirm_close: false,
            closed: false,
        }
    }

    fn overlay(&self) -> bool {
        self.setup.mode == Mode::Overlay
    }

    fn emit(&mut self, output: Output, cx: &mut Context<Self>) -> bool {
        let handler = self.setup.on_output.clone();
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

    pub(crate) fn show_toast(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
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

    fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let image = self.session.export();
        if self.emit(Output::Copy(image), cx) {
            self.session.mark_copied();
            if self.setup.exit_on_copy {
                self.close(window, cx);
            }
        }
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let image = self.session.export();
        if self.emit(Output::Save(image), cx) {
            self.saved(window, cx);
        }
        cx.notify();
    }

    fn saved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.mark_saved();
        if self.setup.exit_on_save {
            self.close(window, cx);
        }
    }

    fn save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.path.as_ref().and_then(|p| p.parent()).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        // Like copying and saving, Save As settles the editor (see `Session::export`).
        self.session.settle();
        let chosen = cx.prompt_for_new_path(&dir, name.as_deref());
        if self.overlay() {
            // The dialog would open underneath the overlay, so step aside until it's
            // answered, then come back just as things were.
            let (session, path, setup) = (self.session.clone(), self.path.clone(), self.setup.clone());
            self.closed = true;
            window.remove_window();
            cx.default_global::<Overlays>().parked += 1;
            let app: &mut App = cx;
            app.spawn(async move |cx| {
                let chosen = chosen.await.ok().and_then(Result::ok).flatten();
                cx.update(|cx| cx.default_global::<Overlays>().parked -= 1);
                cx.update(|cx| match crate::open_session(session, path, setup.clone(), cx) {
                    Ok(handle) => {
                        if let Some(chosen) = chosen {
                            let _ = handle.update(cx, |e, window, cx| e.save_to(chosen, window, cx));
                        }
                    }
                    Err(e) => {
                        tracing::error!("cannot reopen the editor: {e:#}");
                        let _ = (setup.on_output)(Output::Closed { style: Style::default() }, cx);
                    }
                });
            })
            .detach();
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = chosen.await else { return };
            let _ = this.update_in(cx, |e, window, cx| e.save_to(path, window, cx));
        })
        .detach();
    }

    fn save_to(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let image = self.session.export();
        if self.emit(Output::SaveAs(image, path.clone()), cx) {
            self.path = Some(path);
            self.saved(window, cx);
        }
        cx.notify();
    }

    fn done(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let image = self.session.export();
        let (copied, saved) = (self.session.is_copied(), self.session.is_saved());
        if self.emit(Output::Done { image, copied, saved }, cx) {
            self.close(window, cx);
        }
    }

    /// What Done will do, as a sentence and as a button label: only what's configured
    /// and not already done for the image as it is.
    fn done_says(&self) -> (&'static str, &'static str) {
        let on_done = self.setup.on_done;
        let has_file = on_done.save || self.path.is_some() || self.session.has_file();
        let save = has_file && !self.session.is_saved();
        let copy = on_done.copy && !self.session.is_copied();
        match (save, copy) {
            (true, true) => ("Save, copy and close", "Save & copy"),
            (true, false) => ("Save and close", "Save"),
            (false, true) => ("Copy and close", "Copy"),
            (false, false) => ("Close", "Done"),
        }
    }

    /// Close, asking first if annotations would be lost.
    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session.commit_text();
        if self.session.has_unsaved_work() {
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
        if self.session.has_unsaved_work() && !self.closed {
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
                ("c", _) if self.session.text_edit().is_none() => self.copy(window, cx),
                ("s", false) => self.save(window, cx),
                ("s", true) => self.save_as(window, cx),
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
        // As an overlay, the capture stays exactly where it was taken (cropping included).
        if let Some(place) = self.placement(bounds.size) {
            let zoom = place.width as f32 / doc.width() as f32;
            return Viewport { origin: bounds.origin + point(px(place.x as f32), px(place.y as f32)), zoom };
        }
        // In crop mode the whole image shows, so the crop can grow again.
        let shown = if self.session.crop_edit().is_some() { doc.bounds() } else { doc.visible() };
        if self.overlay() {
            return self.centred(bounds, shown);
        }
        let (inset_top, inset_bottom, inset_side) = WINDOW_INSETS;
        let avail_w = (f32::from(bounds.size.width) - inset_side * 2.0).max(40.0);
        let avail_h = (f32::from(bounds.size.height) - inset_top - inset_bottom).max(40.0);
        // Never beyond the capture's own size on screen (logical 1:1), so it stays sharp.
        let zoom = (avail_w / shown.width as f32).min(avail_h / shown.height as f32).min(1.0 / doc.scale());
        let (w, h) = (shown.width as f32 * zoom, shown.height as f32 * zoom);
        let left = f32::from(bounds.origin.x) + inset_side + (avail_w - w) / 2.0;
        let top = f32::from(bounds.origin.y) + inset_top + (avail_h - h) / 2.0;
        Viewport { origin: point(px(left - shown.x as f32 * zoom), px(top - shown.y as f32 * zoom)), zoom }
    }

    /// An overlay's capture, when it isn't in place: logical 1:1, or shrunk if it nearly
    /// fills the screen, and centred together with the bars below it.
    fn centred(&self, bounds: Bounds<Pixels>, shown: Rect) -> Viewport {
        let doc = self.session.doc();
        let (screen_w, screen_h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let (w, h) = (shown.width as f32 / doc.scale(), shown.height as f32 / doc.scale());
        let fit = if w > screen_w * CENTRED_LIMIT || h > screen_h * CENTRED_LIMIT {
            (screen_w * CENTRED_FIT / w).min(screen_h * CENTRED_FIT / h)
        } else {
            1.0
        };
        let zoom = fit / doc.scale();
        let (w, h) = (w * fit, h * fit);
        let bars = BARS_HEIGHT + BAR_GAP;
        let top = if h + bars + SCREEN_MARGIN * 2.0 <= screen_h { (screen_h - h - bars) / 2.0 } else { (screen_h - h) / 2.0 };
        let left = f32::from(bounds.origin.x) + (screen_w - w) / 2.0;
        let top = f32::from(bounds.origin.y) + top;
        Viewport { origin: point(px(left - shown.x as f32 * zoom), px(top - shown.y as f32 * zoom)), zoom }
    }

    /// Where the overlay shows the capture in place: the spot it was taken from, if that
    /// is on this screen.
    fn placement(&self, screen: Size<Pixels>) -> Option<Rect> {
        let place = self.setup.placement.filter(|_| self.overlay())?;
        let screen = Rect::new(0.0, 0.0, f64::from(screen.width), f64::from(screen.height));
        (place.width >= 1.0 && screen.intersection(&place) == Some(place)).then_some(place)
    }

    /// The window rectangle the (visible part of the) capture occupies.
    fn shown(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        let doc = self.session.doc();
        let vp = self.viewport(bounds);
        vp.rect(if self.session.crop_edit().is_some() { doc.bounds() } else { doc.visible() })
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

        // Centred over the dimmed screen, the capture (often of that very screen) needs a
        // clear edge: a deeper shadow lifts it, and a dark outer and light inner hairline
        // keep the edge visible whatever the capture's colours.
        let floating = self.overlay() && self.placement(bounds.size).is_none();
        if floating {
            window.paint_drop_shadows(shown, px(0.).into(), &[
                shadow(0x000000a6, 18.0, 56.0),
                shadow(0x00000080, 2.0, 8.0),
            ]);
        } else {
            window.paint_drop_shadows(shown, px(2.).into(), &[
                shadow(0x00000080, 10.0, 36.0),
                shadow(0x00000066, 1.0, 3.0),
            ]);
        }
        let (composite, tile) = self.raster.update(&self.session);
        let tile = tile.map(|t| (t.image.clone(), t.rect));
        let _ = window.paint_image(shown, full, px(0.).into(), composite, 0, false);
        if let Some((image, rect)) = tile {
            let tile_bounds = vp.rect(rect);
            let _ = window.paint_image(shown.intersect(&tile_bounds), tile_bounds, px(0.).into(), image, 0, false);
        }
        if floating {
            let outer = Bounds::new(shown.origin - point(px(1.), px(1.)), shown.size + size(px(2.), px(2.)));
            window.paint_quad(quad(outer, px(0.), gpui::transparent_black(), px(1.), rgba(0x000000b3), BorderStyle::Solid));
            window.paint_quad(quad(shown, px(0.), gpui::transparent_black(), px(1.), rgba(0xffffff4d), BorderStyle::Solid));
        } else {
            window.paint_quad(quad(shown, px(2.), gpui::transparent_black(), px(1.), rgba(0xffffff14), BorderStyle::Solid));
        }

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
            .child(self.button_for("copy", cx, |e, window, cx| e.copy(window, cx)).icon(Icon::Copy).tooltip("Copy  Ctrl+C"))
            .child(
                self.button_for("save", cx, |e, window, cx| e.save(window, cx))
                    .icon(Icon::Download)
                    .tooltip("Save  Ctrl+S · Save as  Ctrl+Shift+S"),
            )
            .child(
                div().ml_1().child(
                    self.button_for("done", cx, |e, window, cx| e.done(window, cx))
                        .icon(Icon::Check)
                        .label("Done")
                        .tooltip(format!("{}  Enter", self.done_says().0))
                        .style(ButtonStyle::Accent),
                ),
            )
    }

    /// The tool whose options apply: the selection's, or the current one.
    fn styled_tool(&self) -> Tool {
        match self.session.selected().or_else(|| self.session.text_edit().and_then(|t| self.session.doc().shape(t.id)))
        {
            Some(shape) => tool_for(&shape.kind),
            None => self.session.tool(),
        }
    }

    /// Wheel travel resizes (a run of it on one shape is one undo step).
    fn scroll_size(&mut self, delta: ScrollDelta, cx: &mut Context<Self>) {
        if !self.styled_tool().uses_size() || self.session.crop_edit().is_some() {
            return;
        }
        // In notches: GPUI reports a wheel click as 3 lines; touchpads scroll pixels.
        self.scrolled += match delta {
            ScrollDelta::Lines(d) => d.y / 3.0,
            ScrollDelta::Pixels(d) => f32::from(d.y) / 40.0,
        };
        let steps = self.scrolled.trunc();
        if steps != 0.0 {
            self.scrolled -= steps;
            if self.session.scroll_size(steps as i32) {
                cx.notify();
            }
        }
    }

    /// Colour, size and per-tool options, for the selection or the current tool.
    fn style_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(crop) = self.session.crop_edit() {
            return Some(self.crop_bar(crop, cx).into_any_element());
        }
        let tool = self.styled_tool();
        if !(tool.uses_color() || tool.uses_size() || tool == Tool::Redact) {
            return None;
        }
        let style = self.session.style();
        let mut bar = hud::panel().gap_1();

        if tool.uses_color() {
            for (i, c) in self.setup.palette.iter().copied().enumerate() {
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
            bar = bar.child(hud::separator()).child(self.size_stepper(style.size, cx));
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

    /// Smaller / current size / larger. The wheel over it (or Ctrl+wheel anywhere)
    /// steps too.
    fn size_stepper(&self, size: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let index = Style::size_index(size);
        let (smallest, largest) = (Style::SIZES[0], Style::SIZES[Style::SIZES.len() - 1]);
        let dot = 3.0 + index as f32 * 1.6;
        let this = cx.entity();
        div()
            .flex()
            .flex_row()
            .items_center()
            .child(
                self.button_for("size-down", cx, |e, _, _| e.session.step_size(-1))
                    .icon(Icon::Minus)
                    .tooltip("Smaller  [")
                    .disabled(size <= smallest),
            )
            .child(
                div()
                    .id("size")
                    .w(px(30.))
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .tooltip(move |window, cx| {
                        let tip = format!("Size {index} of {}  ·  Ctrl+scroll, 1–9, 0", Style::SIZES.len());
                        gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                    })
                    .on_scroll_wheel(move |e, _, cx| {
                        this.update(cx, |editor, cx| editor.scroll_size(e.delta, cx));
                    })
                    .child(div().size(px(dot)).rounded_full().bg(color::text())),
            )
            .child(
                self.button_for("size-up", cx, |e, _, _| e.session.step_size(1))
                    .icon(Icon::Plus)
                    .tooltip("Larger  ]")
                    .disabled(size >= largest),
            )
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
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("Keep your annotations?"))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(color::text_dim())
                    .child("They'll be lost if you close without copying or saving them."),
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
                        self.button_for("finish", cx, |e, window, cx| e.done(window, cx))
                            .label(self.done_says().1)
                            .style(ButtonStyle::Accent),
                    ),
            );
        div().absolute().inset_0().bg(color::scrim(0.45)).flex().items_center().justify_center().child(card)
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_window_edited(self.session.has_unsaved_work());
        let this = cx.entity();
        let canvas = canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox: Hitbox, window, cx| {
                this.update(cx, |editor, cx| editor.paint_canvas(bounds, &hitbox, window, cx));
            },
        )
        .absolute()
        .inset_0();

        let bars = if self.overlay() { self.overlay_bars(window, cx) } else { self.window_bars(cx) };
        div()
            .id("editor")
            .size_full()
            .relative()
            .bg(match (self.overlay(), self.placement(window.viewport_size()).is_some()) {
                (true, true) => color::scrim(OVERLAY_DIM),
                (true, false) => color::scrim(CENTRED_DIM),
                (false, _) => workspace(),
            })
            .font_family(screenie_ui_kit::FONT)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(|e, event: &ScrollWheelEvent, _, cx| {
                if event.modifiers.control {
                    e.scroll_size(event.delta, cx);
                }
            }))
            .child(canvas)
            .children(bars)
            .when(self.confirm_close, |d| d.child(self.close_prompt(cx)))
    }
}

impl Editor {
    fn toast_pill(&self) -> Option<gpui::Div> {
        self.toast.as_ref().map(|(message, _)| hud::pill(message.clone()))
    }

    /// In a window: history, tools and actions along the top, style bar and toasts along
    /// the bottom.
    fn window_bars(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
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
        let toast = self.toast_pill().map(|pill| {
            div().absolute().bottom(px(70.)).left_0().right_0().flex().flex_row().justify_center().child(pill)
        });
        let mut out = vec![top.into_any_element()];
        out.extend(bottom.map(IntoElement::into_any_element));
        out.extend(toast.map(IntoElement::into_any_element));
        out
    }

    /// As an overlay: one stack of bars hanging off the capture, below it if there's
    /// room, else above, else inside its bottom edge (a full-screen capture), centred on
    /// it and kept on screen.
    fn overlay_bars(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let screen = window.viewport_size();
        let (w, h) = (f32::from(screen.width), f32::from(screen.height));
        let shown = self.shown(Bounds::new(point(px(0.), px(0.)), screen));
        let (top, bottom) = (f32::from(shown.top()), f32::from(shown.bottom()));
        let measured = self.bar_width.get();
        let need = BARS_HEIGHT;

        #[derive(PartialEq)]
        enum Side {
            Below,
            Above,
            Inside,
        }
        let side = if bottom + BAR_GAP + need <= h - SCREEN_MARGIN {
            Side::Below
        } else if top - BAR_GAP - need >= SCREEN_MARGIN {
            Side::Above
        } else {
            Side::Inside
        };

        let row = || div().h(px(BAR_HEIGHT)).flex().flex_row().items_center().justify_center();
        let slot = self.bar_width.clone();
        let main = row()
            .relative()
            .gap_2()
            .child(self.history_bar(cx))
            .child(self.tool_bar(cx))
            .child(self.action_bar(cx))
            .child(
                canvas(
                    move |bounds, window, _| {
                        if slot.get() != Some(bounds.size.width) {
                            slot.set(Some(bounds.size.width));
                            window.on_next_frame(|window, _| window.refresh());
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        let mut stack = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(BAR_GAP - 2.0))
            .when(measured.is_none(), |d| d.opacity(0.));
        // The main bar sits nearest the capture.
        let style = self.style_bar(cx).map(|bar| row().child(bar));
        if side == Side::Below {
            stack = stack.child(main).children(style);
        } else {
            stack = stack.children(style).child(main);
        }

        // Centred on the capture; against the nearer screen edge when too wide for that.
        let center = f32::from(shown.center().x).clamp(0.0, w);
        let half = (center - SCREEN_MARGIN).min(w - SCREEN_MARGIN - center).max(0.0);
        let fits = measured.is_none_or(|width| f32::from(width) <= half * 2.0);
        let mut column = div().absolute().flex().flex_col().gap(px(BAR_GAP - 2.0));
        column = if fits {
            column.left(px(center - half)).w(px(half * 2.0)).items_center()
        } else if center < w / 2.0 {
            column.left(px(SCREEN_MARGIN)).right(px(SCREEN_MARGIN)).items_start()
        } else {
            column.left(px(SCREEN_MARGIN)).right(px(SCREEN_MARGIN)).items_end()
        };
        // Toasts go on the far side of the bars, so the bars never move for them.
        let toast = self.toast_pill();
        column = match side {
            Side::Below => column.top(px(bottom + BAR_GAP)).child(stack).children(toast),
            Side::Above => column.bottom(px(h - top + BAR_GAP)).children(toast).child(stack),
            Side::Inside => {
                column.bottom(px((h - bottom).max(0.0) + BAR_GAP + 4.0)).children(toast).child(stack)
            }
        };
        vec![column.into_any_element()]
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
