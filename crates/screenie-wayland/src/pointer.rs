//! The pointer for a window captured by itself.
//!
//! Compositors don't paint the pointer into a window's frames the way they do into an
//! output's: wlroots ignores `paint_cursors` for windows. What they do offer is a cursor
//! session (`ext_image_copy_capture_cursor_session_v1`): where the pointer is over a
//! source and what it looks like, for the client to draw itself. This is how screen
//! sharing gets the pointer too (the portal's "metadata" cursor mode).
//!
//! Sessions are opened on the window itself, which Hyprland answers with positions in
//! the window's pixels, and on every output, which is all wlroots (sway) and niri
//! answer. An output's position is mapped into the window's frame through where the
//! window is on the desktop ([`Placement`], kept current by the caller from compositor
//! IPC). wlroots only reports a hardware cursor: with software cursors
//! (`WLR_NO_HARDWARE_CURSORS`) nothing is reported, and nothing is drawn.
//!
//! Each session's picture is captured like any other source, into shared memory, and
//! only changes when the cursor's shape does.

use std::sync::{Arc, Mutex, Weak};

use screenie_core::{Image, OutputInfo, PixelFormat, Pointer, Rect};
use wayland_client::protocol::{wl_pointer, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_cursor_session_v1::{
    self, ExtImageCopyCaptureCursorSessionV1,
};

use crate::state::State;

/// What a cursor session watches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Over {
    /// The captured window: positions are in its frames' pixels.
    Window,
    /// An output, by index into `State::outputs`: positions are in its pixels.
    Output(usize),
}

/// One cursor session.
pub(crate) struct CursorFeed {
    pub session: ExtImageCopyCaptureCursorSessionV1,
    /// The capture of the cursor's picture: a slot in `State::captures`.
    pub slot: usize,
    pub report: Report,
    /// Captures of the picture that failed in a row.
    pub failures: u32,
}

/// What a cursor session has said.
#[derive(Debug, Clone)]
pub(crate) struct Report {
    pub over: Over,
    /// Whether the compositor has said anything at all: sessions it can't serve stay
    /// silent.
    pub heard: bool,
    pub entered: bool,
    /// Where the pointer is, in the source's pixels.
    pub position: (i32, i32),
    /// The pointer's point within the picture.
    pub hotspot: (i32, i32),
    /// The picture, trimmed to what shows, and where that part sits in the whole.
    pub image: Option<(Arc<Image>, (i32, i32))>,
}

impl Report {
    pub fn new(over: Over) -> Self {
        Self {
            over,
            heard: false,
            entered: false,
            position: (0, 0),
            hotspot: (0, 0),
            image: None,
        }
    }

    /// Where the picture's visible part goes, in the source's pixels, and the picture.
    fn sprite(&self) -> Option<(Arc<Image>, (i32, i32))> {
        if !self.entered {
            return None;
        }
        let (image, (tx, ty)) = self.image.clone()?;
        let at = (
            self.position.0 - self.hotspot.0 + tx,
            self.position.1 - self.hotspot.1 + ty,
        );
        Some((image, at))
    }
}

/// Where the recorded window is on the desktop (logical, global), or `None` while it
/// isn't shown (on another workspace): then the pointer isn't over it, wherever it is.
/// Shared with whoever keeps it current.
#[derive(Clone)]
pub struct Placement(Arc<Mutex<Option<Rect>>>);

impl Placement {
    pub(crate) fn new(rect: Rect) -> Self {
        Self(Arc::new(Mutex::new(Some(rect))))
    }

    pub fn set(&self, rect: Option<Rect>) {
        *self.0.lock().unwrap() = rect;
    }

    pub fn get(&self) -> Option<Rect> {
        *self.0.lock().unwrap()
    }

    /// A handle that doesn't keep the stream's placement alive: [`Tracker::placement`]
    /// is `None` once the stream is gone.
    pub fn tracker(&self) -> Tracker {
        Tracker(Arc::downgrade(&self.0))
    }
}

/// A [`Placement`] kept current from elsewhere, for as long as its stream lasts.
pub struct Tracker(Weak<Mutex<Option<Rect>>>);

