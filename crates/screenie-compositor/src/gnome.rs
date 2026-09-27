//! GNOME's Mutter, over D-Bus.
//!
//! Mutter tells no client where windows are (gnome-shell's `Introspect` answers only its
//! own portal, and has no positions anyway), so there are none to snap to. The layout
//! comes from `org.gnome.Mutter.DisplayConfig`, and "the screen the user is on" is the
//! primary one, where the top bar is: nothing reports focus or the pointer.

use std::collections::HashMap;

use screenie_core::WindowInfo;
use screenie_desktop::Desktop;
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
        Ok(Vec::new())
    }

    fn focused_output(&self) -> Result<Option<String>> {
        async_io::block_on(async {
            let connection = zbus::Connection::session().await.map_err(dbus)?;
            let (_, _, logical, _) = DisplayConfigProxy::new(&connection)
                .await
                .map_err(dbus)?
                .get_current_state()
                .await
                .map_err(dbus)?;
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
