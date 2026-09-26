//! The GPUI side of the selector: one [`OutputView`] per output, all rendering one shared
//! [`Session`].

use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    AnyElement, Bounds, BoxShadow, Context, CursorStyle, DispatchPhase, Entity, FocusHandle, Hitbox,
    HitboxBehavior, Hsla, KeyDownEvent, KeyUpEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, RenderImage, Window, canvas, div, fill, point, px, quad, rgba, size,
};
use screenie_core::{Image, OutputInfo, Point, Rect, Snapshot};
use screenie_ui_kit::hud::{self, ButtonStyle, HudButton, color};
use screenie_ui_kit::{Icon, KeyboardGrab, Tip, ui, ui_px};

use crate::model::{Handle, Key, Mode, Model, Modifiers, Outcome, Purpose, Selection};
use crate::{Choice, RecordOptions, SelectorConfig};

/// State shared by every output's view.
pub(crate) struct Session {
    pub model: Model,
    pub config: SelectorConfig,
    pub magnifier: bool,
    pub record: RecordOptions,
    pub active_output: Option<String>,
    pub snapshot: Option<Arc<Snapshot>>,
    pub done: Option<async_channel::Sender<Option<Choice>>>,
    /// The choice goes out only once the keys that made it are let go, so they don't leak
    /// into the app beneath when the selector closes (see `KeyboardGrab`).
    pub grab: Entity<KeyboardGrab>,
}

impl Session {
    fn apply(&mut self, outcome: Outcome, cx: &mut Context<Self>) {
        match outcome {
            Outcome::Nothing => {}
            Outcome::Redraw => cx.notify(),
            Outcome::Confirm(selection) => self.finish(Some(selection), cx),
            Outcome::Cancel => self.finish(None, cx),
        }
    }

    /// End with `selection` (none: cancelled), once no keys are held. The selector
    /// looks gone straight away.
    pub fn finish(&mut self, selection: Option<Selection>, cx: &mut Context<Self>) {
        let Some(done) = self.done.take() else { return };
        let choice = selection.map(|selection| Choice { selection, record: self.record });
        self.grab.update(cx, |grab, cx| {
            grab.when_released(
                move |_| {
                    let _ = done.try_send(choice);
                },
                cx,
            );
        });
        cx.notify();
    }

    /// Physical size of a logical rect as it would be captured.
    pub fn pixel_size(&self, r: Rect) -> (u32, u32) {
        if let Some(s) = &self.snapshot {
            return s.region_pixel_size(r);
        }
        let scale = self
            .model
            .outputs()
            .iter()
            .filter(|o| o.logical.intersection(&r).is_some())
            .map(|o| o.scale)
            .fold(1.0, f64::max);
        ((r.width * scale).round() as u32, (r.height * scale).round() as u32)
    }

    fn confirm_on(&mut self, output: &str, cx: &mut Context<Self>) {
        let selection = self.model.editing().cloned().or_else(|| {
            self.model.outputs().iter().find(|o| o.name == output).cloned().map(Selection::Output)
        });
        if selection.is_some() {
            self.finish(selection, cx);
        }
    }
}

pub(crate) struct OutputView {
    session: Entity<Session>,
    output: OutputInfo,
    frozen: Option<(Arc<RenderImage>, Image)>,
    focus: FocusHandle,
    /// The pointer is on the toolbar, where the loupe would only cover its tooltips.
    over_toolbar: bool,
}

fn modifiers(m: &gpui::Modifiers) -> Modifiers {
    Modifiers { shift: m.shift, ctrl: m.control, alt: m.alt }
}

fn map_key(key: &str) -> Option<Key> {
    Some(match key {
        "escape" => Key::Escape,
        "enter" => Key::Enter,
        "space" => Key::Space,
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "tab" => Key::Tab,
        "1" | "a" => Key::Mode(Mode::Area),
        "2" | "w" => Key::Mode(Mode::Window),
        "3" | "s" => Key::Mode(Mode::Screen),
        _ => return None,
    })
}

fn cursor_for(handle: Option<Handle>) -> CursorStyle {
    match handle {
        None => CursorStyle::Crosshair,
        Some(Handle::TopLeft | Handle::BottomRight) => CursorStyle::ResizeUpLeftDownRight,
        Some(Handle::TopRight | Handle::BottomLeft) => CursorStyle::ResizeUpRightDownLeft,
        Some(Handle::Top | Handle::Bottom) => CursorStyle::ResizeUpDown,
        Some(Handle::Left | Handle::Right) => CursorStyle::ResizeLeftRight,
        Some(Handle::Inside) => CursorStyle::ClosedHand,
    }
}

