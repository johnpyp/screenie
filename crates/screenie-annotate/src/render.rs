//! Drawing documents with tiny-skia. The editor and export share this code, so what you
//! see is exactly what you get.
//!
//! Everything draws into a pixmap that covers some image-space area: the whole image (or
//! the crop) for export, one tile of the canvas in the editor. [`render_area`] draws any
//! area the way the export draws it, so tiles put together match it.

use screenie_core::{Image, PixelFormat, Point, Rect};
use tiny_skia::{
    BlendMode, FillRule, IntRect, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap,
    Stroke, Transform,
};

use crate::Color;
use crate::document::Document;
use crate::effects;
use crate::shape::{Kind, Redaction, Shape};
use crate::text::TextBlock;

/// Shadow under strokes and text, in logical pixels: blur, y offset, opacity.
const SHADOW_BLUR: f32 = 2.0;
const SHADOW_OFFSET: f32 = 1.0;
const SHADOW_ALPHA: f32 = 0.35;
/// How dark a spotlight makes everything around it.
const SPOTLIGHT_DIM: f32 = 0.55;

/// How far a shadow reaches beyond its shape, in image pixels.
pub fn shadow_margin(scale: f32) -> f64 {
    effects::shadow_reach(SHADOW_BLUR * scale, shadow_offset(scale)) as f64
}

fn shadow_offset(scale: f32) -> i32 {
    (SHADOW_OFFSET * scale).round() as i32
}

/// Premultiplied pixels of `image`.
pub fn pixmap_from_image(image: &Image) -> Pixmap {
    let (w, h) = (image.width().max(1), image.height().max(1));
    let mut pixmap = Pixmap::new(w, h).expect("non-empty");
    if image.width() == 0 || image.height() == 0 {
        return pixmap;
    }
    let rgba = image.convert(PixelFormat::Rgba);
    for (dst, src) in pixmap
        .pixels_mut()
        .iter_mut()
        .zip(rgba.data().as_chunks::<4>().0)
    {
        *dst = tiny_skia::ColorU8::from_rgba(src[0], src[1], src[2], src[3]).premultiply();
    }
    pixmap
}

/// Straight-alpha RGBA copy of `pixmap`.
pub fn image_from_pixmap(pixmap: &Pixmap) -> Image {
    let mut data = pixmap.data().to_vec();
    for (px, p) in data.as_chunks_mut::<4>().0.iter_mut().zip(pixmap.pixels()) {
        // Opaque pixels, most of a screenshot, are the same either way.
        if p.alpha() != 255 {
            let c = p.demultiply();
            *px = [c.red(), c.green(), c.blue(), c.alpha()];
        }
    }
    let (w, h) = (pixmap.width(), pixmap.height());
    Image::from_raw(w, h, w as usize * 4, PixelFormat::Rgba, data)
}

impl Document {
    /// The finished image: every shape drawn, cropped.
    pub fn export(&self) -> Image {
        let area = self.visible().round();
        let area = IntRect::from_xywh(
            area.x as i32,
            area.y as i32,
            area.width.max(1.0) as u32,
            area.height.max(1.0) as u32,
        )
        .expect("non-empty");
        image_from_pixmap(&render_area(self, area, |_| true))
    }
}

