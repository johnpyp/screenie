//! On-screen chrome for a recording, on the recorded output: the countdown, a border just
//! outside the recorded region, and the control pill (timer, pause, stop, discard).
//!
//! Screen capture includes every surface, so nothing here may overlap the region while
//! recording. The border is drawn outside it, and the pill goes below, above or beside it.
//! A window recorded by itself sees none of this, and can move away from where it was: it
//! gets no border, and the pill may go over it.
//!
//! Nothing covers a fullscreen app once the countdown is over. An app filling its output
//! (a game) is scanned out directly, with no compositing and so the lowest latency (and
//! tearing, where allowed), only while it's the only thing there: anything over it,
//! however small or transparent, costs that. So an output a window fills gets neither
//! border nor pill. Neither does a region filling its output, nor one without room for
//! the pill next to it. The recording is then stopped with the same shortcut (or
//! `screenie stop`), which the countdown says.
//!
//! Only the pill takes input, and the keyboard only while the pointer is on it (see
//! [`Hover`]): the recorded app keeps its typing, and a game's pointer lock can't trap
//! the pointer on the pill.

use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt, App, AsyncApp, Context, Entity, FontWeight, Window, WindowHandle, div,
    px, rgba, size,
};
use screenie_core::{OutputInfo, Point, Rect, Size};
use screenie_ui_kit::hud::{self, HudButton, color};
use screenie_ui_kit::{Hover, Icon, LayerSpec, Tip, layer_options, ui};

use crate::daemon::Daemon;

/// Pill size in logical pixels (fixed, so it can be placed before it's laid out).
/// As tall as a panel of buttons, so the buttons in it sit concentric with its corners.
const PILL: Size = Size {
    width: 196.0,
    height: 38.0,
};
/// Space between the region and the chrome.
const GAP: f64 = 12.0;
/// The border ring sits this far outside the region.
const BORDER_OFFSET: f64 = 3.0;

/// What the recording captures, which decides what chrome it can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chrome {
    /// A fixed part of the screen, chrome included.
    Region,
    /// A window by itself.
    Window,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Countdown(u32),
    Recording,
}

/// Whether `rect` covers all of `output`: a whole screen, or a window filling it.
pub(crate) fn fills(rect: Rect, output: &OutputInfo) -> bool {
    let o = output.logical.inset(1.0);
    o.intersection(&rect) == Some(o)
}

/// Where the pill goes, top-left in `home`'s coordinates: below the region, else above,
/// else beside it. A window recorded by itself can have it over its bottom instead. `k`
/// is the interface scale.
fn place_pill(region: Rect, chrome: Chrome, home: &OutputInfo, k: f64) -> Option<Point> {
    let pill = pill_size(k);
    let gap = GAP * k;
    let o = home.logical;
    let fits_x = o.width >= pill.width + 2.0 * gap;
    let centered_x = (region.center().x - pill.width / 2.0)
        .clamp(o.x + gap, (o.right() - pill.width - gap).max(o.x));
    let local = |x: f64, y: f64| Some(Point::new(x - o.x, y - o.y));
    if fits_x && o.bottom() - region.bottom() >= pill.height + 2.0 * gap {
        return local(centered_x, region.bottom() + gap);
    }
    if fits_x && region.y - o.y >= pill.height + 2.0 * gap {
        return local(centered_x, region.y - gap - pill.height);
    }
    let side_y =
        (region.bottom() - pill.height).clamp(o.y + gap, (o.bottom() - pill.height - gap).max(o.y));
    if o.right() - region.right() >= pill.width + 2.0 * gap {
        return local(region.right() + gap, side_y);
    }
    if region.x - o.x >= pill.width + 2.0 * gap {
        return local(region.x - gap - pill.width, side_y);
    }
    (chrome == Chrome::Window).then(|| {
        Point::new(
            (o.width - pill.width) / 2.0,
            o.height - pill.height - 48.0 * k,
        )
    })
}

/// The pill's size at interface scale `k`.
fn pill_size(k: f64) -> Size {
    Size {
        width: PILL.width * k,
        height: PILL.height * k,
    }
}

/// What the chrome shows.
#[derive(Debug, Clone, Copy)]
struct Layout {
    /// The region, in the output's coordinates.
    region: Rect,
    /// Draw the border (not around a whole output, nor over a fullscreen app).
    border: bool,
    pill: Option<Point>,
    /// Shown during the countdown when nothing else tells the user how to stop.
    stop_hint: bool,
}

