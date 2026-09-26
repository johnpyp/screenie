//! Continuous frame sources, the input side of recording.

use std::time::{Duration, Instant};

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

    /// Produce at most `fps` frames a second. Sources that can stop the compositor
    /// making more than that (rather than dropping the rest) do.
    fn pace(&mut self, _fps: u32) {}
}

/// Ticks on a fixed clock of `interval`: a tick that comes late doesn't push the ones
/// after it back, so the average rate holds even when each one is a little late. After
/// falling a whole interval behind, the clock restarts from now rather than catching up.
#[derive(Debug, Clone)]
pub struct Pacer {
    interval: Duration,
    next: Option<Instant>,
}

impl Pacer {
    /// `None`: every tick is due at once.
    pub fn new(fps: Option<u32>) -> Self {
        let interval = fps.map_or(Duration::ZERO, |fps| Duration::from_secs_f64(1.0 / fps.max(1) as f64));
        Self { interval, next: None }
    }

    /// When the next tick is due, if not yet at `now`.
    pub fn wait_until(&self, now: Instant) -> Option<Instant> {
        self.next.filter(|next| *next > now)
    }

    /// Take the tick due at `now`.
    pub fn tick(&mut self, now: Instant) {
        let due = self.next.unwrap_or(now);
        let base = if due + self.interval >= now { due } else { now };
        self.next = Some(base + self.interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_ticks_keep_the_clock() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut pacer = Pacer::new(Some(100));
        assert_eq!(pacer.wait_until(t0), None);
        pacer.tick(t0);
        assert_eq!(pacer.wait_until(ms(4)), Some(ms(10)));
        pacer.tick(ms(13)); // late
        assert_eq!(pacer.wait_until(ms(13)), Some(ms(20)));
        pacer.tick(ms(45)); // far behind: start over
        assert_eq!(pacer.wait_until(ms(45)), Some(ms(55)));
        let mut unpaced = Pacer::new(None);
        unpaced.tick(t0);
        assert_eq!(unpaced.wait_until(t0), None);
    }
}
