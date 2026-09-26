//! Keeps the canvas cheap to redraw: everything settled is rendered once into a
//! full-size composite, and only the shape under the user's hands is re-rendered, into a
//! small tile over it, while it changes.

use std::sync::Arc;

use gpui::RenderImage;
use screenie_annotate::render::{draw_shapes, render};
use screenie_annotate::tiny_skia::{IntRect, Pixmap};
use screenie_annotate::{Kind, Shape, ShapeId};
use screenie_core::Rect;

use crate::session::Session;

pub(crate) struct Layer {
    pub image: Arc<RenderImage>,
    /// Where it goes, in image pixels.
    pub rect: Rect,
}

#[derive(Default)]
pub(crate) struct Raster {
    composite: Option<(Vec<Shape>, Pixmap, Arc<RenderImage>)>,
    tile: Option<(Vec<Shape>, Layer)>,
    /// Images replaced since the last paint, for the view to evict from the GPU atlas.
    stale: Vec<Arc<RenderImage>>,
}

impl Raster {
    /// The composite and the live tile for the session's current state.
    pub fn update(&mut self, session: &Session) -> (Arc<RenderImage>, Option<&Layer>) {
        let doc = session.doc();
        let live_id = session.live();
        // Spotlights dim as one layer, so moving one re-renders them all.
        let live_is_spotlight = live_id
            .and_then(|id| doc.shape(id))
            .is_some_and(|s| matches!(s.kind, Kind::Spotlight { .. }));
        let is_live = |s: &Shape| {
            Some(s.id) == live_id || (live_is_spotlight && matches!(s.kind, Kind::Spotlight { .. }))
        };

        let settled: Vec<Shape> = doc
            .shapes()
            .iter()
            .filter(|s| !is_live(s))
            .cloned()
            .collect();
        if self
            .composite
            .as_ref()
            .is_none_or(|(key, _, _)| *key != settled)
        {
            let started = std::time::Instant::now();
            let mut pixmap = Pixmap::new(doc.width(), doc.height()).expect("non-empty image");
            let ids: Vec<ShapeId> = settled.iter().map(|s| s.id).collect();
            render(doc, &mut pixmap, (0, 0), |s| ids.contains(&s.id));
            let image = upload(&pixmap);
            if let Some((_, _, old)) = self.composite.replace((settled, pixmap, image)) {
                self.stale.push(old);
            }
            tracing::trace!(elapsed = ?started.elapsed(), "composite rendered");
        }
        let (_, composite, composite_image) = self.composite.as_ref().expect("just rendered");

        let live: Vec<Shape> = doc
            .shapes()
            .iter()
            .filter(|s| is_live(s))
            .cloned()
            .collect();
        if live.is_empty() {
            if let Some((_, old)) = self.tile.take() {
                self.stale.push(old.image);
            }
        } else if self.tile.as_ref().is_none_or(|(key, _)| *key != live) {
            let area = live
                .iter()
                .map(|s| reach(s, doc))
                .reduce(|a, b| a.union(&b))
                .and_then(|r| r.round().intersection(&doc.bounds()));
            let tile = area.and_then(|area| {
                let rect = IntRect::from_xywh(
                    area.x as i32,
                    area.y as i32,
                    area.width as u32,
                    area.height as u32,
                )?;
                let mut pixmap = composite.clone_rect(rect)?;
                let refs: Vec<&Shape> = live.iter().collect();
                draw_shapes(doc, &refs, &mut pixmap, (rect.x(), rect.y()));
                Some(Layer {
                    image: upload(&pixmap),
                    rect: area,
                })
            });
            match tile {
                Some(layer) => {
                    if let Some((_, old)) = self.tile.replace((live, layer)) {
                        self.stale.push(old.image);
                    }
                }
                None => {
                    if let Some((_, old)) = self.tile.take() {
                        self.stale.push(old.image);
                    }
                }
            }
        }
        (
            composite_image.clone(),
            self.tile.as_ref().map(|(_, layer)| layer),
        )
    }

    pub fn take_stale(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.stale)
    }
}

/// The area a shape can change when drawn: its paint bounds, plus what a blur reads.
fn reach(shape: &Shape, doc: &screenie_annotate::Document) -> Rect {
    let scale = doc.scale();
    match shape.kind {
        Kind::Spotlight { .. } => doc.bounds(),
        Kind::Redact { rect, .. } => rect.inset(-(shape.style.blur_radius() * scale * 3.0) as f64),
        _ => shape.paint_bounds(scale),
    }
}

/// A GPU-ready copy of `pixmap`. GPUI takes premultiplied BGRA, tiny-skia gives
/// premultiplied RGBA.
fn upload(pixmap: &Pixmap) -> Arc<RenderImage> {
    let mut bytes = pixmap.data().to_vec();
    for px in bytes.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(pixmap.width(), pixmap.height(), bytes)
        .expect("buffer matches dimensions");
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
        buffer
    )]))
}
