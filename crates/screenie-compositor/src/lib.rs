//! Window geometry and focus from the compositor's IPC.
//!
//! Wayland deliberately doesn't let clients see other clients' windows, so knowing where
//! windows are (for click-to-capture-window) needs compositor-specific IPC. This crate
//! wraps the ones worth supporting behind [`Compositor`]. Everything here is optional:
//! [`detect`] falls back to [`Generic`], which knows nothing, and screenie keeps working
//! without window snapping.

mod gnome;
mod hyprland;
mod kwin;
mod niri;
mod sway;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use screenie_core::WindowInfo;

pub use gnome::Gnome;
pub use hyprland::Hyprland;
pub use kwin::Kwin;
pub use niri::Niri;
pub use sway::Sway;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("compositor ipc i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("unexpected compositor reply: {0}")]
    Reply(String),
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Reply(e.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub trait Compositor: Send + Sync {
    /// Short identifier, e.g. `"sway"`.
    fn name(&self) -> &'static str;

    /// Windows visible right now, topmost first, in global logical coordinates.
    fn windows(&self) -> Result<Vec<WindowInfo>>;

    /// Whether [`Compositor::windows`] can list any at all.
    fn lists_windows(&self) -> bool {
        true
    }

    /// Connector name of the output with keyboard focus.
    fn focused_output(&self) -> Result<Option<String>>;

    /// Whether a capture of one window has the pointer painted in when asked for it
    /// (ext-image-copy-capture's `paint_cursors`). Where it doesn't, screenie draws the
    /// pointer over the window itself, from where [`Compositor::windows`] puts it.
    fn paints_pointer_into_windows(&self) -> bool {
        true
    }
}

/// Pick the IPC for the running compositor from its environment variables.
pub fn detect() -> Box<dyn Compositor> {
    if let Some(c) = Hyprland::from_env() {
        return Box::new(c);
    }
    if let Some(c) = Niri::from_env() {
        return Box::new(c);
    }
    if let Some(c) = Sway::from_env() {
        return Box::new(c);
    }
    if let Some(c) = Kwin::from_env() {
        return Box::new(c);
    }
    if let Some(c) = Gnome::from_env() {
        return Box::new(c);
    }
    Box::new(Generic)
}

/// A compositor we have no IPC for.
pub struct Generic;

impl Compositor for Generic {
    fn name(&self) -> &'static str {
        "generic"
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        Ok(Vec::new())
    }

    fn lists_windows(&self) -> bool {
        false
    }

    fn focused_output(&self) -> Result<Option<String>> {
        Ok(None)
    }
}

const IPC_TIMEOUT: Duration = Duration::from_secs(1);

fn connect(path: &Path) -> Result<UnixStream> {
    let stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
    stream.set_write_timeout(Some(IPC_TIMEOUT))?;
    Ok(stream)
}

/// Send `request` and read until the peer closes the connection.
fn request_to_eof(path: &Path, request: &[u8]) -> Result<Vec<u8>> {
    let mut stream = connect(path)?;
    stream.write_all(request)?;
    stream.shutdown(std::net::Shutdown::Write).ok();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;
    Ok(buf)
}
