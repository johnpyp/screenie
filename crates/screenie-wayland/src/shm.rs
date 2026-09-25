//! Shared-memory buffers the compositor copies frames into, and conversion of their
//! contents into upright [`Image`]s.

use std::fs::File;

use memmap2::MmapMut;
use rustix::fs::{MemfdFlags, memfd_create};
use screenie_core::{Image, PixelFormat, Transform};
use wayland_client::protocol::{wl_buffer::WlBuffer, wl_shm, wl_shm::WlShm};
use wayland_client::{Dispatch, QueueHandle};

use crate::Error;

/// Formats we can read, in order of preference. 32-bit formats convert for free (they
/// are already an [`Image`] layout). Packed 24-bit and 10-bit formats need a pass; some
/// compositors (e.g. Hyprland's screencopy) only offer the output's native format, which
/// can be either.
pub(crate) const SUPPORTED_FORMATS: &[wl_shm::Format] = &[
    wl_shm::Format::Xrgb8888,
    wl_shm::Format::Argb8888,
    wl_shm::Format::Xbgr8888,
    wl_shm::Format::Abgr8888,
    wl_shm::Format::Rgb888,
    wl_shm::Format::Bgr888,
    wl_shm::Format::Xrgb2101010,
    wl_shm::Format::Argb2101010,
    wl_shm::Format::Xbgr2101010,
    wl_shm::Format::Abgr2101010,
];

/// Pick the most preferred format from those the compositor offers.
pub(crate) fn choose_format(offered: &[wl_shm::Format]) -> Option<wl_shm::Format> {
    SUPPORTED_FORMATS.iter().copied().find(|f| offered.contains(f))
}

/// Bytes per pixel of a supported format.
pub(crate) fn bytes_per_pixel(format: wl_shm::Format) -> u32 {
    match format {
        wl_shm::Format::Rgb888 | wl_shm::Format::Bgr888 => 3,
        _ => 4,
    }
}

/// How a supported format's pixels are laid out in memory. (wl_shm formats are DRM
/// fourccs: components listed high bit to low, stored little-endian.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// 4 bytes, already an [`Image`] byte order.
    Native,
    /// 3 bytes, already in [`Image`] order minus the padding byte.
    Packed24,
    /// 32-bit words of 2:10:10:10. `bgr`: blue in the high bits.
    Deep { bgr: bool, alpha: bool },
}

fn layout(format: wl_shm::Format) -> Layout {
    match format {
        wl_shm::Format::Rgb888 | wl_shm::Format::Bgr888 => Layout::Packed24,
        wl_shm::Format::Xrgb2101010 => Layout::Deep { bgr: false, alpha: false },
        wl_shm::Format::Argb2101010 => Layout::Deep { bgr: false, alpha: true },
        wl_shm::Format::Xbgr2101010 => Layout::Deep { bgr: true, alpha: false },
        wl_shm::Format::Abgr2101010 => Layout::Deep { bgr: true, alpha: true },
        _ => Layout::Native,
    }
}

/// The [`PixelFormat`] of the image decoded from a buffer of this shm format.
fn pixel_format(format: wl_shm::Format) -> PixelFormat {
    // Screen contents are opaque even when the format carries alpha (compositors fill it
    // with garbage or zeroes on some drivers), so treat everything as padded.
    match format {
        // R in the low byte: R,G,B(,x) in memory.
        wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888 | wl_shm::Format::Bgr888 => PixelFormat::Rgbx,
        _ => PixelFormat::Bgrx,
    }
}

