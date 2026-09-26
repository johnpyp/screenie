//! Shapes: what they are, where they are, and how they respond to editing (hit testing,
//! handles, moving). All coordinates are image pixels.

use screenie_core::{Point, Rect};

use crate::Color;
use crate::text::TextBlock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShapeId(pub(crate) u64);

/// How a redaction hides what's underneath.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Redaction {
    #[default]
    Pixelate,
    Blur,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Arrow {
        from: Point,
        to: Point,
    },
    Line {
        from: Point,
        to: Point,
    },
    Rectangle {
        rect: Rect,
    },
    Ellipse {
        rect: Rect,
    },
    /// A freehand stroke.
    Pen {
        points: Vec<Point>,
    },
    /// A translucent (multiplied) marker stroke.
    Highlighter {
        points: Vec<Point>,
    },
    /// `origin` is the top-left of the first line.
    Text {
        origin: Point,
        text: String,
    },
    /// A numbered circle. Numbers follow the order of steps in the document.
    Step {
        center: Point,
    },
    Redact {
        rect: Rect,
        mode: Redaction,
    },
    /// Dims everything outside the spotlit rectangles.
    Spotlight {
        rect: Rect,
    },
}

/// Appearance shared by every shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub color: Color,
    /// Stroke width in logical pixels (see [`crate::Document::scale`]). Other sizes (text,
    /// steps, redaction strength) derive from it, so one control sizes every tool.
    pub size: f32,
    /// Filled rectangles/ellipses, and text on a coloured label.
    pub fill: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            color: Color::rgb(0xff, 0x3b, 0x30),
            size: 4.0,
            fill: false,
        }
    }
}

impl Style {
    /// The stroke widths the editor steps through.
    pub const SIZES: [f32; 10] = [1.0, 2.0, 4.0, 6.0, 8.0, 12.0, 16.0, 20.0, 26.0, 32.0];

    /// The size `steps` presets up (or down, if negative) from `size`, which needn't be
    /// a preset itself.
    pub fn step_size(size: f32, steps: i32) -> f32 {
        let sizes = Self::SIZES;
        let last = sizes.len() as i32 - 1;
        // The first preset at or above `size`; a size between two presets steps to its
        // neighbours as if it sat just below the upper one.
        let above = sizes
            .iter()
            .position(|s| *s >= size)
            .map_or(last + 1, |i| i as i32);
        let exact = sizes.get(above as usize) == Some(&size);
        let target = if steps > 0 && !exact {
            above + steps - 1
        } else {
            above + steps
        };
        sizes[target.clamp(0, last) as usize]
    }

    /// Where `size` sits among the presets, from 1.
    pub fn size_index(size: f32) -> usize {
        Self::SIZES
            .iter()
            .position(|s| *s >= size)
            .unwrap_or(Self::SIZES.len() - 1)
            + 1
    }

    pub fn font_size(&self) -> f32 {
        8.0 + self.size * 3.5
    }

    pub fn step_diameter(&self) -> f32 {
        16.0 + self.size * 4.0
    }

    pub fn highlighter_width(&self) -> f32 {
        8.0 + self.size * 3.0
    }

    pub fn pixel_block(&self) -> f32 {
        4.0 + self.size * 2.0
    }

    pub fn blur_radius(&self) -> f32 {
        3.0 + self.size * 1.5
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub id: ShapeId,
    pub kind: Kind,
    pub style: Style,
}

/// A draggable point on a selected shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Start,
    End,
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    pub const BOX: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// Corner handles are drawn; edge handles work but stay invisible.
    pub fn is_visible(self) -> bool {
        !matches!(
            self,
            Handle::Top | Handle::Right | Handle::Bottom | Handle::Left
        )
    }