impl Layout {
    /// `covered`: a window fills the output, so nothing may stay over it.
    fn new(region: Rect, chrome: Chrome, home: &OutputInfo, covered: bool, k: f64) -> Self {
        let pill = if covered {
            None
        } else {
            place_pill(region, chrome, home, k)
        };
        Layout {
            region: region.translate(-home.logical.x, -home.logical.y),
            border: chrome == Chrome::Region && !covered && !fills(region, home),
            pill,
            stop_hint: pill.is_none(),
        }
    }

    /// Whether it has anything to show in `phase`.
    fn shows(&self, phase: Phase) -> bool {
        matches!(phase, Phase::Countdown(_)) || self.border || self.pill.is_some()
    }
}

/// The chrome's surface.
pub(crate) struct Controls {
    layout: Layout,
    phase: Phase,
    /// Input, on the pill (the only area).
    hover: Entity<Hover<()>>,
}

/// Open the chrome for `region` on `home`, if it has anything to show. `covered`: a
/// window fills `home` (a fullscreen app), which then keeps it to itself after the
/// countdown.
pub(crate) fn open(
    region: Rect,
    chrome: Chrome,
    home: &OutputInfo,
    covered: bool,
    phase: Phase,
    cx: &mut AsyncApp,
) -> Option<WindowHandle<Controls>> {
    let k = cx.update(|cx| f64::from(screenie_ui_kit::ui_scale(cx)));
    let layout = Layout::new(region, chrome, home, covered, k);
    if !layout.shows(phase) {
        return None;
    }
    let spec = LayerSpec::fullscreen_overlay(
        "screenie-recording",
        &home.name,
        size(
            px(home.logical.width as f32),
            px(home.logical.height as f32),
        ),
    )
    .passive();
    cx.update(|cx| {
        cx.open_window(layer_options(cx, &spec), |window, cx| {
            screenie_ui_kit::conceal::track(window, &spec, cx);
            screenie_ui_kit::track_ui_scale(window, cx);
            let hover = Hover::new(window, cx);
            cx.new(|cx| {
                // Keep the timer fresh, redrawing only when it changes: every frame we
                // draw is damage the recording has to encode.
                cx.spawn_in(window, async move |this, cx| {
                    let mut shown = None;
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(100))
                            .await;
                        let alive = this.update(cx, |_, cx| {
                            let now = pill_status(cx)
                                .map(|(elapsed, paused)| (elapsed.as_secs(), paused));
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
                Controls {
                    layout,
                    phase,
                    hover,
                }
            })
        })
    })
    .map_err(|e| tracing::warn!("cannot show recording controls on {}: {e}", home.name))
    .ok()
}

/// Move the chrome on to `phase`, closing it if it's left with nothing to show.
pub(crate) fn set_phase(handle: Option<WindowHandle<Controls>>, phase: Phase, cx: &mut AsyncApp) {
    let _ = handle.map(|handle| {
        handle.update(cx, |c, window, cx| {
            c.phase = phase;
            if c.layout.shows(phase) {
                cx.notify();
            } else {
                window.remove_window();
            }
        })
    });
}

/// Close the chrome once the keys held on it are let go.
pub(crate) fn close(handle: Option<WindowHandle<Controls>>, cx: &mut App) {
    let _ = handle.map(|handle| {
        handle.update(cx, |controls, _, cx| {
            controls.hover.update(cx, |hover, cx| {
                hover.when_released(
                    move |cx| _ = handle.update(cx, |_, window, _| window.remove_window()),
                    cx,
                );
            })
        })
    });
}

/// Elapsed time and paused state of the running recording.
fn pill_status(cx: &App) -> Option<(Duration, bool)> {
    let recording = Daemon::get(cx).recording.as_ref()?.recording.as_ref()?;
    Some((recording.elapsed(), recording.is_paused()))
}

fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
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
                        Animation::new(Duration::from_millis(350))
                            .with_easing(gpui::ease_out_quint()),
                        |el, t| el.opacity(0.4 + 0.6 * t),
                    ),
            )
            .when(self.layout.stop_hint, |el| {
                el.child(hud::pill(
                    "Stop with your record shortcut or `screenie stop`",
                ))
            })
    }

    fn pill(&self, at: Point, cx: &mut Context<Self>) -> impl IntoElement {
        let (elapsed, paused) = pill_status(cx).unwrap_or_default();
        let counting = matches!(self.phase, Phase::Countdown(_));
        let size = pill_size(f64::from(screenie_ui_kit::ui_scale(cx)));

        // Static on purpose: an animation would repaint (and so re-encode) constantly.
        let dot = div()
            .size(ui(10.))
            .rounded_full()
            .bg(if paused || counting {
                color::text_dim()
            } else {
                color::record()
            });
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
                    .text_color(if paused {
                        color::text_dim()
                    } else {
                        color::text()
                    })
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
                        .rounded(hud::inner_radius())
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
                    .tooltip(if counting {
                        "Cancel"
                    } else {
                        "Discard recording"
                    })
                    .on_click(|_, _, cx| {
                        cx.spawn(async move |cx| super::cancel(cx).await).detach();
                    }),
            )
            .child(Hover::area(&self.hover, ()))
    }
}

