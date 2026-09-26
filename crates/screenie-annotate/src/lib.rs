//! Annotation documents: a captured image plus editable shapes (arrows, boxes, text,
//! numbered steps, redactions…), with undo history and a tiny-skia renderer.
//!
//! No UI dependencies: the editor drives a [`Document`] and draws it with [`render`];
//! export is the same code, so it can be tested headlessly.

mod color;
mod document;
pub mod effects;
pub mod render;
mod shape;
pub mod text;

pub use color::{Color, ParseColorError};
pub use document::{Document, State};
pub use render::{image_from_pixmap, pixmap_from_image};
pub use shape::{Handle, Kind, Redaction, Shape, ShapeId, Style, box_from_drag, resize_box, snap_angle};
pub use tiny_skia;