impl Tracker {
    pub fn placement(&self) -> Option<Placement> {
        self.0.upgrade().map(Placement)
    }
}

/// The pointer over a `frame`-sized frame of the window, from what the feeds say: the
/// window's own session if the compositor serves it, else the output the pointer is on.
pub(crate) fn locate(
    feeds: &[&Report],
    outputs: &[Option<OutputInfo>],
    window: Option<Rect>,
    frame: (u32, u32),
) -> Option<Pointer> {
    if let Some(feed) = feeds.iter().find(|f| f.over == Over::Window && f.heard) {
        let (image, (x, y)) = feed.sprite()?;
        let pointer = Pointer {
            x,
            y,
            width: image.width(),
            height: image.height(),
            image,
        };
        return pointer.over(frame).then_some(pointer);
    }
    let window = window?;
    feeds.iter().find_map(|feed| {
        let Over::Output(index) = feed.over else {
            return None;
        };
        let output = outputs.get(index)?.as_ref()?;
        let (image, at) = feed.sprite()?;
        let pointer = place(image, at, output, window, frame);
        pointer.over(frame).then_some(pointer)
    })
}

/// A picture at `at` in `output`'s pixels, placed over a `frame`-sized frame of the
/// window at `window` (logical): through logical coordinates, scaled from the output's
/// scale to the frame's.
fn place(
    image: Arc<Image>,
    (x, y): (i32, i32),
    output: &OutputInfo,
    window: Rect,
    frame: (u32, u32),
) -> Pointer {
    let frame_scale = if window.width > 0.0 {
        frame.0 as f64 / window.width
    } else {
        1.0
    };
    let to_frame = frame_scale / output.scale.max(f64::EPSILON);
    let logical = |v: i32, origin: f64| origin + v as f64 / output.scale.max(f64::EPSILON);
    let fx = (logical(x, output.logical.x) - window.x) * frame_scale;
    let fy = (logical(y, output.logical.y) - window.y) * frame_scale;
    Pointer {
        x: fx.round() as i32,
        y: fy.round() as i32,
        width: ((image.width() as f64 * to_frame).round() as u32).max(1),
        height: ((image.height() as f64 * to_frame).round() as u32).max(1),
        image,
    }
}

