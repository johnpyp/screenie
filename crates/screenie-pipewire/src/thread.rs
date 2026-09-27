//! The PipeWire side of a [`Stream`](crate::Stream): a loop on its own thread that
//! negotiates the cast's format and turns each buffer into a [`Frame`].

use std::sync::{Arc, Once};
use std::time::Duration;

use pipewire as pw;
use pw::spa;
use screenie_core::{Frame, Image, PixelFormat, PixelRect, Pixels, Point, Pointer};
use spa::buffer::meta::{MetaCursor, MetaHeader, MetaHeaderFlags, MetaVideoCrop};
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::pod::Pod;

use crate::{Crop, Error, Remote, Result, Shared, format};

/// Asked of the loop from outside.
pub(crate) enum Command {
    /// At most this many frames a second.
    Pace(u32),
    /// Carry on (true) or stop (false) producing frames.
    Active(bool),
    Quit,
}

/// Steers the loop; quits it and waits for the thread when dropped.
pub(crate) struct Control {
    sender: pw::channel::Sender<Command>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Control {
    pub(crate) fn send(&self, command: Command) {
        // The loop is gone only once the stream ended, which the consumer hears of.
        let _ = self.sender.send(command);
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        self.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(crate) fn spawn(
    remote: Remote,
    node: u32,
    pointer: crate::Pointer,
    crop: Option<Crop>,
    shared: Arc<Shared>,
) -> Result<Control> {
    static INIT: Once = Once::new();
    INIT.call_once(pw::init);
    let (sender, receiver) = pw::channel::channel();
    // Connecting happens on the thread (PipeWire's objects stay on the one that made
    // them); its outcome comes back here.
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("screenie-pipewire".into())
        .spawn(move || {
            let metadata = pointer == crate::Pointer::Metadata;
            let wants = Wants { metadata, crop };
            if let Err(e) = run(remote, node, wants, shared.clone(), receiver, &started_tx) {
                let _ = started_tx.send(Err(e.to_string()));
                shared.update(|s| s.ended = Some(e.to_string()));
            }
        })?;
    match started_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(Control {
            sender,
            thread: Some(thread),
        }),
        Ok(Err(e)) => Err(Error::Ended(e)),
        Err(_) => Err(Error::Timeout),
    }
}

/// What the consumer asked of the cast.
#[derive(Clone, Copy)]
struct Wants {
    /// The pointer comes as metadata.
    metadata: bool,
    crop: Option<Crop>,
}

/// What the stream's callbacks keep between buffers.
struct Negotiated {
    format: Option<PixelFormat>,
    size: (u32, u32),
    /// The last picture, so a buffer that only moves the pointer can be sent again.
    picture: Option<Image>,
    /// The last pointer sprite; a cursor update leaves it out when unchanged.
    sprite: Option<Arc<Image>>,
    pointer: Option<Pointer>,
    shared: Arc<Shared>,
    wants: Wants,
}

fn run(
    remote: Remote,
    node: u32,
    wants: Wants,
    shared: Arc<Shared>,
    commands: pw::channel::Receiver<Command>,
    started: &std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<()> {
    let main_loop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main_loop, None)?;
    let core = match remote {
        Remote::Session => context.connect_rc(None)?,
        Remote::Fd(fd) => context.connect_fd_rc(fd, None)?,
    };
    let stream = pw::stream::StreamRc::new(
        core,
        "screenie",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let negotiated = Negotiated {
        format: None,
        size: (0, 0),
        picture: None,
        sprite: None,
        pointer: None,
        shared: shared.clone(),
        wants,
    };
    let _listener = stream
        .add_local_listener_with_user_data(negotiated)
        .state_changed(|_, n, old, new| {
            tracing::debug!(?old, ?new, "screen cast stream");
            let ended = match new {
                pw::stream::StreamState::Error(e) => Some(e),
                // Once streaming, going back to unconnected means the node went away
                // (a cast window closed, the cast was stopped).
                pw::stream::StreamState::Unconnected
                    if !matches!(old, pw::stream::StreamState::Connecting) =>
                {
                    Some("the stream disconnected".to_string())
                }
                _ => None,
            };
            if let Some(why) = ended {
                n.shared.update(|s| s.ended = Some(why));
            }
        })
        .param_changed(|stream, n, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut info = VideoInfoRaw::default();
            if let Err(e) = info.parse(param) {
                tracing::warn!("unreadable screen cast format: {e}");
                return;
            }
            n.format = format::pixel_format(info.format());
            n.size = (info.size().width, info.size().height);
            tracing::debug!(format = ?info.format(), size = ?n.size, "screen cast format");
            if n.format.is_none() {
                tracing::warn!(format = ?info.format(), "screen cast in a format screenie can't read");
                return;
            }
            let buffers = format::buffers();
            let metas = format::metas(n.wants.metadata);
            let mut params: Vec<&Pod> = std::iter::once(&buffers)
                .chain(&metas)
                .filter_map(|bytes| Pod::from_bytes(bytes))
                .collect();
            if let Err(e) = stream.update_params(&mut params) {
                tracing::warn!("can't ask for the screen cast's buffers: {e}");
            }
        })
        .process(|stream, n| {
            // Only the newest picture matters; older buffers go straight back.
            let mut newest = None;
            while let Some(buffer) = stream.dequeue_buffer() {
                newest = Some(buffer);
            }
            if let Some(mut buffer) = newest {
                receive(&mut buffer, n);
            }
        })
        .register()?;

    let format = format::enum_format(None);
    let mut params = [Pod::from_bytes(&format).expect("a serialized pod")];
    stream.connect(
        spa::utils::Direction::Input,
        Some(node),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    let _commands = commands.attach(main_loop.loop_(), {
        let (main_loop, stream) = (main_loop.downgrade(), stream.downgrade());
        move |command| {
            let (Some(main_loop), Some(stream)) = (main_loop.upgrade(), stream.upgrade()) else {
                return;
            };
            match command {
                Command::Pace(fps) => {
                    let format = format::enum_format(Some(fps));
                    let mut params = [Pod::from_bytes(&format).expect("a serialized pod")];
                    if let Err(e) = stream.update_params(&mut params) {
                        tracing::debug!("can't pace the screen cast: {e}");
                    }
                }
                Command::Active(active) => {
                    if let Err(e) = stream.set_active(active) {
                        tracing::warn!("can't pause the screen cast: {e}");
                    }
                }
                Command::Quit => main_loop.quit(),
            }
        }
    });
    let _ = started.send(Ok(()));
    main_loop.run();
    let _ = stream.disconnect();
    Ok(())
}

/// Turn a buffer into a frame for the consumer.
fn receive(buffer: &mut pw::buffer::Buffer<'_>, n: &mut Negotiated) {
    let Some(format) = n.format else { return };
    let header = buffer.find_meta::<MetaHeader>().cloned();
    if header
        .as_ref()
        .is_some_and(|h| h.flags().contains(MetaHeaderFlags::CORRUPTED))
    {
        return;
    }
    let (width, height) = n.size;
    let whole = PixelRect::new(0, 0, width, height);
    // What the compositor marks as content (a window within a bigger buffer), then the
    // part of that the consumer wants.
    let content = buffer
        .find_meta::<MetaVideoCrop>()
        .map(|c| c.meta_region().clone())
        .filter(|c| c.is_valid())
        .and_then(|c| {
            let (p, s) = (c.position(), c.size());
            PixelRect::new(p.x, p.y, s.width, s.height).intersection(&whole)
        })
        .unwrap_or(whole);
    let keep = match n.wants.crop {
        Some(crop) if crop.of.width > 0.0 => {
            let scale = f64::from(content.width) / crop.of.width;
            let part = crop.region.to_pixels(Point::new(0.0, 0.0), scale);
            PixelRect::new(
                content.x + part.x,
                content.y + part.y,
                part.width,
                part.height,
            )
            .intersection(&content)
        }
        _ => Some(content),
    };
    let Some(PixelRect {
        x,
        y,
        width: w,
        height: h,
    }) = keep
    else {
        return;
    };
    let pointer_moved = n.wants.metadata && update_pointer(buffer, n, (x, y));
    let (x, y) = (x as u32, y as u32);
    let picture = buffer.datas_mut().first_mut().and_then(|data| {
        let chunk = data.chunk();
        let (offset, size, stride) = (
            chunk.offset() as usize,
            chunk.size() as usize,
            chunk.stride().unsigned_abs() as usize,
        );
        // An empty chunk only updates the pointer.
        if size == 0 || w == 0 || h == 0 {
            return None;
        }
        let bytes = data.data()?;
        let bytes = bytes.get(offset..offset + size)?;
        let row = w as usize * 4;
        let mut pixels = Vec::with_capacity(row * h as usize);
        for r in y..y + h {
            let start = r as usize * stride + x as usize * 4;
            pixels.extend_from_slice(bytes.get(start..start + row)?);
        }
        Some(Image::from_raw(w, h, row, format, pixels))
    });

    let presented = header.and_then(|h| presented(h.pts()));
    let frame = match picture {
        Some(picture) => {
            n.picture = Some(picture.clone());
            Frame {
                pixels: Pixels::Cpu(picture),
                presented,
                pointer: n.pointer.clone(),
            }
        }
        // The pointer moved over a still picture: the same picture again, with it.
        None if pointer_moved => match &n.picture {
            Some(picture) => Frame {
                pixels: Pixels::Cpu(picture.clone()),
                presented,
                pointer: n.pointer.clone(),
            },
            None => return,
        },
        None => return,
    };
    let latest = n.picture.clone();
    n.shared.update(|s| {
        s.frame = Some(frame);
        s.latest = latest;
    });
}

/// Read the pointer from the buffer's cursor metadata, placed relative to `origin` (the
/// kept part's top-left in the buffer). True if it changed.
fn update_pointer(buffer: &pw::buffer::Buffer<'_>, n: &mut Negotiated, origin: (i32, i32)) -> bool {
    let Some(cursor) = buffer.find_meta::<MetaCursor>() else {
        return false;
    };
    let before = n.pointer.clone();
    if cursor.id() == 0 {
        // Off this cast, or hidden.
        n.pointer = None;
        return before.is_some();
    }
    if let Some(bitmap) = cursor.bitmap()
        && let Some(sprite) = sprite(bitmap)
    {
        n.sprite = Some(Arc::new(sprite));
    }
    let Some(sprite) = n.sprite.clone() else {
        return false;
    };
    let (cx, cy) = origin;
    let pointer = Pointer {
        x: cursor.position().x - cursor.hotspot().x - cx,
        y: cursor.position().y - cursor.hotspot().y - cy,
        width: sprite.width(),
        height: sprite.height(),
        image: sprite,
    };
    let changed = before.as_ref() != Some(&pointer);
    n.pointer = Some(pointer);
    changed
}

/// A cursor bitmap as premultiplied BGRA.
fn sprite(bitmap: &spa::buffer::meta::MetaBitmap) -> Option<Image> {
    let size = bitmap.size();
    let data = bitmap.bitmap_data()?;
    if size.width == 0 || size.height == 0 {
        return None;
    }
    let stride = bitmap.stride().unsigned_abs() as usize;
    let format = match bitmap.format() {
        VideoFormat::BGRA => PixelFormat::Bgra,
        VideoFormat::RGBA => PixelFormat::Rgba,
        other => {
            tracing::debug!(format = ?other, "pointer sprite in an unknown format");
            return None;
        }
    };
    let row = size.width as usize * 4;
    let mut pixels = Vec::with_capacity(row * size.height as usize);
    for r in 0..size.height as usize {
        pixels.extend_from_slice(data.get(r * stride..r * stride + row)?);
    }
    Some(Image::from_raw(size.width, size.height, row, format, pixels).convert(PixelFormat::Bgra))
}

/// A buffer's timestamp as a presentation time on CLOCK_MONOTONIC, where it plausibly is
/// one (Mutter and KWin stamp frames with it; others count from elsewhere).
fn presented(pts: i64) -> Option<Duration> {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    let now = Duration::new(now.tv_sec as u64, now.tv_nsec as u32);
    let pts = Duration::from_nanos(u64::try_from(pts).ok()?);
    (now.abs_diff(pts) < Duration::from_secs(1)).then_some(pts)
}
