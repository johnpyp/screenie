//! Screen casts over PipeWire, as [`FrameSource`]s.
//!
//! Desktops without Wayland capture protocols cast the screen to a PipeWire node instead:
//! Mutter's ScreenCast D-Bus API, KWin's `zkde_screencast`, and the ScreenCast portal all
//! hand out a node id (and the portal a restricted remote to reach it through).
//! [`Stream::connect`] consumes one: it negotiates a format screenie reads (8-bit RGB in
//! any byte order), and yields each new picture as a [`Frame`], cropped to what the
//! compositor marks as content, with the pointer from the cast's metadata where it sends
//! it that way.
//!
//! Each stream runs its own PipeWire loop on a thread of its own; frames reach the
//! consumer through a slot that only ever holds the newest one, so a slow consumer
//! skips frames rather than holding up the compositor.

mod format;
mod thread;

use std::os::fd::OwnedFd;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use screenie_core::{Frame, FrameSource, Image, Next, Rect, Size, SourceError};

/// Where a cast's node lives.
#[derive(Debug)]
pub enum Remote {
    /// The session's PipeWire daemon (Mutter's and KWin's own casts).
    Session,
    /// A remote the ScreenCast portal opened, which shows only the cast's nodes.
    Fd(OwnedFd),
}

/// What part of a cast's pictures to keep.
#[derive(Debug, Clone, Copy, Default)]
pub enum Keep {
    /// All of it.
    #[default]
    All,
    /// A region, for a region of a screen when the cast is of all of it.
    Region(Crop),
    /// The opaque part: a window without the shadow it draws around itself. Measured
    /// from the first picture, and again whenever the size changes.
    Opaque,
}

/// A region of a cast.
#[derive(Debug, Clone, Copy)]
pub struct Crop {
    /// Logical, relative to the cast's top-left.
    pub region: Rect,
    /// The cast's logical size. Its scale is measured from each frame.
    pub of: Size,
}

/// How the cast shows the pointer, as the caller asked for it when starting the cast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pointer {
    /// Not at all, or painted into the frames.
    InFrames,
    /// As metadata beside the frames: each [`Frame`] carries it, for the consumer to draw.
    Metadata,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("pipewire: {0}")]
    PipeWire(#[from] pipewire::Error),
    #[error("the screen cast ended: {0}")]
    Ended(String),
    #[error("the screen cast sent no picture in time")]
    Timeout,
    #[error("can't start the pipewire thread: {0}")]
    Thread(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What the PipeWire thread shares with the consumer.
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    /// The newest frame the consumer hasn't taken.
    frame: Option<Frame>,
    /// The newest picture, for [`FrameSource::snapshot`].
    latest: Option<Image>,
    /// Why the stream is over, once it is.
    ended: Option<String>,
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.state.lock().unwrap());
        self.changed.notify_all();
    }
}

/// A live screen cast. Dropping it disconnects.
pub struct Stream {
    shared: Arc<Shared>,
    control: thread::Control,
    pointer: Pointer,
}

impl Stream {
    /// Connect to the cast's `node` on `remote`, keeping `keep` of each picture. Frames
    /// start coming once PipeWire has negotiated the format; [`Stream::first_picture`]
    /// waits for one.
    pub fn connect(remote: Remote, node: u32, pointer: Pointer, keep: Keep) -> Result<Stream> {
        let shared = Arc::new(Shared::default());
        let control = thread::spawn(remote, node, pointer, keep, shared.clone())?;
        Ok(Stream {
            shared,
            control,
            pointer,
        })
    }

    /// Wait for the first picture, for a still.
    pub fn first_picture(&mut self, timeout: Duration) -> Result<Image> {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.state.lock().unwrap();
        loop {
            if let Some(image) = state.latest.clone() {
                return Ok(image);
            }
            if let Some(why) = &state.ended {
                return Err(Error::Ended(why.clone()));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout);
            }
            state = self.shared.changed.wait_timeout(state, left).unwrap().0;
        }
    }
}

impl FrameSource for Stream {
    fn next_frame(&mut self, timeout: Duration) -> Result<Next, SourceError> {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.state.lock().unwrap();
        loop {
            if let Some(frame) = state.frame.take() {
                return Ok(Next::Frame(frame));
            }
            if state.ended.is_some() {
                return Ok(Next::Ended);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(Next::Unchanged);
            }
            state = self.shared.changed.wait_timeout(state, left).unwrap().0;
        }
    }

    fn pace(&mut self, fps: u32) {
        self.control.send(thread::Command::Pace(fps));
    }

    fn draws_pointer(&self) -> bool {
        self.pointer == Pointer::Metadata
    }

    fn set_paused(&mut self, paused: bool) -> Result<(), SourceError> {
        self.control.send(thread::Command::Active(!paused));
        Ok(())
    }

    fn snapshot(&mut self) -> Option<Image> {
        self.shared.state.lock().unwrap().latest.clone()
    }
}
