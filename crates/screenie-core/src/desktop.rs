//! What the desktop looked like at one instant: outputs, their pixels, and the windows on
//! them. A [`Snapshot`] is taken before any capture UI is shown so the UI never ends up
//! in the picture, and every crop is served from it.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::geom::{PixelRect, Point, Rect, Transform};
use crate::image::{Image, PixelFormat};

/// A monitor as the compositor lays it out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputInfo {
    /// Connector name, e.g. `DP-1`. Stable across sessions; what users configure.
    pub name: String,
    /// Human readable make/model, when the compositor provides it.
    pub description: String,
    /// Position and size in the global logical layout.
    pub logical: Rect,
    /// Integer or fractional scale the compositor reports. Informational: the effective
    /// scale of a capture is derived from its pixel size (see [`OutputCapture::scale`]).
    pub scale: f64,
    pub transform: Transform,
}

/// A window, as reported by the compositor's IPC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Compositor-specific identifier.
    pub id: String,
    pub title: String,
    pub app_id: String,
    /// Global logical geometry, excluding server-side decorations where possible.
    pub rect: Rect,
    pub focused: bool,
    pub floating: bool,
    /// Its `ext-foreign-toplevel-list` identifier, where the compositor's IPC reports it,
    /// for capturing the window itself.
    #[serde(default)]
    pub toplevel: Option<String>,
}

/// One output's frozen pixels. `image` is upright (output transform already applied).
#[derive(Debug, Clone)]
pub struct OutputCapture {
    pub output: OutputInfo,
    pub image: Image,
}

impl OutputCapture {
    /// Physical pixels per logical pixel, measured from the captured buffer.
    pub fn scale(&self) -> f64 {
        if self.output.logical.width <= 0.0 {
            return 1.0;
        }
        self.image.width() as f64 / self.output.logical.width
    }
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub outputs: Vec<OutputCapture>,
    /// Visible windows, topmost first.
    pub windows: Vec<WindowInfo>,
    pub taken_at: SystemTime,
}

impl Snapshot {
    /// Bounding box of all outputs in the logical layout.
    pub fn layout_bounds(&self) -> Rect {
        self.outputs
            .iter()
            .map(|o| o.output.logical)
            .reduce(|a, b| a.union(&b))
            .unwrap_or_default()
    }

    pub fn output_named(&self, name: &str) -> Option<&OutputCapture> {
        self.outputs.iter().find(|o| o.output.name == name)
    }

    pub fn output_at(&self, p: Point) -> Option<&OutputCapture> {
        self.outputs.iter().find(|o| o.output.logical.contains(p))
    }

    /// The topmost window containing `p`.
    pub fn window_at(&self, p: Point) -> Option<&WindowInfo> {
        self.windows.iter().find(|w| w.rect.contains(p))
    }

    /// Render the logical `region` to an image. Where the region spans outputs of
    /// different scales it is rendered at the highest one, so no source pixels are lost;
    /// areas outside every output are transparent.
    pub fn render_region(&self, region: Rect) -> Image {
        let hits: Vec<_> = self
            .outputs
            .iter()
            .filter_map(|o| region.intersection(&o.output.logical).map(|i| (o, i)))
            .collect();
        let Some(scale) = hits.iter().map(|(o, _)| o.scale()).reduce(f64::max) else {
            return Image::new(0, 0, PixelFormat::Rgba);
        };

        let target = region.to_pixels(region.origin(), scale);
        // A region inside a single output keeps its native format (usually opaque BGRx).
        if let [(o, inter)] = hits.as_slice()
            && inter == &region
        {
            return o
                .image
                .crop(region.to_pixels(o.output.logical.origin(), o.scale()));
        }

        let mut canvas = Image::new(target.width, target.height, PixelFormat::Rgba);
        for (o, inter) in hits {
            let src = inter.to_pixels(o.output.logical.origin(), o.scale());
            let dst = inter.to_pixels(region.origin(), scale);
            let mut piece = o.image.crop(src);
            if (piece.width(), piece.height()) != (dst.width, dst.height) {
                piece = piece.resize(dst.width, dst.height);
            }
            canvas.blit(&piece, dst.x, dst.y);
        }
        canvas
    }

    /// Physical pixel size `render_region` would produce, without rendering.
    pub fn region_pixel_size(&self, region: Rect) -> (u32, u32) {
        let scale = self
            .outputs
            .iter()
            .filter(|o| region.intersection(&o.output.logical).is_some())
            .map(|o| o.scale())
            .reduce(f64::max)
            .unwrap_or(1.0);
        let px: PixelRect = region.to_pixels(region.origin(), scale);
        (px.width, px.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Image {
        Image::from_raw(
            w,
            h,
            w as usize * 4,
            PixelFormat::Rgba,
            rgba.repeat((w * h) as usize),
        )
    }

    fn output(name: &str, logical: Rect, scale: f64, color: [u8; 4]) -> OutputCapture {
        OutputCapture {
            output: OutputInfo {
                name: name.into(),
                description: String::new(),
                logical,
                scale,
                transform: Transform::Normal,
            },
            image: solid(
                (logical.width * scale) as u32,
                (logical.height * scale) as u32,
                color,
            ),
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            outputs: vec![
                output("A", Rect::new(0.0, 0.0, 100.0, 50.0), 1.0, [255, 0, 0, 255]),
                output(
                    "B",
                    Rect::new(100.0, 0.0, 100.0, 50.0),
                    2.0,
                    [0, 0, 255, 255],
                ),
            ],
            windows: vec![],
            taken_at: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn single_output_region_is_native() {
        let snap = snapshot();
        let img = snap.render_region(Rect::new(110.0, 10.0, 20.0, 10.0));
        assert_eq!((img.width(), img.height()), (40, 20));
        assert_eq!(img.rgba_at(0, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn spanning_region_uses_max_scale() {
        let snap = snapshot();
        let region = Rect::new(90.0, 0.0, 20.0, 10.0);
        let img = snap.render_region(region);
        assert_eq!((img.width(), img.height()), (40, 20));
        assert_eq!(snap.region_pixel_size(region), (40, 20));
        assert_eq!(img.rgba_at(5, 5), [255, 0, 0, 255]);
        assert_eq!(img.rgba_at(35, 5), [0, 0, 255, 255]);
    }

    #[test]
    fn outside_outputs_is_transparent() {
        let snap = snapshot();
        let img = snap.render_region(Rect::new(90.0, 40.0, 20.0, 20.0));
        assert_eq!(img.rgba_at(0, 39), [0, 0, 0, 0]);
    }
}
