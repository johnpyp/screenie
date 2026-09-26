//! GPU buffers (DMA-BUFs) the compositor renders frames into, for streams whose consumer
//! can take them: no read-back into memory for the compositor, no copy for us, and the
//! encoder on that GPU reads them directly.
//!
//! Buffers are allocated with GBM on the compositor's own render node, in a format and
//! layout (modifier) both it and the consumer accept, and handed out with a lease: a
//! buffer isn't captured into again until every holder of its frame lets go.

use std::fs::File;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use screenie_core::gpu::fourcc_name;
use screenie_core::{Dmabuf, DmabufFormat, DmabufPlane, GpuDevice, PixelRect};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::{Dispatch, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
};

use crate::Error;

/// Buffers per stream: one being captured into, one waiting its turn, and the rest on
/// their way into the encoder, whose converter lets go of each once it has converted it.
const POOL_SIZE: usize = 6;

/// GBM on one GPU.
pub(crate) struct Allocator {
    gbm: gbm::Device<File>,
    pub device: GpuDevice,
}

impl Allocator {
    pub fn open(device: GpuDevice) -> Result<Self, Error> {
        let node = device.render_node.as_ref().ok_or_else(|| {
            Error::Unsupported(format!("no render node for GPU {}", device.describe()))
        })?;
        let file = File::options().read(true).write(true).open(node)?;
        let gbm = gbm::Device::new(file)?;
        Ok(Self { gbm, device })
    }
}

/// A capture stream's GPU buffers, all of one size and format.
pub(crate) struct Pool {
    pub width: u32,
    pub height: u32,
    buffers: Vec<Buffer>,
    /// Where to look for a free buffer next: in turn, so the one just let go of (whose
    /// last reads may still be on the GPU) is the last to be written again.
    next: usize,
}

struct Buffer {
    wl_buffer: WlBuffer,
    /// Keeps the memory alive (the planes' fds would too).
    _bo: gbm::BufferObject<()>,
    fourcc: u32,
    modifier: u64,
    planes: Vec<DmabufPlane>,
    id: u64,
    /// Held by frames handed out; clear when they're all gone.
    leased: Arc<AtomicBool>,
}

/// Returns its buffer to the pool when the last clone of a frame goes.
struct Lease(Arc<AtomicBool>);

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl Pool {
    pub fn new<D>(
        allocator: &Allocator,
        linux_dmabuf: &ZwpLinuxDmabufV1,
        (width, height): (u32, u32),
        format: &DmabufFormat,
        qh: &QueueHandle<D>,
    ) -> Result<Self, Error>
    where
        D: Dispatch<ZwpLinuxBufferParamsV1, ()> + Dispatch<WlBuffer, ()> + 'static,
    {
        let fourcc = gbm::Format::try_from(format.fourcc).map_err(|_| {
            Error::Unsupported(format!("unknown DRM format {}", fourcc_name(format.fourcc)))
        })?;
        let buffers = (0..POOL_SIZE)
            .map(|_| {
                allocate(
                    allocator,
                    linux_dmabuf,
                    (width, height),
                    fourcc,
                    &format.modifiers,
                    qh,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        tracing::debug!(
            width,
            height,
            format = fourcc_name(format.fourcc),
            modifier = format_args!("{:#x}", buffers[0].modifier),
            gpu = allocator.device.describe(),
            "GPU capture buffers"
        );
        Ok(Self {
            width,
            height,
            buffers,
            next: 0,
        })
    }

    /// The next buffer no frame holds.
    pub fn free(&mut self) -> Option<usize> {
        let n = self.buffers.len();
        let found = (0..n)
            .map(|k| (self.next + k) % n)
            .find(|&i| !self.buffers[i].leased.load(Ordering::Acquire))?;
        self.next = (found + 1) % n;
        Some(found)
    }

    pub fn wl_buffer(&self, index: usize) -> &WlBuffer {
        &self.buffers[index].wl_buffer
    }

    /// The captured frame in buffer `index`, leased until it's dropped.
    pub fn take(&self, index: usize, crop: Option<PixelRect>) -> Dmabuf {
        let buffer = &self.buffers[index];
        buffer.leased.store(true, Ordering::Release);
        Dmabuf {
            width: self.width,
            height: self.height,
            fourcc: buffer.fourcc,
            modifier: buffer.modifier,
            planes: buffer.planes.clone(),
            crop,
            buffer: buffer.id,
            lease: Arc::new(Lease(buffer.leased.clone())),
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // Frames still out keep their memory through the planes' fds.
        for buffer in &self.buffers {
            buffer.wl_buffer.destroy();
        }
    }
}

fn allocate<D>(
    allocator: &Allocator,
    linux_dmabuf: &ZwpLinuxDmabufV1,
    (width, height): (u32, u32),
    fourcc: gbm::Format,
    modifiers: &[u64],
    qh: &QueueHandle<D>,
) -> Result<Buffer, Error>
where
    D: Dispatch<ZwpLinuxBufferParamsV1, ()> + Dispatch<WlBuffer, ()> + 'static,
{
    let usage = gbm::BufferObjectFlags::RENDERING;
    let explicit: Vec<u64> = modifiers
        .iter()
        .copied()
        .filter(|&m| m != screenie_core::gpu::MODIFIER_INVALID)
        .collect();
    let bo = if explicit.is_empty() {
        allocator
            .gbm
            .create_buffer_object::<()>(width, height, fourcc, usage)
    } else {
        allocator.gbm.create_buffer_object_with_modifiers2::<()>(
            width,
            height,
            fourcc,
            explicit.iter().map(|&m| gbm::Modifier::from(m)),
            usage,
        )
    }
    .map_err(|e| Error::Capture(format!("allocating a {width}x{height} GPU buffer: {e}")))?;

    let modifier: u64 = bo.modifier().into();
    let params = linux_dmabuf.create_params(qh, ());
    let mut planes = Vec::new();
    for plane in 0..bo.plane_count() as i32 {
        let fd: OwnedFd = bo
            .fd_for_plane(plane)
            .map_err(|e| Error::Capture(format!("exporting a GPU buffer: {e}")))?;
        let (offset, stride) = (bo.offset(plane), bo.stride_for_plane(plane));
        params.add(
            fd.as_fd(),
            plane as u32,
            offset,
            stride,
            (modifier >> 32) as u32,
            modifier as u32,
        );
        planes.push(DmabufPlane {
            fd: Arc::new(fd),
            offset,
            stride,
        });
    }
    let wl_buffer = params.create_immed(
        width as i32,
        height as i32,
        fourcc as u32,
        zwp_linux_buffer_params_v1::Flags::empty(),
        qh,
        (),
    );
    params.destroy();
    Ok(Buffer {
        wl_buffer,
        _bo: bo,
        fourcc: fourcc as u32,
        modifier,
        planes,
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        leased: Arc::default(),
    })
}
