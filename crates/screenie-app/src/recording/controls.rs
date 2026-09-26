//! On-screen chrome for a recording: the countdown, a border just outside the recorded
//! region, and the control pill (timer, pause, stop, discard).
//!
//! Screen capture includes every surface, so nothing here may overlap the region while
//! recording. The border is drawn outside it, and the pill goes below, above or beside
//! it, or onto another output. When the region fills the only output, there's no pill and
//! the recording is stopped with the same shortcut (or `screenie stop`). The countdown
//! says so.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt, App, AsyncApp, Bounds, Context, FontWeight, Pixels, Window, WindowHandle, canvas,
    div, px, rgba, size,
};
use screenie_core::{OutputInfo, Point, Rect, Size};
use screenie_ui_kit::hud::{self, HudButton, color};
use screenie_ui_kit::{Icon, LayerSpec, Tip, layer_options, ui};

use crate::daemon::Daemon;

/// Pill size in logical pixels (fixed, so it can be placed before it's laid out).
const PILL: Size = Size { width: 196.0, height: 40.0 };
/// Space between the region and the chrome.
const GAP: f64 = 12.0;
/// The border ring sits this far outside the region.
const BORDER_OFFSET: f64 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Countdown(u32),
    Recording,
}

/// Where the pill goes: the output to show it on and its top-left in that output's
/// coordinates. Prefers below the region, then above, then beside it, then another
/// output. `k` is the interface scale.
pub(crate) fn place_pill(region: Rect, home: &OutputInfo, outputs: &[OutputInfo], k: f64) -> Option<(String, Point)> {
    let pill = pill_size(k);
    let gap = GAP * k;
    let o = home.logical;
    let fits_x = o.width >= pill.width + 2.0 * gap;
    let centered_x = (region.center().x - pill.width / 2.0).clamp(o.x + gap, (o.right() - pill.width - gap).max(o.x));
    let local = |x: f64, y: f64| Some((home.name.clone(), Point::new(x - o.x, y - o.y)));
    if fits_x && o.bottom() - region.bottom() >= pill.height + 2.0 * gap {
        return local(centered_x, region.bottom() + gap);
    }
    if fits_x && region.y - o.y >= pill.height + 2.0 * gap {
        return local(centered_x, region.y - gap - pill.height);
    }
    let side_y = (region.bottom() - pill.height).clamp(o.y + gap, (o.bottom() - pill.height - gap).max(o.y));
    if o.right() - region.right() >= pill.width + 2.0 * gap {
        return local(region.right() + gap, side_y);
    }
    if region.x - o.x >= pill.width + 2.0 * gap {
        return local(region.x - gap - pill.width, side_y);
    }
    let other = outputs.iter().filter(|x| x.name != home.name).max_by(|a, b| a.logical.area().total_cmp(&b.logical.area()))?;
    let l = other.logical;
    Some((other.name.clone(), Point::new((l.width - pill.width) / 2.0, l.height - pill.height - 48.0 * k)))
}

/// The pill's size at interface scale `k`.
fn pill_size(k: f64) -> Size {
    Size { width: PILL.width * k, height: PILL.height * k }
}

/// One surface's worth of chrome.
pub(crate) struct Controls {
    /// The region in this surface's coordinates, when it's on this output.
    region: Option<Rect>,
    /// Draw the border (not when the region is the whole output).
    border: bool,
    pill: Option<Point>,
    /// Shown during the countdown when nothing else tells the user how to stop.
    stop_hint: bool,
    phase: Phase,
    pill_bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
}