fn to_px(v: f64) -> Pixels {
    px(v as f32)
}

fn gbounds(r: Rect) -> Bounds<Pixels> {
    Bounds::new(point(to_px(r.x), to_px(r.y)), size(to_px(r.width), to_px(r.height)))
}

/// The parts of `outer` not covered by `hole` (which lies within it).
fn surround(outer: Rect, hole: Rect) -> impl Iterator<Item = Rect> {
    [
        Rect::new(outer.x, outer.y, outer.width, hole.y - outer.y),
        Rect::new(outer.x, hole.bottom(), outer.width, outer.bottom() - hole.bottom()),
        Rect::new(outer.x, hole.y, hole.x - outer.x, hole.height),
        Rect::new(hole.right(), hole.y, outer.right() - hole.right(), hole.height),
    ]
    .into_iter()
    .filter(|r| !r.is_empty())
}

fn shadow(alpha: u32, y: f32, blur: f32) -> BoxShadow {
    BoxShadow {
        color: rgba(alpha).into(),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }
}

impl OutputView {
    pub fn new(
        session: Entity<Session>,
        output: OutputInfo,
        frozen: Option<(Arc<RenderImage>, Image)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&session, |_, _, cx| cx.notify()).detach();
        screenie_ui_kit::track_ui_scale(window, cx);
        // Tests wait for this before typing (see tests/e2e).
        let name = output.name.clone();
        cx.observe_window_activation(window, move |_, window, _| {
            tracing::debug!(output = name, active = window.is_window_active(), "selector keyboard focus");
        })
        .detach();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self { session, output, frozen, focus, over_toolbar: false }
    }

    fn local(&self, r: Rect) -> Rect {
        let o = self.output.logical.origin();
        r.translate(-o.x, -o.y)
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let mods = modifiers(&event.keystroke.modifiers);
        self.session.update(cx, |s, cx| {
            if key == "m" {
                s.magnifier = !s.magnifier && s.snapshot.is_some();
                cx.notify();
                return;
            }
            let outcome = match map_key(key) {
                Some(k) => s.model.key_pressed(k, mods),
                None => s.model.set_modifiers(mods),
            };
            s.apply(outcome, cx);
        });
        cx.stop_propagation();
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = map_key(event.keystroke.key.as_str());
        self.session.update(cx, |s, cx| {
            if let Some(k) = key {
                let outcome = s.model.key_released(k);
                s.apply(outcome, cx);
            }
        });
    }

    fn on_modifiers(&mut self, event: &ModifiersChangedEvent, _: &mut Window, cx: &mut Context<Self>) {
        let mods = modifiers(&event.modifiers);
        self.session.update(cx, |s, cx| {
            let outcome = s.model.set_modifiers(mods);
            s.apply(outcome, cx);
        });
    }

    /// Backdrop, dimming, and selection chrome, plus the pointer handling that has to see
    /// every event (drags continue outside this output's surface).
    fn scene(&self, cx: &mut Context<Self>) -> AnyElement {
        let session = self.session.clone();
        let origin = self.output.logical.origin();
        let frozen = self.frozen.as_ref().map(|(img, _)| img.clone());
        let this = cx.entity().downgrade();
        let output_name = self.output.name.clone();
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox: Hitbox, window, cx| {
                let to_global = move |p: gpui::Point<Pixels>| Point::new(origin.x + f64::from(p.x), origin.y + f64::from(p.y));
                let s = session.read(cx);
                let local = |r: Rect| r.translate(-origin.x, -origin.y);

                if let Some(img) = &frozen {
                    let _ = window.paint_image(bounds, bounds, Default::default(), img.clone(), 0, false);
                }

                let view = Rect::new(0.0, 0.0, f64::from(bounds.size.width), f64::from(bounds.size.height));
                let selection = s.model.selection_rect();
                let hover = s.model.hover_target();
                let cutout = selection.or_else(|| hover.as_ref().map(Selection::rect));
                let dim = color::scrim(s.config.dim as f32);
                match cutout.map(local).and_then(|r| r.intersection(&view)) {
                    Some(hole) => surround(view, hole).for_each(|r| window.paint_quad(fill(gbounds(r), dim))),
                    None => window.paint_quad(fill(bounds, dim)),
                }

                if let Some(sel) = selection {
                    let r = local(sel);
                    // Crisp 1px white edge with a faint dark halo for light backgrounds.
                    let halo = gbounds(r.inset(-2.0));
                    window.paint_quad(quad(halo, px(0.), gpui::transparent_black(), px(1.), rgba(0x00000059), Default::default()));
                    let edge = gbounds(r.inset(-1.0));
                    window.paint_quad(quad(edge, px(0.), gpui::transparent_black(), px(1.), rgba(0xfffffff2), Default::default()));
                    if s.model.editing().is_some() && !s.model.is_drawing() {
                        let d = ui_px(window, 9.);
                        for h in Handle::RESIZE {
                            let p = h.position(&r);
                            let dot = Bounds::new(point(to_px(p.x) - d / 2., to_px(p.y) - d / 2.), size(d, d));
                            window.paint_drop_shadows(dot, (d / 2.).into(), &[shadow(0x00000059, 1.0, 3.0)]);
                            window.paint_quad(quad(dot, d / 2., gpui::white(), px(1.), rgba(0x00000040), Default::default()));
                        }
                    }
                } else if let Some(target) = &hover {
                    let r = gbounds(local(target.rect()));
                    match (target, s.model.mode()) {
                        (Selection::Window(_), Mode::Area) => {
                            window.paint_quad(quad(r, px(0.), gpui::transparent_black(), px(1.5), rgba(0xffffff8c), Default::default()));
                        }
                        _ => {
                            let accent = color::accent();
                            window.paint_quad(fill(r, Hsla { a: 0.10, ..accent }));
                            window.paint_quad(quad(r, px(0.), gpui::transparent_black(), px(3.), accent, Default::default()));
                        }
                    }
                }

                let handle = s.model.hover_handle();
                window.set_cursor_style(cursor_for(handle), &hitbox);

                // Pointer events. Presses only count on the backdrop (not the toolbar);
                // moves and releases are tracked everywhere so drags survive leaving the
                // surface.
                let (sess, hb) = (session.clone(), hitbox.clone());
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || !hb.is_hovered(window) {
                        return;
                    }
                    let p = to_global(e.position);
                    sess.update(cx, |s, cx| {
                        let outcome = match e.button {
                            MouseButton::Left if e.click_count >= 2 => s.model.double_clicked(p),
                            MouseButton::Left => {
                                s.model.set_modifiers(modifiers(&e.modifiers));
                                s.model.pressed(p)
                            }
                            MouseButton::Right => Outcome::Cancel,
                            _ => Outcome::Nothing,
                        };
                        s.apply(outcome, cx);
                    });
                });
                let sess = session.clone();
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                        return;
                    }
                    let p = to_global(e.position);
                    sess.update(cx, |s, cx| {
                        let outcome = s.model.released(p);
                        s.apply(outcome, cx);
                    });
                });
                let (sess, name, this) = (session.clone(), output_name.clone(), this.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let p = to_global(e.position);
                    tracing::trace!(output = name, ?p, "pointer moved");
                    sess.update(cx, |s, cx| {
                        if e.pressed_button.is_none() && s.active_output.as_deref() != Some(name.as_str()) {
                            s.active_output = Some(name.clone());
                        }
                        s.model.set_modifiers(modifiers(&e.modifiers));
                        let outcome = s.model.pointer_moved(p);
                        s.apply(outcome, cx);
                    });
                    let _ = this.update(cx, |_, cx| cx.notify());
                });
            },
        )
        .size_full()
        .absolute()
        .top_0()
        .left_0()
        .into_any_element()
    }

    /// Size readout, window labels, and the magnifier. `k` is the interface scale.
    fn annotations(&self, s: &Session, k: f64) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let (w, h) = (self.output.logical.width, self.output.logical.height);
        let model = &s.model;

        let loupe = s.magnifier && !model.is_grabbing() && model.editing().is_none();
        let loupe_here = loupe
            && !self.over_toolbar
            && self.frozen.is_some()
            && model.cursor().is_some_and(|c| self.output.logical.contains(c));

        if let Some(sel) = model.selection_rect() {
            let (pw, ph) = s.pixel_size(sel);
            let r = self.local(sel);
            let text = format!("{pw} × {ph}");
            // While drawing with the loupe up, the size lives in the loupe's readout.
            let trailing = model.is_drawing().then(|| model.cursor()).flatten();
            let pos = match trailing {
                Some(_) if loupe_here => None,
                Some(c) => {
                    let c = self.local(Rect::new(c.x, c.y, 0.0, 0.0));
                    Some((c.x + 16.0 * k, c.y + 18.0 * k))
                }
                None if r.bottom() + 36.0 * k < h => Some((r.x, r.bottom() + 8.0 * k)),
                None => Some((r.x + 8.0 * k, r.bottom() - 32.0 * k)),
            };
            if let Some(pos) = pos
                && self.output.logical.intersection(&sel).is_some()
            {
                out.push(
                    div()
                        .absolute()
                        .left(to_px(pos.0.clamp(4.0, (w - 110.0 * k).max(4.0))))
                        .top(to_px(pos.1.clamp(4.0, (h - 30.0 * k).max(4.0))))
                        .child(hud::pill(text))
                        .into_any_element(),
                );
            }
        } else if let Some(target) = model.hover_target()
            && model.mode() != Mode::Area
        {
            let r = self.local(target.rect());
            let visible = r.intersection(&Rect::new(0.0, 0.0, w, h));
            if let Some(v) = visible {
                let (pw, ph) = s.pixel_size(target.rect());
                let label = match &target {
                    Selection::Window(win) if !win.app_id.is_empty() => win.app_id.clone(),
                    Selection::Window(win) => win.title.clone(),
                    Selection::Output(o) => o.name.clone(),
                    Selection::Region(_) => String::new(),
                };
                out.push(
                    div()
                        .absolute()
                        .left(to_px(v.x + 12.0 * k))
                        .top(to_px(v.y + 12.0 * k))
                        .child(hud::pill(format!("{label}   {pw} × {ph}")))
                        .into_any_element(),
                );
            }
        }

        if loupe_here && let (Some(cursor), Some((_, image))) = (model.cursor(), &self.frozen) {
            let size = model.is_drawing().then(|| model.selection_rect()).flatten().map(|r| s.pixel_size(r));
            out.push(self.loupe(cursor, image, size, k));
        }
        out
    }

    /// A magnifier of the physical pixels around the cursor, with the center pixel's color
    /// and the cursor position.
    fn loupe(&self, cursor: Point, image: &Image, selection_size: Option<(u32, u32)>, k: f64) -> AnyElement {
        const CELLS: i64 = 15;
        let cell = 8.0 * k;
        let side = CELLS as f64 * cell;
        let o = &self.output.logical;
        let scale = image.width() as f64 / o.width;
        let local = Point::new(cursor.x - o.x, cursor.y - o.y);
        let cx_px = ((local.x * scale).floor() as i64).clamp(0, image.width() as i64 - 1);
        let cy_px = ((local.y * scale).floor() as i64).clamp(0, image.height() as i64 - 1);

        let gap = 26.0 * k;
        let label_h = 40.0 * k;
        let mut x = local.x + gap;
        let mut y = local.y + gap;
        if x + side > o.width - 4.0 {
            x = local.x - gap - side;
        }
        if y + side + label_h > o.height - 4.0 {
            y = local.y - gap - side - label_h;
        }

        let mut cells = Vec::with_capacity((CELLS * CELLS) as usize);
        for j in 0..CELLS {
            for i in 0..CELLS {
                let (px_x, px_y) = (cx_px + i - CELLS / 2, cy_px + j - CELLS / 2);
                let rgba = if px_x >= 0 && px_y >= 0 && px_x < image.width() as i64 && px_y < image.height() as i64 {
                    image.rgba_at(px_x as u32, px_y as u32)
                } else {
                    [20, 20, 20, 255]
                };
                cells.push((i, j, rgba));
            }
        }
        let [r, g, b, _] = image.rgba_at(cx_px as u32, cy_px as u32);
        let center_color = gpui::Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 };

        let grid = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let o = bounds.origin;
                let last = CELLS - 1;
                for (i, j, [r, g, b, _]) in &cells {
                    let cell = Bounds::new(
                        point(o.x + px((*i as f64 * cell) as f32), o.y + px((*j as f64 * cell) as f32)),
                        size(px(cell as f32), px(cell as f32)),
                    );
                    // Round the four corner cells so the loupe's corners stay clean.
                    let mut radii = gpui::Corners::default();
                    let corner = ui_px(window, 7.);
                    match (*i, *j) {
                        (0, 0) => radii.top_left = corner,
                        (i, 0) if i == last => radii.top_right = corner,
                        (0, j) if j == last => radii.bottom_left = corner,
                        (i, j) if i == last && j == last => radii.bottom_right = corner,
                        _ => {}
                    }
                    let c = gpui::Rgba { r: *r as f32 / 255.0, g: *g as f32 / 255.0, b: *b as f32 / 255.0, a: 1.0 };
                    window.paint_quad(quad(cell, radii, c, px(0.), gpui::transparent_black(), Default::default()));
                }
                let line: Hsla = rgba(0x0000001f).into();
                for k in 1..CELLS {
                    let d = px((k as f64 * cell) as f32);
                    window.paint_quad(fill(Bounds::new(point(o.x + d, o.y), size(px(1.), bounds.size.height)), line));
                    window.paint_quad(fill(Bounds::new(point(o.x, o.y + d), size(bounds.size.width, px(1.))), line));
                }
                let c = px(((CELLS / 2) as f64 * cell) as f32);
                let center = Bounds::new(point(o.x + c - px(1.), o.y + c - px(1.)), size(px(cell as f32 + 2.), px(cell as f32 + 2.)));
                window.paint_quad(quad(center, px(1.), gpui::transparent_black(), px(1.), rgba(0x000000cc), Default::default()));
                let inner = Bounds::new(point(o.x + c, o.y + c), size(px(cell as f32), px(cell as f32)));
                window.paint_quad(quad(inner, px(0.), gpui::transparent_black(), px(1.), gpui::white(), Default::default()));
            },
        )
        .size_full();

        let readout = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1p5()
            .child(div().size(ui(10.)).rounded_full().bg(center_color).border_1().border_color(rgba(0xffffffb3)))
            .child(format!("#{r:02X}{g:02X}{b:02X}"))
            .child(div().text_color(color::text_dim()).child(match selection_size {
                Some((w, h)) => format!("{w} × {h}"),
                None => format!("{:.0}, {:.0}", cursor.x.floor(), cursor.y.floor()),
            }));

        div()
            .absolute()
            .left(to_px(x))
            .top(to_px(y))
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .child(
                div()
                    .size(to_px(side))
                    .rounded(ui(8.))
                    .shadow(vec![shadow(0x00000073, 4.0, 14.0)])
                    .child(grid)
                    .child(div().absolute().inset_0().rounded(ui(8.)).border_2().border_color(rgba(0xffffffe6))),
            )
            .child(
                div()
                    .px(ui(8.))
                    .py(ui(3.))
                    .rounded(ui(7.))
                    .bg(rgba(0x1c1c1ee0))
                    .border_1()
                    .border_color(color::hairline())
                    .text_color(color::text())
                    .text_size(ui(12.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(readout),
            )
            .into_any_element()
    }

    fn toolbar(&self, s: &Session, k: f64, this: Entity<Self>) -> AnyElement {
        let model = &s.model;
        let editing = model.editing().is_some();
        let purpose = model.purpose();
        let mode = model.mode();
        let mut bar = hud::panel();

        let area_note = match purpose {
            Purpose::Screenshot => "Drag a region or click a window · Enter for the screen",
            Purpose::Recording => "Drag a region or click a window",
        };
        for (m, icon, tip, id) in [
            (Mode::Area, Icon::Area, Tip::new("Area").key("1").note(area_note), "mode-area"),
            (Mode::Window, Icon::Window, Tip::new("Window").key("2").key("Space").note("Click a window"), "mode-window"),
            (Mode::Screen, Icon::Screen, Tip::new("Screen").key("3").note("Click a screen"), "mode-screen"),
        ] {
            let session = self.session.clone();
            bar = bar.child(HudButton::new(id).icon(icon).tooltip(tip).selected(mode == m).on_click(move |_, _, cx| {
                session.update(cx, |s, cx| {
                    let outcome = s.model.set_mode(m);
                    s.apply(outcome, cx);
                });
            }));
        }

        if purpose == Purpose::Recording {
            bar = bar.child(hud::separator());
            let rec = s.record;
            let session = self.session.clone();
            bar = bar.child(
                HudButton::new("system-audio")
                    .icon(if rec.system_audio { Icon::Volume } else { Icon::VolumeOff })
                    .tooltip(Tip::new("Record system audio").note(if rec.system_audio { "On" } else { "Off" }))
                    .selected(rec.system_audio)
                    .on_click(move |_, _, cx| {
                        session.update(cx, |s, cx| {
                            s.record.system_audio = !s.record.system_audio;
                            cx.notify();
                        })
                    }),
            );
            let session = self.session.clone();
            bar = bar.child(
                HudButton::new("microphone")
                    .icon(if rec.microphone { Icon::Mic } else { Icon::MicOff })
                    .tooltip(Tip::new("Record microphone").note(if rec.microphone { "On" } else { "Off" }))
                    .selected(rec.microphone)
                    .on_click(move |_, _, cx| {
                        session.update(cx, |s, cx| {
                            s.record.microphone = !s.record.microphone;
                            cx.notify();
                        })
                    }),
            );
        }

        let show_action = purpose == Purpose::Recording || editing;
        if show_action {
            let session = self.session.clone();
            let name = self.output.name.clone();
            let (action, tip) = match purpose {
                Purpose::Screenshot => {
                    (HudButton::new("confirm").icon(Icon::Camera).label("Capture").style(ButtonStyle::Accent), "Capture")
                }
                Purpose::Recording => {
                    (HudButton::new("confirm").icon(Icon::Video).label("Record").style(ButtonStyle::Record), "Start recording")
                }
            };
            let tip = Tip::new(tip).key("Enter");
            let action = action.tooltip(if editing { tip.note("Arrows nudge the area · Ctrl+arrows resize it") } else { tip });
            bar = bar.child(div().ml_1().child(action.on_click(move |_, _, cx| {
                session.update(cx, |s, cx| s.confirm_on(&name, cx));
            })));
        }

        let session = self.session.clone();
        bar = bar.child(HudButton::new("cancel").icon(Icon::Close).tooltip(Tip::new("Cancel").key("Esc")).on_click(move |_, _, cx| {
            session.update(cx, |s, cx| s.finish(None, cx));
        }));

        // Keep clear of a selection near the bottom edge.
        let o = &self.output.logical;
        let band = Rect::new(o.x, o.bottom() - 140.0 * k, o.width, 140.0 * k);
        let at_top = model.selection_rect().is_some_and(|r| r.intersection(&band).is_some());
        let row = div().absolute().left_0().right_0().flex().flex_row().justify_center();
        let row = if at_top { row.top(ui(36.)) } else { row.bottom(ui(36.)) };
        let bar = bar.id("toolbar").on_hover(move |hovered, _, cx| {
            this.update(cx, |view, cx| {
                view.over_toolbar = *hovered;
                cx.notify();
            })
        });
        row.child(bar).into_any_element()
    }
}