/// The image and the shapes `include` accepts, in `area` (image pixels). Redactions read
/// around what they cover, beyond `area` where they need to, so tiles put together match
/// the export (up to tiny-skia antialiasing a path a hair differently where it's cut
/// off). The spotlight dim is the document's: it's there whenever the document has a
/// spotlight, with holes for the included ones.
pub fn render_area(doc: &Document, area: IntRect, include: impl Fn(&Shape) -> bool) -> Pixmap {
    let shapes: Vec<&Shape> = doc.shapes().iter().filter(|s| include(s)).collect();
    // One redaction reads what those under it made of their surroundings, so the reaches
    // of overlapping ones add up.
    let reads = |s: &&Shape| reads_around(s, doc.scale());
    let all: i32 = shapes.iter().map(reads).sum();
    let near = |s: &&&Shape| {
        let b = s.bounds(doc.scale()).inset(-all as f64);
        b.x < area.right() as f64
            && b.right() > area.x() as f64
            && b.y < area.bottom() as f64
            && b.bottom() > area.y() as f64
    };
    let margin: i32 = shapes.iter().filter(near).map(reads).sum();
    let (w, h) = (doc.width() as i32, doc.height() as i32);
    let outer = IntRect::from_ltrb(
        area.left().min((area.left() - margin).max(0)),
        area.top().min((area.top() - margin).max(0)),
        area.right().max((area.right() + margin).min(w)),
        area.bottom().max((area.bottom() + margin).min(h)),
    )
    .expect("contains area");
    let mut pixmap = Pixmap::new(outer.width(), outer.height()).expect("non-empty");
    draw_under(doc, &shapes, &mut pixmap, (outer.x(), outer.y()));
    if outer != area {
        let inner = IntRect::from_xywh(
            area.x() - outer.x(),
            area.y() - outer.y(),
            area.width(),
            area.height(),
        );
        pixmap = inner
            .and_then(|inner| pixmap.clone_rect(inner))
            .expect("area is inside");
    }
    // The rest goes onto `area` itself, as the editor draws a shape being dragged over its
    // tile, so letting go of it changes nothing.
    draw_over(doc, &shapes, &mut pixmap, (area.x(), area.y()));
    pixmap
}

/// Draw `shapes` over `target` (whose top-left is image pixel `origin`), which already
/// holds everything under them. Only for shapes that go over the rest: redactions and
/// spotlights go under everything, so they're skipped.
pub fn draw_over(doc: &Document, shapes: &[&Shape], target: &mut Pixmap, origin: (i32, i32)) {
    let mut ctx = Ctx {
        doc,
        origin,
        scale: doc.scale(),
    };
    for shape in shapes.iter().filter(|s| goes_over(s)) {
        ctx.shape(shape, target);
    }
}

/// Whether a shape is drawn over the rest, rather than into the image under them like
/// redactions and spotlights.
pub fn goes_over(shape: &Shape) -> bool {
    !matches!(shape.kind, Kind::Redact { .. } | Kind::Spotlight { .. })
}

/// What's under the shapes that go over: the image, then redactions (they hide what was
/// captured, not the annotations), then the spotlight dim.
fn draw_under(doc: &Document, shapes: &[&Shape], target: &mut Pixmap, origin: (i32, i32)) {
    copy_base(doc, target, origin);
    let mut ctx = Ctx {
        doc,
        origin,
        scale: doc.scale(),
    };
    for shape in shapes
        .iter()
        .filter(|s| matches!(s.kind, Kind::Redact { .. }))
    {
        ctx.redact(shape, target);
    }
    if doc
        .shapes()
        .iter()
        .any(|s| matches!(s.kind, Kind::Spotlight { .. }))
    {
        let spots: Vec<Rect> = shapes
            .iter()
            .filter_map(|s| {
                if let Kind::Spotlight { rect } = s.kind {
                    Some(rect)
                } else {
                    None
                }
            })
            .collect();
        ctx.spotlight(&spots, target);
    }
}

/// Copy the image into `target`, whose top-left is image pixel `origin`.
fn copy_base(doc: &Document, target: &mut Pixmap, origin: (i32, i32)) {
    let base = doc.base();
    let (bw, tw) = (base.width() as i32, target.width() as i32);
    let (x0, x1) = (origin.0.max(0), (origin.0 + tw).min(bw));
    let (y0, y1) = (
        origin.1.max(0),
        (origin.1 + target.height() as i32).min(base.height() as i32),
    );
    if x0 >= x1 {
        return;
    }
    let len = (x1 - x0) as usize;
    let pixels = target.pixels_mut();
    for y in y0..y1 {
        let src = &base.pixels()[(y * bw + x0) as usize..][..len];
        pixels[((y - origin.1) * tw + x0 - origin.0) as usize..][..len].copy_from_slice(src);
    }
}

/// How far around itself a shape reads what's under it.
fn reads_around(shape: &Shape, scale: f32) -> i32 {
    match shape.kind {
        Kind::Redact {
            mode: Redaction::Pixelate,
            ..
        } => pixel_block(shape, scale) as i32 - 1,
        Kind::Redact {
            mode: Redaction::Blur,
            ..
        } => effects::blur_reach(shape.style.blur_radius() * scale),
        _ => 0,
    }
}

fn pixel_block(shape: &Shape, scale: f32) -> u32 {
    (shape.style.pixel_block() * scale).round().max(2.0) as u32
}

