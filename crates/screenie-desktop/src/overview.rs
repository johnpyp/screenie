//! GNOME's overview (Activities), which covers the desktop, and takes a window that opens
//! meanwhile in as a thumbnail: a selector opened over it would be one. It's up after
//! logging in, and on its way out (animating) after launching an app from it. So a
//! capture leaves it first, and waits until it's gone.

use std::time::Duration;

use futures_lite::{FutureExt, StreamExt};

/// How long the overview may take to go: its animation is a quarter of a second.
const TIMEOUT: Duration = Duration::from_secs(1);

#[zbus::proxy(
    interface = "org.gnome.Shell",
    default_service = "org.gnome.Shell",
    default_path = "/org/gnome/Shell",
    gen_blocking = false
)]
trait Shell {
    /// True until the overview has finished hiding.
    #[zbus(property)]
    fn overview_active(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_overview_active(&self, active: bool) -> zbus::Result<()>;
}

/// Leave the overview if it's up, returning once it's gone (or has taken too long).
/// Whether it was up.
pub fn leave() -> zbus::Result<bool> {
    async_io::block_on(async {
        let connection = zbus::Connection::session().await?;
        let shell = ShellProxy::builder(&connection)
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await?;
        if !shell.overview_active().await? {
            return Ok(false);
        }
        let properties = zbus::fdo::PropertiesProxy::builder(&connection)
            .destination("org.gnome.Shell")?
            .path("/org/gnome/Shell")?
            .build()
            .await?;
        let mut changes = properties.receive_properties_changed().await?;
        shell.set_overview_active(false).await?;
        let gone = async {
            while let Some(change) = changes.next().await {
                let Ok(args) = change.args() else { continue };
                let hidden = args
                    .changed_properties()
                    .get("OverviewActive")
                    .and_then(|v| bool::try_from(v).ok())
                    == Some(false);
                if hidden {
                    return;
                }
            }
        };
        let timeout = async {
            async_io::Timer::after(TIMEOUT).await;
            tracing::warn!("the overview is taking long to go");
        };
        gone.or(timeout).await;
        Ok(true)
    })
}
