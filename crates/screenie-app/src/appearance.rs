//! The interface scale: `ui_scale` from the config or, with `auto`, the desktop's text
//! scaling, read and then followed through the settings portal.

use ashpd::desktop::settings::Settings;
use futures_lite::StreamExt as _;
use gpui::{App, AsyncApp, Global};
use screenie_config::UiScale;

use crate::daemon::Daemon;

/// GTK's text scaling, as GNOME's "Large Text" and `gsettings` set it. Provided by the
/// GNOME and GTK portal backends.
const NAMESPACE: &str = "org.gnome.desktop.interface";
const KEY: &str = "text-scaling-factor";

/// The desktop's text scaling, once the portal has said.
struct DesktopScale(f64);

impl Global for DesktopScale {}

/// Apply the scale now, and follow the desktop's from here on.
pub(crate) fn start(cx: &mut App) {
    apply(cx);
    cx.spawn(async move |cx| follow_desktop(cx).await).detach();
}

/// Apply the scale the config asks for. Called again whenever the config changes.
pub(crate) fn apply(cx: &mut App) {
    let scale = match Daemon::get(cx).config.ui_scale {
        UiScale::Fixed(scale) => scale,
        UiScale::Auto => cx.try_global::<DesktopScale>().map_or(1.0, |d| d.0),
    };
    screenie_ui_kit::set_ui_scale(scale as f32, cx);
}

async fn follow_desktop(cx: &mut AsyncApp) {
    let settings = match Settings::new().await {
        Ok(settings) => settings,
        Err(e) => return tracing::debug!("no settings portal ({e}); ui_scale auto means 1"),
    };
    let desktop = |scale: f64, cx: &mut AsyncApp| {
        if scale.is_finite() && scale > 0.0 {
            tracing::debug!(scale, "desktop text scaling");
            cx.update(|cx| {
                cx.set_global(DesktopScale(scale));
                apply(cx);
            });
        }
    };
    match settings.read::<f64>(NAMESPACE, KEY).await {
        Ok(scale) => desktop(scale, cx),
        Err(e) => tracing::debug!("the settings portal has no {NAMESPACE} {KEY} ({e}); ui_scale auto means 1"),
    }
    let mut changes = match settings.receive_setting_changed_with_args::<f64>(NAMESPACE, KEY).await {
        Ok(changes) => changes,
        Err(e) => return tracing::debug!("can't follow {KEY}: {e}"),
    };
    while let Some(change) = changes.next().await {
        if let Ok(scale) = change {
            desktop(scale, cx);
        }
    }
}
