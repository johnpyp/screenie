//! GNOME's Mutter, over D-Bus.
//!
//! Mutter tells no client where windows are (gnome-shell's `Introspect` answers only its
//! own portal, and has no positions anyway). Screenie's GNOME Shell extension does
//! (`screenie_desktop::shell`); without it there are none to snap to. The layout comes
//! from `org.gnome.Mutter.DisplayConfig`, and "the screen the user is on" is the one
//! with the pointer, where the extension says, or else the primary one, where the top
//! bar is.

use std::collections::HashMap;

use screenie_core::WindowInfo;
use screenie_desktop::Desktop;
use screenie_desktop::shell::{self, Feature, Shell};
use zbus::zvariant::OwnedValue;

use crate::{Compositor, Error, Result};

/// A monitor: connector, vendor, product, serial.
type MonitorSpec = (String, String, String, String);
/// A logical monitor: x, y, scale, transform, primary, its monitors, properties.
type LogicalMonitor = (
    i32,
    i32,
    f64,
    u32,
    bool,
    Vec<MonitorSpec>,
    HashMap<String, OwnedValue>,
);
/// A mode: id, width, height, refresh, preferred scale, supported scales, properties.
type Mode = (
    String,
    i32,
    i32,
    f64,
    f64,
    Vec<f64>,
    HashMap<String, OwnedValue>,
);
type Monitor = (MonitorSpec, Vec<Mode>, HashMap<String, OwnedValue>);

#[zbus::proxy(
    interface = "org.gnome.Mutter.DisplayConfig",
    default_service = "org.gnome.Mutter.DisplayConfig",
    default_path = "/org/gnome/Mutter/DisplayConfig",
    gen_blocking = false
)]
trait DisplayConfig {
    #[allow(clippy::type_complexity)]
    fn get_current_state(
        &self,
    ) -> zbus::Result<(
        u32,
        Vec<Monitor>,
        Vec<LogicalMonitor>,
        HashMap<String, OwnedValue>,
    )>;
}

pub struct Gnome;

impl Gnome {
    /// On a GNOME session.
    pub fn from_env() -> Option<Self> {
        (Desktop::current() == Desktop::Gnome).then_some(Gnome)
    }
}

impl Compositor for Gnome {
    fn name(&self) -> &'static str {
        "gnome"
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        async_io::block_on(async {
            let connection = zbus::Connection::session().await.map_err(dbus)?;
            let Some(shell) = Shell::connect(&connection, Feature::Windows).await else {
                return Ok(Vec::new());
            };
            let windows = shell.windows().await.map_err(dbus)?;
            Ok(windows
                .into_iter()
                .map(|w| WindowInfo {
                    id: w.id.to_string(),
                    title: w.title,
                    app_id: w.app_id,
                    rect: w.rect,
                    focused: w.focused,
                    floating: true,
                    toplevel: None,
                })
                .collect())
        })
    }

    fn lists_windows(&self) -> bool {
        shell::offers(Feature::Windows)
    }

    /// The output with the pointer, where the extension says; else the primary one.
    fn focused_output(&self) -> Result<Option<String>> {
        async_io::block_on(async {
            let connection = zbus::Connection::session().await.map_err(dbus)?;
            let (_, _, logical, _) = DisplayConfigProxy::new(&connection)
                .await
                .map_err(dbus)?
                .get_current_state()
                .await
                .map_err(dbus)?;
            if let Some(shell) = Shell::connect(&connection, Feature::Pointer).await {
                let outputs: Vec<&str> = logical
                    .iter()
                    .flat_map(|l| l.5.iter().map(|spec| spec.0.as_str()))
                    .collect();
                match shell.pointer_output(&outputs).await {
                    Ok(Some(output)) => return Ok(Some(output)),
                    Ok(None) => {}
                    Err(e) => tracing::debug!("where the pointer is: {e}"),
                }
            }
            Ok(logical
                .into_iter()
                .find(|l| l.4)
                .and_then(|l| l.5.into_iter().next())
                .map(|spec| spec.0))
        })
    }
}

fn dbus(e: zbus::Error) -> Error {
    Error::Reply(format!("Mutter: {e}"))
}
