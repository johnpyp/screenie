//! Continuous frame sources, the input side of recording.

use std::time::{Duration, Instant};

use crate::{Dmabuf, DmabufFormat, GpuDevice, GpuOffer, Image};

pub type SourceError = Box<dyn std::error::Error + Send + Sync>;

/// What a [`FrameSource`] had within the wait.
#[derive(Debug, Clone)]
pub enum Next {
    Frame(Frame),
    /// Nothing changed in time.
    Unchanged,
    /// The source is gone for good, as when a recorded window closes.
    Ended,
}

/// One frame of a stream.
#[derive(Debug, Clone)]
pub struct Frame {
    pub pixels: Pixels,
    /// When the compositor presented this content (CLOCK_MONOTONIC), if it said.
    pub presented: Option<Duration>,
}

/// Where a frame's pixels are.
#[derive(Debug, Clone)]
pub enum Pixels {
    /// In memory, upright.
    Cpu(Image),
    /// On the GPU that rendered them.
    Gpu(Dmabuf),
}

impl Frame {
    pub fn cpu(image: Image) -> Self {
        Self {
            pixels: Pixels::Cpu(image),
            presented: None,
        }
    }

    /// The size of what's recorded of it.
    pub fn size(&self) -> (u32, u32) {
        match &self.pixels {
            Pixels::Cpu(image) => (image.width(), image.height()),
            Pixels::Gpu(buf) => buf
                .crop
                .map_or((buf.width, buf.height), |c| (c.width, c.height)),
        }
    }

    pub fn image(&self) -> Option<&Image> {
        match &self.pixels {
            Pixels::Cpu(image) => Some(image),
            Pixels::Gpu(_) => None,
        }
    }
}

/// A live view of (part of) the screen, or of one window, that yields a new [`Frame`]
/// whenever the content changes. Frames can change size along the way (a window being
/// resized). Implemented by the Wayland screencopy stream, and by the portal's PipeWire
/// stream where native capture is unavailable.
///
/// Frames come as CPU images, unless the consumer switches to GPU buffers it can take
/// ([`FrameSource::gpu_offer`], [`FrameSource::use_gpu`]).
pub trait FrameSource: Send {
    /// Wait up to `timeout` for the next frame.
    fn next_frame(&mut self, timeout: Duration) -> Result<Next, SourceError>;

    /// Produce at most `fps` frames a second. Sources that can stop the compositor
    /// making more than that (rather than dropping the rest) do.
    fn pace(&mut self, _fps: u32) {}

    /// The GPU frames are rendered on, where known (once a frame has come): the
    /// encoder there is the best one, even for frames that come as CPU images.
    fn gpu(&self) -> Option<GpuDevice> {
        self.gpu_offer().map(|offer| offer.device)
    }

    /// The GPU buffers frames could come in instead, known once a frame has come.
    fn gpu_offer(&self) -> Option<GpuOffer> {
        None
    }

    /// From the next frame on, deliver frames in GPU buffers of `format` (its fourcc,
    /// and the modifiers acceptable of those offered), or as CPU images (`None`).
    fn use_gpu(&mut self, format: Option<DmabufFormat>) -> Result<(), SourceError> {
        match format {
            None => Ok(()),
            Some(_) => Err("this source only delivers CPU images".into()),
        }
    }

    /// Stop producing frames while a recording is paused, or start again. Sources
    /// that can stop the compositor copying frames do; others keep delivering them,
    /// and the consumer drops them.
    fn set_paused(&mut self, _paused: bool) -> Result<(), SourceError> {
        Ok(())
    }

    /// What it shows right now, as an image, even if nothing has changed since the
    /// last frame (whose pixels may be on a GPU).
    fn snapshot(&mut self) -> Option<Image> {
        None
    }
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
        let interval = fps.map_or(Duration::ZERO, |fps| {
            Duration::from_secs_f64(1.0 / fps.max(1) as f64)
        });
        Self {
            interval,
            next: None,
        }
    }

    /// The time between ticks (zero when unpaced).
    pub fn interval(&self) -> Duration {
        self.interval
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
