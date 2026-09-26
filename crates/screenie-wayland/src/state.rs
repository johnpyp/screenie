//! Wayland protocol state: bound globals, output bookkeeping, and the per-capture state
//! machines for both capture protocols. Everything here is driven by [`crate::Capturer`].

use screenie_core::gpu::fourcc_name;
use screenie_core::{DmabufFormat, GpuDevice, OutputInfo, Rect, Transform};
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop, event_created_child};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1,
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
    zwp_linux_dmabuf_feedback_v1::{self, ZwpLinuxDmabufFeedbackV1},
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

use crate::dmabuf::{Allocator, Pool};
use crate::shm::ShmBuffer;

#[derive(Default)]
pub(crate) struct State {
    pub shm: Option<wl_shm::WlShm>,
    pub xdg_output_manager: Option<ZxdgOutputManagerV1>,
    pub ext_copy: Option<ExtImageCopyCaptureManagerV1>,
    pub ext_output_sources: Option<ExtOutputImageCaptureSourceManagerV1>,
    pub ext_toplevel_sources: Option<ExtForeignToplevelImageCaptureSourceManagerV1>,
    pub wlr_screencopy: Option<ZwlrScreencopyManagerV1>,
    /// For GPU buffers (see `dmabuf`).
    pub linux_dmabuf: Option<ZwpLinuxDmabufV1>,
    /// What the compositor says of GPU buffers in general (linux-dmabuf v4+).
    pub feedback: Feedback,
    pub outputs: Vec<OutputState>,
    /// Bound only to capture a window: listing them is otherwise wasted traffic.
    pub toplevel_list: Option<ExtForeignToplevelListV1>,
    pub toplevels: Vec<Toplevel>,
    pub captures: Vec<Capture>,
}

pub(crate) struct Toplevel {
    pub handle: ExtForeignToplevelHandleV1,
    pub info: ToplevelInfo,
}

/// A window as `ext-foreign-toplevel-list` describes it.
#[derive(Debug, Clone, Default)]
pub(crate) struct ToplevelInfo {
    pub identifier: String,
    pub app_id: String,
    pub title: String,
    pub closed: bool,
}

#[derive(Default)]
pub(crate) struct OutputState {
    pub wl_output: Option<wl_output::WlOutput>,
    /// Held so the object (and its events) stay alive.
    #[allow(dead_code)]
    pub xdg_output: Option<ZxdgOutputV1>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub make_model: Option<String>,
    pub position: (i32, i32),
    pub transform: Transform,
    pub scale: i32,
    pub mode: Option<(i32, i32)>,
    pub logical_position: Option<(i32, i32)>,
    pub logical_size: Option<(i32, i32)>,
    pub done: bool,
}

impl OutputState {
    /// Assemble what we know about this output. `None` until it has a mode.
    pub fn info(&self, index: usize) -> Option<OutputInfo> {
        let (mw, mh) = self.mode?;
        let (pw, ph) = if self.transform.swaps_axes() { (mh, mw) } else { (mw, mh) };
        let scale_int = self.scale.max(1);
        let (lw, lh) = self.logical_size.unwrap_or((pw / scale_int, ph / scale_int));
        let (lx, ly) = self.logical_position.unwrap_or(self.position);
        let scale = if lw > 0 { pw as f64 / lw as f64 } else { scale_int as f64 };
        Some(OutputInfo {
            name: self.name.clone().unwrap_or_else(|| format!("output-{index}")),
            description: self.description.clone().or_else(|| self.make_model.clone()).unwrap_or_default(),
            logical: Rect::new(lx as f64, ly as f64, lw as f64, lh as f64),
            scale,
            transform: self.transform,
        })
    }
}