/// Decode raw buffer memory into an 8-bit [`Image`], flipping rows if `y_invert`.
fn decode(format: wl_shm::Format, mem: &[u8], width: u32, height: u32, stride: usize, y_invert: bool) -> Image {
    let (w, h) = (width as usize, height as usize);
    let row_bytes = w * bytes_per_pixel(format) as usize;
    let layout = layout(format);
    let mut data = Vec::with_capacity(w * 4 * h);
    for y in 0..h {
        let src_y = if y_invert { h - 1 - y } else { y };
        let row = &mem[src_y * stride..src_y * stride + row_bytes];
        match layout {
            Layout::Native => data.extend_from_slice(row),
            Layout::Packed24 => {
                for &[a, b, c] in row.as_chunks::<3>().0 {
                    data.extend_from_slice(&[a, b, c, 255]);
                }
            }
            Layout::Deep { bgr, alpha } => {
                for &px in row.as_chunks::<4>().0 {
                    let v = u32::from_le_bytes(px);
                    let hi = ((v >> 20) & 0x3ff) >> 2;
                    let mid = ((v >> 10) & 0x3ff) >> 2;
                    let lo = (v & 0x3ff) >> 2;
                    let a = if alpha { (((v >> 30) & 0x3) * 85) as u8 } else { 255 };
                    // Written out in our Bgrx order: xRGB has red high, xBGR blue high.
                    let (b, r) = if bgr { (hi, lo) } else { (lo, hi) };
                    data.extend_from_slice(&[b as u8, mid as u8, r as u8, a]);
                }
            }
        }
    }
    let out_format = if matches!(layout, Layout::Deep { .. }) { PixelFormat::Bgrx } else { pixel_format(format) };
    Image::from_raw(width, height, w * 4, out_format, data)
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
        decode(self.format, &self.map, self.width, self.height, self.stride as usize, y_invert)
    }
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        self.wl_buffer.destroy();
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

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// A 2x2 image, top row red, bottom row blue, stored with padded rows.
    fn buffer(format: wl_shm::Format, red: &[u8], blue: &[u8]) -> Vec<u8> {
        let stride = 2 * bytes_per_pixel(format) as usize + 4;
        let mut mem = vec![0xee; stride * 2];
        for (y, px) in [red, blue].into_iter().enumerate() {
            for x in 0..2 {
                let at = y * stride + x * px.len();
                mem[at..at + px.len()].copy_from_slice(px);
            }
        }
        mem
    }

    fn check(format: wl_shm::Format, red: &[u8], blue: &[u8]) {
        let bpp = bytes_per_pixel(format) as usize;
        let mem = buffer(format, red, blue);
        let image = decode(format, &mem, 2, 2, 2 * bpp + 4, false);
        assert_eq!(image.rgba_at(1, 0), RED, "{format:?} top row");
        assert_eq!(image.rgba_at(0, 1), BLUE, "{format:?} bottom row");
        let flipped = decode(format, &mem, 2, 2, 2 * bpp + 4, true);
        assert_eq!(flipped.rgba_at(0, 0), BLUE, "{format:?} y-invert");
    }

    #[test]
    fn decodes_every_supported_format() {
        use wl_shm::Format::*;
        // Memory bytes of one red and one blue pixel in each format.
        check(Xrgb8888, &[0, 0, 255, 0], &[255, 0, 0, 0]);
        check(Argb8888, &[0, 0, 255, 255], &[255, 0, 0, 255]);
        check(Xbgr8888, &[255, 0, 0, 0], &[0, 0, 255, 0]);
        check(Abgr8888, &[255, 0, 0, 255], &[0, 0, 255, 255]);
        check(Rgb888, &[0, 0, 255], &[255, 0, 0]);
        check(Bgr888, &[255, 0, 0], &[0, 0, 255]);
        let deep = |hi: u32, lo: u32| ((3 << 30) | (hi << 20) | lo).to_le_bytes();
        check(Xrgb2101010, &deep(0x3ff, 0), &deep(0, 0x3ff));
        check(Xbgr2101010, &deep(0, 0x3ff), &deep(0x3ff, 0));
        check(Argb2101010, &deep(0x3ff, 0), &deep(0, 0x3ff));
        check(Abgr2101010, &deep(0, 0x3ff), &deep(0x3ff, 0));
        assert_eq!(SUPPORTED_FORMATS.len(), 10, "every supported format is covered above");
    }
}
