//! GNOME's clipboard, for a process with no window in focus.
//!
//! Wayland lets a client set the clipboard only while one of its surfaces has keyboard
//! focus, and GNOME has no data-control protocol to get around that. What it has is the
//! clipboard of a remote desktop session (`org.gnome.Mutter.RemoteDesktop`), which
//! Mutter serves like any other: a session that's never started shows no indicator and
//! controls nothing, but can own the clipboard. Mutter's clipboard manager copies what
//! it's offered right away (images up to 200 MB), so a copy outlives the session.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use futures_lite::StreamExt;
use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue, Value};

#[zbus::proxy(
    interface = "org.gnome.Mutter.RemoteDesktop",
    default_service = "org.gnome.Mutter.RemoteDesktop",
    default_path = "/org/gnome/Mutter/RemoteDesktop",
    gen_blocking = false
)]
trait RemoteDesktop {
    fn create_session(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.gnome.Mutter.RemoteDesktop.Session",
    default_service = "org.gnome.Mutter.RemoteDesktop",
    gen_blocking = false
)]
trait Session {
    fn enable_clipboard(&self, options: HashMap<&str, Value<'_>>) -> zbus::Result<()>;
    fn set_selection(&self, options: HashMap<&str, Value<'_>>) -> zbus::Result<()>;
    fn selection_write(&self, serial: u32) -> zbus::Result<OwnedFd>;
    fn selection_write_done(&self, serial: u32, success: bool) -> zbus::Result<()>;
    #[zbus(signal)]
    fn selection_transfer(&self, mime_type: String, serial: u32) -> zbus::Result<()>;
    #[zbus(signal)]
    fn selection_owner_changed(&self, options: HashMap<String, OwnedValue>) -> zbus::Result<()>;
}

/// What's on offer: each MIME type's bytes.
pub type Offer = Vec<(String, Arc<[u8]>)>;

/// The clipboard of a (never started) Mutter remote desktop session. The session ends
/// with its D-Bus connection: `Stop` refuses one that never started.
pub struct MutterClipboard {
    session: SessionProxy<'static>,
    offer: Arc<Mutex<Offer>>,
}

impl MutterClipboard {
    /// Open a session and serve its clipboard from a thread of its own. Fails where
    /// there's no Mutter.
    pub fn connect() -> zbus::Result<MutterClipboard> {
        async_io::block_on(async {
            let connection = zbus::Connection::session().await?;
            let path = RemoteDesktopProxy::new(&connection)
                .await?
                .create_session()
                .await?;
            let session = SessionProxy::builder(&connection)
                .path(path)?
                .cache_properties(zbus::proxy::CacheProperties::No)
                .build()
                .await?;
            session.enable_clipboard(HashMap::new()).await?;
            let offer = Arc::new(Mutex::new(Offer::new()));
            let transfers = session.receive_selection_transfer().await?;
            let serving = (session.clone(), offer.clone());
            std::thread::Builder::new()
                .name("screenie-clipboard".into())
                .spawn(move || async_io::block_on(serve(serving.0, serving.1, transfers)))
                .map_err(|e| zbus::Error::Failure(e.to_string()))?;
            Ok(MutterClipboard { session, offer })
        })
    }

    /// Put `offer` on the clipboard.
    pub fn set(&self, offer: Offer) -> zbus::Result<()> {
        let types: Vec<String> = offer.iter().map(|(mime, _)| mime.clone()).collect();
        *self.offer.lock().unwrap() = offer;
        async_io::block_on(
            self.session
                .set_selection(HashMap::from([("mime-types", Value::from(types))])),
        )
    }
}

/// Answer requests for the clipboard's content, until the session's gone.
async fn serve(
    session: SessionProxy<'static>,
    offer: Arc<Mutex<Offer>>,
    mut transfers: SelectionTransferStream,
) {
    while let Some(transfer) = transfers.next().await {
        let Ok(args) = transfer.args() else { continue };
        let (mime, serial) = (args.mime_type, args.serial);
        let bytes = offer
            .lock()
            .unwrap()
            .iter()
            .find(|(m, _)| *m == mime)
            .map(|(_, b)| b.clone());
        let written = match bytes {
            Some(bytes) => match session.selection_write(serial).await {
                Ok(fd) => {
                    let mut file = std::fs::File::from(std::os::fd::OwnedFd::from(fd));
                    file.write_all(&bytes)
                        .inspect_err(|e| tracing::debug!("clipboard transfer of {mime}: {e}"))
                        .is_ok()
                }
                Err(e) => {
                    tracing::debug!("clipboard transfer of {mime}: {e}");
                    false
                }
            },
            None => false,
        };
        let _ = session.selection_write_done(serial, written).await;
    }
}