pub(crate) enum Protocol {
    Ext {
        source: ExtImageCaptureSourceV1,
        session: ExtImageCopyCaptureSessionV1,
        frame: Option<ExtImageCopyCaptureFrameV1>,
    },
    Wlr {
        frame: Option<ZwlrScreencopyFrameV1>,
        /// Logical region relative to the output, for `capture_output_region`.
        region: Option<Rect>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Phase {
    /// Waiting for the compositor to describe acceptable buffers.
    Negotiating,
    /// Buffer attached, copy requested.
    Copying,
    Ready,
    Failed(String),
    /// The source went away for good (ext `stopped`).
    Stopped,
}

/// Buffer constraints advertised by the compositor.
#[derive(Default)]
pub(crate) struct Constraints {
    pub size: Option<(u32, u32)>,
    pub formats: Vec<wl_shm::Format>,
    /// wlr-screencopy dictates the stride per format.
    pub strides: Vec<(wl_shm::Format, u32)>,
    /// The GPU the compositor renders with, when it takes GPU buffers: from ext's
    /// constraints, or for wlr from the linux-dmabuf feedback.
    pub dmabuf_device: Option<u64>,
    pub dmabuf_formats: Vec<DmabufFormat>,
    pub done: bool,
}

/// The compositor's linux-dmabuf feedback: the GPU it renders with, and the formats and
/// modifiers that GPU takes. It says which GPU a capture's frames are on when the
/// capture protocol doesn't (wlr-screencopy, or ext without GPU buffers).
#[derive(Default)]
pub(crate) struct Feedback {
    pub main_device: Option<u64>,
    /// The formats of the tranches for the main device.
    pub formats: Vec<DmabufFormat>,
    /// Being received: the format table, and the tranches so far.
    table: Vec<(u32, u64)>,
    tranche_device: Option<u64>,
    tranche: Vec<u16>,
    pending: Vec<(u64, Vec<u16>)>,
    pending_main: Option<u64>,
}

impl Feedback {
    /// The modifiers the main device takes `fourcc` in.
    pub fn modifiers(&self, fourcc: u32) -> Vec<u64> {
        self.formats.iter().find(|f| f.fourcc == fourcc).map(|f| f.modifiers.clone()).unwrap_or_default()
    }

    /// All parameters are in: keep the formats of the main device's tranches.
    fn done(&mut self) {
        self.main_device = self.pending_main.take();
        let tranches = std::mem::take(&mut self.pending);
        let Some(main) = self.main_device.map(GpuDevice::from_dev) else { return };
        let mut formats: Vec<DmabufFormat> = Vec::new();
        for (device, indices) in tranches {
            if device != main.dev && !GpuDevice::from_dev(device).same_as(&main) {
                continue;
            }
            for &(fourcc, modifier) in indices.iter().filter_map(|&i| self.table.get(i as usize)) {
                match formats.iter_mut().find(|f| f.fourcc == fourcc) {
                    Some(f) if !f.modifiers.contains(&modifier) => f.modifiers.push(modifier),
                    Some(_) => {}
                    None => formats.push(DmabufFormat { fourcc, modifiers: vec![modifier] }),
                }
            }
        }
        tracing::debug!(gpu = main.describe(), formats = formats.len(), "compositor GPU");
        self.formats = formats;
    }
}

/// A dev_t sent as an array, in native byte order.
fn dev_t(bytes: &[u8]) -> Option<u64> {
    bytes.try_into().ok().map(u64::from_ne_bytes)
}

/// A capture's GPU buffers: in the format the consumer asked for, allocated once the
/// compositor has said which GPU and size.
pub(crate) struct Gpu {
    pub format: DmabufFormat,
    pub allocator: Option<Allocator>,
    pub pool: Option<Pool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InFlight {
    Shm,
    Gpu(usize),
}

/// What a capture copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// An output, by index into [`State::outputs`].
    Output(usize),
    /// A window, by index into [`State::toplevels`].
    Toplevel(usize),
}

pub(crate) struct Capture {
    pub target: Target,
    pub protocol: Protocol,
    pub cursor: bool,
    /// The buffers the compositor takes, as it last said.
    pub constraints: Constraints,
    /// wlr: what the frame being negotiated says, until it's said it all. (Each wlr
    /// frame says it again; the last complete set stays in `constraints` meanwhile.)
    pub incoming: Constraints,
    /// The shared-memory buffer the next copy goes into, and a second one to copy the
    /// next frame into while this one is read.
    pub buffer: Option<ShmBuffer>,
    pub spare: Option<ShmBuffer>,
    /// GPU buffers instead, when the consumer asked for them.
    pub gpu: Option<Gpu>,
    /// Which buffer the frame in flight (or ready) went into.
    pub in_flight: Option<InFlight>,
    pub phase: Phase,
    pub transform: Transform,
    pub y_invert: bool,
    /// CLOCK_MONOTONIC presentation time of the last ready frame.
    pub presented: Option<std::time::Duration>,
    /// Whether the next copy should wait for new content (streams) or copy right away.
    pub wait_for_damage: bool,
    pub frames_captured: u64,
}

impl Capture {
    /// wlr: the frame being negotiated has said all it will.
    fn take_incoming(&mut self) {
        self.constraints = std::mem::take(&mut self.incoming);
        self.constraints.done = true;
    }
}

pub(crate) fn transform_from_wl(t: WEnum<wl_output::Transform>) -> Transform {
    use wl_output::Transform as T;
    match t {
        WEnum::Value(T::_90) => Transform::Rotate90,
        WEnum::Value(T::_180) => Transform::Rotate180,
        WEnum::Value(T::_270) => Transform::Rotate270,
        WEnum::Value(T::Flipped) => Transform::Flipped,
        WEnum::Value(T::Flipped90) => Transform::Flipped90,
        WEnum::Value(T::Flipped180) => Transform::Flipped180,
        WEnum::Value(T::Flipped270) => Transform::Flipped270,
        _ => Transform::Normal,
    }
}

fn shm_format(f: WEnum<wl_shm::Format>) -> Option<wl_shm::Format> {
    match f {
        WEnum::Value(f) => Some(f),
        WEnum::Unknown(_) => None,
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Outputs appearing or disappearing mid-capture are handled by the capture failing;
        // each Capturer is short-lived or re-created.
    }
}

impl Dispatch<wl_output::WlOutput, usize> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let out = &mut state.outputs[*idx];
        match event {
            wl_output::Event::Geometry { x, y, make, model, transform, .. } => {
                out.position = (x, y);
                out.transform = transform_from_wl(transform);
                out.make_model = Some(format!("{make} {model}").trim().to_string());
            }
            wl_output::Event::Mode { flags, width, height, .. } => {
                if let WEnum::Value(flags) = flags
                    && flags.contains(wl_output::Mode::Current)
                {
                    out.mode = Some((width, height));
                }
            }
            wl_output::Event::Scale { factor } => out.scale = factor,
            wl_output::Event::Name { name } => out.name = Some(name),
            wl_output::Event::Description { description } => out.description = Some(description),
            wl_output::Event::Done => out.done = true,
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, usize> for State {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let out = &mut state.outputs[*idx];
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => out.logical_position = Some((x, y)),
            zxdg_output_v1::Event::LogicalSize { width, height } => out.logical_size = Some((width, height)),
            zxdg_output_v1::Event::Name { name } => {
                out.name.get_or_insert(name);
            }
            zxdg_output_v1::Event::Description { description } => {
                out.description.get_or_insert(description);
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, usize> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event;
        let cap = &mut state.captures[*idx];
        match event {
            Event::BufferSize { width, height } => {
                // Constraints may be re-sent (e.g. on mode change); start over.
                cap.constraints = Constraints { size: Some((width, height)), ..Default::default() };
            }
            Event::ShmFormat { format } => cap.constraints.formats.extend(shm_format(format)),
            Event::DmabufDevice { device } => cap.constraints.dmabuf_device = dev_t(&device),
            Event::DmabufFormat { format, modifiers } => {
                let modifiers = modifiers
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|m| u64::from_ne_bytes(*m))
                    .collect();
                cap.constraints.dmabuf_formats.push(DmabufFormat { fourcc: format, modifiers });
            }
            Event::Done => cap.constraints.done = true,
            Event::Stopped => cap.phase = Phase::Stopped,
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, usize> for State {
    fn event(
        state: &mut Self,
        frame: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event;
        let cap = &mut state.captures[*idx];
        match event {
            Event::Transform { transform } => cap.transform = transform_from_wl(transform),
            Event::PresentationTime { tv_sec_hi, tv_sec_lo, tv_nsec } => {
                let secs = ((tv_sec_hi as u64) << 32) | tv_sec_lo as u64;
                cap.presented = Some(std::time::Duration::new(secs, tv_nsec));
            }
            Event::Ready => {
                frame.destroy();
                if let Protocol::Ext { frame, .. } = &mut cap.protocol {
                    *frame = None;
                }
                cap.phase = Phase::Ready;
            }
            Event::Failed { reason } => {
                frame.destroy();
                if let Protocol::Ext { frame, .. } = &mut cap.protocol {
                    *frame = None;
                }
                use ext_image_copy_capture_frame_v1::FailureReason as R;
                cap.phase = match reason {
                    WEnum::Value(R::Stopped) => Phase::Stopped,
                    WEnum::Value(R::BufferConstraints) => {
                        // Renegotiate: new constraints will follow.
                        cap.buffer = None;
                        Phase::Negotiating
                    }
                    other => Phase::Failed(format!("capture failed ({other:?})")),
                };
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, usize> for State {
    fn event(
        state: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event;
        let cap = &mut state.captures[*idx];
        match event {
            Event::LinuxDmabuf { format, width, height } => {
                // Only the format: the modifiers and the GPU come from the feedback.
                let modifiers = state.feedback.modifiers(format);
                tracing::trace!(format = fourcc_name(format), modifiers = modifiers.len(), "wlr GPU buffer format");
                if let Some(device) = state.feedback.main_device
                    && !modifiers.is_empty()
                {
                    cap.incoming.size.get_or_insert((width, height));
                    cap.incoming.dmabuf_device = Some(device);
                    cap.incoming.dmabuf_formats.push(DmabufFormat { fourcc: format, modifiers });
                }
            }
            Event::Buffer { format, width, height, stride } => {
                if let Some(format) = shm_format(format) {
                    cap.incoming.size = Some((width, height));
                    cap.incoming.formats.push(format);
                    cap.incoming.strides.push((format, stride));
                }
                // Before v3 there is exactly one buffer event and no buffer_done.
                if frame.version() < 3 {
                    cap.take_incoming();
                }
            }
            Event::BufferDone => cap.take_incoming(),
            Event::Flags { flags: WEnum::Value(flags) } => {
                cap.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
            }
            Event::Ready { tv_sec_hi, tv_sec_lo, tv_nsec } => {
                let secs = ((tv_sec_hi as u64) << 32) | tv_sec_lo as u64;
                cap.presented = Some(std::time::Duration::new(secs, tv_nsec));
                frame.destroy();
                if let Protocol::Wlr { frame, .. } = &mut cap.protocol {
                    *frame = None;
                }
                cap.phase = Phase::Ready;
            }
            Event::Failed => {
                frame.destroy();
                if let Protocol::Wlr { frame, .. } = &mut cap.protocol {
                    *frame = None;
                }
                cap.phase = Phase::Failed("compositor refused the screencopy".into());
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpLinuxDmabufFeedbackV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_linux_dmabuf_feedback_v1::Event;
        let feedback = &mut state.feedback;
        match event {
            Event::FormatTable { fd, size } => feedback.table = read_format_table(fd, size as usize),
            Event::MainDevice { device } => feedback.pending_main = dev_t(&device),
            Event::TrancheTargetDevice { device } => feedback.tranche_device = dev_t(&device),
            Event::TrancheFormats { indices } => {
                feedback.tranche.extend(indices.as_chunks::<2>().0.iter().map(|i| u16::from_ne_bytes(*i)));
            }
            Event::TrancheDone => {
                let indices = std::mem::take(&mut feedback.tranche);
                if let Some(device) = feedback.tranche_device.take() {
                    feedback.pending.push((device, indices));
                }
            }
            Event::Done => feedback.done(),
            _ => {}
        }
    }
}

/// The feedback's format table: (fourcc, modifier) pairs, 16 bytes each.
fn read_format_table(fd: std::os::fd::OwnedFd, size: usize) -> Vec<(u32, u64)> {
    use std::os::unix::fs::FileExt;
    let mut bytes = vec![0; size];
    if let Err(e) = std::fs::File::from(fd).read_exact_at(&mut bytes, 0) {
        tracing::debug!("cannot read the compositor's GPU buffer formats: {e}");
        return Vec::new();
    }
    bytes
        .as_chunks::<16>()
        .0
        .iter()
        .map(|entry| {
            let fourcc = u32::from_ne_bytes(entry[..4].try_into().expect("4 bytes"));
            (fourcc, u64::from_ne_bytes(entry[8..].try_into().expect("8 bytes")))
        })
        .collect()
}

impl Dispatch<ExtForeignToplevelListV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state.toplevels.push(Toplevel { handle: toplevel, info: ToplevelInfo::default() });
        }
    }

    event_created_child!(State, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_foreign_toplevel_handle_v1::Event;
        let Some(toplevel) = state.toplevels.iter_mut().find(|t| &t.handle == handle).map(|t| &mut t.info) else {
            return;
        };
        match event {
            Event::Identifier { identifier } => toplevel.identifier = identifier,
            Event::AppId { app_id } => toplevel.app_id = app_id,
            Event::Title { title } => toplevel.title = title,
            Event::Closed => toplevel.closed = true,
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore ZwpLinuxDmabufV1);
delegate_noop!(State: ignore ZwpLinuxBufferParamsV1);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ZxdgOutputManagerV1);
delegate_noop!(State: ExtImageCopyCaptureManagerV1);
delegate_noop!(State: ExtOutputImageCaptureSourceManagerV1);
delegate_noop!(State: ExtForeignToplevelImageCaptureSourceManagerV1);
delegate_noop!(State: ExtImageCaptureSourceV1);
delegate_noop!(State: ZwlrScreencopyManagerV1);