    /// Where this handle sits on a box.
    pub fn position(self, r: &Rect) -> Point {
        let (cx, cy) = (r.x + r.width / 2.0, r.y + r.height / 2.0);
        match self {
            Handle::TopLeft | Handle::Start => Point::new(r.x, r.y),
            Handle::Top => Point::new(cx, r.y),
            Handle::TopRight => Point::new(r.right(), r.y),
            Handle::Right => Point::new(r.right(), cy),
            Handle::BottomRight | Handle::End => Point::new(r.right(), r.bottom()),
            Handle::Bottom => Point::new(cx, r.bottom()),
            Handle::BottomLeft => Point::new(r.x, r.bottom()),
            Handle::Left => Point::new(r.x, cy),
        }
    }
}

impl Shape {
    /// Stroke width in image pixels.
    pub fn stroke(&self, scale: f32) -> f64 {
        (self.style.size * scale) as f64
    }

    /// Whether the shape draws with a soft drop shadow.
    pub fn has_shadow(&self) -> bool {
        !matches!(
            self.kind,
            Kind::Highlighter { .. } | Kind::Redact { .. } | Kind::Spotlight { .. }
        )
    }

    /// Whether the whole area counts when clicking, rather than just the stroke.
    fn is_solid(&self) -> bool {
        match self.kind {
            Kind::Rectangle { .. } | Kind::Ellipse { .. } => self.style.fill,
            Kind::Text { .. }
            | Kind::Step { .. }
            | Kind::Redact { .. }
            | Kind::Spotlight { .. } => true,
            _ => false,
        }
    }

    /// Text layout for text shapes.
    pub fn text_block(&self, scale: f32) -> Option<TextBlock> {
        match &self.kind {
            Kind::Text { text, .. } => Some(TextBlock::new(text, self.style.font_size() * scale)),
            _ => None,
        }
    }

    /// Padding around a text shape's label (when filled), in image pixels.
    pub fn label_padding(&self, scale: f32) -> (f64, f64) {
        let size = (self.style.font_size() * scale) as f64;
        (size * 0.35, size * 0.18)
    }

    /// The geometry's bounds, excluding stroke width and shadow.
    pub fn bounds(&self, scale: f32) -> Rect {
        match &self.kind {
            Kind::Arrow { from, to } | Kind::Line { from, to } => Rect::from_corners(*from, *to),
            Kind::Rectangle { rect }
            | Kind::Ellipse { rect }
            | Kind::Redact { rect, .. }
            | Kind::Spotlight { rect } => *rect,
            Kind::Pen { points } | Kind::Highlighter { points } => points_bounds(points),
            Kind::Text { origin, .. } => {
                let block = self.text_block(scale).expect("text shape");
                let (px, py) = if self.style.fill {
                    self.label_padding(scale)
                } else {
                    (0.0, 0.0)
                };
                Rect::new(
                    origin.x - px,
                    origin.y - py,
                    block.width() as f64 + px * 2.0,
                    block.height() as f64 + py * 2.0,
                )
            }
            Kind::Step { center } => {
                let r = (self.style.step_diameter() * scale / 2.0) as f64;
                Rect::new(center.x - r, center.y - r, r * 2.0, r * 2.0)
            }
        }
    }

    /// Everything the shape may paint, including stroke, arrowhead and shadow.
    pub fn paint_bounds(&self, scale: f32) -> Rect {
        let w = self.stroke(scale);
        let margin = match &self.kind {
            Kind::Arrow { .. } => w * 4.0 + 8.0 * scale as f64,
            Kind::Highlighter { .. } => (self.style.highlighter_width() * scale) as f64 / 2.0 + 1.0,
            Kind::Line { .. }
            | Kind::Rectangle { .. }
            | Kind::Ellipse { .. }
            | Kind::Pen { .. } => w,
            Kind::Text { .. } | Kind::Step { .. } => 2.0 * scale as f64,
            Kind::Redact { .. } | Kind::Spotlight { .. } => 0.0,
        };
        let shadow = if self.has_shadow() {
            crate::render::shadow_margin(scale)
        } else {
            0.0
        };
        self.bounds(scale).inset(-(margin + shadow + 1.0))
    }

