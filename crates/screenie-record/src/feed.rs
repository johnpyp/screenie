//! Frames on their way into the pipeline: the appsrc end.
//!
//! A frame in memory is wrapped without a copy (letterboxed first if its aspect ratio
//! strayed from the video's). A frame in a GPU buffer is wrapped as DMA-BUF memory, which
//! the encoder's chain imports on the GPU. Its buffer goes back to the capture pool when
//! the pipeline lets go of the frame.
//!
//! Each frame is stamped with when the compositor presented it, so the video's timing is
//! the screen's, however unevenly frames reach us.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::Duration;

use gst::prelude::*;
use gst_allocators::prelude::*;
use screenie_core::{Dmabuf, Frame, Image, Pixels, PixelFormat};

use crate::encoder::drm_format_string;
use crate::{even_size, letterbox};

/// Raw frames the appsrc holds before the pipeline counts as behind.
pub(crate) const PIPELINE_FRAMES: u64 = 2;

pub(crate) struct Feed {
    pub appsrc: gst_app::AppSrc,
    pub fps: u32,
    /// The video's size.
    pub size: (u32, u32),
    caps: Option<gst::Caps>,
    pub pushed: u64,
    pub pushed_gpu: u64,
    last_pts: Option<gst::ClockTime>,
    /// DMA-BUF memories per capture buffer, made once: importers cache what they made
    /// of a memory (VA surfaces, GL textures), so the same buffer imports once.
    memories: HashMap<u64, Vec<gst::Memory>>,
    allocator: Option<gst_allocators::DmaBufAllocator>,
}

impl Feed {
    pub fn new(appsrc: gst_app::AppSrc, fps: u32, size: (u32, u32)) -> Self {
        Self {
            appsrc,
            fps,
            size,
            caps: None,
            pushed: 0,
            pushed_gpu: 0,
            last_pts: None,
            memories: HashMap::new(),
            allocator: None,
        }
    }

    /// Whether the pipeline can take another frame now.
    pub fn has_room(&self) -> bool {
        self.appsrc.property::<u64>("current-level-buffers") < PIPELINE_FRAMES
    }

    pub fn push(&mut self, frame: &Frame) {
        let pts = self.timestamp(frame.presented);
        let buffer = match &frame.pixels {
            Pixels::Cpu(image) => self.cpu_buffer(image),
            Pixels::Gpu(dmabuf) => self.gpu_buffer(dmabuf),
        };
        let Some(mut buffer) = buffer else { return };
        buffer.get_mut().expect("fresh buffer").set_pts(pts);
        self.pushed += 1;
        // Only fails when flushing or at EOS, i.e. while stopping.
        let _ = self.appsrc.push_buffer(buffer);
    }

    /// The running time to stamp a frame with: when it was presented, if the compositor
    /// said (on the pipeline's clock, CLOCK_MONOTONIC), else now. Always after the last.
    fn timestamp(&mut self, presented: Option<Duration>) -> Option<gst::ClockTime> {
        let clock = self.appsrc.clock()?;
        let base = self.appsrc.base_time()?;
        let now = clock.time();
        let at = presented
            .map(|p| gst::ClockTime::from_nseconds(p.as_nanos() as u64))
            // A compositor on another clock would be way off: ignore it.
            .filter(|p| p.nseconds().abs_diff(now.nseconds()) < gst::ClockTime::SECOND.nseconds())
            .unwrap_or(now);
        let mut pts = at.saturating_sub(base);
        if let Some(last) = self.last_pts {
            pts = pts.max(last + gst::ClockTime::from_mseconds(1));
        }
        self.last_pts = Some(pts);
        Some(pts)
    }

    fn set_caps(&mut self, caps: gst::Caps) {
        if self.caps.as_ref() != Some(&caps) {
            if self.caps.is_some() {
                tracing::debug!(%caps, "recorded frames changed");
            }
            self.appsrc.set_caps(Some(&caps));
            self.caps = Some(caps);
        }
    }

    /// A frame in memory, cropped to even dimensions by describing its layout.
    fn cpu_buffer(&mut self, image: &Image) -> Option<gst::Buffer> {
        let padded = letterbox(image, self.size);
        let image = padded.as_ref().unwrap_or(image);
        let format = video_format(image.format());
        let (width, height) = even_size(image.width(), image.height());
        if width < 2 || height < 2 {
            return None;
        }
        let caps = gst_video::VideoInfo::builder(format, width, height)
            .fps(gst::Fraction::new(self.fps as i32, 1))
            .build()
            .and_then(|info| info.to_caps())
            .map_err(|e| tracing::warn!("cannot describe a {width}x{height} frame: {e}"))
            .ok()?;
        self.set_caps(caps);
        let mut buffer = gst::Buffer::from_slice(Borrowed(image.shared_data()));
        if image.stride() != width as usize * 4 {
            let meta = gst_video::VideoMeta::add_full(
                buffer.get_mut().expect("fresh buffer"),
                gst_video::VideoFrameFlags::empty(),
                format,
                width,
                height,
                &[0],
                &[image.stride() as i32],
            );
            if let Err(e) = meta {
                tracing::warn!("cannot describe frame layout: {e}");
                return None;
            }
        }
        Some(buffer)
    }