/// Open the chrome for `region` on every output that needs some.
pub(crate) fn open(
    region: Rect,
    home: &OutputInfo,
    outputs: &[OutputInfo],
    phase: Phase,
    cx: &mut AsyncApp,
) -> Vec<WindowHandle<Controls>> {
    let k = cx.update(|cx| f64::from(screenie_ui_kit::ui_scale(cx)));
    let pill = place_pill(region, home, outputs, k);
    let whole_output = home.logical.inset(1.0).intersection(&region) == Some(home.logical.inset(1.0));
    let mut surfaces: Vec<(&OutputInfo, Controls)> = vec![(
        home,
        Controls {
            region: Some(region.translate(-home.logical.x, -home.logical.y)),
            border: !whole_output,
            pill: None,
            stop_hint: pill.is_none(),
            phase,
            pill_bounds: Rc::default(),
        },
    )];
    if let Some((name, at)) = pill {
        match surfaces.iter_mut().find(|(o, _)| o.name == name) {
            Some((_, controls)) => controls.pill = Some(at),
            None => {
                if let Some(other) = outputs.iter().find(|o| o.name == name) {
                    surfaces.push((
                        other,
                        Controls {
                            region: None,
                            border: false,
                            pill: Some(at),
                            stop_hint: false,
                            phase,
                            pill_bounds: Rc::default(),
                        },
                    ));
                }
            }
        }
    }

    surfaces
        .into_iter()
        .filter_map(|(output, controls)| {
            let spec = LayerSpec::fullscreen_overlay(
                "screenie-recording",
                &output.name,
                size(px(output.logical.width as f32), px(output.logical.height as f32)),
            )
            .passive();
            cx.update(|cx| {
                cx.open_window(layer_options(cx, &spec), |window, cx| {
                    screenie_ui_kit::track_ui_scale(window, cx);
                    cx.new(|cx| {
                        // Keep the timer fresh, redrawing only when it changes: every
                        // frame we draw is damage the recording has to encode.
                        cx.spawn_in(window, async move |this, cx| {
                            let mut shown = None;
                            loop {
                                cx.background_executor().timer(Duration::from_millis(100)).await;
                                let alive = this.update(cx, |_, cx| {
                                    let now = pill_status(cx).map(|(elapsed, paused)| (elapsed.as_secs(), paused));
                                    if now != shown {
                                        shown = now;
                                        cx.notify();
                                    }
                                });
                                if alive.is_err() {
                                    break;
                                }
                            }
                        })
                        .detach();
                        window.set_input_region(Some(&[]));
                        controls
                    })
                })
            })
            .map_err(|e| tracing::warn!("cannot show recording controls on {}: {e}", output.name))
            .ok()
        })
        .collect()
}

pub(crate) fn set_phase(handles: &[WindowHandle<Controls>], phase: Phase, cx: &mut AsyncApp) {
    for handle in handles {
        let _ = handle.update(cx, |c, _, cx| {
            c.phase = phase;
            cx.notify();
        });
    }
}

