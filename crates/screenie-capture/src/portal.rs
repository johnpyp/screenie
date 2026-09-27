//! xdg-desktop-portal, for desktops with neither Wayland capture protocols nor a
//! compositor API screenie speaks: the Screenshot portal for stills, the ScreenCast
//! portal for streams.
//!
//! Screen casts ask as `dev.johnpyp.Screenie` (registering the connection as that app,
//! which needs the desktop entry installed), so the dialog names screenie and the choice
//! of screens, restored from a token without asking until they change, is screenie's
//! own. Screenshots ask as an unidentified app instead: GNOME lets an app with an id ask
//! for screenshot access only while it's the focused app, which a daemon never is, but
//! asks for unidentified ones any time (the one permission covers all of them).

use std::sync::Arc;

use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{
    CursorMode, Screencast, SelectSourcesOptions, SourceType, Stream as PortalStream,
};
use ashpd::desktop::screenshot::Screenshot;
use screenie_core::{APP_ID, Image, OutputCapture, OutputInfo, Point, Size};

use crate::{Error, Result};

/// Where the ScreenCast portal's restore token is kept between casts.
pub trait RestoreTokens: Send + Sync {
    fn load(&self) -> Option<String>;
    fn save(&self, token: Option<String>);
}

/// Keeps nothing: every cast asks.
pub(crate) struct Forget;

impl RestoreTokens for Forget {
    fn load(&self) -> Option<String> {
        None
    }

    fn save(&self, _: Option<String>) {}
}

/// Whether a portal is there to ask.
pub(crate) fn available() -> bool {
    async_io::block_on(async {
        let connection = zbus::Connection::session().await?;
        zbus::fdo::DBusProxy::new(&connection)
            .await?
            .name_has_owner(
                "org.freedesktop.portal.Desktop"
                    .try_into()
                    .expect("a bus name"),
            )
            .await
            .map_err(zbus::Error::from)
    })
    .unwrap_or(false)
}

/// A connection of its own, registered as screenie where the portal can (1.19.4+; the
/// registration has to come before any other call on it).
async fn identified() -> Result<zbus::Connection> {
    let connection = zbus::Connection::session().await.map_err(dbus)?;
    let app: ashpd::AppID = APP_ID.parse().expect("a valid app id");
    if let Err(e) = ashpd::register_host_app_with_connection(connection.clone(), app).await {
        tracing::debug!("the portal didn't take screenie's app id: {e}");
    }
    Ok(connection)
}

/// A still of every output, cut from the one picture the Screenshot portal takes.
pub(crate) fn screenshot(outputs: &[OutputInfo]) -> Result<Vec<OutputCapture>> {
    let path = async_io::block_on(async {
        // Not `identified` (see the module docs).
        let connection = zbus::Connection::session().await.map_err(dbus)?;
        let response = Screenshot::request()
            .interactive(false)
            .modal(false)
            .connection(Some(connection))
            .send()
            .await
            .map_err(portal_error)?
            .response()
            .map_err(portal_error)?;
        let uri = response.uri().as_str();
        url::Url::parse(uri)
            .ok()
            .and_then(|url| url.to_file_path().ok())
            .ok_or_else(|| Error::Cast(format!("the portal's screenshot isn't a file: {uri}")))
    })?;
    let image = Image::load_png(&path);
    // The portal saved it where it saves screenshots, which nobody asked for here.
    if let Err(e) = std::fs::remove_file(&path) {
        tracing::warn!(path = %path.display(), "can't remove the portal's screenshot: {e}");
    }
    let image = image.map_err(|e| Error::Cast(format!("the portal's screenshot: {e}")))?;
    Ok(split(&image, outputs))
}

/// Cut each output out of a picture of the whole layout. It's at one scale throughout (the
/// highest, on GNOME); each output is brought back to its own.
fn split(image: &Image, outputs: &[OutputInfo]) -> Vec<OutputCapture> {
    let Some(bounds) = outputs.iter().map(|o| o.logical).reduce(|a, b| a.union(&b)) else {
        return Vec::new();
    };
    let scale = f64::from(image.width()) / bounds.width;
    outputs
        .iter()
        .filter_map(|output| {
            let part = output
                .logical
                .to_pixels(bounds.origin(), scale)
                .intersection(&image.bounds())?;
            let mut picture = image.crop(part);
            let (width, height) = (
                (output.logical.width * output.scale).round() as u32,
                (output.logical.height * output.scale).round() as u32,
            );
            if width > 0 && height > 0 && (picture.width(), picture.height()) != (width, height) {
                picture = picture.resize(width, height);
            }
            Some(OutputCapture {
                output: output.clone(),
                image: picture,
            })
        })
        .collect()
}