struct Ctx<'a> {
    doc: &'a Document,
    origin: (i32, i32),
    scale: f32,
}

impl Ctx<'_> {
    fn redact(&mut self, shape: &Shape, target: &mut Pixmap) {
        let Kind::Redact { rect, mode } = shape.kind else {
            return;
        };
        let r = rect.round();
        let (x, y) = (r.x as i32 - self.origin.0, r.y as i32 - self.origin.1);
        let Some(rect) =
            IntRect::from_xywh(x, y, r.width.max(1.0) as u32, r.height.max(1.0) as u32)
        else {
            return;
        };
        match mode {
            // Blocks align to the shape, not the tile, so tiles match export.
            Redaction::Pixelate => effects::pixelate(target, rect, pixel_block(shape, self.scale)),
            Redaction::Blur => effects::blur(target, rect, shape.style.blur_radius() * self.scale),
        }
    }

    fn spotlight(&mut self, spots: &[Rect], target: &mut Pixmap) {
        let Some(mut mask) = Mask::new(target.width(), target.height()) else {
            return;
        };
        let transform = self.transform();
        for rect in spots {
            if let Some(path) = rounded_rect(*rect, 4.0 * self.scale as f64) {
                mask.fill_path(&path, FillRule::Winding, true, transform);
            }
        }
        mask.invert();
        let paint = solid(Color::BLACK.with_alpha((SPOTLIGHT_DIM * 255.0) as u8));
        let all =
            tiny_skia::Rect::from_xywh(0.0, 0.0, target.width() as f32, target.height() as f32)
                .expect("non-empty");
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
        // The layer takes in the parts of the shape just outside the target too, whose
        // shadow falls on it.
        let reach = shadow_margin(self.scale);
        let (x, y) = (self.origin.0 as f64 - reach, self.origin.1 as f64 - reach);
        let around = Rect::new(
            x,
            y,
            target.width() as f64 + 2.0 * reach,
            target.height() as f64 + 2.0 * reach,
        );
        let Some(area) = shape.paint_bounds(self.scale).intersection(&around) else {
            return;
        };
        let (x0, y0) = (area.x.floor() as i32, area.y.floor() as i32);
        let (x1, y1) = (area.right().ceil() as i32, area.bottom().ceil() as i32);
        let Some(mut layer) = Pixmap::new((x1 - x0) as u32, (y1 - y0) as u32) else {
            return;
        };
        self.draw(
            shape,
            &mut layer,
            Transform::from_translate(-x0 as f32, -y0 as f32),
        );
        let at = (x0 - self.origin.0, y0 - self.origin.1);
        effects::draw_with_shadow(
            target,
            &layer,
            at,
            SHADOW_BLUR * self.scale,
            shadow_offset(self.scale),
            SHADOW_ALPHA,
        );
    }

    fn draw(&mut self, shape: &Shape, target: &mut Pixmap, transform: Transform) {
        let w = shape.stroke(self.scale) as f32;
        let color = shape.style.color;
        match &shape.kind {
            Kind::Arrow { from, to } => {
                if let Some(path) = arrow_path(*from, *to, w as f64, self.scale as f64) {
                    target.fill_path(&path, &solid(color), FillRule::Winding, transform, None);
                    // Round off the polygon's corners.
                    let stroke = Stroke {
                        width: w * 0.3,
                        line_join: LineJoin::Round,
                        ..Default::default()
                    };
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
                    if let Some(path) = rounded_rect(
                        *rect,
                        (w as f64 * 0.75).min(rect.width.min(rect.height) / 2.0),
                    ) {
                        target.fill_path(&path, &solid(color), FillRule::Winding, transform, None);
                    }
                } else if let Some(path) = outline(*rect, PathBuilder::from_rect) {
                    let stroke = Stroke {
                        width: w,
                        line_join: LineJoin::Round,
                        ..Default::default()
                    };
                    target.stroke_path(&path, &solid(color), &stroke, transform, None);
                }
            }
            Kind::Ellipse { rect } => {
                if shape.style.fill {
                    if let Some(path) = skia_rect(*rect)
                        .filter(|_| !rect.is_empty())
                        .and_then(PathBuilder::from_oval)
                    {
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
                stroke_points(
                    target,
                    points,
                    &paint,
                    shape.style.highlighter_width() * self.scale,
                    transform,
                );
            }
            Kind::Text { origin, .. } => self.text(shape, *origin, target, transform),
            Kind::Step { center } => self.step(shape, *center, target, transform),
            Kind::Redact { .. } | Kind::Spotlight { .. } => {}
        }
    }

    fn text(&mut self, shape: &Shape, origin: Point, target: &mut Pixmap, transform: Transform) {
        let Some(block) = shape.text_block(self.scale) else {
            return;
        };
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
            target.fill_path(
                &disc,
                &solid(shape.style.color),
                FillRule::Winding,
                transform,
                None,
            );
            let stroke = Stroke {
                width: ring,
                ..Default::default()
            };
            target.stroke_path(&disc, &solid(Color::WHITE), &stroke, transform, None);
        }
        let number = self.doc.step_number(shape.id).to_string();
        let font = d * if number.len() <= 2 { 0.52 } else { 0.4 };
        let block = TextBlock::new(&number, font);
        // Centre the digits themselves (cap height), not the line box.
        let (x, y) = map(transform, center);
        let top = y + font * CAP_HEIGHT / 2.0 - block.baseline();
        block.draw(
            target,
            x - block.width() / 2.0,
            top,
            shape.style.color.contrasting(),
        );
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
    Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    }
}

