//! Drawing documents with tiny-skia. The editor and export share this code, so what you
//! see is exactly what you get.
//!
//! Everything draws into a pixmap that covers some image-space area starting at
//! `origin`: the whole image for export, a small tile around a shape being dragged in the
//! editor.

use screenie_core::{Image, PixelFormat, Point, Rect};
use tiny_skia::{
    BlendMode, FillRule, IntRect, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint,
    PremultipliedColorU8, Stroke, Transform,
};

use crate::document::Document;
use crate::effects;
use crate::shape::{Kind, Redaction, Shape};
use crate::text::TextBlock;
use crate::Color;

/// Shadow under strokes and text, in logical pixels: blur, y offset, opacity.
const SHADOW_BLUR: f32 = 2.0;
const SHADOW_OFFSET: f32 = 1.0;
const SHADOW_ALPHA: f32 = 0.35;
/// How dark a spotlight makes everything around it.
const SPOTLIGHT_DIM: f32 = 0.55;

/// How far a shadow reaches beyond its shape, in image pixels.
pub fn shadow_margin(scale: f32) -> f64 {
    ((SHADOW_BLUR * 3.0 + SHADOW_OFFSET) * scale).ceil() as f64
}

/// Premultiplied pixels of `image`.
pub fn pixmap_from_image(image: &Image) -> Pixmap {
    let (w, h) = (image.width().max(1), image.height().max(1));
    let mut pixmap = Pixmap::new(w, h).expect("non-empty");
    if image.width() == 0 || image.height() == 0 {
        return pixmap;
    }
    let rgba = image.convert(PixelFormat::Rgba);
    for (dst, src) in pixmap.pixels_mut().iter_mut().zip(rgba.data().as_chunks::<4>().0) {
        *dst = tiny_skia::ColorU8::from_rgba(src[0], src[1], src[2], src[3]).premultiply();
    }
    pixmap
}

/// Straight-alpha RGBA copy of `pixmap`.
pub fn image_from_pixmap(pixmap: &Pixmap) -> Image {
    let data = pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    });
    let (w, h) = (pixmap.width(), pixmap.height());
    Image::from_raw(w, h, w as usize * 4, PixelFormat::Rgba, data.collect())
}

impl Document {
    /// The finished image: every shape drawn, cropped.
    pub fn export(&self) -> Image {
        let area = self.visible().round();
        let mut pixmap = Pixmap::new(area.width.max(1.0) as u32, area.height.max(1.0) as u32).expect("non-empty");
        render(self, &mut pixmap, (area.x as i32, area.y as i32), |_| true);
        image_from_pixmap(&pixmap)
    }
}

/// Draw the image and the shapes `include` accepts into `target`, whose top-left is image
/// pixel `origin`.
pub fn render(doc: &Document, target: &mut Pixmap, origin: (i32, i32), include: impl Fn(&Shape) -> bool) {
    draw_base(doc, target, origin);
    let shapes: Vec<&Shape> = doc.shapes().iter().filter(|s| include(s)).collect();
    draw_shapes(doc, &shapes, target, origin);
}

/// Copy the underlying image into `target`.
pub fn draw_base(doc: &Document, target: &mut Pixmap, origin: (i32, i32)) {
    let paint = PixmapPaint { blend_mode: BlendMode::Source, ..Default::default() };
    target.draw_pixmap(-origin.0, -origin.1, (**doc.base()).as_ref(), &paint, Transform::identity(), None);
}

/// Draw `shapes` over whatever `target` holds. Redactions go first (they hide what was
/// captured, not the annotations), then spotlights as one dimmed layer, then the rest,
/// each group in document order.
pub fn draw_shapes(doc: &Document, shapes: &[&Shape], target: &mut Pixmap, origin: (i32, i32)) {
    let mut ctx = Ctx { doc, origin, scale: doc.scale() };
    for shape in shapes.iter().filter(|s| matches!(s.kind, Kind::Redact { .. })) {
        ctx.redact(shape, target);
    }
    let spots: Vec<Rect> =
        shapes.iter().filter_map(|s| if let Kind::Spotlight { rect } = s.kind { Some(rect) } else { None }).collect();
    if !spots.is_empty() {
        ctx.spotlight(&spots, target);
    }
    for shape in shapes.iter().filter(|s| !matches!(s.kind, Kind::Redact { .. } | Kind::Spotlight { .. })) {
        ctx.shape(shape, target);
    }
}

