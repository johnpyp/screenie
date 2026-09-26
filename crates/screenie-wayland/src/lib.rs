//! Screen capture on Wayland through the compositor's own protocols.
//!
//! Two protocols are supported, picked automatically:
//!
//! * `ext-image-copy-capture-v1` (the standard; wlroots 0.19+, niri, COSMIC, recent
//!   Hyprland, …)
//! * `wlr-screencopy-unstable-v1` (older wlroots compositors and most tiling WMs)
//!
//! A [`Capturer`] captures still frames of any set of outputs concurrently, and turns into
//! a [`FrameStream`] for continuous capture (recording) of an output, or of one window
//! where the compositor offers `ext-foreign-toplevel-image-capture-source-v1` (the window's
//! own pixels, wherever it is and whatever covers it). Frames are always returned upright
//! with transforms and y-inversion undone. Compositors without either protocol (GNOME,
//! KDE) are served by `screenie-portal` instead.

mod shm;
mod state;

use std::time::{Duration, Instant};

use screenie_core::{Image, Next, OutputCapture, OutputInfo, Pacer, Rect, WindowInfo};
use wayland_client::globals::{GlobalList, registry_queue_init};
use wayland_client::protocol::wl_output;
use wayland_client::{Connection, EventQueue, Proxy, QueueHandle};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1::Options;