impl Render for Controls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let k = f64::from(screenie_ui_kit::ui_scale(cx));
        let Layout {
            region,
            border,
            pill,
            ..
        } = self.layout;
        let counting = match self.phase {
            Phase::Countdown(n) => Some(n),
            Phase::Recording => None,
        };
        let root = div()
            .size_full()
            .relative()
            .font_family(screenie_ui_kit::FONT)
            .when(border, |el| el.child(self.border(region)))
            .when_some(counting, |el, n| el.child(self.countdown(n, region, k)))
            .when_some(pill, |el, at| el.child(self.pill(at, cx)));
        let root = screenie_ui_kit::conceal::root(root, window, cx);
        Hover::root(&self.hover, root, cx)
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
        let p = place_pill(
            Rect::new(100.0, 100.0, 400.0, 300.0),
            Chrome::Region,
            &o,
            1.0,
        )
        .unwrap();
        assert_eq!(p.y, 412.0);
        let p = place_pill(
            Rect::new(100.0, 700.0, 400.0, 370.0),
            Chrome::Region,
            &o,
            1.0,
        )
        .unwrap();
        assert_eq!(p.y, 700.0 - GAP - PILL.height);
        let p = place_pill(Rect::new(0.0, 0.0, 1400.0, 1080.0), Chrome::Region, &o, 1.0).unwrap();
        assert_eq!(p.x, 1400.0 + GAP);
    }

    #[test]
    fn a_whole_screen_has_no_pill() {
        let a = output("A", 0.0, 1920.0, 1080.0);
        let layout = Layout::new(a.logical, Chrome::Region, &a, false, 1.0);
        assert!(!layout.border && layout.pill.is_none() && layout.stop_hint);
        assert!(!layout.shows(Phase::Recording));
    }

    #[test]
    fn a_window_with_no_room_around_it_has_its_pill_over_it() {
        let a = output("A", 0.0, 1920.0, 1080.0);
        let maximized = Rect::new(0.0, 30.0, 1920.0, 1050.0);
        let p = place_pill(maximized, Chrome::Window, &a, 1.0).unwrap();
        assert_eq!(p.y, 1080.0 - PILL.height - 48.0);
    }

    #[test]
    fn nothing_stays_over_a_fullscreen_app() {
        let a = output("A", 0.0, 1920.0, 1080.0);
        let region = Rect::new(560.0, 240.0, 800.0, 600.0);
        // A region of a screen a game fills, or the game recorded by itself.
        for (rect, chrome) in [(region, Chrome::Region), (a.logical, Chrome::Window)] {
            let layout = Layout::new(rect, chrome, &a, true, 1.0);
            assert!(layout.shows(Phase::Countdown(3)));
            assert!(!layout.shows(Phase::Recording), "{chrome:?}");
            assert!(layout.stop_hint);
        }
        let windowed = Layout::new(region, Chrome::Region, &a, false, 1.0);
        assert!(windowed.border && windowed.pill.is_some() && windowed.shows(Phase::Recording));
    }

    #[test]
    fn elapsed_formatting() {
        assert_eq!(format_elapsed(Duration::from_secs(5)), "0:05");
        assert_eq!(format_elapsed(Duration::from_secs(754)), "12:34");
        assert_eq!(format_elapsed(Duration::from_secs(3723)), "1:02:03");
    }
}