/// The part of a cursor picture that shows (the hardware cursor's buffer is often
/// 256×256 around a small arrow), and where it sits in the whole; `None` if nothing
/// does. Returned premultiplied BGRA, however it came.
pub(crate) fn trim(image: &Image) -> Option<(Image, (i32, i32))> {
    let image = image.convert(PixelFormat::Bgra);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..image.height() {
        let row = &image.data()[y as usize * image.stride()..][..image.width() as usize * 4];
        for (x, px) in row.as_chunks::<4>().0.iter().enumerate() {
            if px[3] != 0 {
                let x = x as u32;
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
    }
    if x1 == 0 {
        return None;
    }
    let rect = screenie_core::PixelRect::new(x0 as i32, y0 as i32, x1 - x0, y1 - y0);
    Some((image.crop(rect), (x0 as i32, y0 as i32)))
}

impl Dispatch<ExtImageCopyCaptureCursorSessionV1, usize> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureCursorSessionV1,
        event: ext_image_copy_capture_cursor_session_v1::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_cursor_session_v1::Event;
        let feed = &mut state.cursors[*idx].report;
        feed.heard = true;
        match event {
            Event::Enter => {
                tracing::debug!(over = ?feed.over, "the pointer entered a cursor session");
                feed.entered = true;
            }
            Event::Leave => {
                tracing::debug!(over = ?feed.over, "the pointer left a cursor session");
                feed.entered = false;
            }
            Event::Position { x, y } => {
                tracing::trace!(over = ?feed.over, x, y, "pointer position");
                feed.position = (x, y);
            }
            Event::Hotspot { x, y } => {
                if feed.hotspot != (x, y) {
                    tracing::debug!(over = ?feed.over, x, y, "pointer hotspot");
                }
                feed.hotspot = (x, y);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            state.seat_has_pointer = capabilities.contains(wl_seat::Capability::Pointer);
        }
    }
}

// Only for naming the seat's pointer to the cursor sessions: it never has a surface of
// ours to enter.
impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_pointer::WlPointer,
        _: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use screenie_core::Transform;

    use super::*;

    fn output(x: f64, scale: f64) -> OutputInfo {
        OutputInfo {
            name: "DP-1".into(),
            description: String::new(),
            logical: Rect::new(x, 0.0, 1920.0, 1080.0),
            scale,
            transform: Transform::Normal,
        }
    }

    fn picture(width: u32, height: u32) -> Arc<Image> {
        Arc::new(Image::new(width, height, PixelFormat::Bgra))
    }

    #[test]
    fn an_outputs_pointer_lands_where_it_is_over_the_window() {
        // A 2x output to the right of another; the window at (2020, 100) logical,
        // 800x600, captured at 2x. The pointer at output pixel (400, 400) is logical
        // (2120, 200): 100, 100 into the window, 200, 200 into its frame.
        let p = place(
            picture(48, 48),
            (400, 400),
            &output(1920.0, 2.0),
            Rect::new(2020.0, 100.0, 800.0, 600.0),
            (1600, 1200),
        );
        assert_eq!((p.x, p.y, p.width, p.height), (200, 200, 48, 48));
        // The same window captured at 1x (a 1x output's frame): half of everything.
        let p = place(
            picture(48, 48),
            (400, 400),
            &output(1920.0, 2.0),
            Rect::new(2020.0, 100.0, 800.0, 600.0),
            (800, 600),
        );
        assert_eq!((p.x, p.y, p.width, p.height), (100, 100, 24, 24));
    }

    fn feed(over: Over, entered: bool, position: (i32, i32)) -> Report {
        Report {
            heard: true,
            entered,
            position,
            hotspot: (4, 2),
            image: Some((picture(10, 16), (1, 1))),
            ..Report::new(over)
        }
    }

    #[test]
    fn the_windows_own_session_wins_and_is_in_frame_pixels() {
        let outputs = [Some(output(0.0, 1.0))];
        let window = Some(Rect::new(0.0, 0.0, 800.0, 600.0));
        let feeds = [
            feed(Over::Output(0), true, (500, 500)),
            feed(Over::Window, true, (50, 60)),
        ];
        let p = locate(&[&feeds[0], &feeds[1]], &outputs, window, (800, 600)).unwrap();
        // The hotspot (4, 2) at (50, 60), the picture's visible part 1, 1 in.
        assert_eq!((p.x, p.y), (47, 59));
        // A silent window session defers to the outputs.
        let mut feeds = feeds;
        feeds[1].heard = false;
        assert_eq!(
            locate(&[&feeds[0], &feeds[1]], &outputs, window, (800, 600)).map(|p| (p.x, p.y)),
            Some((497, 499))
        );
    }

    #[test]
    fn no_pointer_off_the_window_or_while_its_hidden() {
        let outputs = [Some(output(0.0, 1.0))];
        let feeds = [feed(Over::Output(0), true, (1500, 900))];
        let window = Rect::new(0.0, 0.0, 800.0, 600.0);
        assert!(locate(&[&feeds[0]], &outputs, Some(window), (800, 600)).is_none());
        let feeds = [feed(Over::Output(0), true, (100, 100))];
        assert!(locate(&[&feeds[0]], &outputs, Some(window), (800, 600)).is_some());
        assert!(locate(&[&feeds[0]], &outputs, None, (800, 600)).is_none());
        let feeds = [feed(Over::Output(0), false, (100, 100))];
        assert!(locate(&[&feeds[0]], &outputs, Some(window), (800, 600)).is_none());
    }

    #[test]
    fn pictures_are_trimmed_to_what_shows() {
        let mut image = Image::new(64, 64, PixelFormat::Bgra);
        image.blit(
            &Image::from_raw(2, 3, 8, PixelFormat::Bgra, vec![255; 24]),
            5,
            7,
        );
        let (trimmed, at) = trim(&image).unwrap();
        assert_eq!((trimmed.width(), trimmed.height(), at), (2, 3, (5, 7)));
        assert!(trim(&Image::new(64, 64, PixelFormat::Bgra)).is_none());
    }
}
