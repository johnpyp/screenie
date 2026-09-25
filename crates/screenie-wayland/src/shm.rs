//! Shared-memory buffers the compositor copies frames into, and conversion of their
//! contents into upright [`Image`]s.

use std::fs::File;

use memmap2::MmapMut;
use rustix::fs::{MemfdFlags, memfd_create};
use screenie_core::{Image, PixelFormat, Transform};
use wayland_client::protocol::{wl_buffer::WlBuffer, wl_shm, wl_shm::WlShm};
use wayland_client::{Dispatch, QueueHandle};

use crate::Error;

/// Formats we can read, in order of preference. 8-bit formats convert for free (they are
/// already an [`Image`] layout); 10-bit ones appear on HDR/deep-color outputs, where some
/// compositors only offer the output's native format.
pub(crate) const SUPPORTED_FORMATS: &[wl_shm::Format] = &[
    wl_shm::Format::Xrgb8888,
    wl_shm::Format::Argb8888,
    wl_shm::Format::Xbgr8888,
    wl_shm::Format::Abgr8888,
    wl_shm::Format::Xrgb2101010,
    wl_shm::Format::Argb2101010,
    wl_shm::Format::Xbgr2101010,
    wl_shm::Format::Abgr2101010,
];

/// Pick the most preferred format from those the compositor offers.
pub(crate) fn choose_format(offered: &[wl_shm::Format]) -> Option<wl_shm::Format> {
    SUPPORTED_FORMATS.iter().copied().find(|f| offered.contains(f))
}

pub(crate) struct ShmBuffer {
    pub wl_buffer: WlBuffer,
    map: MmapMut,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: wl_shm::Format,
}

impl ShmBuffer {
    pub fn new<D>(
        shm: &WlShm,
        width: u32,
        height: u32,
        stride: u32,
        format: wl_shm::Format,
        qh: &QueueHandle<D>,
    ) -> Result<Self, Error>
    where
        D: Dispatch<wayland_client::protocol::wl_shm_pool::WlShmPool, ()> + Dispatch<WlBuffer, ()> + 'static,
    {
        let size = stride as usize * height as usize;
        let fd = memfd_create("screenie-frame", MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)
            .map_err(|e| Error::Io(e.into()))?;
        let file = File::from(fd);
        file.set_len(size as u64)?;
        // SAFETY: the memfd is private to us and the compositor, and we only read it
        // after the compositor signals the copy is complete.
        let map = unsafe { MmapMut::map_mut(&file)? };
        use std::os::fd::AsFd;
        let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
        let wl_buffer = pool.create_buffer(0, width as i32, height as i32, stride as i32, format, qh, ());
        pool.destroy();
        Ok(Self { wl_buffer, map, width, height, stride, format })
    }

    pub fn matches(&self, width: u32, height: u32, format: wl_shm::Format) -> bool {
        self.width == width && self.height == height && self.format == format
    }

    /// Convert the buffer contents into an upright image, applying the vertical flip and
    /// output transform the compositor reported.
    pub fn to_image(&self, y_invert: bool, transform: Transform) -> Image {
        let raw = self.read(y_invert);
        transform_image(&raw, transform)
    }

    fn read(&self, y_invert: bool) -> Image {
        let (w, h, stride) = (self.width, self.height, self.stride as usize);
        let row_bytes = w as usize * 4;
        let mut data = Vec::with_capacity(row_bytes * h as usize);
        let deep = deep_color_layout(self.format);
        for y in 0..h as usize {
            let src_y = if y_invert { h as usize - 1 - y } else { y };
            let row = &self.map[src_y * stride..src_y * stride + row_bytes];
            match deep {
                None => data.extend_from_slice(row),
                Some(bgr_order) => {
                    for px in row.as_chunks::<4>().0 {
                        let v = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
                        let hi = ((v >> 20) & 0x3ff) >> 2;
                        let mid = ((v >> 10) & 0x3ff) >> 2;
                        let lo = (v & 0x3ff) >> 2;
                        let a = if has_alpha(self.format) { (((v >> 30) & 0x3) * 85) as u8 } else { 255 };
                        // xRGB2101010 stores R in the high bits: B,G,R,A bytes when packed
                        // as our Bgra layout. xBGR is the reverse.
                        let (b, r) = if bgr_order { (hi, lo) } else { (lo, hi) };
                        data.extend_from_slice(&[b as u8, mid as u8, r as u8, a]);
                    }
                }
            }
        }
        Image::from_raw(w, h, row_bytes, pixel_format(self.format), data)
    }
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        self.wl_buffer.destroy();
    }
}

fn has_alpha(format: wl_shm::Format) -> bool {
    matches!(
        format,
        wl_shm::Format::Argb8888 | wl_shm::Format::Abgr8888 | wl_shm::Format::Argb2101010 | wl_shm::Format::Abgr2101010
    )
}

/// For 10-bit formats, whether the high component is blue (xBGR); `None` for 8-bit.
fn deep_color_layout(format: wl_shm::Format) -> Option<bool> {
    match format {
        wl_shm::Format::Xrgb2101010 | wl_shm::Format::Argb2101010 => Some(false),
        wl_shm::Format::Xbgr2101010 | wl_shm::Format::Abgr2101010 => Some(true),
        _ => None,
    }
}

/// The [`PixelFormat`] of the image produced from a buffer of this shm format.
fn pixel_format(format: wl_shm::Format) -> PixelFormat {
    // Screen contents are opaque even when the format carries alpha (compositors fill it
    // with garbage or zeroes on some drivers), so treat everything as padded.
    match format {
        wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888 => PixelFormat::Rgbx,
        _ => PixelFormat::Bgrx,
    }
}

/// Undo an output transform: turn buffer-oriented pixels into the upright image the user
/// sees. `transform` is the `wl_output.transform` of the output (or the transform the
/// capture protocol reports as applied to the buffer).
pub fn transform_image(src: &Image, transform: Transform) -> Image {
    if transform == Transform::Normal {
        return src.clone();
    }
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (dw, dh) = if transform.swaps_axes() { (sh, sw) } else { (sw, sh) };
    let s = src.data();
    let stride = src.stride();
    let mut out = vec![0u8; dw * dh * 4];
    // wl_output transforms rotate counter-clockwise; the buffer holds content rotated by
    // the transform, so to display it upright we map each destination pixel back to its
    // source position.
    for dy in 0..dh {
        for dx in 0..dw {
            let (sx, sy) = match transform {
                Transform::Normal => (dx, dy),
                Transform::Rotate90 => (dy, sh - 1 - dx),
                Transform::Rotate180 => (sw - 1 - dx, sh - 1 - dy),
                Transform::Rotate270 => (sw - 1 - dy, dx),
                Transform::Flipped => (sw - 1 - dx, dy),
                Transform::Flipped90 => (dy, dx),
                Transform::Flipped180 => (dx, sh - 1 - dy),
                Transform::Flipped270 => (sw - 1 - dy, sh - 1 - dx),
            };
            let si = sy * stride + sx * 4;
            let di = (dy * dw + dx) * 4;
            out[di..di + 4].copy_from_slice(&s[si..si + 4]);
        }
    }
    Image::from_raw(dw as u32, dh as u32, dw * 4, src.format(), out)
}
