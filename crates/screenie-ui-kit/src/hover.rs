//! Input for overlays that float over other apps without owning the keyboard (preview
//! cards, the recording pill): they take the pointer only on their interactive areas,
//! and the keyboard only while the pointer is on one.
//!
//! One rule covers it: **a surface holds the keyboard exactly while the pointer is on one
//! of its areas**, and until the keys pressed there are let go. On sway, pointer focus
//! without keyboard focus is a trap. The app that has the keyboard gets its pointer lock
//! back (a fullscreen game, as soon as it's focused), and while the locked surface isn't
//! the one under the cursor, every motion is dropped. The cursor would sit frozen on the
//! overlay. Holding the keyboard lifts the lock, and giving it back once the pointer is off
//! returns the app everything. It also makes keys pressed with the pointer on a card mean
//! the card: Esc dismisses it, rather than going to the app beneath.
//!
//! A surface starts with an empty input region and gets its areas once it's on screen.
//! Compositors re-pick the pointer's surface when a surface maps, but not when an input
//! region grows. So an overlay that appears under a resting pointer isn't entered (and
//! doesn't take the typing of someone whose mouse happens to be there) until the pointer
//! moves.
//!
//! Letting go of the keyboard, it goes back to a surface that owns it, if one is open
//! (see [`crate::KeyboardGrab::for_window`]).
//!
//! Hover follows the raw pointer events, in the capture phase, so no element can hide a
//! move from it, and it's exact at the edges of the areas it hands the keyboard over at.
//!
//! ```ignore
//! // Opening the surface (with no keyboard interactivity):
//! let hover = Hover::new(window, cx);
//! cx.observe(&hover, |_, _, cx| cx.notify()).detach();
//! // Rendering: mark the areas, then finish the root.
//! let pill = div().relative().child(...).child(Hover::area(&self.hover, Part::Pill));
//! Hover::root(&self.hover, div().size_full().child(pill), cx)
//! // Closing:
//! self.hover.update(cx, |h, cx| h.when_released(move |cx| close(cx), cx));
//! ```

use gpui::layer_shell::KeyboardInteractivity;
use gpui::prelude::*;
use gpui::{
    AnyWindowHandle, App, Bounds, Context, DispatchPhase, Entity, FocusHandle, MouseExitEvent,
    MouseMoveEvent, Pixels, Point, Window, canvas,
};

use crate::keys::{RELEASE_TIMEOUT, Release, Then, owners, run};

/// Input for one layer surface opened without keyboard interactivity. `K` names its
/// interactive areas. Observe it to redraw when the hovered area changes.
pub struct Hover<K: 'static> {
    window: AnyWindowHandle,
    focus: FocusHandle,
    /// Areas as the current frame paints them.
    painting: Vec<(K, Bounds<Pixels>)>,
    /// Areas as last painted.
    areas: Vec<(K, Bounds<Pixels>)>,
    /// The input region as last set.
    region: Vec<Bounds<Pixels>>,
    /// Whether the surface has been on screen, and its areas take input.
    shown: bool,
    /// Where the pointer is on the surface, if it is.
    pointer: Option<Point<Pixels>>,
    hovered: Option<K>,
    /// Whether the surface has asked for the keyboard.
    taken: bool,
    /// Bumped each time the pointer leaves the areas, so a stale timeout does nothing.
    epoch: u64,
    release: Release<Then>,
}

