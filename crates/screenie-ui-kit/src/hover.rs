//! Pointer input for overlays that float over other apps (preview cards, the recording
//! pill): they take the pointer on their interactive areas only, and never the keyboard.
//!
//! Never taking the keyboard is the point. Keyboard focus that moves while a key is held
//! splits the press: the app beneath saw it go down, the overlay gets its release. Apps
//! that don't catch up when focus returns keep the key held: a Proton game's Steam
//! overlay, fed through Xwayland (which never delivers releases that happened elsewhere),
//! later reads Shift as Shift+Tab. Notifications, bars and docks work the same way, for
//! the same reason: what they offer, the pointer does.
//!
//! HACK: sway has a bug. It enforces a pointer constraint (a game's pointer lock or confine)
//! for the surface that has the keyboard, and drops every motion while the cursor is over
//! another surface, so a game that locks the pointer while the cursor is on a card would
//! freeze it there. The pointer still reports relative motion to the card, which is how
//! GPUI tells that it's stuck ([`Window::observe_pointer_stuck`]). The surface then steps
//! aside: its input region empties and it has sway pick the surface under the cursor
//! again, which is now the game, and its areas come back a moment later. The other compositors (Hyprland,
//! KWin, niri, mutter) give the constrained surface the pointer themselves, and a
//! pointer never gets stuck there.
//!
//! A surface starts with an empty input region and gets its areas once it's on screen.
//! Compositors re-pick the pointer's surface when a surface maps, but not when an input
//! region grows, so an overlay that appears under a resting pointer isn't entered when it
//! appears. sway does re-pick later without any motion (on a button release, after a
//! layout change), so a pointer parked on an area is entered then.
//!
//! Hover follows the raw pointer events, in the capture phase, so no element can hide a
//! move from it, and it's exact at the areas' edges.
//!
//! ```ignore
//! // Opening the surface (with no keyboard interactivity):
//! let hover = Hover::new(window, cx);
//! cx.observe(&hover, |_, _, cx| cx.notify()).detach();
//! // Rendering: mark the areas, then finish the root.
//! let pill = div().relative().child(...).child(Hover::area(&self.hover, Part::Pill));
//! Hover::root(&self.hover, div().size_full().child(pill), cx)
//! ```

use std::time::Duration;

use gpui::layer_shell::Anchor;
use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, DispatchPhase, Entity, MouseExitEvent, MouseMoveEvent, Pixels, Point,
    Subscription, Window, canvas, px, size,
};

use crate::layer::{LayerSpec, layer_options};

/// How long a surface whose pointer got stuck takes no input.
const STEP_ASIDE: Duration = Duration::from_millis(1500);
/// How long [`repick_pointer`]'s pixel stays mapped.
const REPICK: Duration = Duration::from_millis(100);

/// Pointer input for one layer surface opened without keyboard interactivity. `K` names
/// its interactive areas. Observe it to redraw when the hovered area changes.
pub struct Hover<K: 'static> {
    /// Areas as the current frame paints them.
    painting: Vec<(K, Bounds<Pixels>)>,
    /// Areas as last painted.
    areas: Vec<(K, Bounds<Pixels>)>,
    /// The input region as last set.
    region: Vec<Bounds<Pixels>>,
    /// Whether the surface has been on screen, and its areas take input.
    shown: bool,
    /// Whether it's stepping aside for a pointer that got stuck on it.
    aside: bool,
    /// Bumped each time it steps aside, so a stale timer does nothing.
    epoch: u64,
    /// Where the pointer is on the surface, if it is.
    pointer: Option<Point<Pixels>>,
    hovered: Option<K>,
    _stuck: Subscription,
}

