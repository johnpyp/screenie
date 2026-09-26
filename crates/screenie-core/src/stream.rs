//! Continuous frame sources, the input side of recording.

use std::time::Duration;

use crate::Image;

pub type SourceError = Box<dyn std::error::Error + Send + Sync>;

/// What a [`FrameSource`] had within the wait.
#[derive(Debug, Clone)]
pub enum Next {
    Frame(Image),
    /// Nothing changed in time.
    Unchanged,
    /// The source is gone for good, as when a recorded window closes.
    Ended,
}

/// A live view of (part of) the screen, or of one window, that yields a new [`Image`]
/// whenever the content changes. Frames can change size along the way (a window being
/// resized). Implemented by the Wayland screencopy stream, and by the portal's PipeWire
/// stream where native capture is unavailable.
pub trait FrameSource: Send {
    /// Wait up to `timeout` for the next frame.
    fn next_frame(&mut self, timeout: Duration) -> Result<Next, SourceError>;
}
