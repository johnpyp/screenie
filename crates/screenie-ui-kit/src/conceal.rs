//! Keeping screenie out of its own captures.
//!
//! Screen capture copies everything on an output, screenie's own surfaces included: the
//! last shot's preview card, the recording ring and pill, an overlay editor, a selector.
//! Each of them is [`track`]ed when it opens and finishes its root element with [`root`],
//! which draws it fully transparent while it's concealed ([`Hover`](crate::Hover) also
//! drops its input region then). [`conceal`] conceals the surfaces a [`Scope`] covers and
//! resolves once the compositor has shown them gone, so a capture taken then can't
//! contain them. Nothing is waited for when none of them is on screen.
//!
//! Surfaces stay mapped while concealed: unmapping would run compositors' close and open
//! animations (Hyprland fades layers), and their element state (a card's slide-in, the
//! editor's canvas) carries on as it was.

use std::time::Duration;

use gpui::{AnyWindowHandle, App, AsyncApp, BorrowAppContext, Global, Styled, Window, WindowId};

use crate::LayerSpec;

/// The longest [`conceal`] waits for the compositor to show a concealed frame. It takes
/// a frame or two; a surface on an output that's off never shows one.
const PRESENT_TIMEOUT: Duration = Duration::from_millis(150);

/// Which surfaces to conceal.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    /// Only surfaces with this layer namespace.
    namespace: Option<&'static str>,
    /// Only surfaces on this output (and those whose output isn't known).
    output: Option<String>,
}

impl Scope {
    /// Every surface, on every output: for a screenshot.
    pub fn everything() -> Self {
        Self::default()
    }

    /// Only surfaces opened with the layer namespace `namespace`.
    pub fn only(namespace: &'static str) -> Self {
        Self {
            namespace: Some(namespace),
            output: None,
        }
    }

    /// Only those on `output`.
    pub fn on(mut self, output: impl Into<String>) -> Self {
        self.output = Some(output.into());
        self
    }

    fn covers(&self, namespace: &str, output: Option<&str>) -> bool {
        self.namespace.is_none_or(|n| n == namespace)
            && match (self.output.as_deref(), output) {
                (Some(want), Some(on)) => want == on,
                _ => true,
            }
    }
}

struct Surface {
    window: AnyWindowHandle,
    namespace: &'static str,
    output: Option<String>,
}

impl Surface {
    fn in_scope(&self, scope: &Scope) -> bool {
        scope.covers(self.namespace, self.output.as_deref())
    }
}

#[derive(Default)]
struct Registry {
    surfaces: Vec<Surface>,
    /// Concealments in force, by id.
    holds: Vec<(u64, Scope)>,
    next: u64,
}

impl Global for Registry {}

impl Registry {
    fn surface(&self, window: WindowId) -> Option<&Surface> {
        self.surfaces
            .iter()
            .find(|s| s.window.window_id() == window)
    }

    fn concealed(&self, surface: &Surface) -> bool {
        self.holds.iter().any(|(_, scope)| surface.in_scope(scope))
    }

    /// Forget surfaces whose windows are gone.
    fn prune(&mut self, open: &[AnyWindowHandle]) {
        self.surfaces
            .retain(|s| open.iter().any(|w| w.window_id() == s.window.window_id()));
    }
}

/// Keep `window`, a layer surface opened as `spec` describes, out of captures. Call it
/// when the window is built, before its first frame, and finish its root with [`root`].
pub fn track(window: &Window, spec: &LayerSpec, cx: &mut App) {
    let open = cx.windows();
    let registry = cx.default_global::<Registry>();
    registry.prune(&open);
    registry.surfaces.push(Surface {
        window: window.window_handle(),
        namespace: spec.namespace,
        output: spec.output.clone(),
    });
}

/// Whether `window` is concealed right now.
pub fn hidden(window: &Window, cx: &App) -> bool {
    cx.try_global::<Registry>().is_some_and(|r| {
        r.surface(window.window_handle().window_id())
            .is_some_and(|s| r.concealed(s))
    })
}

