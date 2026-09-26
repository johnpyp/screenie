//! "Freeze the desktop now": one entry point that picks the right capture backend and
//! returns a [`Snapshot`] of every output plus the window layout.
//!
//! Native Wayland protocols are preferred (fast, silent, per-output, exact pixels). The
//! xdg-desktop-portal backend covers compositors without them (KDE, GNOME).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use screenie_compositor::Compositor;
use screenie_config::CaptureBackend;
use screenie_core::{
    DmabufFormat, Frame, FrameSource, GpuOffer, Image, Next, OutputInfo, Rect, Snapshot, SourceError, WindowInfo,
};
use screenie_wayland::{Backend, Capturer, Support};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Wayland(#[from] screenie_wayland::Error),
    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy)]
pub struct SnapshotOptions {
    pub cursor: bool,
    pub backend: CaptureBackend,
    /// Ask the compositor for window geometry (for window snapping).
    pub windows: bool,
}

/// Long-lived capture context: remembers the compositor IPC and which protocol works.
pub struct CaptureContext {
    compositor: Arc<dyn Compositor>,
    support: Support,
    /// The protocol that last worked in `auto` mode; tried first next time.
    working: Mutex<Option<Backend>>,
}

impl CaptureContext {
    pub fn new() -> Self {
        let support = Support::probe().unwrap_or_else(|e| {
            tracing::warn!("probing wayland globals failed: {e}");
            Support::default()
        });
        let compositor: Arc<dyn Compositor> = Arc::from(screenie_compositor::detect());
        tracing::info!(
            compositor = compositor.name(),
            ext = support.ext_image_copy_capture,
            wlr = support.wlr_screencopy,
            layer_shell = support.layer_shell,
            "capture context"
        );
        Self { compositor, support, working: Mutex::new(None) }
    }

    pub fn compositor(&self) -> &Arc<dyn Compositor> {
        &self.compositor
    }

    pub fn support(&self) -> &Support {
        &self.support
    }

