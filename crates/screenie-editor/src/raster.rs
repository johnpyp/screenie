//! Keeps the canvas cheap to redraw. The image is shown as a grid of tiles, each its own
//! GPU image, and a change re-renders only the tiles it reaches: dragging a long diagonal
//! arrow redraws the tiles along it, not its whole bounding box.
//!
//! Settled shapes are rendered into the composite, tile by tile as they change. The shape
//! under the user's hands (being drawn, moved, resized or typed into) is drawn over the
//! composite's tiles it reaches, each time it changes; a stroke being drawn redraws only
//! the tiles around its end. Tiles render in parallel.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use gpui::RenderImage;
use rayon::prelude::*;
use screenie_annotate::render::{draw_over, goes_over, render_area};
use screenie_annotate::tiny_skia::{IntRect, Pixmap};
use screenie_annotate::{Document, Kind, Shape, ShapeId};
use screenie_core::Rect;

/// A tile's image is its cell and a pixel of the neighbouring cells all round, so a
/// scaled canvas blends across cell edges as it would across one image. 256px tiles pack
/// GPUI's 1024px atlas textures.
const TILE: u32 = 256;
const CELL: u32 = TILE - 2;

pub(crate) struct Tile {
    pub image: Arc<RenderImage>,
    /// The cell it shows, in image pixels.
    pub cell: Rect,
    /// What the image covers: the cell and a pixel around it, within the image.
    pub rect: Rect,
}

/// A shape as drawn: a step shows its number, which the steps before it decide.
#[derive(Clone, PartialEq)]
struct Drawn {
    shape: Shape,
    number: usize,
}

#[derive(Default)]
pub(crate) struct Raster {
    canvas: Option<Canvas>,
    /// Images replaced since the last paint, for the view to evict from the GPU atlas.
    stale: Vec<Arc<RenderImage>>,
}

impl Raster {
    /// The tile to show for each cell, for `doc` with `live` under the user's hands.
    pub fn update(&mut self, doc: &Document, live: Option<ShapeId>) -> impl Iterator<Item = &Tile> {
        let drawn = |s: &Shape| {
            let number = if matches!(s.kind, Kind::Step { .. }) {
                doc.step_number(s.id)
            } else {
                0
            };
            Drawn {
                shape: s.clone(),
                number,
            }
        };
        let (live, settled): (Vec<&Shape>, Vec<&Shape>) =
            doc.shapes().iter().partition(|s| Some(s.id) == live);
        let (live, settled): (Vec<Drawn>, Vec<Drawn>) = (
            live.into_iter().map(drawn).collect(),
            settled.into_iter().map(drawn).collect(),
        );

        let current = self
            .canvas
            .as_ref()
            .is_some_and(|c| Arc::ptr_eq(&c.base, doc.base()));
        if !current {
            let old = self.canvas.replace(Canvas::new(doc));
            let tiles = old
                .into_iter()
                .flat_map(|old| old.tiles.into_iter().chain(old.live_tiles.into_values()));
            self.stale.extend(tiles.map(|t| t.image));
        }
        let canvas = self.canvas.as_mut().expect("just made");
        let started = std::time::Instant::now();
        let changed = canvas.settle(doc, settled, &mut self.stale);
        let redrawn = canvas.draw_live(doc, live, &changed, &mut self.stale);
        if changed.iter().any(|c| *c) || redrawn > 0 {
            let changed = changed.iter().filter(|c| **c).count();
            tracing::trace!(elapsed = ?started.elapsed(), changed, redrawn, "canvas tiles rendered");
        }
        canvas
            .tiles
            .iter()
            .enumerate()
            .map(|(i, tile)| canvas.live_tiles.get(&i).unwrap_or(tile))
    }

    pub fn take_stale(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.stale)
    }
}

struct Canvas {
    /// The document image this is for.
    base: Arc<Pixmap>,
    cols: u32,
    rows: u32,
    width: u32,
    height: u32,
    /// Every settled shape over the image, full size: what live tiles start from.
    composite: Pixmap,
    /// The shapes the composite shows, and whether the spotlight dim is on.
    settled: Vec<Drawn>,
    dimmed: bool,
    tiles: Vec<Tile>,
    live: Vec<Drawn>,
    live_tiles: BTreeMap<usize, Tile>,
}