fn skia_rect(r: Rect) -> Option<tiny_skia::Rect> {
    tiny_skia::Rect::from_xywh(r.x as f32, r.y as f32, r.width as f32, r.height as f32)
}

/// A box shape's path (`PathBuilder::from_rect` or `from_oval`). A flat one still
/// strokes, as a line, but a point has no outline and tiny-skia fails on it (as on a
/// drag's first frame).
fn outline<P: Into<Option<Path>>>(
    r: Rect,
    path: impl FnOnce(tiny_skia::Rect) -> P,
) -> Option<Path> {
    if r.width <= 0.0 && r.height <= 0.0 {
        return None;
    }
    skia_rect(r).and_then(|r| path(r).into())
}

/// A freehand stroke, smoothed by running quadratic curves through the midpoints.
fn stroke_points(
    target: &mut Pixmap,
    points: &[Point],
    paint: &Paint,
    width: f32,
    transform: Transform,
) {
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
        pb.quad_to(
            a.x as f32,
            a.y as f32,
            ((a.x + b.x) / 2.0) as f32,
            ((a.y + b.y) / 2.0) as f32,
        );
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
        let p = Point::new(
            to.x - dx * along + nx * across,
            to.y - dy * along + ny * across,
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::Style;

    fn white(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h, PixelFormat::Rgba);
        img.blit(
            &Image::from_raw(
                w,
                h,
                w as usize * 4,
                PixelFormat::Rgba,
                vec![255; (w * h * 4) as usize],
            ),
            0,
            0,
        );
        img
    }

    fn add(doc: &mut Document, kind: Kind, style: Style) {
        let s = doc.make(kind, style);
        doc.add(s);
    }

    #[test]
    fn export_draws_shapes_and_crops() {
        let mut doc = Document::new(&white(200, 100), 1.0);
        add(
            &mut doc,
            Kind::Rectangle {
                rect: Rect::new(20.0, 20.0, 60.0, 40.0),
            },
            Style {
                fill: true,
                ..Style::default()
            },
        );
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
            let path = if oval {
                outline(r, PathBuilder::from_oval)
            } else {
                outline(r, PathBuilder::from_rect)
            };
            path.and_then(|p| p.stroke(&round_stroke(4.0), 1.0))
                .is_some_and(|p| {
                    let b = p.bounds();
                    b.width() > 1.0 && b.height() > 1.0
                })
        };
        for oval in [false, true] {
            assert!(
                strokes(30.0, 20.0, oval) && strokes(30.0, 0.0, oval) && strokes(0.0, 20.0, oval)
            );
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
        add(
            &mut doc,
            Kind::Line {
                from: Point::new(10.0, 50.0),
                to: Point::new(90.0, 50.0),
            },
            Style::default(),
        );
        let out = doc.export();
        // Just below the 4px line: shadowed, so darker than the white page.
        let below = out.rgba_at(50, 54);
        assert!(below[0] < 250 && below[0] > 150, "{below:?}");
        assert_eq!(out.rgba_at(50, 50), [0xff, 0x3b, 0x30, 255]);
    }

    #[test]
    fn spotlight_dims_outside_only() {
        let mut doc = Document::new(&white(100, 100), 1.0);
        add(
            &mut doc,
            Kind::Spotlight {
                rect: Rect::new(20.0, 20.0, 40.0, 40.0),
            },
            Style::default(),
        );
        add(
            &mut doc,
            Kind::Spotlight {
                rect: Rect::new(40.0, 40.0, 40.0, 40.0),
            },
            Style::default(),
        );
        let out = doc.export();
        assert_eq!(out.rgba_at(40, 40), [255, 255, 255, 255]);
        assert_eq!(out.rgba_at(70, 70), [255, 255, 255, 255]);
        let dim = out.rgba_at(5, 5);
        assert!((110..=120).contains(&dim[0]), "{dim:?}");
    }

    #[test]
    fn highlighter_multiplies() {
        let mut doc = Document::new(&white(100, 40), 1.0);
        let yellow = Style {
            color: Color::rgb(255, 204, 0),
            ..Style::default()
        };
        add(
            &mut doc,
            Kind::Highlighter {
                points: vec![Point::new(10.0, 20.0), Point::new(90.0, 20.0)],
            },
            yellow,
        );
        assert_eq!(doc.export().rgba_at(50, 20), [255, 204, 0, 255]);
    }

    /// A patterned image with every kind of shape on it, several crossing each other.
    fn busy(scale: f32) -> Document {
        let (w, h) = (160, 120);
        let data = (0..w * h).flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [
                (x * 7 % 256) as u8,
                (y * 5 % 256) as u8,
                if (x / 3 + y / 3) % 2 == 0 { 255 } else { 40 },
                255,
            ]
        });
        let mut doc = Document::new(
            &Image::from_raw(w, h, w as usize * 4, PixelFormat::Rgba, data.collect()),
            scale,
        );
        let s = Style::default();
        let p = Point::new;
        let r = Rect::new;
        for (kind, style) in [
            (
                Kind::Redact {
                    rect: r(10.0, 10.0, 50.0, 40.0),
                    mode: Redaction::Blur,
                },
                s,
            ),
            (
                Kind::Redact {
                    rect: r(40.0, 30.0, 45.0, 37.0),
                    mode: Redaction::Blur,
                },
                Style { size: 8.0, ..s },
            ),
            (
                Kind::Redact {
                    rect: r(95.0, 5.0, 53.0, 41.0),
                    mode: Redaction::Pixelate,
                },
                s,
            ),
            (
                Kind::Spotlight {
                    rect: r(20.0, 60.0, 70.0, 50.0),
                },
                s,
            ),
            (
                Kind::Arrow {
                    from: p(5.0, 115.0),
                    to: p(150.0, 8.0),
                },
                s,
            ),
            (
                Kind::Line {
                    from: p(3.0, 50.0),
                    to: p(157.0, 53.0),
                },
                Style { size: 8.0, ..s },
            ),
            (
                Kind::Rectangle {
                    rect: r(30.0, 20.0, 100.0, 80.0),
                },
                s,
            ),
            (
                Kind::Ellipse {
                    rect: r(60.0, 40.0, 70.0, 50.0),
                },
                Style { fill: true, ..s },
            ),
            (
                Kind::Pen {
                    points: (0..30)
                        .map(|i| p(10.0 + i as f64 * 5.0, 90.0 + (i as f64 / 3.0).sin() * 20.0))
                        .collect(),
                },
                s,
            ),
            (
                Kind::Highlighter {
                    points: vec![p(5.0, 70.0), p(150.0, 75.0)],
                },
                Style {
                    color: Color::rgb(255, 204, 0),
                    ..s
                },
            ),
            (
                Kind::Text {
                    origin: p(70.0, 60.0),
                    text: "Tiles".into(),
                },
                s,
            ),
            (
                Kind::Step {
                    center: p(100.0, 100.0),
                },
                s,
            ),
        ] {
            let kind = match kind {
                Kind::Pen { points } => Kind::Pen {
                    points: points
                        .into_iter()
                        .map(|q| p(q.x * scale as f64, q.y * scale as f64))
                        .collect(),
                },
                other => other,
            };
            add(&mut doc, kind, style);
        }
        doc
    }

    /// Render `doc` in `size`px tiles and list the pixels that differ from the export by
    /// more than rounding: where, and by how much.
    fn tile_differences(doc: &Document, size: u32) -> Vec<(u32, u32, u8)> {
        let full = doc.export();
        let mut differences = Vec::new();
        for y in (0..full.height()).step_by(size as usize) {
            for x in (0..full.width()).step_by(size as usize) {
                let (w, h) = (size.min(full.width() - x), size.min(full.height() - y));
                let rect = IntRect::from_xywh(x as i32, y as i32, w, h).unwrap();
                let tile = image_from_pixmap(&render_area(doc, rect, |_| true));
                for (ty, tx) in (0..h).flat_map(|ty| (0..w).map(move |tx| (ty, tx))) {
                    let (a, b) = (tile.rgba_at(tx, ty), full.rgba_at(x + tx, y + ty));
                    let diff = a.iter().zip(b).map(|(a, b)| a.abs_diff(b)).max().unwrap();
                    if diff > 2 {
                        differences.push((x + tx, y + ty, diff));
                    }
                }
            }
        }
        differences
    }

    #[test]
    fn tiles_put_together_match_the_export() {
        // Shadows, redactions reading around themselves (one over another) and the
        // spotlight dim: everything tiling itself is responsible for. Straight edges and
        // text rasterize the same however they're cut.
        let mut doc = busy(1.0);
        doc.state_mut().shapes.retain(|s| {
            matches!(
                s.kind,
                Kind::Redact { .. }
                    | Kind::Spotlight { .. }
                    | Kind::Rectangle { .. }
                    | Kind::Text { .. }
            )
        });
        for size in [16, 37, 64] {
            assert_eq!(tile_differences(&doc, size), [], "{size}px tiles");
        }
    }

    #[test]
    fn tiles_differ_from_the_export_only_in_antialiasing() {
        // tiny-skia antialiases a path a little differently where a tile cuts it off: a
        // few edge pixels may be off a little (and the shadows they cast, a hair).
        // Anything missing or misplaced would be off a lot, or across many pixels.
        for scale in [1.0, 2.0] {
            let doc = busy(scale);
            let pixels = (doc.width() * doc.height()) as usize;
            for size in [16, 37, 64] {
                let off = tile_differences(&doc, size);
                let worst = off.iter().max_by_key(|d| d.2);
                assert!(
                    worst.is_none_or(|d| d.2 <= 64),
                    "{size}px tiles at scale {scale}: {worst:?}"
                );
                let noticeable = off.iter().filter(|d| d.2 > 4).count();
                assert!(
                    noticeable * 200 < pixels,
                    "{size}px tiles at scale {scale}: {noticeable} pixels off"
                );
            }
        }
    }

    #[test]
    fn arrowheads_scale_with_width_but_fit_short_arrows() {
        let long = arrow_path(Point::new(0.0, 0.0), Point::new(200.0, 0.0), 4.0, 1.0)
            .unwrap()
            .bounds();
        let thick = arrow_path(Point::new(0.0, 0.0), Point::new(200.0, 0.0), 8.0, 1.0)
            .unwrap()
            .bounds();
        assert!(thick.height() > long.height());
        let short = arrow_path(Point::new(0.0, 0.0), Point::new(10.0, 0.0), 8.0, 1.0)
            .unwrap()
            .bounds();
        assert!(short.width() <= 10.5, "{short:?}");
    }

    #[test]
    fn pixelate_hides_detail() {
        let mut img = white(64, 64);
        img.blit(
            &Image::from_raw(2, 2, 8, PixelFormat::Rgba, [0, 0, 0, 255].repeat(4)),
            10,
            10,
        );
        let mut doc = Document::new(&img, 1.0);
        add(
            &mut doc,
            Kind::Redact {
                rect: Rect::new(0.0, 0.0, 64.0, 64.0),
                mode: Redaction::Pixelate,
            },
            Style::default(),
        );
        let out = doc.export();
        // The black speck is averaged into its 12px block.
        let p = out.rgba_at(10, 10);
        assert!(p[0] > 200 && p[0] < 255, "{p:?}");
        assert_eq!(out.rgba_at(10, 10), out.rgba_at(0, 0));
    }
}