    /// Whether `p` is on the shape, within `tolerance` image pixels.
    pub fn hit(&self, p: Point, tolerance: f64, scale: f32) -> bool {
        let half = self.stroke(scale) / 2.0 + tolerance;
        match &self.kind {
            Kind::Arrow { from, to } | Kind::Line { from, to } => {
                segment_distance(p, *from, *to) <= half.max(tolerance * 1.5)
            }
            Kind::Pen { points } => polyline_distance(p, points) <= half,
            Kind::Highlighter { points } => {
                polyline_distance(p, points)
                    <= (self.style.highlighter_width() * scale) as f64 / 2.0 + tolerance
            }
            Kind::Rectangle { rect } if !self.is_solid() => {
                rect.inset(-half).contains(p) && !rect.inset(half).contains(p)
            }
            Kind::Ellipse { rect } => ellipse_hit(rect, p, half, self.is_solid()),
            Kind::Step { center } => {
                center.distance(p) <= (self.style.step_diameter() * scale) as f64 / 2.0 + tolerance
            }
            _ => self.bounds(scale).inset(-tolerance).contains(p),
        }
    }

    /// Handles to show when selected.
    pub fn handles(&self) -> Vec<(Handle, Point)> {
        match &self.kind {
            Kind::Arrow { from, to } | Kind::Line { from, to } => {
                vec![(Handle::Start, *from), (Handle::End, *to)]
            }
            Kind::Rectangle { rect }
            | Kind::Ellipse { rect }
            | Kind::Redact { rect, .. }
            | Kind::Spotlight { rect } => {
                Handle::BOX.iter().map(|h| (*h, h.position(rect))).collect()
            }
            // Freehand strokes, text and steps only move.
            _ => Vec::new(),
        }
    }

    /// Drag `handle` to `p`. `constrain` (Shift) snaps lines to 15° steps and keeps
    /// boxes square.
    pub fn drag_handle(&mut self, handle: Handle, p: Point, constrain: bool) {
        match &mut self.kind {
            Kind::Arrow { from, to } | Kind::Line { from, to } => {
                let (moving, anchor) = if handle == Handle::Start {
                    (from, *to)
                } else {
                    (to, *from)
                };
                *moving = if constrain { snap_angle(anchor, p) } else { p };
            }
            Kind::Rectangle { rect }
            | Kind::Ellipse { rect }
            | Kind::Redact { rect, .. }
            | Kind::Spotlight { rect } => {
                *rect = resize_box(*rect, handle, p, constrain);
            }
            _ => {}
        }
    }

    pub fn translate(&mut self, dx: f64, dy: f64) {
        let mv = |p: &mut Point| *p = p.offset(dx, dy);
        match &mut self.kind {
            Kind::Arrow { from, to } | Kind::Line { from, to } => {
                mv(from);
                mv(to);
            }
            Kind::Rectangle { rect }
            | Kind::Ellipse { rect }
            | Kind::Redact { rect, .. }
            | Kind::Spotlight { rect } => {
                *rect = rect.translate(dx, dy);
            }
            Kind::Pen { points } | Kind::Highlighter { points } => points.iter_mut().for_each(mv),
            Kind::Text { origin, .. } => mv(origin),
            Kind::Step { center } => mv(center),
        }
    }

    /// Whether the shape is too small to keep (a click that was meant as nothing).
    pub fn is_degenerate(&self) -> bool {
        match &self.kind {
            Kind::Arrow { from, to } | Kind::Line { from, to } => from.distance(*to) < 3.0,
            Kind::Rectangle { rect }
            | Kind::Ellipse { rect }
            | Kind::Redact { rect, .. }
            | Kind::Spotlight { rect } => rect.width < 3.0 || rect.height < 3.0,
            Kind::Pen { points } | Kind::Highlighter { points } => points.is_empty(),
            Kind::Text { text, .. } => text.trim().is_empty(),
            Kind::Step { .. } => false,
        }
    }
}