impl Canvas {
    /// The bare image; the first [`Self::settle`] draws the shapes.
    fn new(doc: &Document) -> Self {
        let (width, height) = (doc.width(), doc.height());
        let (cols, rows) = (width.div_ceil(CELL), height.div_ceil(CELL));
        let composite = (**doc.base()).clone();
        let mut canvas = Self {
            base: doc.base().clone(),
            cols,
            rows,
            width,
            height,
            composite,
            settled: Vec::new(),
            dimmed: false,
            tiles: Vec::new(),
            live: Vec::new(),
            live_tiles: BTreeMap::new(),
        };
        canvas.tiles = (0..(cols * rows) as usize)
            .into_par_iter()
            .map(|i| {
                let rect = canvas.tile_rect(i);
                let pixmap = canvas
                    .composite
                    .clone_rect(rect)
                    .expect("tile is inside the image");
                Tile {
                    image: upload(&pixmap),
                    cell: rect_of(canvas.cell(i)),
                    rect: rect_of(rect),
                }
            })
            .collect();
        canvas
    }

    /// Bring the composite up to date with `settled`, re-rendering the tiles that what
    /// changed reaches. Returns which cells changed.
    fn settle(
        &mut self,
        doc: &Document,
        settled: Vec<Drawn>,
        stale: &mut Vec<Arc<RenderImage>>,
    ) -> Vec<bool> {
        let dimmed = doc
            .shapes()
            .iter()
            .any(|s| matches!(s.kind, Kind::Spotlight { .. }));
        let mut changed = vec![false; self.tiles.len()];
        if dimmed != self.dimmed || !same_order(&self.settled, &settled) {
            changed.fill(true);
        } else {
            let gone = self.settled.iter().filter(|d| !settled.contains(d));
            let came = settled.iter().filter(|d| !self.settled.contains(d));
            for d in gone.chain(came) {
                self.reach(&d.shape, doc.scale(), &mut changed);
            }
        }
        self.settled = settled;
        self.dimmed = dimmed;
        let ids: HashSet<ShapeId> = self.settled.iter().map(|d| d.shape.id).collect();
        let this = &*self;
        let rendered: Vec<(usize, Pixmap, Arc<RenderImage>)> = (0..changed.len())
            .into_par_iter()
            .filter(|i| changed[*i])
            .map(|i| {
                let pixmap = render_area(doc, this.tile_rect(i), |s| ids.contains(&s.id));
                let image = upload(&pixmap);
                (i, pixmap, image)
            })
            .collect();
        for (i, pixmap, image) in rendered {
            self.put(&pixmap, self.tile_rect(i), self.cell(i));
            stale.push(std::mem::replace(&mut self.tiles[i].image, image));
        }
        changed
    }