/// Finish a tracked surface's root element: fully transparent while it's concealed. It's
/// still laid out and painted, so nothing about it is lost.
pub fn root<E: Styled>(root: E, window: &Window, cx: &App) -> E {
    if hidden(window, cx) {
        root.opacity(0.)
    } else {
        root
    }
}

/// While this lives, the surfaces its [`Scope`] covers stay concealed (new ones too).
#[must_use = "the surfaces are shown again as soon as this is dropped"]
pub struct Concealed {
    id: u64,
    cx: AsyncApp,
}

impl Drop for Concealed {
    fn drop(&mut self) {
        // Dropping can happen mid-update (with the daemon's state, say): let go after.
        let id = self.id;
        self.cx
            .spawn(async move |cx| cx.update(|cx| release(id, cx)))
            .detach();
    }
}

/// Conceal the surfaces `scope` covers, and wait until the compositor has shown the
/// frames without them (at most [`PRESENT_TIMEOUT`]). Returns straight away if none of
/// them is on screen.
pub async fn conceal(scope: Scope, cx: &mut AsyncApp) -> Concealed {
    let (id, fresh) = cx.update(|cx| {
        let open = cx.windows();
        let registry = cx.default_global::<Registry>();
        registry.prune(&open);
        // Surfaces another concealment already hides are gone from the screen.
        let fresh: Vec<AnyWindowHandle> = registry
            .surfaces
            .iter()
            .filter(|s| s.in_scope(&scope) && !registry.concealed(s))
            .map(|s| s.window)
            .collect();
        let id = registry.next;
        registry.next += 1;
        registry.holds.push((id, scope));
        (id, fresh)
    });
    let concealed = Concealed { id, cx: cx.clone() };
    if fresh.is_empty() {
        return concealed;
    }

    let (shown, frames) = async_channel::bounded::<()>(fresh.len());
    let mut waiting = 0;
    for window in fresh {
        let shown = shown.clone();
        let redrawn = window.update(cx, |_, window, _| {
            window.refresh();
            // The first callback runs as the concealed frame is drawn. The next frame
            // only comes once the compositor has shown that one (GPUI waits for its
            // frame callback), and has taken the surface's old content off the screen.
            window.on_next_frame(move |window, _| {
                window.on_next_frame(move |_, _| _ = shown.try_send(()))
            });
        });
        waiting += usize::from(redrawn.is_ok());
    }
    drop(shown);
    let presented = async {
        for _ in 0..waiting {
            if frames.recv().await.is_err() {
                break;
            }
        }
    };
    let timeout = async {
        cx.background_executor().timer(PRESENT_TIMEOUT).await;
        tracing::debug!("a concealed surface wasn't shown in time; capturing anyway");
    };
    futures_lite::future::or(presented, timeout).await;
    concealed
}

/// End concealment `id`, and redraw the surfaces it hid.
fn release(id: u64, cx: &mut App) {
    let shown: Vec<AnyWindowHandle> = cx.update_global::<Registry, _>(|registry, _| {
        let Some(at) = registry.holds.iter().position(|(h, _)| *h == id) else {
            return Vec::new();
        };
        let (_, scope) = registry.holds.remove(at);
        registry
            .surfaces
            .iter()
            .filter(|s| s.in_scope(&scope) && !registry.concealed(s))
            .map(|s| s.window)
            .collect()
    });
    for window in shown {
        let _ = window.update(cx, |_, window, _| window.refresh());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes() {
        let everything = Scope::everything();
        assert!(everything.covers("screenie-preview", Some("DP-1")));
        assert!(everything.covers("screenie-editor", None));

        let cards = Scope::only("screenie-preview").on("DP-1");
        assert!(cards.covers("screenie-preview", Some("DP-1")));
        assert!(!cards.covers("screenie-preview", Some("DP-2")));
        assert!(!cards.covers("screenie-recording", Some("DP-1")));
        // Wherever the compositor put it, it might be there.
        assert!(cards.covers("screenie-preview", None));
    }
}