impl<K: Clone + PartialEq + 'static> Hover<K> {
    /// For the layer surface `window`, opened with no keyboard interactivity.
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
        window.set_input_region(Some(&[]));
        let handle = window.window_handle();
        cx.new(|cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            Self {
                window: handle,
                focus,
                painting: Vec::new(),
                areas: Vec::new(),
                region: Vec::new(),
                shown: false,
                pointer: None,
                hovered: None,
                taken: false,
                epoch: 0,
                release: Release::default(),
            }
        })
    }

    /// The area the pointer is on.
    pub fn hovered(&self) -> Option<&K> {
        self.hovered.as_ref()
    }

    /// Whether the surface has (or has asked for) the keyboard. Removing it now would
    /// release its held keys into the app beneath; [`Hover::when_released`] waits.
    pub fn has_keyboard(&self) -> bool {
        self.taken
    }

    /// Whether [`Hover::when_released`] was called.
    pub fn leaving(&self) -> bool {
        self.release.leaving()
    }

    /// Make the element this is a child of an interactive area: it takes the pointer, and
    /// the keyboard while the pointer is on it. Fills the parent, which must be `relative`.
    pub fn area(this: &Entity<Self>, key: K) -> impl IntoElement {
        let this = this.clone();
        canvas(
            move |bounds, _, cx| this.update(cx, |h, _| h.painting.push((key, bounds))),
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// Finish the surface's root element, once its children are in: it holds the focus,
    /// tracks the keys held on the surface and the pointer over its areas, and limits
    /// input to those areas.
    pub fn root<E: InteractiveElement + ParentElement>(
        this: &Entity<Self>,
        root: E,
        cx: &App,
    ) -> E {
        let (down, up, mods) = (this.clone(), this.clone(), this.clone());
        let root = root
            .track_focus(&this.read(cx).focus)
            // Capture phase, so the view's own handlers can't hide a key from it. Once
            // leaving, the view sees no more keys.
            .capture_key_down(move |event, _, cx| {
                if down.update(cx, |h, _| {
                    h.release.held.press(event);
                    h.release.leaving()
                }) {
                    cx.stop_propagation();
                }
            })
            .capture_key_up(move |event, _, cx| {
                if up.update(cx, |h, cx| {
                    h.release.held.release(event);
                    h.settle(cx);
                    h.release.leaving()
                }) {
                    cx.stop_propagation();
                }
            })
            .on_modifiers_changed(move |event, _, cx| {
                mods.update(cx, |h, cx| {
                    h.release.held.set_modifiers(event);
                    h.settle(cx);
                })
            });
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

    /// Run `then` (typically removing the surface) once no keys are held on it: now, on
    /// the last release, or after [`RELEASE_TIMEOUT`]. It runs once, outside any event
    /// dispatch, however many times this is called.
    pub fn when_released(&mut self, then: impl FnOnce(&mut App) + 'static, cx: &mut Context<Self>) {
        if self.release.leaving() {
            return;
        }
        self.release.leave(Box::new(then));
        self.settle(cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RELEASE_TIMEOUT).await;
            let _ = this.update(cx, |h, cx| run(h.release.take(), cx));
        })
        .detach();
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

    fn apply_region(&mut self, window: &mut Window) {
        let region: Vec<_> = self.areas.iter().map(|(_, b)| *b).collect();
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
        if hovered == self.hovered {
            return;
        }
        if hovered.is_none() {
            // A release can go missing (focus taken mid-press): don't hold on for good.
            self.epoch += 1;
            let epoch = self.epoch;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(RELEASE_TIMEOUT).await;
                let _ = this.update(cx, |h, cx| {
                    if h.epoch == epoch {
                        h.release.held = Default::default();
                        h.settle(cx);
                    }
                });
            })
            .detach();
        }
        self.hovered = hovered;
        self.settle(cx);
        cx.notify();
    }

    /// Take or give back the keyboard as the pointer and the held keys say, and close if
    /// that's waiting on the keys.
    fn settle(&mut self, cx: &mut Context<Self>) {
        let held = self.release.held.any();
        run(self.release.ready(), cx);
        // Leaving, it keeps the keyboard only for the keys still held.
        let take = (self.hovered.is_some() && !self.release.leaving()) || (self.taken && held);
        if take == self.taken {
            return;
        }
        self.taken = take;
        tracing::debug!(taken = take, "hover keyboard");
        let interactivity = if take {
            KeyboardInteractivity::Exclusive
        } else {
            KeyboardInteractivity::None
        };
        // Letting go, the keyboard goes back to a surface that owns it (an editor) if
        // there is one. Compositors don't do that themselves: sway only looks for another
        // keyboard-owning surface on this surface's output, and otherwise gives the keys
        // to the last focused window. Re-asserting the owner's claim first moves focus
        // straight there, never through the window beneath.
        let owners = if take { Vec::new() } else { owners(cx) };
        // Not from inside the window's own event dispatch.
        let window = self.window;
        cx.defer(move |cx| {
            for owner in owners {
                let _ = owner.update(cx, |_, window, _| {
                    window.set_keyboard_interactivity(KeyboardInteractivity::Exclusive)
                });
            }
            let _ = window.update(cx, |_, window, _| {
                window.set_keyboard_interactivity(interactivity)
            });
        });
    }
}