pub(crate) fn close(handles: &[WindowHandle<Controls>], cx: &mut App) {
    for handle in handles {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

/// Elapsed time and paused state of the running recording.
fn pill_status(cx: &App) -> Option<(Duration, bool)> {
    let recording = Daemon::get(cx).recording.as_ref()?.recording.as_ref()?;
    Some((recording.elapsed(), recording.is_paused()))
}

fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

impl Controls {
    fn border(&self, region: Rect) -> impl IntoElement {
        let ring = |offset: f64, width: f32, color: gpui::Rgba| {
            div()
                .absolute()
                .left(px((region.x - offset) as f32))
                .top(px((region.y - offset) as f32))
                .w(px((region.width + 2.0 * offset) as f32))
                .h(px((region.height + 2.0 * offset) as f32))
                .border(px(width))
                .border_color(color)
                .rounded(px(offset as f32 + 1.0))
        };
        // A dark hairline outside a red ring reads on any background. No shadows: they'd
        // bleed into the recorded area.
        div()
            .absolute()
            .inset_0()
            .child(ring(BORDER_OFFSET + 1.0, 1.0, rgba(0x00000066)))
            .child(ring(BORDER_OFFSET, 2.0, rgba(0xff453ae6)))
    }

    /// `k` is the interface scale.
    fn countdown(&self, n: u32, region: Rect, k: f64) -> impl IntoElement {
        let c = region.center();
        let diameter = 112.0 * k;
        div()
            .absolute()
            .left(px((c.x - 150.0 * k) as f32))
            .top(px((c.y - diameter / 2.0) as f32))
            .w(px((300.0 * k) as f32))
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .child(
                div()
                    .id(("countdown", n))
                    .size(px(diameter as f32))
                    .rounded_full()
                    .bg(color::panel())
                    .border_1()
                    .border_color(color::hairline())
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(color::text())
                    .text_size(ui(52.))
                    .font_weight(FontWeight::BOLD)
                    .child(n.to_string())
                    .with_animation(
                        ("countdown-pop", n),
                        Animation::new(Duration::from_millis(350)).with_easing(gpui::ease_out_quint()),
                        |el, t| el.opacity(0.4 + 0.6 * t),
                    ),
            )
            .when(self.stop_hint, |el| {
                el.child(hud::pill("Stop with your record shortcut or `screenie stop`"))
            })
    }

    fn pill(&self, at: Point, cx: &mut Context<Self>) -> impl IntoElement {
        let (elapsed, paused) = pill_status(cx).unwrap_or_default();
        let counting = matches!(self.phase, Phase::Countdown(_));
        let sink = self.pill_bounds.clone();
        let size = pill_size(f64::from(screenie_ui_kit::ui_scale(cx)));

        // Static on purpose: an animation would repaint (and so re-encode) constantly.
        let dot = div().size(ui(10.)).rounded_full().bg(if paused || counting { color::text_dim() } else { color::record() });
        let label = match self.phase {
            Phase::Countdown(n) => format!("Starting in {n}"),
            Phase::Recording => format_elapsed(elapsed),
        };

        hud::panel()
            .absolute()
            .left(px(at.x as f32))
            .top(px(at.y as f32))
            .w(px(size.width as f32))
            .h(px(size.height as f32))
            .pl_3()
            .gap_1()
            .child(dot)
            .child(
                div()
                    .flex_1()
                    .pl_1()
                    .text_size(ui(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if paused { color::text_dim() } else { color::text() })
                    .child(label),
            )
            .when(!counting, |el| {
                el.child(
                    HudButton::new("pause")
                        .icon(if paused { Icon::Play } else { Icon::Pause })
                        .tooltip(if paused { "Resume" } else { "Pause" })
                        .on_click(|_, _, cx| {
                            super::toggle_pause(cx);
                        }),
                )
                .child(
                    // The universal stop glyph: a solid square.
                    div()
                        .id("stop")
                        .size(ui(30.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(ui(9.))
                        .bg(color::record())
                        .hover(|s| s.bg(color::record_hover()))
                        .cursor_pointer()
                        .child(div().size(ui(10.)).rounded(ui(2.5)).bg(gpui::white()))
                        .tooltip(Tip::new("Stop and save").builder())
                        .on_click(|_, _, cx| {
                            cx.spawn(async move |cx| super::stop(cx).await).detach();
                        }),
                )
            })
            .child(
                HudButton::new("discard")
                    .icon(if counting { Icon::Close } else { Icon::Trash })
                    .tooltip(if counting { "Cancel" } else { "Discard recording" })
                    .on_click(|_, _, cx| {
                        cx.spawn(async move |cx| super::cancel(cx).await).detach();
                    }),
            )
            .child(canvas(move |bounds, _, _| sink.borrow_mut().push(bounds), |_, _, _, _| {}).absolute().inset_0())
    }
}

impl Render for Controls {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.pill_bounds.borrow_mut().clear();
        let bounds = self.pill_bounds.clone();
        let k = f64::from(screenie_ui_kit::ui_scale(cx));
        div()
            .size_full()
            .relative()
            .font_family(screenie_ui_kit::FONT)
            .when_some(self.region.filter(|_| self.border), |el, r| el.child(self.border(r)))
            .when_some(self.region.zip(match self.phase {
                Phase::Countdown(n) => Some(n),
                Phase::Recording => None,
            }), |el, (r, n)| el.child(self.countdown(n, r, k)))
            .when_some(self.pill, |el, at| el.child(self.pill(at, cx)))
            // Painted last: only the pill takes input.
            .child(
                canvas(|_, _, _| {}, move |_, _, window, _| window.set_input_region(Some(&bounds.borrow())))
                    .absolute()
                    .size_0(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenie_core::Transform;

    fn output(name: &str, x: f64, w: f64, h: f64) -> OutputInfo {
        OutputInfo {
            name: name.into(),
            description: String::new(),
            logical: Rect::new(x, 0.0, w, h),
            scale: 1.0,
            transform: Transform::Normal,
        }
    }

    #[test]
    fn pill_goes_below_then_above_then_beside() {
        let o = output("A", 0.0, 1920.0, 1080.0);
        let outputs = [o.clone()];
        let (_, p) = place_pill(Rect::new(100.0, 100.0, 400.0, 300.0), &o, &outputs, 1.0).unwrap();
        assert_eq!(p.y, 412.0);
        let (_, p) = place_pill(Rect::new(100.0, 700.0, 400.0, 370.0), &o, &outputs, 1.0).unwrap();
        assert_eq!(p.y, 700.0 - GAP - PILL.height);
        let (_, p) = place_pill(Rect::new(0.0, 0.0, 1400.0, 1080.0), &o, &outputs, 1.0).unwrap();
        assert_eq!(p.x, 1400.0 + GAP);
    }

    #[test]
    fn full_screen_pill_moves_to_another_output_or_nowhere() {
        let a = output("A", 0.0, 1920.0, 1080.0);
        let b = output("B", 1920.0, 1706.0, 960.0);
        let outputs = [a.clone(), b];
        let (name, p) = place_pill(a.logical, &a, &outputs, 1.0).unwrap();
        assert_eq!(name, "B");
        assert!(p.x > 0.0 && p.y > 0.0, "local coordinates");
        assert!(place_pill(a.logical, &a, std::slice::from_ref(&a), 1.0).is_none());
    }

    #[test]
    fn elapsed_formatting() {
        assert_eq!(format_elapsed(Duration::from_secs(5)), "0:05");
        assert_eq!(format_elapsed(Duration::from_secs(754)), "12:34");
        assert_eq!(format_elapsed(Duration::from_secs(3723)), "1:02:03");
    }
}
