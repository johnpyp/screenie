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
//!
//! A stream's frames come in shared memory, read into [`Image`]s, or, where the consumer
//! can take them, in GPU buffers on the compositor's GPU (see `dmabuf`), which neither
//! side copies.

mod dmabuf;
mod focus;
mod shm;
mod state;

use std::time::{Duration, Instant};

use screenie_core::{
    Dmabuf, DmabufFormat, Frame, GpuDevice, GpuOffer, Image, Next, OutputCapture, OutputInfo,
    Pacer, PixelRect, Pixels, Rect, Transform, WindowInfo,
};
use wayland_client::globals::{GlobalList, registry_queue_init};
use wayland_client::protocol::wl_output;
use wayland_client::{Connection, EventQueue, Proxy, QueueHandle};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1::Options;

pub use focus::focused_output;
pub use shm::transform_image;
use state::{
    Capture, Constraints, Gpu, InFlight, OutputState, Phase, Protocol, State, Target, ToplevelInfo,
};

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
        let has = |name: &str| {
            globals
                .contents()
                .with_list(|l| l.iter().any(|g| g.interface == name))
        };
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
            linux_dmabuf: globals.bind(&qh, 3..=5, ()).ok(),
            ..Default::default()
        };
        if state.shm.is_none() {
            return Err(Error::Unsupported("compositor has no wl_shm".into()));
        }
        // Which GPU the compositor renders with, and what it takes: arrives with the
        // roundtrips below. (v6 drops the main device, so this binds at most v5.)
        if let Some(linux_dmabuf) = state.linux_dmabuf.as_ref().filter(|d| d.version() >= 4) {
            linux_dmabuf.get_default_feedback(&qh, ());
        }

        let ext = state.ext_copy.is_some() && state.ext_output_sources.is_some();
        let wlr = state.wlr_screencopy.is_some();
        let backend = match preferred {
            Some(Backend::ExtImageCopyCapture) if ext => Backend::ExtImageCopyCapture,
            Some(Backend::WlrScreencopy) if wlr => Backend::WlrScreencopy,
            Some(b) => {
                return Err(Error::Unsupported(format!(
                    "compositor does not support {}",
                    b.name()
                )));
            }
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
        tracing::debug!(
            backend = backend.name(),
            outputs = state.outputs.len(),
            "wayland capturer ready"
        );

        Ok(Self {
            conn,
            globals,
            queue,
            qh,
            state,
            backend,
        })
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// All outputs with a known mode, in compositor order.
    pub fn outputs(&self) -> Vec<OutputInfo> {
        self.state
            .outputs
            .iter()
            .enumerate()
            .filter_map(|(i, o)| o.info(i))
            .collect()
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
    pub fn capture_outputs(
        &mut self,
        names: Option<&[&str]>,
        cursor: bool,
    ) -> Result<Vec<OutputCapture>> {
        let indices: Vec<usize> = match names {
            Some(names) => names
                .iter()
                .map(|n| self.output_index(n))
                .collect::<Result<_>>()?,
            None => (0..self.state.outputs.len())
                .filter(|&i| self.state.outputs[i].info(i).is_some())
                .collect(),
        };
        let slots: Vec<usize> = indices
            .iter()
            .map(|&i| self.new_capture(Target::Output(i), cursor, None))
            .collect();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            for &slot in &slots {
                self.drive(slot)?;
            }
            let pending = slots.iter().any(|&s| {
                matches!(
                    self.state.captures[s].phase,
                    Phase::Negotiating | Phase::Copying
                )
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
            let info = self.state.outputs[output]
                .info(output)
                .expect("filtered above");
            captures.push(OutputCapture {
                output: info,
                image,
            });
        }
        for slot in slots {
            self.release(slot);
        }
        Ok(captures)
    }

    /// Start continuous capture of one output, optionally restricted to a logical region
    /// relative to the output's top-left corner.
    pub fn into_stream(
        mut self,
        output: &str,
        region: Option<Rect>,
        cursor: bool,
    ) -> Result<FrameStream> {
        let index = self.output_index(output)?;
        let info = self.state.outputs[index]
            .info(index)
            .expect("output has a mode");
        let bounds = Rect::new(0.0, 0.0, info.logical.width, info.logical.height);
        let region = match region {
            Some(r) => Some(
                r.intersection(&bounds)
                    .ok_or_else(|| Error::Capture("region is outside the output".into()))?,
            ),
            None => None,
        };
        // wlr-screencopy can copy just the region; ext always copies the whole output and
        // we crop afterwards.
        let (native_region, crop) = match self.backend {
            Backend::WlrScreencopy => (region, None),
            Backend::ExtImageCopyCapture => (None, region),
        };
        let slot = self.new_capture(Target::Output(index), cursor, native_region);
        let crop = crop.map(|region| Crop {
            region,
            logical_width: info.logical.width,
        });
        Ok(FrameStream::new(self, slot, crop))
    }

    /// Start continuous capture of one window: its own pixels at its own size, following
    /// it across workspaces and outputs, with nothing that covers it. Frames change size
    /// as the window does, and the stream ends when the window closes.
    ///
    /// The window is found by its toplevel identifier where the compositor's IPC reported
    /// one, otherwise by app id and title, which must then be unique.
    pub fn into_window_stream(mut self, window: &WindowInfo, cursor: bool) -> Result<FrameStream> {
        if self.backend != Backend::ExtImageCopyCapture || self.state.ext_toplevel_sources.is_none()
        {
            return Err(Error::Unsupported(
                "this compositor can't capture a window by itself".into(),
            ));
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
        let protocol = self.open_protocol(target, cursor, region, idx);
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
            incoming: Constraints::default(),
            buffer: None,
            spare: None,
            gpu: None,
            in_flight: None,
            phase: Phase::Negotiating,
            transform,
            y_invert: false,
            presented: None,
            wait_for_damage: false,
            frames_captured: 0,
        });
        idx
    }

    /// Start a capture over with a fresh session, whose first frame comes right away
    /// rather than on the next change. Buffers are kept.
    fn restart(&mut self, slot: usize) {
        let cap = &mut self.state.captures[slot];
        let (target, cursor) = (cap.target, cap.cursor);
        let region = match &mut cap.protocol {
            Protocol::Ext {
                session,
                frame,
                source,
            } => {
                if let Some(f) = frame.take() {
                    f.destroy();
                }
                session.destroy();
                source.destroy();
                None
            }
            Protocol::Wlr { frame, region } => {
                if let Some(f) = frame.take() {
                    f.destroy();
                }
                *region
            }
        };
        let protocol = self.open_protocol(target, cursor, region, slot);
        let cap = &mut self.state.captures[slot];
        cap.protocol = protocol;
        cap.constraints = Constraints::default();
        cap.in_flight = None;
        cap.phase = Phase::Negotiating;
        cap.wait_for_damage = false;
        let _ = self.conn.flush();
    }

    fn open_protocol(
        &mut self,
        target: Target,
        cursor: bool,
        region: Option<Rect>,
        idx: usize,
    ) -> Protocol {
        match self.backend {
            Backend::ExtImageCopyCapture => {
                let source = match target {
                    Target::Output(output) => {
                        let wl_output = self.state.outputs[output]
                            .wl_output
                            .as_ref()
                            .expect("bound output");
                        let sources = self
                            .state
                            .ext_output_sources
                            .as_ref()
                            .expect("checked at connect");
                        sources.create_source(wl_output, &self.qh, ())
                    }
                    Target::Toplevel(toplevel) => {
                        let handle = &self.state.toplevels[toplevel].handle;
                        let sources = self
                            .state
                            .ext_toplevel_sources
                            .as_ref()
                            .expect("checked by the caller");
                        sources.create_source(handle, &self.qh, ())
                    }
                };
                let options = if cursor {
                    Options::PaintCursors
                } else {
                    Options::empty()
                };
                let session = self
                    .state
                    .ext_copy
                    .as_ref()
                    .expect("checked at connect")
                    .create_session(&source, options, &self.qh, idx);
                Protocol::Ext {
                    source,
                    session,
                    frame: None,
                }
            }
            Backend::WlrScreencopy => Protocol::Wlr {
                frame: None,
                region,
            },
        }
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
            let manager = self
                .state
                .wlr_screencopy
                .as_ref()
                .expect("checked at connect");
            let Target::Output(output) = cap.target else {
                unreachable!("wlr only captures outputs")
            };
            let wl_output = outputs[output].wl_output.as_ref().expect("bound output");
            let overlay = cap.cursor as i32;
            cap.incoming = Constraints::default();
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
        let (buffer, in_flight) = if let Some(gpu) = &mut cap.gpu {
            let linux_dmabuf = self
                .state
                .linux_dmabuf
                .as_ref()
                .expect("checked by use_gpu");
            let pool = match &mut gpu.pool {
                Some(pool) if (pool.width, pool.height) == (width, height) => pool,
                pool => {
                    let allocator = gpu.allocator.as_ref().expect("opened by use_gpu");
                    pool.insert(dmabuf::Pool::new(
                        allocator,
                        linux_dmabuf,
                        (width, height),
                        &gpu.format,
                        &self.qh,
                    )?)
                }
            };
            // Every buffer is still on its way through the consumer: try again shortly.
            let Some(index) = pool.free() else {
                return Ok(());
            };
            (pool.wl_buffer(index).clone(), InFlight::Gpu(index))
        } else {
            let format = shm::choose_format(&cap.constraints.formats).ok_or_else(|| {
                Error::Unsupported(format!(
                    "no supported shm format among {:?}",
                    cap.constraints.formats
                ))
            })?;
            let stride = cap
                .constraints
                .strides
                .iter()
                .find(|(f, _)| *f == format)
                .map(|(_, s)| *s)
                .unwrap_or(width * shm::bytes_per_pixel(format));
            if cap.buffer.is_none() {
                cap.buffer = cap.spare.take();
            }
            if !cap
                .buffer
                .as_ref()
                .is_some_and(|b| b.matches(width, height, format) && b.stride == stride)
            {
                cap.buffer = Some(shm::ShmBuffer::new(
                    &shm, width, height, stride, format, &self.qh,
                )?);
            }
            (
                cap.buffer
                    .as_ref()
                    .expect("allocated above")
                    .wl_buffer
                    .clone(),
                InFlight::Shm,
            )
        };
        let buffer = &buffer;

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
        cap.in_flight = Some(in_flight);
        cap.phase = Phase::Copying;
        self.conn.flush()?;
        Ok(())
    }

    /// Take a ready capture's buffer, leaving the capture free for the next frame.
    fn take_ready(&mut self, slot: usize) -> Result<Ready> {
        let cap = &mut self.state.captures[slot];
        match &cap.phase {
            Phase::Ready => {}
            Phase::Failed(msg) => return Err(Error::Capture(msg.clone())),
            Phase::Stopped => return Err(Error::Stopped),
            _ => return Err(Error::Capture("capture not finished".into())),
        }
        let ready = match cap.in_flight.take() {
            Some(InFlight::Gpu(index)) => {
                let pool = cap.gpu.as_ref().and_then(|g| g.pool.as_ref());
                Ready::Gpu(
                    pool.ok_or_else(|| Error::Capture("no GPU buffers".into()))?
                        .take(index, None),
                )
            }
            _ => Ready::Shm {
                buffer: cap
                    .buffer
                    .take()
                    .ok_or_else(|| Error::Capture("no buffer".into()))?,
                y_invert: cap.y_invert,
                transform: cap.transform,
            },
        };
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
        Ok(ready)
    }

    /// A shared-memory buffer read out, for the next frame to use.
    fn recycle(&mut self, slot: usize, buffer: shm::ShmBuffer) {
        let cap = &mut self.state.captures[slot];
        if cap.buffer.is_none() {
            cap.buffer = Some(buffer);
        } else {
            cap.spare = Some(buffer);
        }
    }

    /// Read a ready capture out of its buffer.
    fn take_image(&mut self, slot: usize) -> Result<Image> {
        match self.take_ready(slot)? {
            Ready::Shm {
                buffer,
                y_invert,
                transform,
            } => {
                let image = buffer.to_image(y_invert, transform);
                self.recycle(slot, buffer);
                Ok(image)
            }
            Ready::Gpu(_) => Err(Error::Capture(
                "a GPU frame where an image was expected".into(),
            )),
        }
    }

    fn release(&mut self, slot: usize) {
        let cap = &mut self.state.captures[slot];
        match &mut cap.protocol {
            Protocol::Ext {
                source,
                session,
                frame,
            } => {
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
        cap.spare = None;
        cap.gpu = None;
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
            let ts = Timespec {
                tv_sec: timeout.as_secs() as _,
                tv_nsec: timeout.subsec_nanos() as _,
            };
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
            Err(wayland_client::backend::WaylandError::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        Ok(self.queue.dispatch_pending(&mut self.state)? > 0)
    }
}

fn bind_outputs(globals: &GlobalList, qh: &QueueHandle<State>, state: &mut State) {
    let output_globals: Vec<_> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == "wl_output")
            .cloned()
            .collect()
    });
    for global in output_globals {
        let idx = state.outputs.len();
        let version = global.version.min(4);
        let wl_output: wl_output::WlOutput = globals.registry().bind(global.name, version, qh, idx);
        let xdg_output = state
            .xdg_output_manager
            .as_ref()
            .map(|m| m.get_xdg_output(&wl_output, qh, idx));
        state.outputs.push(OutputState {
            wl_output: Some(wl_output),
            xdg_output,
            scale: 1,
            ..Default::default()
        });
    }
}

/// A ready capture's buffer, before it's read.
enum Ready {
    Shm {
        buffer: shm::ShmBuffer,
        y_invert: bool,
        transform: Transform,
    },
    Gpu(Dmabuf),
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
        (None, _) => Err(Error::NoSuchWindow(format!(
            "no window {:?} ({})",
            window.title, window.app_id
        ))),
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

impl Crop {
    /// The region in the pixels of a `width`×`height` frame of the output.
    fn pixels(&self, width: u32, height: u32) -> PixelRect {
        let scale = width as f64 / self.logical_width;
        let r = self.region.to_pixels(Default::default(), scale);
        r.intersection(&PixelRect::new(0, 0, width, height))
            .unwrap_or(PixelRect::new(0, 0, width, height))
    }
}

/// Continuous capture of one output, a region of it, or one window. Frames arrive as the
/// content changes (the compositor paces them); static content yields no frames.
///
/// Every frame is a copy the compositor makes for us, so a stream can be paced
/// ([`FrameStream::set_max_rate`]): the next copy isn't asked for until it's due.
/// Otherwise a game drawing 280 frames a second has the compositor copy each one. It's
/// asked for a little ahead, by how long the compositor has been taking, so it's there
/// on time, and the compositor copies the next frame while this one is read.
pub struct FrameStream {
    capturer: Capturer,
    slot: usize,
    crop: Option<Crop>,
    /// When frames are due.
    pacer: Pacer,
    /// How long the compositor takes to deliver a frame once asked (a running average).
    latency: Duration,
    /// When the frame in flight was asked for.
    requested: Option<Instant>,
    /// Whether frames come the right way up (wlr may flip them): GPU buffers can't be
    /// flipped on the way.
    upright: bool,
    stats: Stats,
}

#[derive(Default)]
struct Stats {
    frames: u64,
    gpu_frames: u64,
    latency_total: Duration,
    latency_max: Duration,
}

impl FrameStream {
    fn new(capturer: Capturer, slot: usize, crop: Option<Crop>) -> Self {
        Self {
            capturer,
            slot,
            crop,
            pacer: Pacer::new(None),
            latency: Duration::ZERO,
            requested: None,
            upright: true,
            stats: Stats::default(),
        }
    }

    /// Ask the compositor for at most `fps` frames a second.
    pub fn set_max_rate(&mut self, fps: u32) {
        self.pacer = Pacer::new(Some(fps));
    }

    fn capture(&self) -> &Capture {
        &self.capturer.state.captures[self.slot]
    }

    fn phase(&self) -> &Phase {
        &self.capture().phase
    }

    /// Ask for the next frame if it's due (or will be, by the time it comes), else say
    /// when to.
    fn request(&mut self) -> Result<Option<Instant>> {
        let now = Instant::now();
        let lead = self.latency.min(self.pacer.interval() * 3 / 4);
        if let Some(due) = self.pacer.wait_until(now + lead) {
            return Ok(Some(due - lead));
        }
        let idle = *self.phase() == Phase::Negotiating;
        self.capturer.drive(self.slot)?;
        if idle && *self.phase() == Phase::Copying {
            self.pacer.tick(now + lead);
            self.requested = Some(now);
        }
        Ok(None)
    }

    /// Waiting on a GPU buffer the consumer still holds.
    fn starved(&self) -> bool {
        let cap = self.capture();
        cap.phase == Phase::Negotiating && cap.constraints.done && cap.gpu.is_some()
    }

    /// Wait up to `timeout` for the next frame. `Ok(None)` means nothing changed in time;
    /// the pending capture stays in flight and a later call picks it up.
    pub fn next_frame(&mut self, timeout: Duration) -> Result<Option<Frame>> {
        let deadline = Instant::now() + timeout;
        loop {
            let due = self.request()?;
            match self.phase() {
                Phase::Ready => {
                    if let Some(at) = self.requested.take() {
                        let took = at.elapsed();
                        self.latency = (self.latency * 7 + took) / 8;
                        self.stats.latency_total += took;
                        self.stats.latency_max = self.stats.latency_max.max(took);
                    }
                    let presented = self.capture().presented;
                    if self.capture().y_invert
                        && matches!(self.capture().in_flight, Some(InFlight::Gpu(_)))
                    {
                        tracing::info!(
                            "the compositor flips GPU frames; taking them through memory"
                        );
                        self.upright = false;
                        self.use_gpu(None)?;
                        continue;
                    }
                    let ready = self.capturer.take_ready(self.slot)?;
                    // Get the compositor working on the next frame (if it's due) while
                    // this one is read.
                    self.request()?;
                    self.stats.frames += 1;
                    let pixels = match ready {
                        Ready::Shm {
                            buffer,
                            y_invert,
                            transform,
                        } => {
                            self.upright = !y_invert;
                            let mut image = buffer.to_image(y_invert, transform);
                            self.capturer.recycle(self.slot, buffer);
                            if let Some(crop) = &self.crop {
                                image = image.crop(crop.pixels(image.width(), image.height()));
                            }
                            Pixels::Cpu(image)
                        }
                        Ready::Gpu(mut buffer) => {
                            self.stats.gpu_frames += 1;
                            buffer.crop = self
                                .crop
                                .as_ref()
                                .map(|c| c.pixels(buffer.width, buffer.height));
                            Pixels::Gpu(buffer)
                        }
                    };
                    return Ok(Some(Frame { pixels, presented }));
                }
                Phase::Failed(msg) => return Err(Error::Capture(msg.clone())),
                Phase::Stopped => return Err(Error::Stopped),
                Phase::Negotiating | Phase::Copying => {}
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let mut wake = due.map_or(deadline, |due| due.min(deadline));
            if self.starved() {
                wake = wake.min(now + Duration::from_millis(2));
            }
            self.capturer
                .dispatch_timeout(wake.saturating_duration_since(now))?;
        }
    }

    /// The GPU the compositor renders frames on, where it says: the encoder on it is
    /// the best one even for frames in memory.
    pub fn gpu(&self) -> Option<GpuDevice> {
        let dev = self.capture().constraints.dmabuf_device.or(self
            .capturer
            .state
            .feedback
            .main_device)?;
        Some(GpuDevice::from_dev(dev))
    }

    /// The GPU buffers the compositor could render frames into, once it has said (with
    /// the first frame). Only upright frames: a rotated or flipped one would need turning.
    pub fn gpu_offer(&self) -> Option<GpuOffer> {
        self.capturer.state.linux_dmabuf.as_ref()?;
        let cap = self.capture();
        if cap.transform != Transform::Normal
            || !self.upright
            || cap.constraints.dmabuf_formats.is_empty()
        {
            return None;
        }
        let device = GpuDevice::from_dev(cap.constraints.dmabuf_device?);
        Some(GpuOffer {
            device,
            formats: cap.constraints.dmabuf_formats.clone(),
        })
    }

    /// Take frames in GPU buffers of `format` from now on, or in shared memory (`None`).
    /// The buffers are allocated here, so a GPU that can't make them fails now. The next
    /// frame comes right away, even if nothing changed.
    pub fn use_gpu(&mut self, format: Option<DmabufFormat>) -> Result<()> {
        self.settle()?;
        let gpu = match format {
            None => None,
            Some(format) => {
                let cap = self.capture();
                let dev = cap
                    .constraints
                    .dmabuf_device
                    .ok_or_else(|| Error::Unsupported("no GPU buffers".into()))?;
                let size = cap
                    .constraints
                    .size
                    .ok_or_else(|| Error::Capture("no buffer size yet".into()))?;
                let linux_dmabuf = self.capturer.state.linux_dmabuf.as_ref().ok_or_else(|| {
                    Error::Unsupported("the compositor takes no GPU buffers".into())
                })?;
                let allocator = dmabuf::Allocator::open(GpuDevice::from_dev(dev))?;
                let pool =
                    dmabuf::Pool::new(&allocator, linux_dmabuf, size, &format, &self.capturer.qh)?;
                Some(Gpu {
                    format,
                    allocator: Some(allocator),
                    pool: Some(pool),
                })
            }
        };
        self.capturer.state.captures[self.slot].gpu = gpu;
        self.capturer.restart(self.slot);
        Ok(())
    }

    /// Let a frame in flight land (and drop it), so the buffers can change.
    fn settle(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(1);
        while *self.phase() == Phase::Copying && Instant::now() < deadline {
            self.capturer.dispatch_timeout(Duration::from_millis(50))?;
        }
        if *self.phase() == Phase::Ready {
            self.requested = None;
            if let Ready::Shm { buffer, .. } = self.capturer.take_ready(self.slot)? {
                self.capturer.recycle(self.slot, buffer);
            }
        }
        Ok(())
    }

    /// What the stream shows right now, as an image, from a capture of its own (a
    /// stream's next frame only comes once something changes).
    pub fn snapshot(&mut self) -> Result<Image> {
        let (target, cursor) = (self.capture().target, self.capture().cursor);
        let region = match &self.capture().protocol {
            Protocol::Wlr { region, .. } => *region,
            Protocol::Ext { .. } => None,
        };
        let slot = self.capturer.new_capture(target, cursor, region);
        let deadline = Instant::now() + Duration::from_secs(2);
        let image = loop {
            self.capturer.drive(slot)?;
            if self.capturer.state.captures[slot].phase == Phase::Ready {
                break self.capturer.take_image(slot);
            }
            if matches!(
                self.capturer.state.captures[slot].phase,
                Phase::Failed(_) | Phase::Stopped
            ) {
                break self.capturer.take_image(slot);
            }
            let now = Instant::now();
            if now >= deadline {
                break Err(Error::Timeout);
            }
            self.capturer.dispatch_timeout(deadline - now)?;
        };
        self.capturer.release(slot);
        let image = image?;
        Ok(match &self.crop {
            Some(crop) => image.crop(crop.pixels(image.width(), image.height())),
            None => image,
        })
    }
}

impl screenie_core::FrameSource for FrameStream {
    fn next_frame(&mut self, timeout: Duration) -> Result<Next, screenie_core::SourceError> {
        match FrameStream::next_frame(self, timeout) {
            Ok(Some(frame)) => Ok(Next::Frame(frame)),
            Ok(None) => Ok(Next::Unchanged),
            Err(Error::Stopped) => Ok(Next::Ended),
            Err(e) => Err(e.into()),
        }
    }

    fn pace(&mut self, fps: u32) {
        self.set_max_rate(fps);
    }

    fn gpu(&self) -> Option<GpuDevice> {
        FrameStream::gpu(self)
    }

    fn gpu_offer(&self) -> Option<GpuOffer> {
        FrameStream::gpu_offer(self)
    }

    fn use_gpu(&mut self, format: Option<DmabufFormat>) -> Result<(), screenie_core::SourceError> {
        Ok(FrameStream::use_gpu(self, format)?)
    }

    fn snapshot(&mut self) -> Option<Image> {
        FrameStream::snapshot(self)
            .map_err(|e| tracing::warn!("snapshot of the stream failed: {e}"))
            .ok()
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        let s = &self.stats;
        if s.frames > 0 {
            tracing::info!(
                frames = s.frames,
                gpu_frames = s.gpu_frames,
                latency_avg = ?s.latency_total / s.frames.max(1) as u32,
                latency_max = ?s.latency_max,
                "capture stream"
            );
        }
        self.capturer.release(self.slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toplevel(identifier: &str, app_id: &str, title: &str) -> ToplevelInfo {
        ToplevelInfo {
            identifier: identifier.into(),
            app_id: app_id.into(),
            title: title.into(),
            closed: false,
        }
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
        assert_eq!(
            find_toplevel(list.iter(), &window(Some("b"), "foot", "~")).unwrap(),
            1
        );
        // Two alike windows can't be told apart without one.
        assert!(find_toplevel(list.iter(), &window(None, "foot", "~")).is_err());
    }

    #[test]
    fn otherwise_by_app_id_and_title() {
        let mut list = [
            toplevel("a", "foot", "~"),
            toplevel("b", "firefox", "Docs"),
            toplevel("c", "foot", "~"),
        ];
        list[0].closed = true;
        assert_eq!(
            find_toplevel(list.iter(), &window(Some("gone"), "foot", "~")).unwrap(),
            2
        );
        assert!(find_toplevel(list.iter(), &window(None, "firefox", "Mail")).is_err());
    }
}