struct Ctx<'a> {
    doc: &'a Document,
    origin: (i32, i32),
    scale: f32,
}

impl Ctx<'_> {
    /// The pixel rectangle of `target` covering image-space `r`.
    fn local(&self, r: Rect, target: &Pixmap) -> Option<IntRect> {
        let r = r.round();
        let rect = IntRect::from_xywh(
            r.x as i32 - self.origin.0,
            r.y as i32 - self.origin.1,
            r.width.max(1.0) as u32,
            r.height.max(1.0) as u32,
        )?;
        rect.intersect(&IntRect::from_xywh(0, 0, target.width(), target.height())?)
    }

    fn redact(&mut self, shape: &Shape, target: &mut Pixmap) {
        let Kind::Redact { rect, mode } = shape.kind else { return };
        let Some(area) = self.local(rect, target) else { return };
        match mode {
            Redaction::Pixelate => {
                // Block alignment follows the shape, not the tile, so tiles match export.
                let block = (shape.style.pixel_block() * self.scale).round().max(2.0) as u32;
                effects::pixelate(target, area, block);
            }
            Redaction::Blur => effects::blur(target, area, shape.style.blur_radius() * self.scale),
        }
    }

    fn spotlight(&mut self, spots: &[Rect], target: &mut Pixmap) {
        let Some(mut mask) = Mask::new(target.width(), target.height()) else { return };
        let transform = self.transform();
        for rect in spots {
            if let Some(path) = rounded_rect(*rect, 4.0 * self.scale as f64) {
                mask.fill_path(&path, FillRule::Winding, true, transform);
            }
        }
        mask.invert();
        let paint = solid(Color::BLACK.with_alpha((SPOTLIGHT_DIM * 255.0) as u8));
        let all = tiny_skia::Rect::from_xywh(0.0, 0.0, target.width() as f32, target.height() as f32).expect("non-empty");
        target.fill_rect(all, &paint, Transform::identity(), Some(&mask));
    }

    fn transform(&self) -> Transform {
        Transform::from_translate(-self.origin.0 as f32, -self.origin.1 as f32)
    }

    /// A vector shape, on a layer so it can cast one soft shadow.
    fn shape(&mut self, shape: &Shape, target: &mut Pixmap) {
        if !shape.has_shadow() {
            self.draw(shape, target, self.transform());
            return;
        }
        let Some(area) = self.local(shape.paint_bounds(self.scale), target) else { return };
        let Some(mut layer) = Pixmap::new(area.width(), area.height()) else { return };
        let transform = Transform::from_translate(
            -(self.origin.0 + area.x()) as f32,
            -(self.origin.1 + area.y()) as f32,
        );
        self.draw(shape, &mut layer, transform);
        let shadow = shadow_of(&layer, self.scale);
        let dy = (SHADOW_OFFSET * self.scale).round() as i32;
        target.draw_pixmap(area.x(), area.y() + dy, shadow.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        target.draw_pixmap(area.x(), area.y(), layer.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }

    fn draw(&mut self, shape: &Shape, target: &mut Pixmap, transform: Transform) {
        let w = shape.stroke(self.scale) as f32;
        let color = shape.style.color;
        match &shape.kind {
            Kind::Arrow { from, to } => {
                if let Some(path) = arrow_path(*from, *to, w as f64, self.scale as f64) {
                    target.fill_path(&path, &solid(color), FillRule::Winding, transform, None);
                    // Round off the polygon's corners.
                    let stroke = Stroke { width: w * 0.3, line_join: LineJoin::Round, ..Default::default() };
                    target.stroke_path(&path, &solid(color), &stroke, transform, None);
                }
            }
            Kind::Line { from, to } => {
                let mut pb = PathBuilder::new();
                pb.move_to(from.x as f32, from.y as f32);
                pb.line_to(to.x as f32, to.y as f32);
                if let Some(path) = pb.finish() {
                    target.stroke_path(&path, &solid(color), &round_stroke(w), transform, None);
                }
            }
            Kind::Rectangle { rect } => {
                if shape.style.fill {
                    if let Some(path) = rounded_rect(*rect, (w as f64 * 0.75).min(rect.width.min(rect.height) / 2.0)) {
                        target.fill_path(&path, &solid(color), FillRule::Winding, transform, None);
                    }
                } else if let Some(path) = outline(*rect, PathBuilder::from_rect) {
                    let stroke = Stroke { width: w, line_join: LineJoin::Round, ..Default::default() };
                    target.stroke_path(&path, &solid(color), &stroke, transform, None);
                }
            }
            Kind::Ellipse { rect } => {
                if shape.style.fill {
                    if let Some(path) = skia_rect(*rect).filter(|_| !rect.is_empty()).and_then(PathBuilder::from_oval) {
                        target.fill_path(&path, &solid(color), FillRule::Winding, transform, None);
                    }
                } else if let Some(path) = outline(*rect, PathBuilder::from_oval) {
                    target.stroke_path(&path, &solid(color), &round_stroke(w), transform, None);
                }
            }
            Kind::Pen { points } => stroke_points(target, points, &solid(color), w, transform),
            Kind::Highlighter { points } => {
                let mut paint = solid(color);
                paint.blend_mode = BlendMode::Multiply;
                stroke_points(target, points, &paint, shape.style.highlighter_width() * self.scale, transform);
            }
            Kind::Text { origin, .. } => self.text(shape, *origin, target, transform),
            Kind::Step { center } => self.step(shape, *center, target, transform),
            Kind::Redact { .. } | Kind::Spotlight { .. } => {}
        }
    }

    fn text(&mut self, shape: &Shape, origin: Point, target: &mut Pixmap, transform: Transform) {
        let Some(block) = shape.text_block(self.scale) else { return };
        let mut color = shape.style.color;
        if shape.style.fill {
            let label = shape.bounds(self.scale);
            let radius = (shape.style.font_size() * self.scale * 0.3) as f64;
            if let Some(path) = rounded_rect(label, radius) {
                target.fill_path(&path, &solid(color), FillRule::Winding, transform, None);
            }
            color = color.contrasting();
        }
        let (x, y) = map(transform, origin);
        block.draw(target, x, y, color);
    }

    fn step(&mut self, shape: &Shape, center: Point, target: &mut Pixmap, transform: Transform) {
        let d = shape.style.step_diameter() * self.scale;
        let (cx, cy) = (center.x as f32, center.y as f32);
        let ring = (d * 0.07).max(1.0);
        if let Some(disc) = PathBuilder::from_circle(cx, cy, d / 2.0 - ring / 2.0) {
            target.fill_path(&disc, &solid(shape.style.color), FillRule::Winding, transform, None);
            let stroke = Stroke { width: ring, ..Default::default() };
            target.stroke_path(&disc, &solid(Color::WHITE), &stroke, transform, None);
        }
        let number = self.doc.step_number(shape.id).to_string();
        let font = d * if number.len() <= 2 { 0.52 } else { 0.4 };
        let block = TextBlock::new(&number, font);
        // Centre the digits themselves (cap height), not the line box.
        let (x, y) = map(transform, center);
        let top = y + font * CAP_HEIGHT / 2.0 - block.baseline();
        block.draw(target, x - block.width() / 2.0, top, shape.style.color.contrasting());
    }
}

/// Inter's cap height, as a fraction of the font size.
const CAP_HEIGHT: f32 = 0.727;

fn map(t: Transform, p: Point) -> (f32, f32) {
    (p.x as f32 * t.sx + t.tx, p.y as f32 * t.sy + t.ty)
}

fn solid(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(color.skia());
    paint.anti_alias = true;
    paint
}

fn round_stroke(width: f32) -> Stroke {
    Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() }
}

