//! Shared domain types for screenie. Deliberately free of GUI and Wayland dependencies so
//! every other crate can use it and it stays fast to compile and easy to test.

pub mod desktop;
pub mod geom;
pub mod gpu;
pub mod image;
pub mod stream;

pub use desktop::{OutputCapture, OutputInfo, Snapshot, WindowInfo};
pub use geom::{PixelRect, Point, Rect, Size, Transform};
pub use gpu::{Dmabuf, DmabufFormat, DmabufPlane, GpuDevice, GpuOffer};
pub use image::{Image, ImageError, PixelFormat};
pub use stream::{Frame, FrameSource, Next, Pacer, Pixels, Pointer, SourceError};

/// Screenie's application id: its windows' app id, its desktop entry's name
/// (`dev.johnpyp.Screenie.desktop`), and who it is to the portals.
pub const APP_ID: &str = "dev.johnpyp.Screenie";
