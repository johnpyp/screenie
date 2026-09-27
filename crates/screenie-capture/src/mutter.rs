//! GNOME: Mutter's own screen casts (`org.gnome.Mutter.ScreenCast`), which gnome-shell's
//! recorder and the portal use too. Any client may start one, with no dialog; frames come
//! over PipeWire on the session's daemon.
//!
//! While a cast runs, gnome-shell shows it in the top bar. A cast marked as a recording
//! gets a dot among the status icons for as long as it runs, and nothing stops it there
//! (only gnome-shell's own recorder gets a stop button). Any other gets the
//! screen-sharing button, whose click stops it, and which stays up for five seconds
//! after. So a still is marked a recording, to be over quickly, and a recording isn't,
//! to be stopped from the top bar like any recorder going through the portal.

use std::collections::HashMap;
use std::time::Duration;

use futures_lite::StreamExt;
use screenie_core::Rect;
use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, Value};

use crate::{Error, Result};

const SERVICE: &str = "org.gnome.Mutter.ScreenCast";

#[zbus::proxy(
    interface = "org.gnome.Mutter.ScreenCast",
    default_service = "org.gnome.Mutter.ScreenCast",
    default_path = "/org/gnome/Mutter/ScreenCast",
    gen_blocking = false
)]
trait ScreenCast {
    fn create_session(&self, properties: HashMap<&str, Value<'_>>)
    -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.gnome.Mutter.ScreenCast.Session",
    default_service = "org.gnome.Mutter.ScreenCast",
    gen_blocking = false
)]
trait Session {
    fn start(&self) -> zbus::Result<()>;
    fn stop(&self) -> zbus::Result<()>;
    fn record_monitor(
        &self,
        connector: &str,
        properties: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<OwnedObjectPath>;
    fn record_area(
        &self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        properties: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<OwnedObjectPath>;
    fn record_window(&self, properties: HashMap<&str, Value<'_>>) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.gnome.Mutter.ScreenCast.Stream",
    default_service = "org.gnome.Mutter.ScreenCast",
    gen_blocking = false
)]
trait Stream {
    #[zbus(signal)]
    fn pipe_wire_stream_added(&self, node_id: u32) -> zbus::Result<()>;
}

/// Whether Mutter's screen casts are there to use.
pub(crate) fn available() -> bool {
    async_io::block_on(async {
        let connection = Connection::session().await?;
        zbus::fdo::DBusProxy::new(&connection)
            .await?
            .name_has_owner(SERVICE.try_into().expect("a valid bus name"))
            .await
            .map_err(zbus::Error::from)
    })
    .unwrap_or(false)
}

/// What to cast.
pub(crate) enum Source<'a> {
    Monitor(&'a str),
    /// A logical area of the desktop.
    Area(Rect),
    /// The window with keyboard focus, by itself.
    FocusedWindow,
}

/// A running cast. Mutter ends it when this is dropped, or when the connection closes.
pub(crate) struct Cast {
    session: SessionProxy<'static>,
    /// The PipeWire node of each source, in the order asked for.
    pub(crate) nodes: Vec<u32>,
}

impl Cast {
    /// Start casting `sources`, the pointer shown as `pointer` (embedded or hidden: the
    /// metadata mode puts it only on the monitor it's on), for `purpose`. Blocks until
    /// Mutter has made the PipeWire nodes.
    pub(crate) fn start(
        sources: &[Source<'_>],
        pointer: CastPointer,
        purpose: Purpose,
    ) -> Result<Cast> {
        async_io::block_on(async {
            let started = Self::start_async(sources, pointer, purpose);
            let timeout = async {
                async_io::Timer::after(Duration::from_secs(3)).await;
                Err(Error::Cast("Mutter didn't start the screen cast".into()))
            };
            futures_lite::future::or(started, timeout).await
        })
    }

    async fn start_async(
        sources: &[Source<'_>],
        pointer: CastPointer,
        purpose: Purpose,
    ) -> Result<Cast> {
        let connection = Connection::session().await.map_err(dbus)?;
        let path = ScreenCastProxy::new(&connection)
            .await
            .map_err(dbus)?
            .create_session(HashMap::new())
            .await
            .map_err(dbus)?;
        let session = SessionProxy::builder(&connection)
            .path(path)
            .map_err(dbus)?
            .build()
            .await
            .map_err(dbus)?;
        // Stopped when dropped, from here on.
        let mut cast = Cast {
            session,
            nodes: Vec::new(),
        };
        let mut added = Vec::new();
        for source in sources {
            let properties = || {
                HashMap::from([
                    ("cursor-mode", Value::from(pointer as u32)),
                    // See the module docs.
                    ("is-recording", Value::from(purpose == Purpose::Still)),
                ])
            };
            let path = match source {
                Source::Monitor(connector) => {
                    cast.session.record_monitor(connector, properties()).await
                }
                Source::Area(rect) => {
                    cast.session
                        .record_area(
                            rect.x.round() as i32,
                            rect.y.round() as i32,
                            rect.width.round() as i32,
                            rect.height.round() as i32,
                            properties(),
                        )
                        .await
                }
                // Without a `window-id`, the focused window.
                Source::FocusedWindow => cast.session.record_window(properties()).await,
            }
            .map_err(dbus)?;
            let stream = StreamProxy::builder(&connection)
                .path(path)
                .map_err(dbus)?
                .build()
                .await
                .map_err(dbus)?;
            // Subscribed before starting, so the node can't be missed.
            added.push(
                stream
                    .receive_pipe_wire_stream_added()
                    .await
                    .map_err(dbus)?,
            );
        }
        cast.session.start().await.map_err(dbus)?;
        for mut signals in added {
            let signal = signals
                .next()
                .await
                .ok_or_else(|| Error::Cast("Mutter ended the screen cast".into()))?;
            cast.nodes.push(signal.args().map_err(dbus)?.node_id);
        }
        Ok(cast)
    }
}

impl Drop for Cast {
    fn drop(&mut self) {
        let _ = async_io::block_on(self.session.stop());
    }
}

/// What a cast is for, which decides how gnome-shell shows it (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Purpose {
    Still,
    Stream,
}

/// Mutter's `cursor-mode`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum CastPointer {
    Hidden = 0,
    Embedded = 1,
}

impl From<bool> for CastPointer {
    /// Painted in, or not shown.
    fn from(shown: bool) -> Self {
        if shown {
            CastPointer::Embedded
        } else {
            CastPointer::Hidden
        }
    }
}

fn dbus(e: zbus::Error) -> Error {
    Error::Cast(format!("Mutter screen cast: {e}"))
}
