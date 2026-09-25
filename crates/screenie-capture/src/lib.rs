//! "Freeze the desktop now": one entry point that picks the right capture backend and
//! returns a [`Snapshot`] of every output plus the window layout.
//!
//! Native Wayland protocols are preferred (fast, silent, per-output, exact pixels). The
//! xdg-desktop-portal backend covers compositors without them (KDE, GNOME).

use std::sync::Arc;
use std::time::{Instant, SystemTime};

use screenie_compositor::Compositor;
use screenie_config::CaptureBackend;
use screenie_core::{FrameSource, OutputInfo, Rect, Snapshot};
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
        Self { compositor, support }
    }

    pub fn compositor(&self) -> &Arc<dyn Compositor> {
        &self.compositor
    }

    pub fn support(&self) -> &Support {
        &self.support
    }

    /// Human-readable name of the backend `backend` resolves to.
    pub fn backend_name(&self, backend: CaptureBackend) -> &'static str {
        match self.resolve(backend) {
            Some(b) => b.name(),
            None => "xdg-desktop-portal",
        }
    }

    fn resolve(&self, backend: CaptureBackend) -> Option<Backend> {
        match backend {
            CaptureBackend::Ext => Some(Backend::ExtImageCopyCapture),
            CaptureBackend::Wlr => Some(Backend::WlrScreencopy),
            CaptureBackend::Portal => None,
            CaptureBackend::Auto if self.support.ext_image_copy_capture => Some(Backend::ExtImageCopyCapture),
            CaptureBackend::Auto if self.support.wlr_screencopy => Some(Backend::WlrScreencopy),
            CaptureBackend::Auto => None,
        }
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

        let outputs = match self.resolve(opts.backend) {
            Some(backend) => Capturer::connect_with(Some(backend))?.capture_outputs(None, opts.cursor)?,
            None => {
                return Err(Error::Unsupported(
                    "this compositor has no screen capture protocol screenie can use (portal support is not built yet)".into(),
                ));
            }
        };

        let windows = windows
            .and_then(|h| h.join().ok())
            .and_then(|r| r.map_err(|e| tracing::debug!("window list unavailable: {e}")).ok())
            .unwrap_or_default();
        tracing::debug!(elapsed = ?started.elapsed(), outputs = outputs.len(), windows = windows.len(), "snapshot");
        Ok(Snapshot { outputs, windows, taken_at })
    }

    /// Start a live stream of `output`, or of `region` (logical, relative to the output's
    /// top-left) within it, for recording.
    pub fn stream(
        &self,
        backend: CaptureBackend,
        output: &str,
        region: Option<Rect>,
        cursor: bool,
    ) -> Result<Box<dyn FrameSource>> {
        match self.resolve(backend) {
            Some(backend) => Ok(Box::new(Capturer::connect_with(Some(backend))?.into_stream(output, region, cursor)?)),
            None => Err(Error::Unsupported(
                "this compositor has no screen capture protocol screenie can use (portal support is not built yet)".into(),
            )),
        }
    }

    /// Output layout without capturing pixels.
    pub fn outputs(&self) -> Result<Vec<OutputInfo>> {
        Ok(Capturer::connect()?.outputs())
    }
}

impl Default for CaptureContext {
    fn default() -> Self {
        Self::new()
    }
}