fn skia_rect(r: Rect) -> Option<tiny_skia::Rect> {
    tiny_skia::Rect::from_xywh(r.x as f32, r.y as f32, r.width as f32, r.height as f32)
}

/// A box shape's path (`PathBuilder::from_rect` or `from_oval`). A flat one still
/// strokes, as a line, but a point has no outline and tiny-skia fails on it (as on a
/// drag's first frame).
fn outline<P: Into<Option<Path>>>(r: Rect, path: impl FnOnce(tiny_skia::Rect) -> P) -> Option<Path> {
    if r.width <= 0.0 && r.height <= 0.0 {
        return None;
    }
    skia_rect(r).and_then(|r| path(r).into())
}

/// A freehand stroke, smoothed by running quadratic curves through the midpoints.
fn stroke_points(target: &mut Pixmap, points: &[Point], paint: &Paint, width: f32, transform: Transform) {
    let Some(first) = points.first() else { return };
    if points.len() == 1 || points.iter().all(|p| p.distance(*first) < 0.5) {
        if let Some(dot) = PathBuilder::from_circle(first.x as f32, first.y as f32, width / 2.0) {
            target.fill_path(&dot, paint, FillRule::Winding, transform, None);
        }
        return;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(first.x as f32, first.y as f32);
    for pair in points.windows(2).skip(1) {
        let (a, b) = (pair[0], pair[1]);
        pb.quad_to(a.x as f32, a.y as f32, ((a.x + b.x) / 2.0) as f32, ((a.y + b.y) / 2.0) as f32);
    }
    let last = points[points.len() - 1];
    pb.line_to(last.x as f32, last.y as f32);
    if let Some(path) = pb.finish() {
        target.stroke_path(&path, paint, &round_stroke(width), transform, None);
    }
}

/// A filled box's path; none if it has no area (tiny-skia can't fill it, and there'd be
/// nothing to see).
pub(crate) fn rounded_rect(r: Rect, radius: f64) -> Option<Path> {
    if r.is_empty() {
        return None;
    }
    let radius = radius.min(r.width / 2.0).min(r.height / 2.0).max(0.0) as f32;
    let (x0, y0, x1, y1) = (r.x as f32, r.y as f32, r.right() as f32, r.bottom() as f32);
    if radius < 0.5 {
        return skia_rect(r).map(PathBuilder::from_rect);
    }
    // Cubic approximation of a quarter circle.
    let k = radius * 0.447_715;
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + radius, y0);
    pb.line_to(x1 - radius, y0);
    pb.cubic_to(x1 - k, y0, x1, y0 + k, x1, y0 + radius);
    pb.line_to(x1, y1 - radius);
    pb.cubic_to(x1, y1 - k, x1 - k, y1, x1 - radius, y1);
    pb.line_to(x0 + radius, y1);
    pb.cubic_to(x0 + k, y1, x0, y1 - k, x0, y1 - radius);
    pb.line_to(x0, y0 + radius);
    pb.cubic_to(x0, y0 + k, x0 + k, y0, x0 + radius, y0);
    pb.close();
    pb.finish()
}

