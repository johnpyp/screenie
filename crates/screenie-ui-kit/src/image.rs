//! Moving pixels between screenie's [`Image`] and GPUI.

use std::sync::Arc;

use gpui::RenderImage;
use screenie_core::{Image, PixelFormat};

/// A GPU-uploadable copy of `image`. GPUI wants BGRA.
pub fn render_image(image: &Image) -> Arc<RenderImage> {
    let bgra = image.convert(PixelFormat::Bgra);
    let (w, h) = (bgra.width(), bgra.height());
    let buffer = image::RgbaImage::from_raw(w, h, bgra.data()[..(w * h * 4) as usize].to_vec())
        .expect("buffer matches dimensions");
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
        buffer
    )]))
}
