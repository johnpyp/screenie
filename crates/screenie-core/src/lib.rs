//! Shared domain types for screenie. Deliberately free of GUI and Wayland dependencies so
//! every other crate can use it and it stays fast to compile and easy to test.

pub mod desktop;
pub mod geom;
pub mod image;
pub mod stream;

pub use desktop::{OutputCapture, OutputInfo, Snapshot, WindowInfo};
pub use geom::{PixelRect, Point, Rect, Size, Transform};
pub use image::{Image, ImageError, PixelFormat};
pub use stream::{FrameSource, Next, SourceError};