/// A running portal screen cast of every screen the user shares. Closed when dropped.
pub(crate) struct PortalCast {
    session: ashpd::desktop::Session<Screencast>,
    pub(crate) remote: std::os::fd::OwnedFd,
    pub(crate) streams: Vec<PortalStream>,
}

impl Drop for PortalCast {
    fn drop(&mut self) {
        let _ = async_io::block_on(self.session.close());
    }
}

impl PortalCast {
    /// Cast the screens, with `pointer` painted in or not. Asks which ones the first
    /// time, and whenever the token in `tokens` no longer restores the choice.
    pub(crate) fn screens(pointer: bool, tokens: &Arc<dyn RestoreTokens>) -> Result<PortalCast> {
        async_io::block_on(async {
            let connection = identified().await?;
            let casts = Screencast::with_connection(connection)
                .await
                .map_err(portal_error)?;
            let session = casts
                .create_session(Default::default())
                .await
                .map_err(portal_error)?;
            let token = tokens.load();
            let options = SelectSourcesOptions::default()
                .set_sources(Some(SourceType::Monitor.into()))
                .set_multiple(true)
                .set_cursor_mode(if pointer {
                    CursorMode::Embedded
                } else {
                    CursorMode::Hidden
                })
                .set_persist_mode(PersistMode::ExplicitlyRevoked)
                .set_restore_token(token.as_deref());
            casts
                .select_sources(&session, options)
                .await
                .map_err(portal_error)?
                .response()
                .map_err(portal_error)?;
            let started = casts.start(&session, None, Default::default()).await;
            let streams = match started.and_then(|r| r.response()) {
                Ok(streams) => streams,
                Err(e) => {
                    // A token that restored nothing (the user cancelled instead) is spent.
                    tokens.save(None);
                    return Err(portal_error(e));
                }
            };
            // A restore token is good once: this cast's replaces it.
            tokens.save(streams.restore_token().map(String::from));
            let remote = casts
                .open_pipe_wire_remote(&session, Default::default())
                .await
                .map_err(portal_error)?;
            Ok(PortalCast {
                session,
                remote,
                streams: streams.streams().to_vec(),
            })
        })
    }

    /// The stream showing `output`: named by its connector where the portal says (KDE,
    /// wlroots), else placed where the output is (GNOME).
    pub(crate) fn stream_of(&self, output: &OutputInfo) -> Option<&PortalStream> {
        let by_name = self
            .streams
            .iter()
            .find(|s| s.mapping_id() == Some(output.name.as_str()));
        let by_place = || {
            self.streams.iter().find(|s| {
                let place = s.position().map(|(x, y)| Point::new(x.into(), y.into()));
                let size = s.size().map(|(w, h)| Size::new(w.into(), h.into()));
                place.is_some_and(|p| p == output.logical.origin())
                    && size.is_none_or(|s| s == output.logical.size())
            })
        };
        let only = || (self.streams.len() == 1).then(|| &self.streams[0]);
        by_name.or_else(by_place).or_else(only)
    }
}

/// The logical size of what a portal stream shows, for cropping a region of it.
pub(crate) fn stream_size(stream: &PortalStream, output: &OutputInfo) -> Size {
    stream
        .size()
        .map(|(w, h)| Size::new(w.into(), h.into()))
        .unwrap_or(output.logical.size())
}

fn portal_error(e: ashpd::Error) -> Error {
    match e {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => {
            Error::Cast("the screen capture was declined".into())
        }
        e => Error::Cast(format!("xdg-desktop-portal: {e}")),
    }
}

fn dbus(e: zbus::Error) -> Error {
    Error::Cast(format!("xdg-desktop-portal: {e}"))
}
