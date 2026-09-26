//! Geometry in two coordinate spaces.
//!
//! * **Logical** coordinates ([`Point`], [`Size`], [`Rect`]) are the compositor's global
//!   layout space, the one window positions and pointer events use. They are `f64` because
//!   fractional scaling makes a logical pixel a non-integer number of physical pixels.
//! * **Physical** coordinates ([`PixelRect`]) index into captured pixel buffers.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn offset(self, dx: f64, dy: f64) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }

    pub fn distance(self, other: Point) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// An axis-aligned rectangle in logical coordinates. Width and height are never negative
/// for rectangles built with [`Rect::from_corners`].
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The rectangle spanned by two opposite corners, in any order.
    pub fn from_corners(a: Point, b: Point) -> Self {
        let (x0, x1) = if a.x <= b.x { (a.x, b.x) } else { (b.x, a.x) };
        let (y0, y1) = if a.y <= b.y { (a.y, b.y) } else { (b.y, a.y) };
        Self::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn origin(&self) -> Point {
        Point::new(self.x, self.y)
    }

    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }

    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn area(&self) -> f64 {
        self.width.max(0.0) * self.height.max(0.0)
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    /// Half-open containment: the right and bottom edges are outside.
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    pub fn contains_rect(&self, other: &Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self.right().max(other.right());
        let y1 = self.bottom().max(other.bottom());
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn translate(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    pub fn inset(&self, d: f64) -> Rect {
        Rect::new(
            self.x + d,
            self.y + d,
            self.width - 2.0 * d,
            self.height - 2.0 * d,
        )
    }

    /// Move (without resizing, where possible) so the rectangle lies inside `bounds`.
    pub fn clamp_within(&self, bounds: &Rect) -> Rect {
        let width = self.width.min(bounds.width);
        let height = self.height.min(bounds.height);
        let x = self.x.clamp(bounds.x, bounds.right() - width);
        let y = self.y.clamp(bounds.y, bounds.bottom() - height);
        Rect::new(x, y, width, height)
    }

    /// Snap edges to the nearest whole logical pixel.
    pub fn round(&self) -> Rect {
        let x0 = self.x.round();
        let y0 = self.y.round();
        let x1 = self.right().round();
        let y1 = self.bottom().round();
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// The physical pixel rectangle this logical rectangle covers at `scale`, relative to
    /// `origin` (the logical position of the buffer's top-left corner).
    pub fn to_pixels(&self, origin: Point, scale: f64) -> PixelRect {
        let x0 = ((self.x - origin.x) * scale).round() as i64;
        let y0 = ((self.y - origin.y) * scale).round() as i64;
        let x1 = ((self.right() - origin.x) * scale).round() as i64;
        let y1 = ((self.bottom() - origin.y) * scale).round() as i64;
        PixelRect {
            x: x0 as i32,
            y: y0 as i32,
            width: (x1 - x0).max(0) as u32,
            height: (y1 - y0).max(0) as u32,
        }
    }
}

impl std::fmt::Display for Rect {
    /// `X,Y WxH`, the format slurp and grim use.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{} {}x{}", self.x, self.y, self.width, self.height)
    }
}

impl std::str::FromStr for Rect {
    type Err = String;

    /// Parses `X,Y WxH` (slurp/grim) or `WxH+X+Y` (X11 geometry).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || format!("invalid geometry {s:?}; expected \"X,Y WxH\" or \"WxH+X+Y\"");
        let num = |v: &str| v.trim().parse::<f64>().map_err(|_| bad());
        if let Some((pos, size)) = s.split_once(' ') {
            let (x, y) = pos.split_once(',').ok_or_else(bad)?;
            let (w, h) = size.split_once('x').ok_or_else(bad)?;
            return Ok(Rect::new(num(x)?, num(y)?, num(w)?, num(h)?));
        }
        let (w, rest) = s.split_once('x').ok_or_else(bad)?;
        let rest = rest.replace('-', "+-");
        let mut parts = rest.split('+').filter(|p| !p.is_empty());
        let h = num(parts.next().ok_or_else(bad)?)?;
        let x = num(parts.next().ok_or_else(bad)?)?;
        let y = num(parts.next().ok_or_else(bad)?)?;
        Ok(Rect::new(x, y, num(w)?, h))
    }
}

/// A rectangle of physical pixels within some buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PixelRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn intersection(&self, other: &PixelRect) -> Option<PixelRect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        (x1 > x0 && y1 > y0).then(|| PixelRect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
    }
}

/// How an output's content is rotated/flipped relative to its physical panel, mirroring
/// `wl_output.transform`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transform {
    #[default]
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl Transform {
    /// Whether width and height swap between the buffer and the logical layout.
    pub fn swaps_axes(self) -> bool {
        matches!(
            self,
            Transform::Rotate90
                | Transform::Rotate270
                | Transform::Flipped90
                | Transform::Flipped270
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_normalize() {
        let r = Rect::from_corners(Point::new(10.0, 20.0), Point::new(5.0, 2.0));
        assert_eq!(r, Rect::new(5.0, 2.0, 5.0, 18.0));
    }

    #[test]
    fn intersection_and_containment() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0);
        let b = Rect::new(50.0, 50.0, 100.0, 100.0);
        assert_eq!(a.intersection(&b), Some(Rect::new(50.0, 50.0, 50.0, 50.0)));
        assert_eq!(a.intersection(&Rect::new(100.0, 0.0, 5.0, 5.0)), None);
        assert!(a.contains(Point::new(0.0, 0.0)));
        assert!(!a.contains(Point::new(100.0, 50.0)));
    }

    #[test]
    fn fractional_scale_pixels() {
        // A 1.5x output at logical x=1920: logical 1920..2020 is physical 0..150.
        let r = Rect::new(1920.0, 10.0, 100.0, 20.0);
        let px = r.to_pixels(Point::new(1920.0, 0.0), 1.5);
        assert_eq!(px, PixelRect::new(0, 15, 150, 30));
    }

    #[test]
    fn parse_geometry() {
        assert_eq!(
            "10,20 300x400".parse::<Rect>().unwrap(),
            Rect::new(10.0, 20.0, 300.0, 400.0)
        );
        assert_eq!(
            "300x400+10+20".parse::<Rect>().unwrap(),
            Rect::new(10.0, 20.0, 300.0, 400.0)
        );
        assert_eq!(
            "300x400+-10+20".parse::<Rect>().unwrap(),
            Rect::new(-10.0, 20.0, 300.0, 400.0)
        );
        assert!("nope".parse::<Rect>().is_err());
    }

    #[test]
    fn clamp_within_bounds() {
        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0);
        let r = Rect::new(90.0, -5.0, 20.0, 20.0).clamp_within(&bounds);
        assert_eq!(r, Rect::new(80.0, 0.0, 20.0, 20.0));
    }
}
