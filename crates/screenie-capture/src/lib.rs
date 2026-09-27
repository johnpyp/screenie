//! "Freeze the desktop now": one entry point that picks the right way to capture and
//! returns a [`Snapshot`] of every output plus the window layout, or a live stream of an
//! output, a region or a window for recording.
//!
//! Native Wayland protocols come first (fast, silent, per-output, exact pixels). KDE
//! Plasma and GNOME have none: KWin's own screenshots and screen casts ([`kwin`],
//! `screenie_wayland::kde_cast`) and Mutter's screen casts ([`mutter`]) stand in, and
//! xdg-desktop-portal ([`portal`]) covers everything else.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use screenie_compositor::Compositor;
use screenie_config::CaptureBackend;
use screenie_core::{
    DmabufFormat, FrameSource, GpuDevice, GpuOffer, Image, Next, OutputCapture, OutputInfo, Rect,
    Snapshot, SourceError, WindowInfo,
};
use screenie_pipewire::{Crop, Pointer, Remote};
use screenie_wayland::{Backend, Capturer, KdePointer, KdeSource, Support, WindowPointer};

mod kwin;
mod mutter;
mod portal;

pub use portal::RestoreTokens;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Wayland(#[from] screenie_wayland::Error),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Cast(String),
}

impl From<screenie_pipewire::Error> for Error {
    fn from(e: screenie_pipewire::Error) -> Self {
        Error::Cast(e.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy)]
pub struct SnapshotOptions {
    pub cursor: bool,
    pub backend: CaptureBackend,
    /// Ask the compositor for window geometry (for window snapping).
    pub windows: bool,
}

/// A way of capturing the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `ext-image-copy-capture-v1`.
    Ext,
    /// `wlr-screencopy-unstable-v1`.
    Wlr,
    /// KWin's screenshots and screen casts.
    Kwin,
    /// Mutter's screen casts.
    Mutter,
    /// xdg-desktop-portal.
    Portal,
}

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::Ext => Backend::ExtImageCopyCapture.name(),
            Method::Wlr => Backend::WlrScreencopy.name(),
            Method::Kwin => "kwin",
            Method::Mutter => "mutter",
            Method::Portal => "xdg-desktop-portal",
        }
    }

    fn wayland(self) -> Option<Backend> {
        match self {
            Method::Ext => Some(Backend::ExtImageCopyCapture),
            Method::Wlr => Some(Backend::WlrScreencopy),
            _ => None,
        }
    }
}

/// What the session has to capture with, found once at startup.
#[derive(Debug, Clone, Default)]
pub struct Offers {
    pub wayland: Support,
    /// KWin's screenshots (they may still refuse screenie; see [`kwin`]).
    pub kwin_screenshots: bool,
    /// KWin's screen casts, which it offers only to clients it trusts.
    pub kwin_casts: bool,
    pub mutter_casts: bool,
    pub portal: bool,
}

impl Offers {
    pub fn probe() -> Offers {
        let wayland = Support::probe().unwrap_or_else(|e| {
            tracing::warn!("probing wayland globals failed: {e}");
            Support::default()
        });
        let native = wayland.native_capture();
        Offers {
            // Only asked of the desktop where they'd be used.
            kwin_screenshots: !native && kwin::available(),
            kwin_casts: !native && screenie_wayland::kde_screencast_available(),
            mutter_casts: !native && mutter::available(),
            portal: portal::available(),
            wayland,
        }
    }
}

/// A still, or a live stream (of a region of an output, or not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Still,
    Stream { region: bool },
}

/// Long-lived capture context: remembers the compositor IPC and which method works.
pub struct CaptureContext {
    compositor: Arc<dyn Compositor>,
    offers: Offers,
    /// The method that last worked in `auto` mode; tried first next time.
    working: Mutex<Option<Method>>,
    tokens: Arc<dyn RestoreTokens>,
}

impl CaptureContext {
    pub fn new() -> Self {
        let offers = Offers::probe();
        let compositor: Arc<dyn Compositor> = Arc::from(screenie_compositor::detect());
        tracing::info!(
            compositor = compositor.name(),
            ext = offers.wayland.ext_image_copy_capture,
            wlr = offers.wayland.wlr_screencopy,
            kwin_screenshots = offers.kwin_screenshots,
            kwin_casts = offers.kwin_casts,
            mutter = offers.mutter_casts,
            portal = offers.portal,
            layer_shell = offers.wayland.layer_shell,
            "capture context"
        );
        Self {
            compositor,
            offers,
            working: Mutex::new(None),
            tokens: Arc::new(portal::Forget),
        }
    }