    /// A frame in a GPU buffer, as DMA-BUF memory (one per plane) with its layout and
    /// crop described, holding the capture buffer until the pipeline is done with it.
    fn gpu_buffer(&mut self, frame: &Dmabuf) -> Option<gst::Buffer> {
        let caps = gst::Caps::builder("video/x-raw")
            .features(["memory:DMABuf"])
            .field("format", "DMA_DRM")
            .field("drm-format", drm_format_string(frame.fourcc, frame.modifier))
            .field("width", frame.width as i32)
            .field("height", frame.height as i32)
            .field("framerate", gst::Fraction::new(self.fps as i32, 1))
            .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
            .build();
        self.set_caps(caps);

        if self.memories.len() > 16 {
            // The capture reallocated its buffers (a resize): forget the old ones.
            self.memories.clear();
        }
        let allocator = self.allocator.get_or_insert_with(gst_allocators::DmaBufAllocator::new).clone();
        if let Entry::Vacant(slot) = self.memories.entry(frame.buffer) {
            let memories = frame
                .planes
                .iter()
                .map(|plane| {
                    let fd = plane.fd.try_clone().ok()?;
                    let size = dmabuf_size(&fd)?;
                    // SAFETY: the fd is a DMA-BUF of `size` bytes, handed over (a dup).
                    unsafe { allocator.alloc_dmabuf(fd, size).ok() }
                })
                .collect::<Option<Vec<_>>>();
            let Some(memories) = memories else {
                tracing::warn!("cannot wrap a GPU buffer for the pipeline");
                return None;
            };
            slot.insert(memories);
        }
        let mut buffer = gst::Buffer::new();
        let buf = buffer.get_mut().expect("fresh buffer");
        let mut offsets = Vec::new();
        let mut before = 0;
        for (plane, memory) in frame.planes.iter().zip(&self.memories[&frame.buffer]) {
            offsets.push(before + plane.offset as usize);
            before += memory.size();
            buf.append_memory(memory.clone());
        }
        let strides: Vec<i32> = frame.planes.iter().map(|p| p.stride as i32).collect();
        // The meta's format isn't what DMA_DRM caps describe; importers read the planes'
        // offsets and strides from it.
        let meta = gst_video::VideoMeta::add_full(
            buf,
            gst_video::VideoFrameFlags::empty(),
            gst_video::VideoFormat::Bgrx,
            frame.width,
            frame.height,
            &offsets,
            &strides,
        );
        if let Err(e) = meta {
            tracing::warn!("cannot describe a GPU frame's layout: {e}");
            return None;
        }
        if let Some(crop) = frame.crop {
            let (width, height) = even_size(crop.width, crop.height);
            gst_video::VideoCropMeta::add(buf, (crop.x.max(0) as u32, crop.y.max(0) as u32, width, height));
        }
        hold_until_done(buf, frame.lease.clone());
        self.pushed_gpu += 1;
        Some(buffer)
    }
}

/// Keep the capture buffer out of the pool until the pipeline lets go of `buffer`: when
/// the chain's first element has converted it. (Not a parent-buffer meta: converters
/// copy metas onto their output, which would hold it until the encoder is done too.)
fn hold_until_done(buffer: &mut gst::BufferRef, lease: Arc<dyn Send + Sync>) {
    unsafe extern "C" fn release(data: gst::glib::ffi::gpointer) {
        // SAFETY: `data` is the box leaked below, released once.
        drop(unsafe { Box::from_raw(data as *mut Arc<dyn Send + Sync>) });
    }
    let quark = gst::glib::Quark::from_str("screenie-capture-lease");
    let data = Box::into_raw(Box::new(lease));
    // SAFETY: a GstBuffer starts with its GstMiniObject; GStreamer calls `release` once,
    // when the buffer is freed.
    unsafe {
        gst::ffi::gst_mini_object_set_qdata(
            buffer.as_mut_ptr() as *mut gst::ffi::GstMiniObject,
            gst::glib::translate::IntoGlib::into_glib(quark),
            data as gst::glib::ffi::gpointer,
            Some(release),
        );
    }
}

/// A DMA-BUF's size, from seeking to its end.
fn dmabuf_size(fd: &std::os::fd::OwnedFd) -> Option<usize> {
    let size = rustix::fs::seek(fd, rustix::fs::SeekFrom::End(0)).ok()?;
    let _ = rustix::fs::seek(fd, rustix::fs::SeekFrom::Start(0));
    Some(size as usize)
}

/// Lets a GStreamer buffer borrow an image's pixels without copying.
struct Borrowed(Arc<Vec<u8>>);

impl AsRef<[u8]> for Borrowed {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

fn video_format(format: PixelFormat) -> gst_video::VideoFormat {
    match format {
        PixelFormat::Bgra => gst_video::VideoFormat::Bgra,
        PixelFormat::Bgrx => gst_video::VideoFormat::Bgrx,
        PixelFormat::Rgba => gst_video::VideoFormat::Rgba,
        PixelFormat::Rgbx => gst_video::VideoFormat::Rgbx,
    }
}
