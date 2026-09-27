//! CPU pixel buffers.
//!
//! Captured frames keep the byte order the compositor handed us (usually BGRx on
//! little-endian machines) so capture never pays for a conversion it may not need; the GTK
//! side can upload any of these formats as-is. Conversion happens at the edges: PNG export,
//! compositing buffers of different formats, and so on.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use crate::geom::PixelRect;

/// Byte order of a 4-byte pixel in memory. `x` variants carry an undefined padding byte
/// where the alpha would be and must be treated as opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra,
    Bgrx,
    Rgba,
    Rgbx,
}

impl PixelFormat {
    pub const BYTES_PER_PIXEL: usize = 4;

    pub fn has_alpha(self) -> bool {
        matches!(self, PixelFormat::Bgra | PixelFormat::Rgba)
    }

    fn is_bgr(self) -> bool {
        matches!(self, PixelFormat::Bgra | PixelFormat::Bgrx)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("png encode error: {0}")]
    Encode(#[from] png::EncodingError),
    #[error("png decode error: {0}")]
    Decode(#[from] png::DecodingError),
    #[error("unsupported image: {0}")]
    Unsupported(String),
}

/// An immutable-by-default 8-bit-per-channel image. Cloning is cheap (the pixels are
/// reference counted); mutation copies on write.
#[derive(Clone)]
pub struct Image {
    width: u32,
    height: u32,
    stride: usize,
    format: PixelFormat,
    data: Arc<Vec<u8>>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stride", &self.stride)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl Image {
    /// A fully transparent (or black, for `x` formats) image.
    pub fn new(width: u32, height: u32, format: PixelFormat) -> Self {
        let stride = width as usize * PixelFormat::BYTES_PER_PIXEL;
        Self {
            width,
            height,
            stride,
            format,
            data: Arc::new(vec![0; stride * height as usize]),
        }
    }

    /// Wrap raw pixel rows. Panics if `data` is too short for the geometry.
    pub fn from_raw(
        width: u32,
        height: u32,
        stride: usize,
        format: PixelFormat,
        data: Vec<u8>,
    ) -> Self {
        assert!(
            stride >= width as usize * PixelFormat::BYTES_PER_PIXEL,
            "stride too small"
        );
        assert!(
            data.len() >= stride * height.saturating_sub(1) as usize + width as usize * 4,
            "buffer too small"
        );
        Self {
            width,
            height,
            stride,
            format,
            data: Arc::new(data),
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Shared handle to the pixel bytes, for zero-copy hand-off (e.g. to a GPU texture).
    pub fn shared_data(&self) -> Arc<Vec<u8>> {
        self.data.clone()
    }

    pub fn bounds(&self) -> PixelRect {
        PixelRect::new(0, 0, self.width, self.height)
    }

    fn row(&self, y: u32) -> &[u8] {
        let start = y as usize * self.stride;
        &self.data[start..start + self.width as usize * 4]
    }

    fn data_mut(&mut self) -> &mut Vec<u8> {
        Arc::make_mut(&mut self.data)
    }

    /// The pixel at `(x, y)` as straight RGBA.
    pub fn rgba_at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = y as usize * self.stride + x as usize * 4;
        let p = &self.data[i..i + 4];
        to_rgba(self.format, [p[0], p[1], p[2], p[3]])
    }

    /// Copy out a sub-rectangle. The rectangle is clipped to the image bounds.
    pub fn crop(&self, rect: PixelRect) -> Image {
        let Some(r) = rect.intersection(&self.bounds()) else {
            return Image::new(0, 0, self.format);
        };
        let row_bytes = r.width as usize * 4;
        let mut data = Vec::with_capacity(row_bytes * r.height as usize);
        for y in r.y..r.bottom() {
            let start = y as usize * self.stride + r.x as usize * 4;
            data.extend_from_slice(&self.data[start..start + row_bytes]);
        }
        Image::from_raw(r.width, r.height, row_bytes, self.format, data)
    }

    /// The smallest rectangle holding every fully opaque pixel: a window without the drop
    /// shadow around it. The whole image if it has no alpha, `None` if nothing's opaque.
    pub fn opaque_bounds(&self) -> Option<PixelRect> {
        if !self.format.has_alpha() {
            return Some(self.bounds());
        }
        let opaque = |y: u32| {
            let row = self.row(y);
            let pixels = row.as_chunks::<4>().0;
            let first = pixels.iter().position(|p| p[3] == 255)?;
            let last = pixels.iter().rposition(|p| p[3] == 255)?;
            Some((first as u32, last as u32))
        };
        let top = (0..self.height).find(|&y| opaque(y).is_some())?;
        let bottom = (top..self.height).rev().find(|&y| opaque(y).is_some())?;
        let (left, right) = (top..=bottom)
            .filter_map(opaque)
            .fold((u32::MAX, 0), |(l, r), (first, last)| {
                (l.min(first), r.max(last))
            });
        Some(PixelRect::new(
            left as i32,
            top as i32,
            right - left + 1,
            bottom - top + 1,
        ))
    }

    /// Re-encode the pixels in another byte order. Padding bytes become opaque alpha.
    pub fn convert(&self, format: PixelFormat) -> Image {
        if format == self.format && self.stride == self.width as usize * 4 {
            return self.clone();
        }
        let mut data = Vec::with_capacity(self.width as usize * self.height as usize * 4);
        for y in 0..self.height {
            for &p in self.row(y).as_chunks::<4>().0 {
                data.extend_from_slice(&from_rgba(format, to_rgba(self.format, p)));
            }
        }
        Image::from_raw(
            self.width,
            self.height,
            self.width as usize * 4,
            format,
            data,
        )
    }

    /// Tightly packed straight-alpha RGBA bytes.
    pub fn to_rgba8(&self) -> Vec<u8> {
        self.convert(PixelFormat::Rgba).data.as_ref().clone()
    }

    /// Whether every pixel is fully opaque (always true for `x` formats).
    pub fn is_opaque(&self) -> bool {
        if !self.format.has_alpha() {
            return true;
        }
        (0..self.height).all(|y| self.row(y).as_chunks::<4>().0.iter().all(|p| p[3] == 255))
    }

    /// Copy `src` into this image with its top-left corner at `(x, y)`, converting formats
    /// as needed. Parts falling outside this image are clipped.
    pub fn blit(&mut self, src: &Image, x: i32, y: i32) {
        let dst_rect = PixelRect::new(x, y, src.width, src.height);
        let Some(clip) = dst_rect.intersection(&self.bounds()) else {
            return;
        };
        let (fmt, stride) = (self.format, self.stride);
        let same = src.format == fmt;
        let data = self.data_mut();
        for dy in clip.y..clip.bottom() {
            let sy = (dy - y) as u32;
            let sx = (clip.x - x) as usize;
            let src_row = &src.row(sy)[sx * 4..(sx + clip.width as usize) * 4];
            let start = dy as usize * stride + clip.x as usize * 4;
            let dst_row = &mut data[start..start + clip.width as usize * 4];
            if same {
                dst_row.copy_from_slice(src_row);
            } else {
                for (d, &s) in dst_row
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(src_row.as_chunks::<4>().0)
                {
                    *d = from_rgba(fmt, to_rgba(src.format, s));
                }
            }
        }
    }

    /// Draw `src` over this image, its top-left corner at `(x, y)`, clipped to this image.
    /// `src` is premultiplied, as Wayland's buffers (a cursor's picture) are; this image's
    /// alpha is kept as it was.
    pub fn draw_premultiplied(&mut self, src: &Image, x: i32, y: i32) {
        let Some(clip) = PixelRect::new(x, y, src.width, src.height).intersection(&self.bounds())
        else {
            return;
        };
        let (fmt, stride) = (self.format, self.stride);
        let data = self.data_mut();
        for dy in clip.y..clip.bottom() {
            let sx = (clip.x - x) as usize;
            let src_row = &src.row((dy - y) as u32)[sx * 4..(sx + clip.width as usize) * 4];
            let start = dy as usize * stride + clip.x as usize * 4;
            let dst_row = &mut data[start..start + clip.width as usize * 4];
            for (d, &s) in dst_row
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(src_row.as_chunks::<4>().0)
            {
                let s = to_rgba(src.format, s);
                match s[3] {
                    0 => continue,
                    255 => {
                        let kept = d[3];
                        *d = from_rgba(fmt, s);
                        d[3] = kept;
                    }
                    a => {
                        let under = to_rgba(fmt, *d);
                        let over = |c: usize| {
                            (s[c] as u32 + (under[c] as u32 * (255 - a as u32) + 127) / 255)
                                .min(255) as u8
                        };
                        let kept = d[3];
                        *d = from_rgba(fmt, [over(0), over(1), over(2), under[3]]);
                        d[3] = kept;
                    }
                }
            }
        }
    }

    /// Bilinear resample to a new size. Used when compositing outputs of different scales
    /// and for thumbnails; not meant for high-quality downscaling by large factors.
    pub fn resize(&self, width: u32, height: u32) -> Image {
        if width == self.width && height == self.height {
            return self.clone();
        }
        if width == 0 || height == 0 || self.width == 0 || self.height == 0 {
            return Image::new(width, height, self.format);
        }
        // Large reductions alias badly with a 2x2 kernel; halve first.
        if width * 2 < self.width && height * 2 < self.height {
            return self.halve().resize(width, height);
        }
        let mut data = vec![0u8; width as usize * height as usize * 4];
        let sx = self.width as f64 / width as f64;
        let sy = self.height as f64 / height as f64;
        for y in 0..height {
            let fy = ((y as f64 + 0.5) * sy - 0.5).clamp(0.0, (self.height - 1) as f64);
            let y0 = fy.floor() as u32;
            let y1 = (y0 + 1).min(self.height - 1);
            let ty = fy - y0 as f64;
            let (r0, r1) = (self.row(y0), self.row(y1));
            for x in 0..width {
                let fx = ((x as f64 + 0.5) * sx - 0.5).clamp(0.0, (self.width - 1) as f64);
                let x0 = fx.floor() as usize;
                let x1 = (x0 + 1).min(self.width as usize - 1);
                let tx = fx - x0 as f64;
                let out = (y as usize * width as usize + x as usize) * 4;
                for c in 0..4 {
                    let top = r0[x0 * 4 + c] as f64 * (1.0 - tx) + r0[x1 * 4 + c] as f64 * tx;
                    let bot = r1[x0 * 4 + c] as f64 * (1.0 - tx) + r1[x1 * 4 + c] as f64 * tx;
                    data[out + c] = (top * (1.0 - ty) + bot * ty).round() as u8;
                }
            }
        }
        Image::from_raw(width, height, width as usize * 4, self.format, data)
    }

    /// Box-filter to half size in each dimension.
    fn halve(&self) -> Image {
        let (w, h) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut data = vec![0u8; w as usize * h as usize * 4];
        for y in 0..h {
            let r0 = self.row((y * 2).min(self.height - 1));
            let r1 = self.row((y * 2 + 1).min(self.height - 1));
            for x in 0..w as usize {
                let (a, b) = (x * 8, (x * 8 + 4).min(r0.len() - 4));
                for c in 0..4 {
                    let sum =
                        r0[a + c] as u32 + r0[b + c] as u32 + r1[a + c] as u32 + r1[b + c] as u32;
                    data[(y as usize * w as usize + x) * 4 + c] = ((sum + 2) / 4) as u8;
                }
            }
        }
        Image::from_raw(w, h, w as usize * 4, self.format, data)
    }

    /// Encode as PNG. Opaque images are written as RGB, which is noticeably smaller.
    pub fn write_png<W: Write>(
        &self,
        out: W,
        compression: png::Compression,
    ) -> Result<(), ImageError> {
        let opaque = self.is_opaque();
        let mut encoder = png::Encoder::new(out, self.width, self.height);
        encoder.set_color(if opaque {
            png::ColorType::Rgb
        } else {
            png::ColorType::Rgba
        });
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(compression);
        let mut writer = encoder.write_header()?;
        let channels = if opaque { 3 } else { 4 };
        let mut packed = Vec::with_capacity(self.width as usize * self.height as usize * channels);
        for y in 0..self.height {
            for &p in self.row(y).as_chunks::<4>().0 {
                packed.extend_from_slice(&to_rgba(self.format, p)[..channels]);
            }
        }
        writer.write_image_data(&packed)?;
        writer.finish()?;
        Ok(())
    }

    pub fn to_png_bytes(&self) -> Result<Vec<u8>, ImageError> {
        let mut buf = Vec::new();
        self.write_png(&mut buf, png::Compression::Fast)?;
        Ok(buf)
    }

    /// Write a PNG to `path` atomically (via a temporary file in the same directory).
    pub fn save_png(&self, path: &Path) -> Result<(), ImageError> {
        let tmp = path.with_extension("png.part");
        {
            let file = std::fs::File::create(&tmp)?;
            let mut w = std::io::BufWriter::new(file);
            self.write_png(&mut w, png::Compression::Balanced)?;
            w.flush()?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Decode a PNG file into an RGBA image.
    pub fn load_png(path: &Path) -> Result<Image, ImageError> {
        let file = std::io::BufReader::new(std::fs::File::open(path)?);
        Self::decode_png(file)
    }

    pub fn decode_png<R: std::io::BufRead + std::io::Seek>(reader: R) -> Result<Image, ImageError> {
        let mut decoder = png::Decoder::new(reader);
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info()?;
        let mut buf = vec![
            0;
            reader
                .output_buffer_size()
                .ok_or_else(|| ImageError::Unsupported("image too large".into()))?
        ];
        let info = reader.next_frame(&mut buf)?;
        let (w, h) = (info.width, info.height);
        let px = w as usize * h as usize;
        let mut rgba = Vec::with_capacity(px * 4);
        match info.color_type {
            png::ColorType::Rgba => rgba.extend_from_slice(&buf[..px * 4]),
            png::ColorType::Rgb => buf[..px * 3]
                .as_chunks::<3>()
                .0
                .iter()
                .for_each(|&[r, g, b]| rgba.extend_from_slice(&[r, g, b, 255])),
            png::ColorType::GrayscaleAlpha => buf[..px * 2]
                .as_chunks::<2>()
                .0
                .iter()
                .for_each(|&[g, a]| rgba.extend_from_slice(&[g, g, g, a])),
            png::ColorType::Grayscale => buf[..px]
                .iter()
                .for_each(|&g| rgba.extend_from_slice(&[g, g, g, 255])),
            other => return Err(ImageError::Unsupported(format!("png color type {other:?}"))),
        }
        Ok(Image::from_raw(
            w,
            h,
            w as usize * 4,
            PixelFormat::Rgba,
            rgba,
        ))
    }
}

fn to_rgba(format: PixelFormat, p: [u8; 4]) -> [u8; 4] {
    let a = if format.has_alpha() { p[3] } else { 255 };
    if format.is_bgr() {
        [p[2], p[1], p[0], a]
    } else {
        [p[0], p[1], p[2], a]
    }
}

fn from_rgba(format: PixelFormat, p: [u8; 4]) -> [u8; 4] {
    let a = if format.has_alpha() { p[3] } else { 255 };
    if format.is_bgr() {
        [p[2], p[1], p[0], a]
    } else {
        [p[0], p[1], p[2], a]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_bounds_leave_the_shadow_out() {
        let mut data = vec![0u8; 6 * 5 * 4];
        // A faint shadow pixel, and an opaque 3×2 block at (2, 1).
        data[3] = 40;
        for y in 1..3 {
            for x in 2..5 {
                data[(y * 6 + x) * 4 + 3] = 255;
            }
        }
        let image = Image::from_raw(6, 5, 24, PixelFormat::Bgra, data);
        assert_eq!(image.opaque_bounds(), Some(PixelRect::new(2, 1, 3, 2)));
        let clear = Image::new(4, 4, PixelFormat::Bgra);
        assert_eq!(clear.opaque_bounds(), None);
        let opaque = Image::new(4, 4, PixelFormat::Bgrx);
        assert_eq!(opaque.opaque_bounds(), Some(opaque.bounds()));
    }

    #[test]
    fn premultiplied_pixels_draw_over() {
        // A white background, and a 2x1 cursor: opaque red, then half-transparent black
        // (premultiplied: all zero but alpha).
        let mut image = Image::from_raw(3, 1, 12, PixelFormat::Bgrx, vec![255; 12]);
        let cursor = Image::from_raw(
            2,
            1,
            8,
            PixelFormat::Bgra,
            vec![0, 0, 255, 255, 0, 0, 0, 128],
        );
        image.draw_premultiplied(&cursor, 1, 0);
        assert_eq!(image.rgba_at(0, 0), [255, 255, 255, 255]);
        assert_eq!(image.rgba_at(1, 0), [255, 0, 0, 255]);
        assert_eq!(image.rgba_at(2, 0), [127, 127, 127, 255]);
        // Off the edge: clipped, not a panic.
        image.draw_premultiplied(&cursor, 2, 0);
        image.draw_premultiplied(&cursor, -5, 3);
        assert_eq!(image.rgba_at(2, 0), [255, 0, 0, 255]);
    }

    fn gradient(w: u32, h: u32) -> Image {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&[x as u8, y as u8, 7, 0]); // BGRx
            }
        }
        Image::from_raw(w, h, w as usize * 4, PixelFormat::Bgrx, data)
    }

    #[test]
    fn crop_and_convert() {
        let img = gradient(10, 10);
        let c = img.crop(PixelRect::new(2, 3, 4, 5));
        assert_eq!((c.width(), c.height()), (4, 5));
        assert_eq!(c.rgba_at(0, 0), [7, 3, 2, 255]);
        let rgba = c.convert(PixelFormat::Rgba);
        assert_eq!(&rgba.data()[..4], &[7, 3, 2, 255]);
    }

    #[test]
    fn crop_clips_to_bounds() {
        let img = gradient(10, 10);
        let c = img.crop(PixelRect::new(8, 8, 10, 10));
        assert_eq!((c.width(), c.height()), (2, 2));
    }

    #[test]
    fn blit_converts_format() {
        let mut dst = Image::new(4, 4, PixelFormat::Rgba);
        dst.blit(&gradient(2, 2), 1, 1);
        assert_eq!(dst.rgba_at(2, 2), [7, 1, 1, 255]);
        assert_eq!(dst.rgba_at(0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn png_roundtrip() {
        let img = gradient(16, 9);
        let bytes = img.to_png_bytes().unwrap();
        let back = Image::decode_png(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!((back.width(), back.height()), (16, 9));
        assert_eq!(back.rgba_at(5, 4), img.rgba_at(5, 4));
    }

    #[test]
    fn resize_preserves_flat_color() {
        let data = [10u8, 20, 30, 255].repeat(100 * 50);
        let img = Image::from_raw(100, 50, 400, PixelFormat::Rgba, data);
        let small = img.resize(13, 7);
        assert_eq!(small.rgba_at(6, 3), [10, 20, 30, 255]);
    }
}
