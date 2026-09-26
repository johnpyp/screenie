//! A document: the captured image, the shapes on it, an optional crop, and undo history.

use std::sync::Arc;

use screenie_core::{Image, Point, Rect};
use tiny_skia::Pixmap;

use crate::shape::{Kind, Shape, ShapeId, Style};

/// The part of a document that undo restores.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub shapes: Vec<Shape>,
    /// Image-pixel rectangle to export. Cropping is non-destructive until export.
    pub crop: Option<Rect>,
}

pub struct Document {
    base: Arc<Pixmap>,
    /// Image pixels per logical pixel (the capture's output scale), so a stroke width
    /// means the same on a HiDPI capture as on a regular one.
    scale: f32,
    state: State,
    next_id: u64,
    undo: Vec<State>,
    redo: Vec<State>,
}

const HISTORY_LIMIT: usize = 200;

impl Document {
    pub fn new(image: &Image, scale: f32) -> Self {
        let state = State { shapes: Vec::new(), crop: None };
        Self {
            base: Arc::new(crate::render::pixmap_from_image(image)),
            scale: if scale.is_finite() && scale > 0.0 { scale } else { 1.0 },
            state,
            next_id: 1,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn base(&self) -> &Arc<Pixmap> {
        &self.base
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn width(&self) -> u32 {
        self.base.width()
    }

    pub fn height(&self) -> u32 {
        self.base.height()
    }

    /// The whole image as a rectangle.
    pub fn bounds(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width() as f64, self.height() as f64)
    }

    /// What gets exported: the crop, or the whole image.
    pub fn visible(&self) -> Rect {
        self.state.crop.unwrap_or_else(|| self.bounds())
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn shapes(&self) -> &[Shape] {
        &self.state.shapes
    }

    pub fn crop(&self) -> Option<Rect> {
        self.state.crop
    }

    pub fn shape(&self, id: ShapeId) -> Option<&Shape> {
        self.state.shapes.iter().find(|s| s.id == id)
    }

    /// A new shape (not yet in the document).
    pub fn make(&mut self, kind: Kind, style: Style) -> Shape {
        let id = ShapeId(self.next_id);
        self.next_id += 1;
        Shape { id, kind, style }
    }

    /// The topmost shape under `p`.
    pub fn hit(&self, p: Point, tolerance: f64) -> Option<ShapeId> {
        self.state.shapes.iter().rev().find(|s| s.hit(p, tolerance, self.scale)).map(|s| s.id)
    }

    /// The number a step shape shows: its position among steps.
    pub fn step_number(&self, id: ShapeId) -> usize {
        let steps = self.state.shapes.iter().filter(|s| matches!(s.kind, Kind::Step { .. }));
        steps.clone().position(|s| s.id == id).unwrap_or_else(|| steps.count()) + 1
    }

    // Edits. Each one is a single undo step; `checkpoint` lets a gesture (a drag) make
    // many changes that undo as one.

    /// Remember the current state as an undo step.
    pub fn checkpoint(&mut self) {
        self.undo.push(self.state.clone());
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Drop the last checkpoint if nothing changed since (a click that didn't drag).
    pub fn discard_checkpoint_if_unchanged(&mut self) {
        if self.undo.last() == Some(&self.state) {
            self.undo.pop();
        }
    }

    /// Mutate without recording an undo step (call [`Self::checkpoint`] first).
    pub fn state_mut(&mut self) -> &mut State {
        &mut self.state
    }

    pub fn shape_mut(&mut self, id: ShapeId) -> Option<&mut Shape> {
        self.state.shapes.iter_mut().find(|s| s.id == id)
    }

    pub fn add(&mut self, shape: Shape) {
        self.checkpoint();
        self.state.shapes.push(shape);
    }

    pub fn remove(&mut self, id: ShapeId) -> Option<Shape> {
        let index = self.state.shapes.iter().position(|s| s.id == id)?;
        self.checkpoint();
        Some(self.state.shapes.remove(index))
    }

    /// Replace a shape's style (one undo step).
    pub fn restyle(&mut self, id: ShapeId, style: Style) {
        if self.shape(id).is_some_and(|s| s.style != style) {
            self.checkpoint();
            if let Some(s) = self.shape_mut(id) {
                s.style = style;
            }
        }
    }

    pub fn set_crop(&mut self, crop: Option<Rect>) {
        let crop = crop.and_then(|c| c.intersection(&self.bounds())).map(|c| c.round()).filter(|c| *c != self.bounds());
        if crop != self.state.crop {
            self.checkpoint();
            self.state.crop = crop;
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.state, previous));
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.state, next));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenie_core::PixelFormat;

    fn doc() -> Document {
        Document::new(&Image::new(200, 100, PixelFormat::Rgbx), 1.0)
    }

    fn step(doc: &mut Document, x: f64) -> ShapeId {
        let s = doc.make(Kind::Step { center: Point::new(x, 50.0) }, Style::default());
        let id = s.id;
        doc.add(s);
        id
    }

    #[test]
    fn undo_and_redo_walk_history() {
        let mut d = doc();
        let a = step(&mut d, 20.0);
        let b = step(&mut d, 80.0);
        assert_eq!(d.shapes().len(), 2);
        assert!(d.undo());
        assert_eq!(d.shapes().len(), 1);
        assert!(d.redo());
        assert_eq!(d.shapes().len(), 2);
        d.remove(a);
        assert_eq!(d.shapes().iter().map(|s| s.id).collect::<Vec<_>>(), vec![b]);
        // A new edit clears redo.
        d.undo();
        step(&mut d, 150.0);
        assert!(!d.can_redo());
    }

    #[test]
    fn steps_renumber_when_one_is_removed() {
        let mut d = doc();
        let a = step(&mut d, 20.0);
        let b = step(&mut d, 60.0);
        let c = step(&mut d, 100.0);
        assert_eq!((d.step_number(a), d.step_number(b), d.step_number(c)), (1, 2, 3));
        d.remove(a);
        assert_eq!((d.step_number(b), d.step_number(c)), (1, 2));
    }

    #[test]
    fn unchanged_gestures_leave_no_undo_step() {
        let mut d = doc();
        d.checkpoint();
        d.discard_checkpoint_if_unchanged();
        assert!(!d.can_undo());
    }

    #[test]
    fn crop_is_clamped_and_whole_image_means_none() {
        let mut d = doc();
        d.set_crop(Some(Rect::new(-10.0, 10.0, 100.5, 50.2)));
        assert_eq!(d.crop(), Some(Rect::new(0.0, 10.0, 91.0, 50.0)));
        assert_eq!(d.visible(), Rect::new(0.0, 10.0, 91.0, 50.0));
        d.set_crop(Some(Rect::new(0.0, 0.0, 300.0, 300.0)));
        assert_eq!(d.crop(), None);
    }
}