impl Render for OutputView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        tracing::trace!(output = self.output.name, "render");
        let grab = self.session.read(cx).grab.clone();
        let root = KeyboardGrab::track(
            &grab,
            div()
                .id("selector")
                .size_full()
                .relative()
                .font_family(screenie_ui_kit::FONT)
                .track_focus(&self.focus)
                .on_key_down(cx.listener(Self::on_key_down))
                .on_key_up(cx.listener(Self::on_key_up))
                .on_modifiers_changed(cx.listener(Self::on_modifiers)),
        );
        if grab.read(cx).leaving() {
            return root;
        }
        let scene = self.scene(cx);
        let this = cx.entity();
        let s = self.session.read(cx);
        let k = f64::from(screenie_ui_kit::ui_scale(cx));
        let busy = s.model.is_drawing() || s.model.is_grabbing();
        let show_toolbar = s.config.toolbar && !busy && s.active_output.as_deref() == Some(self.output.name.as_str());
        // A toolbar that went away can't report the pointer leaving it.
        self.over_toolbar &= show_toolbar;
        let annotations = self.annotations(s, k);
        let toolbar = show_toolbar.then(|| self.toolbar(s, k, this));

        root.child(scene)
            .children(annotations)
            .children(toolbar)
    }
}

pub(crate) fn frozen_parts(snapshot: &Snapshot, output: &str) -> Option<(Arc<RenderImage>, Image)> {
    snapshot
        .output_named(output)
        .map(|c| (screenie_ui_kit::render_image(&c.image), c.image.clone()))
}