/// Snap `p` so the segment from `anchor` is at a multiple of 15°.
pub fn snap_angle(anchor: Point, p: Point) -> Point {
    let (dx, dy) = (p.x - anchor.x, p.y - anchor.y);
    let len = dx.hypot(dy);
    let step = std::f64::consts::PI / 12.0;
    let angle = (dy.atan2(dx) / step).round() * step;
    Point::new(anchor.x + len * angle.cos(), anchor.y + len * angle.sin())
}

/// The box spanned by `anchor` and `p`, made square when `constrain`.
pub fn box_from_drag(anchor: Point, p: Point, constrain: bool) -> Rect {
    if !constrain {
        return Rect::from_corners(anchor, p);
    }
    let (dx, dy) = (p.x - anchor.x, p.y - anchor.y);
    let side = dx.abs().max(dy.abs());
    Rect::from_corners(
        anchor,
        Point::new(anchor.x + side * dx.signum(), anchor.y + side * dy.signum()),
    )
}

/// Move one handle of a box, keeping the opposite side (or corner) in place.
pub fn resize_box(r: Rect, handle: Handle, p: Point, constrain: bool) -> Rect {
    let (mut x0, mut y0, mut x1, mut y1) = (r.x, r.y, r.right(), r.bottom());
    match handle {
        Handle::TopLeft | Handle::TopRight | Handle::BottomRight | Handle::BottomLeft => {
            // Anchor at the opposite corner.
            let anchor = match handle {
                Handle::TopLeft => Point::new(x1, y1),
                Handle::TopRight => Point::new(x0, y1),
                Handle::BottomRight => Point::new(x0, y0),
                _ => Point::new(x1, y0),
            };
            return box_from_drag(anchor, p, constrain);
        }
        Handle::Top => y0 = p.y,
        Handle::Bottom => y1 = p.y,
        Handle::Left => x0 = p.x,
        Handle::Right => x1 = p.x,
        Handle::Start | Handle::End => {}
    }
    Rect::from_corners(Point::new(x0, y0), Point::new(x1, y1))
}

fn points_bounds(points: &[Point]) -> Rect {
    let Some(first) = points.first() else {
        return Rect::default();
    };
    points
        .iter()
        .skip(1)
        .fold(Rect::new(first.x, first.y, 0.0, 0.0), |r, p| {
            r.union(&Rect::new(p.x, p.y, 0.0, 0.0))
        })
}

pub(crate) fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.distance(Point::new(a.x + t * dx, a.y + t * dy))
}

fn polyline_distance(p: Point, points: &[Point]) -> f64 {
    match points {
        [] => f64::INFINITY,
        [only] => p.distance(*only),
        _ => points
            .windows(2)
            .map(|w| segment_distance(p, w[0], w[1]))
            .fold(f64::INFINITY, f64::min),
    }
}