/// A filled arrow: a shaft that widens from half the stroke width at the tail to the full
/// width, and a swept-back head whose size follows the stroke width.
pub(crate) fn arrow_path(from: Point, to: Point, w: f64, scale: f64) -> Option<Path> {
    let len = from.distance(to);
    if len < 1.0 {
        return None;
    }
    let (dx, dy) = ((to.x - from.x) / len, (to.y - from.y) / len);
    let (nx, ny) = (-dy, dx);
    let head = (w * 3.0 + 6.0 * scale).min(len * 0.8);
    let half = head * 0.55;
    let shaft_end = head * 0.72;
    let at = |along: f64, across: f64| {
        let p = Point::new(to.x - dx * along + nx * across, to.y - dy * along + ny * across);
        (p.x as f32, p.y as f32)
    };
    let tail = |across: f64| (from.x + nx * across, from.y + ny * across);
    let shaft = (w / 2.0).min(half * 0.8);
    let points = [
        (tail(w * 0.25).0 as f32, tail(w * 0.25).1 as f32),
        at(shaft_end, shaft),
        at(head, half),
        at(0.0, 0.0),
        at(head, -half),
        at(shaft_end, -shaft),
        (tail(-w * 0.25).0 as f32, tail(-w * 0.25).1 as f32),
    ];
    let mut pb = PathBuilder::new();
    pb.move_to(points[0].0, points[0].1);
    for (x, y) in &points[1..] {
        pb.line_to(*x, *y);
    }
    pb.close();
    pb.finish()
}