pub use shm::transform_image;
use state::{Capture, Constraints, OutputState, Phase, Protocol, State, Target, ToplevelInfo};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot connect to the Wayland compositor: {0}")]
    Connect(#[from] wayland_client::ConnectError),
    #[error("wayland protocol error: {0}")]
    Protocol(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Unsupported(String),
    #[error("no output named {0:?}")]
    NoSuchOutput(String),
    #[error("{0}")]
    NoSuchWindow(String),
    #[error("capture failed: {0}")]
    Capture(String),
    #[error("timed out waiting for the compositor")]
    Timeout,
    #[error("the capture source went away")]
    Stopped,
}

impl From<wayland_client::DispatchError> for Error {
    fn from(e: wayland_client::DispatchError) -> Self {
        Error::Protocol(e.to_string())
    }
}

impl From<wayland_client::globals::GlobalError> for Error {
    fn from(e: wayland_client::globals::GlobalError) -> Self {
        Error::Protocol(e.to_string())
    }
}

impl From<wayland_client::backend::WaylandError> for Error {
    fn from(e: wayland_client::backend::WaylandError) -> Self {
        Error::Protocol(e.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Which capture protocol a [`Capturer`] speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    ExtImageCopyCapture,
    WlrScreencopy,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::ExtImageCopyCapture => "ext-image-copy-capture-v1",
            Backend::WlrScreencopy => "wlr-screencopy-unstable-v1",
        }
    }
}

/// Protocols available on the running compositor, without capturing anything.
#[derive(Debug, Clone, Default)]
pub struct Support {
    pub ext_image_copy_capture: bool,
    /// Capturing a window by itself (see [`Capturer::into_window_stream`]).
    pub window_capture: bool,
    pub wlr_screencopy: bool,
    pub layer_shell: bool,
    pub data_control: bool,
}

impl Support {
    pub fn probe() -> Result<Support> {
        let conn = Connection::connect_to_env()?;
        let (globals, _queue) = registry_queue_init::<State>(&conn)?;
        let has = |name: &str| globals.contents().with_list(|l| l.iter().any(|g| g.interface == name));
        Ok(Support {
            ext_image_copy_capture: has("ext_image_copy_capture_manager_v1")
                && has("ext_output_image_capture_source_manager_v1"),
            window_capture: has("ext_image_copy_capture_manager_v1")
                && has("ext_foreign_toplevel_image_capture_source_manager_v1")
                && has("ext_foreign_toplevel_list_v1"),
            wlr_screencopy: has("zwlr_screencopy_manager_v1"),
            layer_shell: has("zwlr_layer_shell_v1"),
            data_control: has("ext_data_control_manager_v1") || has("zwlr_data_control_manager_v1"),
        })
    }

    pub fn native_capture(&self) -> bool {
        self.ext_image_copy_capture || self.wlr_screencopy
    }
}

/// A connection to the compositor ready to capture outputs.
pub struct Capturer {
    conn: Connection,
    globals: GlobalList,
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
    backend: Backend,
}

impl Capturer {
    /// Connect and pick the best available protocol.
    pub fn connect() -> Result<Self> {
        Self::connect_with(None)
    }

    /// Connect, optionally forcing a protocol.
    pub fn connect_with(preferred: Option<Backend>) -> Result<Self> {
        let conn = Connection::connect_to_env()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let mut state = State {
            shm: globals.bind(&qh, 1..=1, ()).ok(),
            xdg_output_manager: globals.bind(&qh, 2..=3, ()).ok(),
            ext_copy: globals.bind(&qh, 1..=1, ()).ok(),
            ext_output_sources: globals.bind(&qh, 1..=1, ()).ok(),
            ext_toplevel_sources: globals.bind(&qh, 1..=1, ()).ok(),
            wlr_screencopy: globals.bind(&qh, 1..=3, ()).ok(),
            ..Default::default()
        };
        if state.shm.is_none() {
            return Err(Error::Unsupported("compositor has no wl_shm".into()));
        }

        let ext = state.ext_copy.is_some() && state.ext_output_sources.is_some();
        let wlr = state.wlr_screencopy.is_some();
        let backend = match preferred {
            Some(Backend::ExtImageCopyCapture) if ext => Backend::ExtImageCopyCapture,
            Some(Backend::WlrScreencopy) if wlr => Backend::WlrScreencopy,
            Some(b) => return Err(Error::Unsupported(format!("compositor does not support {}", b.name()))),
            None if ext => Backend::ExtImageCopyCapture,
            None if wlr => Backend::WlrScreencopy,
            None => {
                return Err(Error::Unsupported(
                    "compositor supports neither ext-image-copy-capture nor wlr-screencopy".into(),
                ));
            }
        };

        bind_outputs(&globals, &qh, &mut state);
        // First roundtrip delivers wl_output/xdg_output events; the second catches
        // xdg_output info requested in response to the first.
        queue.roundtrip(&mut state)?;
        queue.roundtrip(&mut state)?;
        tracing::debug!(backend = backend.name(), outputs = state.outputs.len(), "wayland capturer ready");

        Ok(Self { conn, globals, queue, qh, state, backend })
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// All outputs with a known mode, in compositor order.
    pub fn outputs(&self) -> Vec<OutputInfo> {
        self.state.outputs.iter().enumerate().filter_map(|(i, o)| o.info(i)).collect()
    }

    fn output_index(&self, name: &str) -> Result<usize> {
        self.state
            .outputs
            .iter()
            .enumerate()
            .find(|(i, o)| o.info(*i).is_some_and(|info| info.name == name))
            .map(|(i, _)| i)
            .ok_or_else(|| Error::NoSuchOutput(name.to_string()))
    }

    /// Capture every output (or the named ones) as one consistent set of frames. Frames
    /// are requested concurrently so they are as close to simultaneous as the compositor
    /// allows.
    pub fn capture_outputs(&mut self, names: Option<&[&str]>, cursor: bool) -> Result<Vec<OutputCapture>> {
        let indices: Vec<usize> = match names {
            Some(names) => names.iter().map(|n| self.output_index(n)).collect::<Result<_>>()?,
            None => (0..self.state.outputs.len()).filter(|&i| self.state.outputs[i].info(i).is_some()).collect(),
        };
        let slots: Vec<usize> = indices.iter().map(|&i| self.new_capture(Target::Output(i), cursor, None)).collect();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            for &slot in &slots {
                self.drive(slot)?;
            }
            let pending = slots.iter().any(|&s| {
                matches!(self.state.captures[s].phase, Phase::Negotiating | Phase::Copying)
            });
            if !pending {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(Error::Timeout);
            }
            self.dispatch_timeout(deadline - now)?;
        }

        let mut captures = Vec::with_capacity(slots.len());
        for (&slot, &output) in slots.iter().zip(&indices) {
            let image = self.take_image(slot)?;
            let info = self.state.outputs[output].info(output).expect("filtered above");
            captures.push(OutputCapture { output: info, image });
        }
        for slot in slots {
            self.release(slot);
        }
        Ok(captures)
    }

    /// Start continuous capture of one output, optionally restricted to a logical region
    /// relative to the output's top-left corner.
    pub fn into_stream(mut self, output: &str, region: Option<Rect>, cursor: bool) -> Result<FrameStream> {
        let index = self.output_index(output)?;
        let info = self.state.outputs[index].info(index).expect("output has a mode");
        let bounds = Rect::new(0.0, 0.0, info.logical.width, info.logical.height);
        let region = match region {
            Some(r) => Some(r.intersection(&bounds).ok_or_else(|| Error::Capture("region is outside the output".into()))?),
            None => None,
        };
        // wlr-screencopy can copy just the region; ext always copies the whole output and
        // we crop afterwards.
        let (native_region, crop) = match self.backend {
            Backend::WlrScreencopy => (region, None),
            Backend::ExtImageCopyCapture => (None, region),
        };
        let slot = self.new_capture(Target::Output(index), cursor, native_region);
        let crop = crop.map(|region| Crop { region, logical_width: info.logical.width });
        Ok(FrameStream::new(self, slot, crop))
    }

    /// Start continuous capture of one window: its own pixels at its own size, following
    /// it across workspaces and outputs, with nothing that covers it. Frames change size
    /// as the window does, and the stream ends when the window closes.
    ///
    /// The window is found by its toplevel identifier where the compositor's IPC reported
    /// one, otherwise by app id and title, which must then be unique.
    pub fn into_window_stream(mut self, window: &WindowInfo, cursor: bool) -> Result<FrameStream> {
        if self.backend != Backend::ExtImageCopyCapture || self.state.ext_toplevel_sources.is_none() {
            return Err(Error::Unsupported("this compositor can't capture a window by itself".into()));
        }
        let list = self
            .globals
            .bind(&self.qh, 1..=1, ())
            .map_err(|_| Error::Unsupported("this compositor doesn't list its windows".into()))?;
        self.state.toplevel_list = Some(list);
        // The list sends every toplevel, then each one's details.
        self.queue.roundtrip(&mut self.state)?;
        self.queue.roundtrip(&mut self.state)?;
        let index = find_toplevel(self.state.toplevels.iter().map(|t| &t.info), window)?;
        let slot = self.new_capture(Target::Toplevel(index), cursor, None);
        Ok(FrameStream::new(self, slot, None))
    }

    fn new_capture(&mut self, target: Target, cursor: bool, region: Option<Rect>) -> usize {
        let idx = self.state.captures.len();
        let protocol = match self.backend {
            Backend::ExtImageCopyCapture => {
                let source = match target {
                    Target::Output(output) => {
                        let wl_output = self.state.outputs[output].wl_output.as_ref().expect("bound output");
                        let sources = self.state.ext_output_sources.as_ref().expect("checked at connect");
                        sources.create_source(wl_output, &self.qh, ())
                    }
                    Target::Toplevel(toplevel) => {
                        let handle = &self.state.toplevels[toplevel].handle;
                        let sources = self.state.ext_toplevel_sources.as_ref().expect("checked by the caller");
                        sources.create_source(handle, &self.qh, ())
                    }
                };
                let options = if cursor { Options::PaintCursors } else { Options::empty() };
                let session = self.state.ext_copy.as_ref().expect("checked at connect").create_session(
                    &source,
                    options,
                    &self.qh,
                    idx,
                );
                Protocol::Ext { source, session, frame: None }
            }
            Backend::WlrScreencopy => Protocol::Wlr { frame: None, region },
        };
        // ext sends each frame's transform; wlr frames come in the output's.
        let transform = match target {
            Target::Output(output) => self.state.outputs[output].transform,
            Target::Toplevel(_) => Default::default(),
        };
        self.state.captures.push(Capture {
            target,
            protocol,
            cursor,
            constraints: Constraints::default(),
            buffer: None,
            phase: Phase::Negotiating,
            transform,
            y_invert: false,
            presented: None,
            wait_for_damage: false,
            frames_captured: 0,
        });
        idx
    }

    /// Advance a capture's state machine as far as the known state allows.
    fn drive(&mut self, slot: usize) -> Result<()> {
        let shm = self.state.shm.clone().expect("checked at connect");
        let outputs = &self.state.outputs;
        let cap = &mut self.state.captures[slot];
        if cap.phase != Phase::Negotiating {
            return Ok(());
        }

        // wlr needs a frame object before it tells us the constraints.
        if let Protocol::Wlr { frame, region } = &mut cap.protocol
            && frame.is_none()
        {
            let manager = self.state.wlr_screencopy.as_ref().expect("checked at connect");
            let Target::Output(output) = cap.target else { unreachable!("wlr only captures outputs") };
            let wl_output = outputs[output].wl_output.as_ref().expect("bound output");
            let overlay = cap.cursor as i32;
            cap.constraints = Constraints::default();
            *frame = Some(match region {
                Some(r) => {
                    let r = r.round();
                    manager.capture_output_region(
                        overlay,
                        wl_output,
                        r.x as i32,
                        r.y as i32,
                        r.width as i32,
                        r.height as i32,
                        &self.qh,
                        slot,
                    )
                }
                None => manager.capture_output(overlay, wl_output, &self.qh, slot),
            });
            return Ok(());
        }

        if !cap.constraints.done {
            return Ok(());
        }
        let (width, height) = cap
            .constraints
            .size
            .ok_or_else(|| Error::Capture("compositor sent no buffer size".into()))?;
        let format = shm::choose_format(&cap.constraints.formats).ok_or_else(|| {
            Error::Unsupported(format!("no supported shm format among {:?}", cap.constraints.formats))
        })?;
        let stride = cap
            .constraints
            .strides
            .iter()
            .find(|(f, _)| *f == format)
            .map(|(_, s)| *s)
            .unwrap_or(width * shm::bytes_per_pixel(format));
        if !cap.buffer.as_ref().is_some_and(|b| b.matches(width, height, format) && b.stride == stride) {
            cap.buffer = Some(shm::ShmBuffer::new(&shm, width, height, stride, format, &self.qh)?);
        }
        let buffer = &cap.buffer.as_ref().expect("allocated above").wl_buffer;

        match &mut cap.protocol {
            Protocol::Ext { session, frame, .. } => {
                let f = session.create_frame(&self.qh, slot);
                f.attach_buffer(buffer);
                f.damage_buffer(0, 0, width as i32, height as i32);
                f.capture();
                *frame = Some(f);
            }
            Protocol::Wlr { frame, .. } => {
                let f = frame.as_ref().expect("created above");
                if cap.wait_for_damage && f.version() >= 2 {
                    f.copy_with_damage(buffer);
                } else {
                    f.copy(buffer);
                }
            }
        }
        cap.phase = Phase::Copying;
        self.conn.flush()?;
        Ok(())
    }

    /// Read a ready capture out of its buffer.
    fn take_image(&mut self, slot: usize) -> Result<Image> {
        let cap = &mut self.state.captures[slot];
        match &cap.phase {
            Phase::Ready => {}
            Phase::Failed(msg) => return Err(Error::Capture(msg.clone())),
            Phase::Stopped => return Err(Error::Stopped),
            _ => return Err(Error::Capture("capture not finished".into())),
        }
        let buffer = cap.buffer.as_ref().ok_or_else(|| Error::Capture("no buffer".into()))?;
        let image = buffer.to_image(cap.y_invert, cap.transform);
        cap.frames_captured += 1;
        // Ready for the next frame of a stream.
        cap.phase = Phase::Negotiating;
        cap.wait_for_damage = true;
        cap.y_invert = false;
        if let Protocol::Ext { .. } = cap.protocol {
            // ext keeps its constraints across frames.
        } else {
            cap.constraints.done = false;
        }
        Ok(image)
    }

    fn release(&mut self, slot: usize) {
        let cap = &mut self.state.captures[slot];
        match &mut cap.protocol {
            Protocol::Ext { source, session, frame } => {
                if let Some(f) = frame.take() {
                    f.destroy();
                }
                session.destroy();
                source.destroy();
            }
            Protocol::Wlr { frame, .. } => {
                if let Some(f) = frame.take() {
                    f.destroy();
                }
            }
        }
        cap.buffer = None;
        cap.phase = Phase::Stopped;
        let _ = self.conn.flush();
    }

    /// Dispatch events, waiting at most `timeout` for new ones. Returns whether any
    /// events were dispatched.
    fn dispatch_timeout(&mut self, timeout: Duration) -> Result<bool> {
        if self.queue.dispatch_pending(&mut self.state)? > 0 {
            return Ok(true);
        }
        self.queue.flush()?;
        let Some(guard) = self.queue.prepare_read() else {
            return Ok(self.queue.dispatch_pending(&mut self.state)? > 0);
        };
        let readable = {
            use rustix::event::{PollFd, PollFlags, Timespec, poll};
            let fd = guard.connection_fd();
            let mut fds = [PollFd::new(&fd, PollFlags::IN | PollFlags::ERR)];
            let ts = Timespec { tv_sec: timeout.as_secs() as _, tv_nsec: timeout.subsec_nanos() as _ };
            loop {
                match poll(&mut fds, Some(&ts)) {
                    Ok(n) => break n > 0,
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(e) => return Err(Error::Io(e.into())),
                }
            }
        };
        if !readable {
            return Ok(false);
        }
        match guard.read() {
            Ok(_) => {}
            Err(wayland_client::backend::WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        Ok(self.queue.dispatch_pending(&mut self.state)? > 0)
    }
}

fn bind_outputs(globals: &GlobalList, qh: &QueueHandle<State>, state: &mut State) {
    let output_globals: Vec<_> = globals
        .contents()
        .with_list(|list| list.iter().filter(|g| g.interface == "wl_output").cloned().collect());
    for global in output_globals {
        let idx = state.outputs.len();
        let version = global.version.min(4);
        let wl_output: wl_output::WlOutput = globals.registry().bind(global.name, version, qh, idx);
        let xdg_output = state.xdg_output_manager.as_ref().map(|m| m.get_xdg_output(&wl_output, qh, idx));
        state.outputs.push(OutputState {
            wl_output: Some(wl_output),
            xdg_output,
            scale: 1,
            ..Default::default()
        });
    }
}

/// One frame of a [`FrameStream`].
#[derive(Debug, Clone)]
pub struct StreamFrame {
    /// Upright pixels of the streamed region.
    pub image: Image,
    /// When the compositor presented this content (CLOCK_MONOTONIC), if it said.
    pub presented: Option<Duration>,
    /// 0 for the first frame of the stream.
    pub sequence: u64,
}

/// The window with `window`'s toplevel identifier, or else the one open window with its
/// app id and title.
fn find_toplevel<'a>(
    toplevels: impl Iterator<Item = &'a ToplevelInfo> + Clone,
    window: &WindowInfo,
) -> Result<usize> {
    let open = || toplevels.clone().enumerate().filter(|(_, t)| !t.closed);
    if let Some(id) = &window.toplevel
        && let Some((i, _)) = open().find(|(_, t)| &t.identifier == id)
    {
        return Ok(i);
    }
    let mut alike = open().filter(|(_, t)| t.app_id == window.app_id && t.title == window.title);
    match (alike.next(), alike.next()) {
        (Some((i, _)), None) => Ok(i),
        (None, _) => Err(Error::NoSuchWindow(format!("no window {:?} ({})", window.title, window.app_id))),
        (Some(_), Some(_)) => Err(Error::NoSuchWindow(format!(
            "several windows are {:?} ({}), and the compositor doesn't say which",
            window.title, window.app_id
        ))),
    }
}

/// Where a stream of a whole output keeps its region.
struct Crop {
    /// Logical, relative to the output.
    region: Rect,
    /// The output's logical width, to find the scale of each frame.
    logical_width: f64,
}

/// Continuous capture of one output, a region of it, or one window. Frames arrive as the
/// content changes (the compositor paces them); static content yields no frames.
///
/// Every frame is a copy the compositor makes for us, so a stream can be paced
/// ([`FrameStream::set_max_rate`]): the next copy isn't asked for until it's due.
/// Otherwise a game drawing 280 frames a second has the compositor copy each one.
pub struct FrameStream {
    capturer: Capturer,
    slot: usize,
    crop: Option<Crop>,
    sequence: u64,
    /// When to ask for frames.
    pacer: Pacer,
}

impl FrameStream {
    fn new(capturer: Capturer, slot: usize, crop: Option<Crop>) -> Self {
        Self { capturer, slot, crop, sequence: 0, pacer: Pacer::new(None) }
    }

    /// Ask the compositor for at most `fps` frames a second.
    pub fn set_max_rate(&mut self, fps: u32) {
        self.pacer = Pacer::new(Some(fps));
    }

    fn phase(&self) -> &Phase {
        &self.capturer.state.captures[self.slot].phase
    }

    /// Ask for the next frame if it's due, else say when it will be.
    fn request(&mut self) -> Result<Option<Instant>> {
        let now = Instant::now();
        if let Some(due) = self.pacer.wait_until(now) {
            return Ok(Some(due));
        }
        let idle = *self.phase() == Phase::Negotiating;
        self.capturer.drive(self.slot)?;
        if idle && *self.phase() == Phase::Copying {
            self.pacer.tick(now);
        }
        Ok(None)
    }

    /// Wait up to `timeout` for the next frame. `Ok(None)` means nothing changed in time;
    /// the pending capture stays in flight and a later call picks it up.
    pub fn next_frame(&mut self, timeout: Duration) -> Result<Option<StreamFrame>> {
        let deadline = Instant::now() + timeout;
        loop {
            let due = self.request()?;
            match self.phase() {
                Phase::Ready => {
                    let presented = self.capturer.state.captures[self.slot].presented;
                    let mut image = self.capturer.take_image(self.slot)?;
                    // Get the compositor working on the next frame (if it's due already)
                    // while we hand this one off.
                    self.request()?;
                    if let Some(crop) = &self.crop {
                        let scale = image.width() as f64 / crop.logical_width;
                        image = image.crop(crop.region.to_pixels(Default::default(), scale));
                    }
                    let frame = StreamFrame { image, presented, sequence: self.sequence };
                    self.sequence += 1;
                    return Ok(Some(frame));
                }
                Phase::Failed(msg) => return Err(Error::Capture(msg.clone())),
                Phase::Stopped => return Err(Error::Stopped),
                Phase::Negotiating | Phase::Copying => {}
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let wake = due.map_or(deadline, |due| due.min(deadline));
            self.capturer.dispatch_timeout(wake.saturating_duration_since(now))?;
        }
    }
}

impl screenie_core::FrameSource for FrameStream {
    fn next_frame(&mut self, timeout: Duration) -> Result<Next, screenie_core::SourceError> {
        match FrameStream::next_frame(self, timeout) {
            Ok(Some(frame)) => Ok(Next::Frame(frame.image)),
            Ok(None) => Ok(Next::Unchanged),
            Err(Error::Stopped) => Ok(Next::Ended),
            Err(e) => Err(e.into()),
        }
    }

    fn pace(&mut self, fps: u32) {
        self.set_max_rate(fps);
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        self.capturer.release(self.slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toplevel(identifier: &str, app_id: &str, title: &str) -> ToplevelInfo {
        ToplevelInfo { identifier: identifier.into(), app_id: app_id.into(), title: title.into(), closed: false }
    }

    fn window(toplevel: Option<&str>, app_id: &str, title: &str) -> WindowInfo {
        WindowInfo {
            id: "1".into(),
            title: title.into(),
            app_id: app_id.into(),
            rect: Rect::default(),
            focused: false,
            floating: false,
            toplevel: toplevel.map(String::from),
        }
    }

    #[test]
    fn windows_are_found_by_identifier_first() {
        let list = [toplevel("a", "foot", "~"), toplevel("b", "foot", "~")];
        assert_eq!(find_toplevel(list.iter(), &window(Some("b"), "foot", "~")).unwrap(), 1);
        // Two alike windows can't be told apart without one.
        assert!(find_toplevel(list.iter(), &window(None, "foot", "~")).is_err());
    }

    #[test]
    fn otherwise_by_app_id_and_title() {
        let mut list = [toplevel("a", "foot", "~"), toplevel("b", "firefox", "Docs"), toplevel("c", "foot", "~")];
        list[0].closed = true;
        assert_eq!(find_toplevel(list.iter(), &window(Some("gone"), "foot", "~")).unwrap(), 2);
        assert!(find_toplevel(list.iter(), &window(None, "firefox", "Mail")).is_err());
    }
}