    /// Keep the ScreenCast portal's restore token in `tokens`, so the screens shared once
    /// are shared again without asking.
    pub fn remembering(mut self, tokens: Arc<dyn RestoreTokens>) -> Self {
        self.tokens = tokens;
        self
    }

    pub fn compositor(&self) -> &Arc<dyn Compositor> {
        &self.compositor
    }

    pub fn support(&self) -> &Support {
        &self.offers.wayland
    }

    pub fn offers(&self) -> &Offers {
        &self.offers
    }

    /// Human-readable name of the method `backend` resolves to for screenshots.
    pub fn backend_name(&self, backend: CaptureBackend) -> &'static str {
        match self.candidates(backend, Kind::Still).first() {
            Some(m) => m.name(),
            None => "none",
        }
    }

    /// Methods to try for `backend` and `kind`, best first. In `auto` mode that's every
    /// one the session offers, starting with the one that last worked: an advertised
    /// protocol can still be unusable (e.g. only offering a pixel format we can't read),
    /// and another may be fine.
    ///
    /// A region of an output goes to wlr-screencopy first, which copies just the region.
    /// ext-image-copy-capture copies the whole output, and its GPU frames then need
    /// cropping, which not every encoder can do on the GPU.
    ///
    /// Stills on GNOME come from the Screenshot portal rather than Mutter's casts: a cast
    /// shows its indicator in the top bar, and so in the picture.
    fn candidates(&self, backend: CaptureBackend, kind: Kind) -> Vec<Method> {
        let offers = &self.offers;
        match backend {
            CaptureBackend::Ext => vec![Method::Ext],
            CaptureBackend::Wlr => vec![Method::Wlr],
            CaptureBackend::Kwin => vec![Method::Kwin],
            CaptureBackend::Mutter => vec![Method::Mutter],
            CaptureBackend::Portal => vec![Method::Portal],
            CaptureBackend::Auto => auto_order(offers, kind, *self.working.lock().unwrap()),
        }
    }

    /// Run `capture` with each candidate method until one succeeds.
    fn with_methods<T>(
        &self,
        backend: CaptureBackend,
        kind: Kind,
        mut capture: impl FnMut(Method) -> Result<T>,
    ) -> Result<T> {
        let candidates = self.candidates(backend, kind);
        let mut last = None;
        for (i, m) in candidates.iter().enumerate() {
            match capture(*m) {
                Ok(value) => {
                    if i > 0 {
                        tracing::info!(method = m.name(), "capturing this way from now on");
                        *self.working.lock().unwrap() = Some(*m);
                    }
                    return Ok(value);
                }
                Err(e) => {
                    if i + 1 < candidates.len() {
                        tracing::warn!(
                            method = m.name(),
                            "capture failed ({e}); trying the next way"
                        );
                    }
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| {
            Error::Unsupported("this desktop offers no way to capture the screen".into())
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
            self.with_methods(opts.backend, Kind::Still, |m| self.stills(m, opts.cursor))?;

        let windows = windows
            .and_then(|h| h.join().ok())
            .and_then(|r| {
                r.map_err(|e| tracing::debug!("window list unavailable: {e}"))
                    .ok()
            })
            .unwrap_or_default();
        tracing::debug!(elapsed = ?started.elapsed(), outputs = outputs.len(), windows = windows.len(), "snapshot");
        Ok(Snapshot {
            outputs,
            windows,
            taken_at,
        })
    }

    /// A still of every output, taken with `method`.
    fn stills(&self, method: Method, cursor: bool) -> Result<Vec<OutputCapture>> {
        if let Some(backend) = method.wayland() {
            return Ok(Capturer::connect_with(Some(backend))?.capture_outputs(None, cursor)?);
        }
        let outputs = self.outputs()?;
        match method {
            Method::Kwin => kwin::capture(&outputs, cursor),
            Method::Portal => portal::screenshot(&outputs),
            Method::Mutter => {
                let sources: Vec<_> = outputs
                    .iter()
                    .map(|o| mutter::Source::Monitor(&o.name))
                    .collect();
                let cast = mutter::Cast::start(&sources, mutter::CastPointer::from(cursor))?;
                let mut streams = cast
                    .nodes
                    .iter()
                    .map(|&node| {
                        screenie_pipewire::Stream::connect(
                            Remote::Session,
                            node,
                            Pointer::InFrames,
                            None,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                outputs
                    .into_iter()
                    .zip(&mut streams)
                    .map(|(output, stream)| {
                        let image = stream.first_picture(Duration::from_secs(2))?;
                        Ok(OutputCapture { output, image })
                    })
                    .collect()
            }
            Method::Ext | Method::Wlr => unreachable!("handled above"),
        }
    }

    /// Start a live stream of `output`, or of `region` (logical, relative to the output's
    /// top-left) within it, for recording. Blocks until the first frame arrives, so a
    /// method that can't actually deliver is caught (and another tried) up front.
    pub fn stream(
        &self,
        backend: CaptureBackend,
        output: &str,
        region: Option<Rect>,
        cursor: bool,
    ) -> Result<Box<dyn FrameSource>> {
        let kind = Kind::Stream {
            region: region.is_some(),
        };
        self.with_methods(backend, kind, |m| {
            let source = self.open_stream(m, output, region, cursor)?;
            prime(source)
        })
    }

    fn open_stream(
        &self,
        method: Method,
        name: &str,
        region: Option<Rect>,
        cursor: bool,
    ) -> Result<Box<dyn FrameSource>> {
        if let Some(backend) = method.wayland() {
            return Ok(Box::new(
                Capturer::connect_with(Some(backend))?.into_stream(name, region, cursor)?,
            ));
        }
        let output = self
            .outputs()?
            .into_iter()
            .find(|o| o.name == name)
            .ok_or_else(|| Error::Unsupported(format!("no output named {name}")))?;
        // Where the cast takes a region itself, it's in desktop coordinates.
        let on_desktop = region.map(|r| r.translate(output.logical.x, output.logical.y));
        let pipewire = |remote, node, crop| {
            screenie_pipewire::Stream::connect(remote, node, Pointer::InFrames, crop)
        };
        match method {
            Method::Kwin => {
                let source = match on_desktop {
                    Some(rect) => KdeSource::Region(rect),
                    None => KdeSource::Output(output.name.clone()),
                };
                let pointer = if cursor {
                    KdePointer::Embedded
                } else {
                    KdePointer::Hidden
                };
                let cast = screenie_wayland::kde_cast(&source, pointer)?;
                let stream = pipewire(Remote::Session, cast.node, None)?;
                Ok(Keeping::boxed(stream, cast))
            }
            Method::Mutter => {
                let source = match on_desktop {
                    Some(rect) => mutter::Source::Area(rect),
                    None => mutter::Source::Monitor(&output.name),
                };
                let cast = mutter::Cast::start(&[source], mutter::CastPointer::from(cursor))?;
                let stream = pipewire(Remote::Session, cast.nodes[0], None)?;
                Ok(Keeping::boxed(stream, cast))
            }
            Method::Portal => {
                let cast = portal::PortalCast::screens(cursor, &self.tokens)?;
                let shown = cast.stream_of(&output).ok_or_else(|| {
                    Error::Cast(format!("{} wasn't among the screens shared", output.name))
                })?;
                let crop = region.map(|region| Crop {
                    region,
                    of: portal::stream_size(shown, &output),
                });
                let remote = cast
                    .remote
                    .try_clone()
                    .map_err(|e| Error::Cast(format!("the portal's PipeWire remote: {e}")))?;
                let stream = pipewire(Remote::Fd(remote), shown.pipe_wire_node_id(), crop)?;
                Ok(Keeping::boxed(stream, cast))
            }
            Method::Ext | Method::Wlr => unreachable!("handled above"),
        }
    }

    /// Whether [`CaptureContext::stream_window`] can work here with `backend`.
    pub fn can_stream_window(&self, backend: CaptureBackend) -> bool {
        let ext = self.offers.wayland.window_capture
            && matches!(backend, CaptureBackend::Auto | CaptureBackend::Ext);
        let kwin = self.offers.kwin_casts
            && self.compositor.name() == "kwin"
            && matches!(backend, CaptureBackend::Auto | CaptureBackend::Kwin);
        ext || kwin
    }

    /// Start a live stream of one window by itself. Blocks until the first frame arrives.
    ///
    /// With `cursor`, a compositor that doesn't paint the pointer into a window's frames
    /// has it drawn over them instead, from where it is on an output: the window's place
    /// on the desktop is kept current from compositor IPC for as long as the stream lasts.
    pub fn stream_window(&self, window: &WindowInfo, cursor: bool) -> Result<Box<dyn FrameSource>> {
        if !self.offers.wayland.window_capture && self.offers.kwin_casts {
            // KWin knows windows by the UUID its IPC reports as their id.
            let pointer = if cursor {
                KdePointer::Embedded
            } else {
                KdePointer::Hidden
            };
            let cast = screenie_wayland::kde_cast(&KdeSource::Window(window.id.clone()), pointer)?;
            let stream = screenie_pipewire::Stream::connect(
                Remote::Session,
                cast.node,
                Pointer::InFrames,
                None,
            )?;
            return prime(Keeping::boxed(stream, cast));
        }
        let capturer = Capturer::connect_with(Some(Backend::ExtImageCopyCapture))?;
        let pointer = match cursor {
            false => WindowPointer::Hidden,
            true if self.compositor.paints_pointer_into_windows() => WindowPointer::Painted,
            true => WindowPointer::Drawn,
        };
        let stream = capturer.into_window_stream(window, pointer)?;
        if let Some(placement) = stream.window_placement() {
            follow_window(
                self.compositor.clone(),
                window.id.clone(),
                placement.tracker(),
            );
        }
        prime(Box::new(stream))
    }

    /// Connector name of the output the user is on: from the compositor's IPC, or else
    /// from where the compositor puts a surface that doesn't pick one (layer-shell). Blocking.
    pub fn focused_output(&self) -> Option<String> {
        match self.compositor.focused_output() {
            Ok(Some(name)) => return Some(name),
            Ok(None) => {}
            Err(e) => tracing::debug!("the compositor didn't say which output is focused: {e}"),
        }
        if !self.offers.wayland.layer_shell {
            return None;
        }
        screenie_wayland::focused_output()
            .inspect_err(|e| tracing::debug!("probing the focused output failed: {e}"))
            .ok()
            .flatten()
    }

    /// Output layout without capturing pixels.
    pub fn outputs(&self) -> Result<Vec<OutputInfo>> {
        Ok(screenie_wayland::outputs()?)
    }

    /// The GPU the compositor renders with, where it says: the one a recording's
    /// frames will be on.
    pub fn gpu(&self) -> Option<GpuDevice> {
        screenie_wayland::gpu()
    }
}

/// The methods `offers` has for `kind`, in the order `auto` tries them, and above all the
/// one that `working` last.
fn auto_order(offers: &Offers, kind: Kind, working: Option<Method>) -> Vec<Method> {
    let region = kind == (Kind::Stream { region: true });
    let mut all = wayland_order(&offers.wayland, region);
    let (kwin, mutter) = match kind {
        Kind::Still => (offers.kwin_screenshots, false),
        Kind::Stream { .. } => (offers.kwin_casts, offers.mutter_casts),
    };
    if kwin {
        all.push(Method::Kwin);
    }
    if mutter {
        all.push(Method::Mutter);
    }
    if offers.portal {
        all.push(Method::Portal);
    }
    // Better a cast's indicator in a still than no still.
    if kind == Kind::Still && offers.mutter_casts {
        all.push(Method::Mutter);
    }
    if let Some(working) = working {
        all.sort_by_key(|m| *m != working);
    }
    all
}

/// The Wayland protocols `support` offers, in the order `auto` tries them: ext first, or
/// wlr for a `region`.
fn wayland_order(support: &Support, region: bool) -> Vec<Method> {
    let mut all = Vec::new();
    if support.ext_image_copy_capture {
        all.push(Method::Ext);
    }
    if support.wlr_screencopy {
        all.push(Method::Wlr);
    }
    if region {
        all.sort_by_key(|m| *m != Method::Wlr);
    }
    all
}

/// Keep a recorded window's place current from compositor IPC, until its stream is gone.
/// A window that isn't shown (on another workspace) has none: the pointer isn't over it.
fn follow_window(compositor: Arc<dyn Compositor>, id: String, tracker: screenie_wayland::Tracker) {
    const EVERY: Duration = Duration::from_millis(250);
    let spawned = std::thread::Builder::new()
        .name("screenie-window".into())
        .spawn(move || {
            while let Some(placement) = tracker.placement() {
                match compositor.windows() {
                    Ok(windows) => {
                        placement.set(windows.iter().find(|w| w.id == id).map(|w| w.rect));
                    }
                    Err(e) => tracing::debug!("where the recorded window is: {e}"),
                }
                drop(placement);
                std::thread::sleep(EVERY);
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("can't follow the recorded window: {e}");
    }
}

/// Wait for `stream`'s first frame, proving the method can actually deliver.
fn prime(mut stream: Box<dyn FrameSource>) -> Result<Box<dyn FrameSource>> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match stream.next_frame(Duration::from_millis(250)) {
            Ok(Next::Frame(frame)) => {
                return Ok(Box::new(Primed {
                    first: Some(frame),
                    stream,
                }));
            }
            Ok(Next::Unchanged) => {}
            Ok(Next::Ended) => {
                return Err(Error::Cast(
                    "the stream ended before its first frame".into(),
                ));
            }
            Err(e) => return Err(Error::Cast(e.to_string())),
        }
        if Instant::now() >= deadline {
            return Err(Error::Wayland(screenie_wayland::Error::Timeout));
        }
    }
}

/// A stream whose first frame was already pulled (to prove the method works).
struct Primed {
    first: Option<screenie_core::Frame>,
    stream: Box<dyn FrameSource>,
}

impl FrameSource for Primed {
    fn next_frame(&mut self, timeout: Duration) -> Result<Next, SourceError> {
        match self.first.take() {
            Some(frame) => Ok(Next::Frame(frame)),
            None => self.stream.next_frame(timeout),
        }
    }

    fn pace(&mut self, fps: u32) {
        self.stream.pace(fps);
    }

    fn draws_pointer(&self) -> bool {
        self.stream.draws_pointer()
    }

    fn gpu(&self) -> Option<GpuDevice> {
        self.stream.gpu()
    }

    fn gpu_offer(&self) -> Option<GpuOffer> {
        self.stream.gpu_offer()
    }

    fn use_gpu(&mut self, format: Option<DmabufFormat>) -> Result<(), SourceError> {
        self.stream.use_gpu(format)
    }

    fn set_paused(&mut self, paused: bool) -> Result<(), SourceError> {
        self.stream.set_paused(paused)
    }

    fn snapshot(&mut self) -> Option<Image> {
        self.stream.snapshot()
    }
}

/// A stream, and what has to live as long as it (the cast it's a view of). The stream
/// goes first.
struct Keeping<K> {
    stream: screenie_pipewire::Stream,
    _kept: K,
}

impl<K: Send + 'static> Keeping<K> {
    fn boxed(stream: screenie_pipewire::Stream, kept: K) -> Box<dyn FrameSource> {
        Box::new(Keeping {
            stream,
            _kept: kept,
        })
    }
}

impl<K: Send> FrameSource for Keeping<K> {
    fn next_frame(&mut self, timeout: Duration) -> Result<Next, SourceError> {
        self.stream.next_frame(timeout)
    }

    fn pace(&mut self, fps: u32) {
        self.stream.pace(fps);
    }

    fn draws_pointer(&self) -> bool {
        self.stream.draws_pointer()
    }

    fn set_paused(&mut self, paused: bool) -> Result<(), SourceError> {
        self.stream.set_paused(paused)
    }

    fn snapshot(&mut self) -> Option<Image> {
        self.stream.snapshot()
    }
}

impl Default for CaptureContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_go_to_the_protocol_that_copies_just_them() {
        let both = Support {
            ext_image_copy_capture: true,
            wlr_screencopy: true,
            ..Default::default()
        };
        assert_eq!(wayland_order(&both, false), [Method::Ext, Method::Wlr]);
        assert_eq!(wayland_order(&both, true), [Method::Wlr, Method::Ext]);
        let ext_only = Support {
            ext_image_copy_capture: true,
            ..Default::default()
        };
        assert_eq!(wayland_order(&ext_only, true), [Method::Ext]);
    }

    #[test]
    fn what_worked_comes_first() {
        let offers = Offers {
            wayland: Support {
                ext_image_copy_capture: true,
                wlr_screencopy: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let region = Kind::Stream { region: true };
        assert_eq!(
            auto_order(&offers, region, Some(Method::Ext)),
            [Method::Ext, Method::Wlr]
        );
        assert_eq!(
            auto_order(&offers, Kind::Still, Some(Method::Wlr)),
            [Method::Wlr, Method::Ext]
        );
    }

    #[test]
    fn desktops_without_protocols_use_their_own_ways() {
        let kde = Offers {
            kwin_screenshots: true,
            kwin_casts: true,
            portal: true,
            ..Default::default()
        };
        assert_eq!(
            auto_order(&kde, Kind::Still, None),
            [Method::Kwin, Method::Portal]
        );
        let stream = Kind::Stream { region: false };
        assert_eq!(
            auto_order(&kde, stream, None),
            [Method::Kwin, Method::Portal]
        );
        // GNOME's stills come from the portal: a cast's indicator would be in them.
        let gnome = Offers {
            mutter_casts: true,
            portal: true,
            ..Default::default()
        };
        assert_eq!(
            auto_order(&gnome, Kind::Still, None),
            [Method::Portal, Method::Mutter]
        );
        assert_eq!(
            auto_order(&gnome, stream, None),
            [Method::Mutter, Method::Portal]
        );
    }
}