    /// Draw the live shapes over the composite where they are, given the cells the
    /// composite just `changed`. Returns how many tiles were redrawn.
    fn draw_live(
        &mut self,
        doc: &Document,
        live: Vec<Drawn>,
        changed: &[bool],
        stale: &mut Vec<Arc<RenderImage>>,
    ) -> usize {
        let scale = doc.scale();
        let mut redraw = vec![false; self.tiles.len()];
        let grown = match (self.live.as_slice(), live.as_slice()) {
            ([old], [new]) => extension(&old.shape, &new.shape),
            _ => None,
        };
        if live == self.live || grown.is_some() {
            // The same shapes, or a stroke that grew: redraw around its end, and wherever
            // the composite changed under it.
            if let Some(end) = &grown {
                self.reach(end, scale, &mut redraw);
            }
            for i in self.live_tiles.keys() {
                redraw[*i] |= changed[*i];
            }
        } else {
            for d in &live {
                self.reach(&d.shape, scale, &mut redraw);
            }
            // The tiles it left show the composite again.
            let left: Vec<usize> = self
                .live_tiles
                .keys()
                .copied()
                .filter(|i| !redraw[*i])
                .collect();
            stale.extend(
                left.iter()
                    .filter_map(|i| self.live_tiles.remove(i))
                    .map(|t| t.image),
            );
        }
        self.live = live;

        // Drawn over the composite, unless a live shape goes under the rest (a redaction
        // or spotlight) or a settled shape above it covers the tile: then from scratch.
        let refs: Vec<&Shape> = self.live.iter().map(|d| &d.shape).collect();
        let over = refs.iter().all(|s| goes_over(s));
        let first = doc
            .shapes()
            .iter()
            .position(|s| refs.iter().any(|l| l.id == s.id));
        let above: Vec<&Shape> = first
            .map(|first| {
                doc.shapes()[first + 1..]
                    .iter()
                    .filter(|s| goes_over(s) && !refs.iter().any(|l| l.id == s.id))
                    .collect()
            })
            .unwrap_or_default();
        let this = &*self;
        let tiles: Vec<(usize, Tile)> = (0..redraw.len())
            .into_par_iter()
            .filter(|i| redraw[*i])
            .map(|i| {
                let rect = this.tile_rect(i);
                let covered = above.iter().any(|s| s.touches(&rect_of(rect), scale));
                let pixmap = if over && !covered {
                    let mut pixmap = this
                        .composite
                        .clone_rect(rect)
                        .expect("tile is inside the image");
                    draw_over(doc, &refs, &mut pixmap, (rect.x(), rect.y()));
                    pixmap
                } else {
                    render_area(doc, rect, |_| true)
                };
                let tile = Tile {
                    image: upload(&pixmap),
                    cell: rect_of(this.cell(i)),
                    rect: rect_of(rect),
                };
                (i, tile)
            })
            .collect();
        let redrawn = tiles.len();
        for (i, tile) in tiles {
            if let Some(old) = self.live_tiles.insert(i, tile) {
                stale.push(old.image);
            }
        }
        redrawn
    }

    /// Mark the cells whose tiles `shape` may draw on.
    fn reach(&self, shape: &Shape, scale: f32, cells: &mut [bool]) {
        let bounds = Rect::new(0.0, 0.0, self.width as f64, self.height as f64);
        let Some(b) = shape.paint_bounds(scale).intersection(&bounds) else {
            return;
        };
        // Tiles take in a pixel of their neighbours.
        let span = |lo: f64, hi: f64, n: u32| {
            ((lo - 1.0).max(0.0) as u32 / CELL)..=((hi + 1.0) as u32 / CELL).min(n - 1)
        };
        for row in span(b.y, b.bottom(), self.rows) {
            for col in span(b.x, b.right(), self.cols) {
                let i = (row * self.cols + col) as usize;
                cells[i] = cells[i] || shape.touches(&rect_of(self.tile_rect(i)), scale);
            }
        }
    }

    fn cell(&self, i: usize) -> IntRect {
        let (col, row) = (i as u32 % self.cols, i as u32 / self.cols);
        let (x, y) = (col * CELL, row * CELL);
        IntRect::from_xywh(
            x as i32,
            y as i32,
            CELL.min(self.width - x),
            CELL.min(self.height - y),
        )
        .expect("cell is in the image")
    }

    fn tile_rect(&self, i: usize) -> IntRect {
        let c = self.cell(i);
        let (right, bottom) = (
            (c.right() + 1).min(self.width as i32),
            (c.bottom() + 1).min(self.height as i32),
        );
        IntRect::from_ltrb((c.left() - 1).max(0), (c.top() - 1).max(0), right, bottom)
            .expect("tile is in the image")
    }

    /// Copy the `cell` part of `pixmap` (which covers `rect`) into the composite.
    fn put(&mut self, pixmap: &Pixmap, rect: IntRect, cell: IntRect) {
        let (width, from) = (self.width as usize, pixmap.width() as usize);
        let (dx, len) = ((cell.x() - rect.x()) as usize, cell.width() as usize);
        let composite = self.composite.pixels_mut();
        for y in cell.top()..cell.bottom() {
            let src = &pixmap.pixels()[(y - rect.y()) as usize * from + dx..][..len];
            composite[y as usize * width + cell.x() as usize..][..len].copy_from_slice(src);
        }
    }
}

/// Whether the shapes both lists have are in the same order.
fn same_order(a: &[Drawn], b: &[Drawn]) -> bool {
    let common = |a: &[Drawn], b: &[Drawn]| -> Vec<ShapeId> {
        a.iter()
            .map(|d| d.shape.id)
            .filter(|id| b.iter().any(|e| e.shape.id == *id))
            .collect()
    };
    common(a, b) == common(b, a)
}

