//! Continuous frame sources, the input side of recording.

use std::time::Duration;

use crate::Image;

pub type SourceError = Box<dyn std::error::Error + Send + Sync>;

/// A live view of (part of) the screen that yields a new [`Image`] whenever the content
/// changes. Implemented by the Wayland screencopy stream, and by the portal's PipeWire
/// stream where native capture is unavailable.
pub trait FrameSource: Send {
    /// Wait up to `timeout` for the next frame. `Ok(None)` means nothing changed in time.
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<Image>, SourceError>;
}