impl<K: Clone + PartialEq + 'static> Hover<K> {
    /// For the layer surface `window`, opened with no keyboard interactivity.
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
        window.set_input_region(Some(&[]));
        cx.new(|cx| {
            let this = cx.weak_entity();
            let stuck = window.observe_pointer_stuck(move |window, cx| {
                let _ = this.update(cx, |h: &mut Self, cx| h.step_aside(window, cx));
            });
            Self {
                painting: Vec::new(),
                areas: Vec::new(),
                region: Vec::new(),
                shown: false,
                aside: false,
                epoch: 0,
                pointer: None,
                hovered: None,
                _stuck: stuck,
            }
        })
    }

    /// The area the pointer is on.
    pub fn hovered(&self) -> Option<&K> {
        self.hovered.as_ref()
    }

    /// Make the element this is a child of an interactive area: it takes the pointer.
    /// Fills the parent, which must be `relative`.
    pub fn area(this: &Entity<Self>, key: K) -> impl IntoElement {
        let this = this.clone();
        canvas(
            move |bounds, _, cx| this.update(cx, |h, _| h.painting.push((key, bounds))),
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// Finish the surface's root element, once its children are in: it tracks the
    /// pointer over the areas, and limits input to them.
    pub fn root<E: ParentElement>(this: &Entity<Self>, root: E, _cx: &App) -> E {
        // Painted after every area has been prepainted.
        let this = this.clone();
        root.child(
            canvas(
                |_, _, _| {},
                move |_, _, window, cx| this.update(cx, |h, cx| h.painted(window, cx)),
            )
            .absolute()
            .size_0(),
        )
    }

    /// The frame's areas are all painted: update the input region, listen to the
    /// pointer, and re-check the hover for areas that moved under it.
    fn painted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.areas = std::mem::take(&mut self.painting);
        // A concealed surface is invisible, so it takes no input either.
        if crate::conceal::hidden(window, cx) {
            self.areas.clear();
        }
        if self.shown {
            self.apply_region(window);
        } else {
            // The next frame comes once the compositor has shown this one.
            let this = cx.entity();
            window.on_next_frame(move |window, cx| {
                this.update(cx, |h, _| {
                    h.shown = true;
                    h.apply_region(window);
                })
            });
        }

        let this = cx.entity();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture {
                this.update(cx, |h, cx| h.pointer_at(Some(e.position), cx));
            }
        });
        let this = cx.entity();
        window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture {
                this.update(cx, |h, cx| h.pointer_at(None, cx));
            }
        });

        // A card dismissed and the next sliding up under a still pointer: re-check once
        // the frame is done, as GPUI does for its own hover.
        if self.area_at(self.pointer) != self.hovered {
            let this = cx.entity();
            cx.defer(move |cx| this.update(cx, |h, cx| h.pointer_at(h.pointer, cx)));
        }
    }

    /// The pointer is stuck on the surface (see the module docs): take no input for a
    /// moment, so it moves on to the surface that holds it.
    ///
    /// HACK: works around a sway bug (1.12 and master as of 2026-09). sway activates a
    /// pointer constraint when its surface gets keyboard focus but leaves pointer focus
    /// on whatever is under the cursor, then drops every motion while that isn't the
    /// constrained surface, although pointer-constraints guarantees the constrained
    /// surface already has pointer focus. Drop this once sway gives it the pointer.
    fn step_aside(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::debug!("overlay stepping aside for a stuck pointer");
        self.aside = true;
        self.epoch += 1;
        self.apply_region(window);
        repick_pointer(cx);
        self.pointer_at(None, cx);
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(STEP_ASIDE).await;
            let _ = this.update(cx, |h, cx| {
                if h.epoch == epoch {
                    h.aside = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn apply_region(&mut self, window: &mut Window) {
        let region: Vec<_> = if self.aside {
            Vec::new()
        } else {
            self.areas.iter().map(|(_, b)| *b).collect()
        };
        if region != self.region {
            window.set_input_region(Some(&region));
            self.region = region;
        }
    }

    fn area_at(&self, pointer: Option<Point<Pixels>>) -> Option<K> {
        let p = pointer?;
        self.areas
            .iter()
            .find(|(_, b)| b.contains(&p))
            .map(|(key, _)| key.clone())
    }

    fn pointer_at(&mut self, pointer: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        self.pointer = pointer;
        let hovered = self.area_at(pointer);
        if hovered != self.hovered {
            self.hovered = hovered;
            cx.notify();
        }
    }
}

/// HACK: part of the sway workaround in [`Hover::step_aside`]. sway (1.12 and master as
/// of 2026-09) doesn't pick the surface under the cursor again when an input region
/// changes, only when a surface maps or unmaps.
///
/// Have the compositor pick the surface under the pointer again, as it does whenever a
/// surface maps or unmaps. Nothing else is sure to: sway (up to 1.12) doesn't for a
/// changed input region, and under a lock no motion does it either. So this maps an empty
/// pixel that takes no input, and takes it away again.
fn repick_pointer(cx: &mut App) {
    let spec = LayerSpec::floating(
        "screenie-repick",
        Anchor::TOP | Anchor::LEFT,
        size(px(1.), px(1.)),
    );
    let opened = cx.open_window(layer_options(cx, &spec), |window, cx| {
        window.set_input_region(Some(&[]));
        cx.new(|_| Blank)
    });
    match opened {
        Ok(handle) => cx
            .spawn(async move |cx| {
                cx.background_executor().timer(REPICK).await;
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            })
            .detach(),
        Err(e) => tracing::warn!("can't map a surface to re-pick the pointer: {e}"),
    }
}

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}