/// A soft black shadow shaped like `layer`'s alpha.
fn shadow_of(layer: &Pixmap, scale: f32) -> Pixmap {
    let (w, h) = (layer.width() as usize, layer.height() as usize);
    let mut mask: Vec<f32> = layer.pixels().iter().map(|p| p.alpha() as f32).collect();
    effects::blur_mask(&mut mask, w, h, SHADOW_BLUR * scale);
    let mut shadow = Pixmap::new(layer.width(), layer.height()).expect("same size as layer");
    for (dst, a) in shadow.pixels_mut().iter_mut().zip(mask) {
        let a = (a * SHADOW_ALPHA).round().clamp(0.0, 255.0) as u8;
        *dst = PremultipliedColorU8::from_rgba(0, 0, 0, a).expect("black is valid premultiplied");
    }
    shadow
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::Style;

    fn white(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h, PixelFormat::Rgba);
        img.blit(&Image::from_raw(w, h, w as usize * 4, PixelFormat::Rgba, vec![255; (w * h * 4) as usize]), 0, 0);
        img
    }

    fn add(doc: &mut Document, kind: Kind, style: Style) {
        let s = doc.make(kind, style);
        doc.add(s);
    }

    #[test]
    fn export_draws_shapes_and_crops() {
        let mut doc = Document::new(&white(200, 100), 1.0);
        add(&mut doc, Kind::Rectangle { rect: Rect::new(20.0, 20.0, 60.0, 40.0) }, Style { fill: true, ..Style::default() });
        let out = doc.export();
        assert_eq!((out.width(), out.height()), (200, 100));
        assert_eq!(out.rgba_at(50, 40), [0xff, 0x3b, 0x30, 255]);
        assert_eq!(out.rgba_at(150, 80), [255, 255, 255, 255]);

        doc.set_crop(Some(Rect::new(10.0, 10.0, 100.0, 60.0)));
        let out = doc.export();
        assert_eq!((out.width(), out.height()), (100, 60));
        assert_eq!(out.rgba_at(40, 30), [0xff, 0x3b, 0x30, 255]);
        assert_eq!(out.rgba_at(2, 2), [255, 255, 255, 255]);
    }

    #[test]
    fn box_outlines_stroke_unless_a_point() {
        // Stroked and then filled, as tiny-skia does: both must succeed.
        let strokes = |w, h, oval: bool| {
            let r = Rect::new(10.0, 10.0, w, h);
            let path = if oval { outline(r, PathBuilder::from_oval) } else { outline(r, PathBuilder::from_rect) };
            path.and_then(|p| p.stroke(&round_stroke(4.0), 1.0)).is_some_and(|p| {
                let b = p.bounds();
                b.width() > 1.0 && b.height() > 1.0
            })
        };
        for oval in [false, true] {
            assert!(strokes(30.0, 20.0, oval) && strokes(30.0, 0.0, oval) && strokes(0.0, 20.0, oval));
        }
        assert!(outline(Rect::new(10.0, 10.0, 0.0, 0.0), PathBuilder::from_rect).is_none());
        assert!(outline(Rect::new(10.0, 10.0, 0.0, 0.0), PathBuilder::from_oval).is_none());
    }

    #[test]
    fn empty_boxes_have_nothing_to_fill() {
        assert!(rounded_rect(Rect::new(10.0, 10.0, 30.0, 20.0), 4.0).is_some());
        assert!(rounded_rect(Rect::new(10.0, 10.0, 30.0, 0.0), 4.0).is_none());
        assert!(rounded_rect(Rect::new(10.0, 10.0, 0.0, 0.0), 0.0).is_none());
    }

    #[test]
    fn shapes_cast_a_shadow() {
        let mut doc = Document::new(&white(100, 100), 1.0);
        add(&mut doc, Kind::Line { from: Point::new(10.0, 50.0), to: Point::new(90.0, 50.0) }, Style::default());
        let out = doc.export();
        // Just below the 4px line: shadowed, so darker than the white page.
        let below = out.rgba_at(50, 54);
        assert!(below[0] < 250 && below[0] > 150, "{below:?}");
        assert_eq!(out.rgba_at(50, 50), [0xff, 0x3b, 0x30, 255]);
    }

    #[test]
    fn spotlight_dims_outside_only() {
        let mut doc = Document::new(&white(100, 100), 1.0);
        add(&mut doc, Kind::Spotlight { rect: Rect::new(20.0, 20.0, 40.0, 40.0) }, Style::default());
        add(&mut doc, Kind::Spotlight { rect: Rect::new(40.0, 40.0, 40.0, 40.0) }, Style::default());
        let out = doc.export();
        assert_eq!(out.rgba_at(40, 40), [255, 255, 255, 255]);
        assert_eq!(out.rgba_at(70, 70), [255, 255, 255, 255]);
        let dim = out.rgba_at(5, 5);
        assert!((110..=120).contains(&dim[0]), "{dim:?}");
    }

    #[test]
    fn highlighter_multiplies() {
        let mut doc = Document::new(&white(100, 40), 1.0);
        let yellow = Style { color: Color::rgb(255, 204, 0), ..Style::default() };
        add(&mut doc, Kind::Highlighter { points: vec![Point::new(10.0, 20.0), Point::new(90.0, 20.0)] }, yellow);
        assert_eq!(doc.export().rgba_at(50, 20), [255, 204, 0, 255]);
    }

    #[test]
    fn a_tile_matches_the_export() {
        let mut doc = Document::new(&white(120, 80), 1.0);
        add(&mut doc, Kind::Arrow { from: Point::new(10.0, 10.0), to: Point::new(100.0, 60.0) }, Style::default());
        add(&mut doc, Kind::Step { center: Point::new(40.0, 50.0) }, Style::default());
        let full = doc.export();
        let mut tile = Pixmap::new(50, 40).unwrap();
        render(&doc, &mut tile, (60, 30), |_| true);
        let tile = image_from_pixmap(&tile);
        for (x, y) in [(10, 10), (30, 20), (35, 25), (45, 35)] {
            assert_eq!(tile.rgba_at(x, y), full.rgba_at(x + 60, y + 30), "at {x},{y}");
        }
    }

    #[test]
    fn arrowheads_scale_with_width_but_fit_short_arrows() {
        let long = arrow_path(Point::new(0.0, 0.0), Point::new(200.0, 0.0), 4.0, 1.0).unwrap().bounds();
        let thick = arrow_path(Point::new(0.0, 0.0), Point::new(200.0, 0.0), 8.0, 1.0).unwrap().bounds();
        assert!(thick.height() > long.height());
        let short = arrow_path(Point::new(0.0, 0.0), Point::new(10.0, 0.0), 8.0, 1.0).unwrap().bounds();
        assert!(short.width() <= 10.5, "{short:?}");
    }

    #[test]
    fn pixelate_hides_detail() {
        let mut img = white(64, 64);
        img.blit(&Image::from_raw(2, 2, 8, PixelFormat::Rgba, [0, 0, 0, 255].repeat(4)), 10, 10);
        let mut doc = Document::new(&img, 1.0);
        add(&mut doc, Kind::Redact { rect: Rect::new(0.0, 0.0, 64.0, 64.0), mode: Redaction::Pixelate }, Style::default());
        let out = doc.export();
        // The black speck is averaged into its 12px block.
        let p = out.rgba_at(10, 10);
        assert!(p[0] > 200 && p[0] < 255, "{p:?}");
        assert_eq!(out.rgba_at(10, 10), out.rgba_at(0, 0));
    }
}