/// If `new` is `old` with points added (a stroke being drawn), the part that draws
/// differently: its end, from the last curve `old` had.
fn extension(old: &Shape, new: &Shape) -> Option<Shape> {
    let points = |s: &Shape| match &s.kind {
        Kind::Pen { points } | Kind::Highlighter { points } => Some(points.clone()),
        _ => None,
    };
    let (a, b) = (points(old)?, points(new)?);
    let same = old.id == new.id
        && old.style == new.style
        && std::mem::discriminant(&old.kind) == std::mem::discriminant(&new.kind);
    if !same || b.len() <= a.len() || b[..a.len()] != a[..] {
        return None;
    }
    let end = b[a.len().saturating_sub(2)..].to_vec();
    let kind = match new.kind {
        Kind::Pen { .. } => Kind::Pen { points: end },
        _ => Kind::Highlighter { points: end },
    };
    Some(Shape {
        kind,
        ..new.clone()
    })
}

fn rect_of(r: IntRect) -> Rect {
    Rect::new(
        r.x() as f64,
        r.y() as f64,
        r.width() as f64,
        r.height() as f64,
    )
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

#[cfg(test)]
mod tests {
    use screenie_annotate::{Redaction, Style};
    use screenie_core::{Image, PixelFormat, Point};

    use super::*;

    /// What the view shows: each tile's cell, put together.
    fn shown(raster: &mut Raster, doc: &Document, live: Option<ShapeId>) -> Image {
        let (w, h) = (doc.width() as usize, doc.height() as usize);
        let mut out = vec![0u8; w * h * 4];
        for tile in raster.update(doc, live) {
            let bytes = tile.image.as_bytes(0).unwrap();
            let (from, stride) = (
                (tile.cell.x - tile.rect.x, tile.cell.y - tile.rect.y),
                tile.rect.width as usize * 4,
            );
            for y in 0..tile.cell.height as usize {
                for x in 0..tile.cell.width as usize {
                    let src =
                        &bytes[(y + from.1 as usize) * stride + (x + from.0 as usize) * 4..][..4];
                    let dst = &mut out
                        [((y + tile.cell.y as usize) * w + x + tile.cell.x as usize) * 4..][..4];
                    dst.copy_from_slice(&[src[2], src[1], src[0], src[3]]);
                }
            }
        }
        Image::from_raw(w as u32, h as u32, w * 4, PixelFormat::Rgba, out)
    }

    /// The canvas shows what the export would, give or take antialiasing where tiles cut
    /// paths off (see `screenie_annotate::render`'s tests).
    fn check(raster: &mut Raster, doc: &Document, live: Option<ShapeId>, step: &str) {
        let (shown, full) = (shown(raster, doc, live), doc.export());
        let mut noticeable = 0;
        for y in 0..full.height() {
            for x in 0..full.width() {
                let (a, b) = (shown.rgba_at(x, y), full.rgba_at(x, y));
                let diff = a.iter().zip(b).map(|(a, b)| a.abs_diff(b)).max().unwrap();
                assert!(
                    diff <= 64,
                    "{step}: off by {diff} at {x},{y}: {a:?} vs {b:?}"
                );
                noticeable += (diff > 4) as usize;
            }
        }
        assert!(
            noticeable * 200 < (full.width() * full.height()) as usize,
            "{step}: {noticeable} pixels off"
        );
    }

    fn doc() -> Document {
        let (w, h) = (600u32, 400u32);
        let data = (0..w * h).flat_map(|i| {
            [
                (i % w * 3 % 256) as u8,
                (i / w * 2 % 256) as u8,
                if (i % w / 4) % 2 == 0 { 230 } else { 60 },
                255,
            ]
        });
        Document::new(
            &Image::from_raw(w, h, w as usize * 4, PixelFormat::Rgba, data.collect()),
            1.0,
        )
    }

    #[test]
    fn the_canvas_keeps_up_with_edits() {
        let mut doc = doc();
        let mut raster = Raster::default();
        check(&mut raster, &doc, None, "empty");

        // A long arrow dragged out corner to corner, then let go.
        let arrow = doc.make(
            Kind::Arrow {
                from: Point::new(10.0, 10.0),
                to: Point::new(12.0, 12.0),
            },
            Style::default(),
        );
        let id = arrow.id;
        doc.add(arrow);
        for to in [(100.0, 80.0), (590.0, 390.0), (300.0, 390.0)] {
            doc.shape_mut(id).unwrap().kind = Kind::Arrow {
                from: Point::new(10.0, 10.0),
                to: Point::new(to.0, to.1),
            };
            check(&mut raster, &doc, Some(id), "dragging an arrow");
        }
        check(&mut raster, &doc, None, "arrow let go");

        // A pen stroke drawn across it, point by point.
        let pen = doc.make(
            Kind::Pen {
                points: vec![Point::new(20.0, 380.0)],
            },
            Style {
                size: 8.0,
                ..Style::default()
            },
        );
        let pen_id = pen.id;
        doc.add(pen);
        for i in 1..40 {
            let p = Point::new(
                20.0 + i as f64 * 14.0,
                380.0 - i as f64 * 9.0 + (i % 3) as f64 * 20.0,
            );
            if let Kind::Pen { points } = &mut doc.shape_mut(pen_id).unwrap().kind {
                points.push(p);
            }
            check(&mut raster, &doc, Some(pen_id), "drawing a stroke");
        }
        check(&mut raster, &doc, None, "stroke done");

        // The arrow, now under the stroke, moved: tiles the stroke covers draw it over.
        doc.shape_mut(id).unwrap().translate(15.0, -20.0);
        check(
            &mut raster,
            &doc,
            Some(id),
            "moving the arrow under the stroke",
        );
        check(&mut raster, &doc, None, "arrow moved");

        // Steps, renumbered when the first goes.
        let steps: Vec<ShapeId> = [(100.0, 300.0), (400.0, 100.0)]
            .map(|(x, y)| {
                let s = doc.make(
                    Kind::Step {
                        center: Point::new(x, y),
                    },
                    Style::default(),
                );
                let id = s.id;
                doc.add(s);
                id
            })
            .into();
        check(&mut raster, &doc, None, "two steps");
        doc.state_mut().shapes.retain(|s| s.id != steps[0]);
        check(&mut raster, &doc, None, "the first step deleted");

        // A redaction and a spotlight dragged over everything.
        let blur = doc.make(
            Kind::Redact {
                rect: Rect::new(200.0, 150.0, 10.0, 10.0),
                mode: Redaction::Blur,
            },
            Style::default(),
        );
        let blur_id = blur.id;
        doc.add(blur);
        for size in [80.0, 180.0] {
            doc.shape_mut(blur_id).unwrap().kind = Kind::Redact {
                rect: Rect::new(200.0, 150.0, size, size * 0.8),
                mode: Redaction::Blur,
            };
            check(&mut raster, &doc, Some(blur_id), "dragging a redaction");
        }
        check(&mut raster, &doc, None, "redaction let go");
        let spot = doc.make(
            Kind::Spotlight {
                rect: Rect::new(50.0, 50.0, 100.0, 80.0),
            },
            Style::default(),
        );
        let spot_id = spot.id;
        doc.add(spot);
        check(&mut raster, &doc, Some(spot_id), "a spotlight drawn");
        for dx in [100.0, 250.0] {
            doc.shape_mut(spot_id).unwrap().translate(dx, 40.0);
            check(&mut raster, &doc, Some(spot_id), "moving a spotlight");
        }
        check(&mut raster, &doc, None, "spotlight let go");

        // Text being typed while the shapes under it change (a shortcut, an undo).
        let text = doc.make(
            Kind::Text {
                origin: Point::new(150.0, 180.0),
                text: "Typing".into(),
            },
            Style {
                size: 8.0,
                ..Style::default()
            },
        );
        let text_id = text.id;
        doc.add(text);
        check(&mut raster, &doc, Some(text_id), "typing");
        doc.shape_mut(pen_id).unwrap().translate(0.0, -60.0);
        check(
            &mut raster,
            &doc,
            Some(text_id),
            "the stroke under the text moved",
        );
        check(&mut raster, &doc, None, "done typing");

        // Undone, all the way.
        while doc.undo() {
            check(&mut raster, &doc, None, "undo");
        }
    }
}