fn ellipse_hit(r: &Rect, p: Point, half: f64, solid: bool) -> bool {
    let (rx, ry) = (r.width / 2.0, r.height / 2.0);
    let c = r.center();
    let norm = |rx: f64, ry: f64| {
        if rx <= 0.0 || ry <= 0.0 {
            return f64::INFINITY;
        }
        ((p.x - c.x) / rx).powi(2) + ((p.y - c.y) / ry).powi(2)
    };
    let outer = norm(rx + half, ry + half) <= 1.0;
    if solid {
        return outer;
    }
    outer && (rx <= half || ry <= half || norm(rx - half, ry - half) > 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_step_through_the_presets() {
        assert_eq!(Style::step_size(4.0, 1), 6.0);
        assert_eq!(Style::step_size(4.0, -2), 1.0);
        assert_eq!(Style::step_size(32.0, 1), 32.0);
        assert_eq!(Style::step_size(1.0, -1), 1.0);
        // Off-preset sizes step to their neighbours.
        assert_eq!(Style::step_size(5.0, 1), 6.0);
        assert_eq!(Style::step_size(5.0, -1), 4.0);
        assert_eq!(Style::step_size(40.0, -1), 32.0);
        assert_eq!(Style::step_size(0.5, 1), 1.0);
        assert_eq!(Style::size_index(1.0), 1);
        assert_eq!(Style::size_index(32.0), 10);
    }

    fn shape(kind: Kind) -> Shape {
        Shape {
            id: ShapeId(1),
            kind,
            style: Style::default(),
        }
    }

    #[test]
    fn hollow_shapes_are_hit_on_the_stroke_only() {
        let rect = shape(Kind::Rectangle {
            rect: Rect::new(10.0, 10.0, 100.0, 50.0),
        });
        assert!(rect.hit(Point::new(10.0, 30.0), 3.0, 1.0));
        assert!(!rect.hit(Point::new(60.0, 35.0), 3.0, 1.0));
        let mut filled = rect.clone();
        filled.style.fill = true;
        assert!(filled.hit(Point::new(60.0, 35.0), 3.0, 1.0));

        let ellipse = shape(Kind::Ellipse {
            rect: Rect::new(0.0, 0.0, 100.0, 50.0),
        });
        assert!(ellipse.hit(Point::new(50.0, 1.0), 3.0, 1.0));
        assert!(!ellipse.hit(Point::new(50.0, 25.0), 3.0, 1.0));
        assert!(!ellipse.hit(Point::new(2.0, 2.0), 3.0, 1.0));
    }

    #[test]
    fn lines_are_hit_near_the_segment() {
        let arrow = shape(Kind::Arrow {
            from: Point::new(0.0, 0.0),
            to: Point::new(100.0, 0.0),
        });
        assert!(arrow.hit(Point::new(50.0, 4.0), 3.0, 1.0));
        assert!(!arrow.hit(Point::new(50.0, 12.0), 3.0, 1.0));
        assert!(!arrow.hit(Point::new(120.0, 0.0), 3.0, 1.0));
    }

    #[test]
    fn snapping_uses_15_degree_steps() {
        let p = snap_angle(Point::new(0.0, 0.0), Point::new(100.0, 3.0));
        assert!((p.y).abs() < 1e-9 && (p.x - 100.045).abs() < 0.01);
        let p = snap_angle(Point::new(0.0, 0.0), Point::new(50.0, 52.0));
        assert!((p.x - p.y).abs() < 1e-9);
    }

    #[test]
    fn resizing_keeps_the_opposite_corner() {
        let mut s = shape(Kind::Rectangle {
            rect: Rect::new(10.0, 10.0, 20.0, 20.0),
        });
        s.drag_handle(Handle::TopLeft, Point::new(0.0, 5.0), false);
        assert_eq!(s.bounds(1.0), Rect::new(0.0, 5.0, 30.0, 25.0));
        // Dragging past the anchor flips instead of going negative.
        s.drag_handle(Handle::Right, Point::new(-10.0, 0.0), false);
        assert_eq!(s.bounds(1.0), Rect::new(-10.0, 5.0, 10.0, 25.0));
        s.drag_handle(Handle::BottomRight, Point::new(40.0, 20.0), true);
        let b = s.bounds(1.0);
        assert_eq!(b.width, b.height);
    }

    #[test]
    fn empty_things_are_degenerate() {
        assert!(
            shape(Kind::Text {
                origin: Point::default(),
                text: "  ".into()
            })
            .is_degenerate()
        );
        assert!(
            shape(Kind::Arrow {
                from: Point::default(),
                to: Point::new(1.0, 1.0)
            })
            .is_degenerate()
        );
        assert!(
            !shape(Kind::Step {
                center: Point::default()
            })
            .is_degenerate()
        );
    }
}
