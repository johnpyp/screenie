//! Letting go of the keyboard cleanly.
//!
//! When a surface with keyboard focus goes away, the compositor hands focus to whatever
//! is beneath, along with the keys still held. That app then sees them as pressed and
//! gets their release: an Esc or Enter meant for us, leaking into it. So a surface that
//! closes from a key press has to wait for the keys to be let go first.
//!
//! [`KeyboardGrab`] does that for every surface that takes the keyboard: attach it to the
//! surface's root with [`KeyboardGrab::track`], and close through
//! [`KeyboardGrab::when_released`].
//!
//! [`crate::Hover`] takes the same care for a surface that has the keyboard only while
//! the pointer is on it.

use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, Entity, InteractiveElement, KeyDownEvent, KeyUpEvent, Modifiers,
    ModifiersChangedEvent,
};
use smallvec::SmallVec;

/// How long to wait for keys to be released before going ahead anyway. A release can go
/// missing: focus taken away mid-press, or a key whose name changed with Shift.
pub const RELEASE_TIMEOUT: Duration = Duration::from_millis(1000);

pub(crate) type Then = Box<dyn FnOnce(&mut App)>;

/// Tracks the keys held on one or more surfaces (a selector spans every output) and runs
/// the closing action once they're all released.
#[derive(Default)]
pub struct KeyboardGrab {
    release: Release<Then>,
}

impl KeyboardGrab {
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self::default())
    }

    /// Track the keys on the surface rooted at `root`. Listens in the capture phase, so
    /// the view's own handlers can't hide a key from it; once leaving, it also swallows
    /// key presses and releases so the view sees nothing more.
    pub fn track<E: InteractiveElement>(this: &Entity<Self>, root: E) -> E {
        let (down, up, mods) = (this.clone(), this.clone(), this.clone());
        root.capture_key_down(move |event, _, cx| {
            if down.update(cx, |grab, _| {
                grab.release.held.press(event);
                grab.release.leaving()
            }) {
                cx.stop_propagation();
            }
        })
        .capture_key_up(move |event, _, cx| {
            if up.update(cx, |grab, cx| {
                grab.release.held.release(event);
                grab.settle(cx);
                grab.release.leaving()
            }) {
                cx.stop_propagation();
            }
        })
        .on_modifiers_changed(move |event, _, cx| {
            mods.update(cx, |grab, cx| {
                grab.release.held.set_modifiers(event);
                grab.settle(cx);
            });
        })
    }

    /// Run `then` (typically closing the surface) as soon as no keys are held: now, on
    /// the last release, or after [`RELEASE_TIMEOUT`]. It runs once, outside any event
    /// dispatch, however many times this is called.
    pub fn when_released(&mut self, then: impl FnOnce(&mut App) + 'static, cx: &mut Context<Self>) {
        if self.release.leaving() {
            return;
        }
        self.release.leave(Box::new(then));
        cx.notify();
        self.settle(cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RELEASE_TIMEOUT).await;
            let _ = this.update(cx, |grab, cx| run(grab.release.take(), cx));
        })
        .detach();
    }

    /// Whether [`Self::when_released`] was called: the surface is on its way out, and
    /// should look like it's gone already.
    pub fn leaving(&self) -> bool {
        self.release.leaving()
    }

    fn settle(&mut self, cx: &mut Context<Self>) {
        run(self.release.ready(), cx);
    }
}

/// Run a closing action once the current event is done with.
pub(crate) fn run(then: Option<Then>, cx: &mut App) {
    if let Some(then) = then {
        cx.defer(then);
    }
}

/// A closing surface's state, free of GPUI: what's held, and the action waiting on it.
pub(crate) struct Release<T> {
    pub(crate) held: HeldKeys,
    then: Option<T>,
    leaving: bool,
}

impl<T> Default for Release<T> {
    fn default() -> Self {
        Self {
            held: HeldKeys::default(),
            then: None,
            leaving: false,
        }
    }
}

impl<T> Release<T> {
    pub(crate) fn leave(&mut self, then: T) {
        if !self.leaving {
            self.leaving = true;
            self.then = Some(then);
        }
    }

    pub(crate) fn leaving(&self) -> bool {
        self.leaving
    }

    /// The action, if it's waiting and nothing is held any more.
    pub(crate) fn ready(&mut self) -> Option<T> {
        if self.held.any() { None } else { self.take() }
    }

    /// The action regardless of keys (the timeout); at most once.
    pub(crate) fn take(&mut self) -> Option<T> {
        self.then.take()
    }
}

/// The keys (modifiers included) held down on a surface.
#[derive(Debug, Default)]
pub(crate) struct HeldKeys {
    keys: SmallVec<[String; 4]>,
    modifiers: Modifiers,
}

impl HeldKeys {
    pub(crate) fn press(&mut self, event: &KeyDownEvent) {
        let key = &event.keystroke.key;
        if !self.keys.contains(key) {
            self.keys.push(key.clone());
        }
        self.modifiers = event.keystroke.modifiers;
    }

    pub(crate) fn release(&mut self, event: &KeyUpEvent) {
        self.keys.retain(|k| *k != event.keystroke.key);
        self.modifiers = event.keystroke.modifiers;
    }

    pub(crate) fn set_modifiers(&mut self, event: &ModifiersChangedEvent) {
        self.modifiers = event.modifiers;
    }

    pub(crate) fn any(&self) -> bool {
        !self.keys.is_empty() || self.modifiers.modified()
    }
}

#[cfg(test)]
mod tests {
    use gpui::Keystroke;

    use super::*;

    fn keystroke(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.into(),
            key_char: None,
        }
    }

    fn down(key: &str, modifiers: Modifiers) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: keystroke(key, modifiers),
            is_held: false,
            prefer_character_input: false,
        }
    }

    fn up(key: &str, modifiers: Modifiers) -> KeyUpEvent {
        KeyUpEvent {
            keystroke: keystroke(key, modifiers),
        }
    }

    const CTRL: Modifiers = Modifiers {
        control: true,
        alt: false,
        shift: false,
        platform: false,
        function: false,
    };

    #[test]
    fn waits_for_keys_and_modifiers() {
        let mut r = Release::default();
        r.held.press(&down("w", CTRL));
        r.leave("close");
        assert_eq!(r.ready(), None);
        r.held.release(&up("w", CTRL));
        assert_eq!(r.ready(), None, "Ctrl is still down");
        r.held.set_modifiers(&ModifiersChangedEvent {
            modifiers: Modifiers::default(),
            ..Default::default()
        });
        assert_eq!(r.ready(), Some("close"));
        assert_eq!(r.ready(), None, "only once");
        assert_eq!(r.take(), None, "not even on the timeout");
    }

    #[test]
    fn ready_at_once_with_nothing_held_and_first_action_wins() {
        let mut r = Release::default();
        r.leave("first");
        r.leave("second");
        assert_eq!(r.ready(), Some("first"));
    }

    #[test]
    fn timeout_takes_it_while_held() {
        let mut r = Release::default();
        r.held.press(&down("enter", Modifiers::default()));
        r.leave("close");
        assert_eq!(r.ready(), None);
        assert_eq!(r.take(), Some("close"));
        r.held.release(&up("enter", Modifiers::default()));
        assert_eq!(r.ready(), None);
    }
}