    /// Human-readable name of the backend `backend` resolves to.
    pub fn backend_name(&self, backend: CaptureBackend) -> &'static str {
        match self.candidates(backend).first() {
            Some(b) => b.name(),
            None => "xdg-desktop-portal",
        }
    }

    /// Native protocols to try for `backend`, best first. In `auto` mode that's every
    /// protocol the compositor offers, starting with the one that last worked: an
    /// advertised protocol can still be unusable (e.g. only offering a pixel format we
    /// can't read), and the other one may be fine.
    fn candidates(&self, backend: CaptureBackend) -> Vec<Backend> {
        match backend {
            CaptureBackend::Ext => vec![Backend::ExtImageCopyCapture],
            CaptureBackend::Wlr => vec![Backend::WlrScreencopy],
            CaptureBackend::Portal => Vec::new(),
            CaptureBackend::Auto => {
                let mut all = Vec::new();
                if self.support.ext_image_copy_capture {
                    all.push(Backend::ExtImageCopyCapture);
                }
                if self.support.wlr_screencopy {
                    all.push(Backend::WlrScreencopy);
                }
                if let Some(working) = *self.working.lock().unwrap() {
                    all.sort_by_key(|b| *b != working);
                }
                all
            }
        }
    }

    /// Run `capture` with each candidate protocol until one succeeds.
    fn with_backends<T>(
        &self,
        backend: CaptureBackend,
        mut capture: impl FnMut(Backend) -> Result<T, screenie_wayland::Error>,
    ) -> Result<T> {
        let candidates = self.candidates(backend);
        let mut last = None;
        for (i, b) in candidates.iter().enumerate() {
            match capture(*b) {
                Ok(value) => {
                    if i > 0 {
                        tracing::info!(backend = b.name(), "capturing with the fallback protocol from now on");
                        *self.working.lock().unwrap() = Some(*b);
                    }
                    return Ok(value);
                }
                Err(e) => {
                    if i + 1 < candidates.len() {
                        tracing::warn!(backend = b.name(), "capture failed ({e}); trying the next protocol");
                    }
                    last = Some(e);
                }
            }
        }
        Err(last.map(Error::from).unwrap_or_else(|| {
            Error::Unsupported(
                "this compositor has no screen capture protocol screenie can use (portal support is not built yet)"
                    .into(),
            )
        }))
    }

    /// Capture every output. Blocking; call off the UI thread.
    pub fn snapshot(&self, opts: SnapshotOptions) -> Result<Snapshot> {
        let started = Instant::now();
        let taken_at = SystemTime::now();
        // Window geometry is fetched concurrently with the pixel capture.
        let windows = opts.windows.then(|| {
            let compositor = self.compositor.clone();
            std::thread::spawn(move || compositor.windows())
        });

        let outputs =
            self.with_backends(opts.backend, |b| Capturer::connect_with(Some(b))?.capture_outputs(None, opts.cursor))?;

        let windows = windows
            .and_then(|h| h.join().ok())
            .and_then(|r| r.map_err(|e| tracing::debug!("window list unavailable: {e}")).ok())
            .unwrap_or_default();
        tracing::debug!(elapsed = ?started.elapsed(), outputs = outputs.len(), windows = windows.len(), "snapshot");
        Ok(Snapshot { outputs, windows, taken_at })
    }

    /// Start a live stream of `output`, or of `region` (logical, relative to the output's
    /// top-left) within it, for recording. Blocks until the first frame arrives, so a
    /// protocol that can't actually deliver is caught (and another tried) up front.
    pub fn stream(
        &self,
        backend: CaptureBackend,
        output: &str,
        region: Option<Rect>,
        cursor: bool,
    ) -> Result<Box<dyn FrameSource>> {
        self.with_backends(backend, |b| prime(Capturer::connect_with(Some(b))?.into_stream(output, region, cursor)?))
    }

    /// Whether [`CaptureContext::stream_window`] can work here with `backend`.
    pub fn can_stream_window(&self, backend: CaptureBackend) -> bool {
        self.support.window_capture && matches!(backend, CaptureBackend::Auto | CaptureBackend::Ext)
    }

    /// Start a live stream of one window by itself (only `ext-image-copy-capture` can).
    /// Blocks until the first frame arrives.
    pub fn stream_window(&self, window: &WindowInfo, cursor: bool) -> Result<Box<dyn FrameSource>> {
        let capturer = Capturer::connect_with(Some(Backend::ExtImageCopyCapture))?;
        Ok(prime(capturer.into_window_stream(window, cursor)?)?)
    }

    /// Output layout without capturing pixels.
    pub fn outputs(&self) -> Result<Vec<OutputInfo>> {
        Ok(Capturer::connect()?.outputs())
    }
}

/// Wait for `stream`'s first frame, proving the protocol can actually deliver.
fn prime(mut stream: screenie_wayland::FrameStream) -> Result<Box<dyn FrameSource>, screenie_wayland::Error> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(frame) = stream.next_frame(Duration::from_millis(250))? {
            return Ok(Box::new(Primed { first: Some(frame), stream }));
        }
        if Instant::now() >= deadline {
            return Err(screenie_wayland::Error::Timeout);
        }
    }
}

/// A stream whose first frame was already pulled (to prove the protocol works).
struct Primed {
    first: Option<Frame>,
    stream: screenie_wayland::FrameStream,
}

impl FrameSource for Primed {
    fn next_frame(&mut self, timeout: Duration) -> Result<Next, SourceError> {
        match self.first.take() {
            Some(frame) => Ok(Next::Frame(frame)),
            None => FrameSource::next_frame(&mut self.stream, timeout),
        }
    }

    fn pace(&mut self, fps: u32) {
        self.stream.set_max_rate(fps);
    }

    fn gpu_offer(&self) -> Option<GpuOffer> {
        self.stream.gpu_offer()
    }

    fn use_gpu(&mut self, format: Option<DmabufFormat>) -> Result<(), SourceError> {
        Ok(self.stream.use_gpu(format)?)
    }

    fn snapshot(&mut self) -> Option<Image> {
        FrameSource::snapshot(&mut self.stream)
    }
}

impl Default for CaptureContext {
    fn default() -> Self {
        Self::new()
    }
}
