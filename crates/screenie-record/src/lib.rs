//! The recording engine: frames from a [`FrameSource`] and optional audio are encoded to
//! an H.264/AAC MP4 with GStreamer.
//!
//! ```text
//! FrameSource ─ capture thread ─▶ appsrc ─▶ videoconvert ─▶ H.264 ─┐
//! pulsesrc (speakers) ─┐                                            ├─▶ mp4mux ─▶ file
//! pulsesrc (mic) ──────┴─▶ audiomixer ─▶ AAC ───────────────────────┘
//! ```
//!
//! Frames arrive only when the screen changes and are capped at the configured frame
//! rate, so a static screen costs almost nothing. Buffers are timestamped with the
//! pipeline's running time, which also keeps audio in sync across pauses.

mod encoder;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gst::prelude::*;
use screenie_config::{EncoderPreference, Quality};
use screenie_core::{FrameSource, Image, PixelFormat};

pub use encoder::Encoder;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GStreamer: {0}")]
    Gst(String),
    #[error("no working H.264 encoder found (install VA-API drivers, or gstreamer1.0-plugins-ugly for x264)")]
    NoEncoder,
    #[error("no audio encoder found (install gstreamer1.0-libav)")]
    NoAudioEncoder,
    #[error("screen capture: {0}")]
    Source(String),
    #[error("the screen sent no frames")]
    NoFrames,
    #[error("the region is too small to record")]
    TooSmall,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<gst::glib::Error> for Error {
    fn from(e: gst::glib::Error) -> Self {
        Error::Gst(e.to_string())
    }
}

impl From<gst::glib::BoolError> for Error {
    fn from(e: gst::glib::BoolError) -> Self {
        Error::Gst(e.to_string())
    }
}

impl From<gst::StateChangeError> for Error {
    fn from(e: gst::StateChangeError) -> Self {
        Error::Gst(e.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioSources {
    /// What the speakers play (the default sink's monitor).
    pub system: bool,
    /// The default microphone.
    pub microphone: bool,
}

impl AudioSources {
    fn any(self) -> bool {
        self.system || self.microphone
    }
}

#[derive(Debug, Clone)]
pub struct RecordSpec {
    pub path: PathBuf,
    /// Upper bound; a static screen produces fewer frames.
    pub framerate: u32,
    pub quality: Quality,
    pub encoder: EncoderPreference,
    pub audio: AudioSources,
}

/// A finished recording.
#[derive(Debug, Clone)]
pub struct Finished {
    pub path: PathBuf,
    pub duration: Duration,
    pub size: (u32, u32),
    pub bytes: u64,
    /// The last frame, for thumbnails.
    pub last_frame: Option<Image>,
}

/// State shared with the capture and bus threads.
#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    paused: AtomicBool,
    failure: Mutex<Option<String>>,
}

impl Shared {
    fn fail(&self, message: String) {
        tracing::error!("recording failed: {message}");
        self.failure.lock().unwrap().get_or_insert(message);
    }
}

#[derive(Debug, Clone, Copy)]
struct Clock {
    started: Instant,
    paused_at: Option<Instant>,
    paused_total: Duration,
}

impl Clock {
    fn elapsed(&self) -> Duration {
        let end = self.paused_at.unwrap_or_else(Instant::now);
        end.duration_since(self.started).saturating_sub(self.paused_total)
    }
}

/// A running recording. Dropping it cancels.
pub struct Recording {
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    shared: Arc<Shared>,
    capture: Option<JoinHandle<Option<Image>>>,
    eos: mpsc::Receiver<Result<(), String>>,
    clock: Mutex<Clock>,
    path: PathBuf,
    temp: PathBuf,
    size: (u32, u32),
    encoder: Encoder,
    finished: bool,
}

impl Recording {
    /// Start recording. Blocks briefly (first frame, encoder probe, pipeline start), so
    /// call it off the UI thread.
    pub fn start(mut source: Box<dyn FrameSource>, spec: RecordSpec) -> Result<Recording> {
        gst::init()?;
        let first = first_frame(source.as_mut())?;
        // 4:2:0 chroma needs even dimensions; the odd last row/column is dropped.
        let size = (first.width() & !1, first.height() & !1);
        if size.0 < 16 || size.1 < 16 {
            return Err(Error::TooSmall);
        }
        let encoder = Encoder::select(spec.encoder).ok_or(Error::NoEncoder)?;
        let fps = spec.framerate.clamp(1, 240);

        if let Some(dir) = spec.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = temp_path(&spec.path);
        let desc = pipeline_description(&spec, encoder, fps, size)?;
        tracing::debug!(%desc, "recording pipeline");
        let pipeline = gst::parse::launch(&desc)?
            .downcast::<gst::Pipeline>()
            .map_err(|_| Error::Gst("not a pipeline".into()))?;
        let by_name = |name: &str| pipeline.by_name(name).ok_or_else(|| Error::Gst(format!("missing {name}")));
        by_name("sink")?.set_property("location", spec.path.to_string_lossy().as_ref());
        by_name("mux")?.set_property("faststart-file", temp.to_string_lossy().as_ref());
        if spec.audio.system {
            by_name("speakers")?.set_property("device", "@DEFAULT_MONITOR@");
        }

        let format = video_format(first.format());
        let appsrc = by_name("video")?
            .downcast::<gst_app::AppSrc>()
            .map_err(|_| Error::Gst("video source is not an appsrc".into()))?;
        let caps = gst_video::VideoInfo::builder(format, size.0, size.1)
            .fps(gst::Fraction::new(fps as i32, 1))
            .build()?
            .to_caps()?;
        appsrc.set_caps(Some(&caps));
        appsrc.set_format(gst::Format::Time);
        appsrc.set_is_live(true);
        appsrc.set_do_timestamp(true);
        // If the encoder falls behind, drop frames rather than queue gigabytes of them.
        appsrc.set_property("max-buffers", 4u64);
        appsrc.set_property_from_str("leaky-type", "downstream");

        let shared = Arc::new(Shared::default());
        let eos = watch_bus(&pipeline, shared.clone());
        pipeline.set_state(gst::State::Playing)?;
        let clock = Clock { started: Instant::now(), paused_at: None, paused_total: Duration::ZERO };
        push(&appsrc, &first, size, format);

        let capture = {
            let (appsrc, shared) = (appsrc.clone(), shared.clone());
            std::thread::Builder::new()
                .name("screenie-record".into())
                .spawn(move || capture_loop(source, appsrc, shared, first, size, format, fps))?
        };
        tracing::info!(path = %spec.path.display(), encoder = encoder.factory, ?size, fps, audio = ?spec.audio, "recording");
        Ok(Recording {
            pipeline,
            appsrc,
            shared,
            capture: Some(capture),
            eos,
            clock: Mutex::new(clock),
            path: spec.path,
            temp,
            size,
            encoder,
            finished: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn encoder(&self) -> Encoder {
        self.encoder
    }

    /// Recorded time so far, excluding pauses.
    pub fn elapsed(&self) -> Duration {
        self.clock.lock().unwrap().elapsed()
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// Why the recording broke, if it did. A failed recording should still be stopped to
    /// salvage what was written.
    pub fn failure(&self) -> Option<String> {
        self.shared.failure.lock().unwrap().clone()
    }

    pub fn set_paused(&self, paused: bool) -> Result<()> {
        if self.shared.paused.swap(paused, Ordering::Relaxed) == paused {
            return Ok(());
        }
        // A live pipeline's running time stops while PAUSED, so both audio and video
        // simply continue where they left off.
        self.pipeline.set_state(if paused { gst::State::Paused } else { gst::State::Playing })?;
        let mut clock = self.clock.lock().unwrap();
        match (paused, clock.paused_at.take()) {
            (true, _) => clock.paused_at = Some(Instant::now()),
            (false, Some(at)) => clock.paused_total += at.elapsed(),
            (false, None) => {}
        }
        Ok(())
    }

    /// Finish the file. Blocks until the muxer has written everything.
    pub fn stop(mut self) -> Result<Finished> {
        let last = self.stop_capture();
        if self.is_paused() {
            self.set_paused(false)?;
        } else if let Some(frame) = &last {
            // Hold the final frame until now, so a still ending isn't cut short.
            push(&self.appsrc, frame, self.size, video_format(frame.format()));
        }
        let duration = self.elapsed();
        self.pipeline.send_event(gst::event::Eos::new());
        let result = match self.eos.recv_timeout(Duration::from_secs(20)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(Error::Gst(e)),
            Err(_) => Err(Error::Gst("timed out finishing the file".into())),
        };
        self.teardown();
        result?;
        let bytes = std::fs::metadata(&self.path)?.len();
        tracing::info!(path = %self.path.display(), ?duration, bytes, "recording saved");
        Ok(Finished { path: self.path.clone(), duration, size: self.size, bytes, last_frame: last })
    }

    /// Stop and delete the file.
    pub fn cancel(mut self) {
        self.stop_capture();
        self.teardown();
        let _ = std::fs::remove_file(&self.path);
        tracing::info!(path = %self.path.display(), "recording cancelled");
    }

    fn stop_capture(&mut self) -> Option<Image> {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.capture.take().and_then(|h| h.join().ok()).flatten()
    }

    fn teardown(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        let _ = std::fs::remove_file(&self.temp);
        self.finished = true;
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        if !self.finished {
            self.stop_capture();
            self.teardown();
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn first_frame(source: &mut dyn FrameSource) -> Result<Image> {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Some(image) = source.next_frame(Duration::from_millis(250)).map_err(|e| Error::Source(e.to_string()))? {
            return Ok(image);
        }
    }
    Err(Error::NoFrames)
}

/// Where mp4mux keeps media data until it can write the index up front ("faststart",
/// so the file streams in browsers and chat apps). Next to the output, not in /tmp,
/// which is often RAM.
fn temp_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}.part"))
}

fn pipeline_description(spec: &RecordSpec, encoder: Encoder, fps: u32, size: (u32, u32)) -> Result<String> {
    // Generous queues: the muxer interleaves audio and video, and encoders have latency.
    const QUEUE: &str = "queue max-size-buffers=0 max-size-bytes=0 max-size-time=3000000000";
    let mut desc = format!(
        "mp4mux name=mux faststart=true ! filesink name=sink \
         appsrc name=video ! {QUEUE} ! videoconvert n-threads=0 ! video/x-raw,format={} \
         ! {} ! h264parse ! {QUEUE} ! mux.",
        encoder.input_format(),
        encoder.element(spec.quality, fps, size)
    );
    if spec.audio.any() {
        let aac = encoder::audio_encoder().ok_or(Error::NoAudioEncoder)?;
        // Our clock drives timestamps; pulsesrc re-times its samples to it.
        let source = |name: &str| {
            format!(" pulsesrc name={name} provide-clock=false ! audioconvert ! audioresample ! {QUEUE}")
        };
        let raw = "audio/x-raw,rate=48000,channels=2";
        if spec.audio.system && spec.audio.microphone {
            desc += &format!(" audiomixer name=mix ! {raw} ! audioconvert ! {aac} ! {QUEUE} ! mux.");
            desc += &(source("speakers") + " ! mix.");
            desc += &(source("microphone") + " ! mix.");
        } else {
            let name = if spec.audio.system { "speakers" } else { "microphone" };
            desc += &format!("{} ! {raw} ! {aac} ! {QUEUE} ! mux.", source(name));
        }
    }
    Ok(desc)
}

/// Forward errors to `shared` and the end of stream to the returned channel.
fn watch_bus(pipeline: &gst::Pipeline, shared: Arc<Shared>) -> mpsc::Receiver<Result<(), String>> {
    let (tx, rx) = mpsc::channel();
    let bus = pipeline.bus().expect("pipelines have a bus");
    std::thread::spawn(move || {
        // Ends when the pipeline shuts down and flushes its bus.
        for msg in bus.iter_timed(gst::ClockTime::NONE) {
            match msg.view() {
                gst::MessageView::Eos(_) => {
                    let _ = tx.send(Ok(()));
                    break;
                }
                gst::MessageView::Error(e) => {
                    let message = format!(
                        "{}{}",
                        e.error(),
                        e.debug().map(|d| format!(" ({d})")).unwrap_or_default()
                    );
                    shared.fail(message.clone());
                    let _ = tx.send(Err(message));
                    break;
                }
                gst::MessageView::Warning(w) => tracing::warn!("recording: {}", w.error()),
                _ => {}
            }
        }
    });
    rx
}

fn video_format(format: PixelFormat) -> gst_video::VideoFormat {
    match format {
        PixelFormat::Bgra => gst_video::VideoFormat::Bgra,
        PixelFormat::Bgrx => gst_video::VideoFormat::Bgrx,
        PixelFormat::Rgba => gst_video::VideoFormat::Rgba,
        PixelFormat::Rgbx => gst_video::VideoFormat::Rgbx,
    }
}

/// Lets a GStreamer buffer borrow an image's pixels without copying.
struct Pixels(Arc<Vec<u8>>);

impl AsRef<[u8]> for Pixels {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Hand a frame to the pipeline. Frames smaller than the stream (a mode change) are
/// dropped; larger ones are cropped by describing their layout with a video meta.
fn push(appsrc: &gst_app::AppSrc, image: &Image, (width, height): (u32, u32), format: gst_video::VideoFormat) {
    if image.width() < width || image.height() < height {
        return;
    }
    let mut buffer = gst::Buffer::from_slice(Pixels(image.shared_data()));
    if image.stride() != width as usize * 4 {
        let buffer = buffer.get_mut().expect("fresh buffer");
        let meta = gst_video::VideoMeta::add_full(
            buffer,
            gst_video::VideoFrameFlags::empty(),
            format,
            width,
            height,
            &[0],
            &[image.stride() as i32],
        );
        if let Err(e) = meta {
            tracing::warn!("cannot describe frame layout: {e}");
            return;
        }
    }
    // Only fails when flushing or at EOS, i.e. while stopping.
    let _ = appsrc.push_buffer(buffer);
}

/// Pull frames from the source until stopped, pushing at most `fps` per second. The
/// newest frame always wins, and one that arrives early is held rather than dropped, so
/// the video never ends on a stale frame. Returns the last frame.
fn capture_loop(
    mut source: Box<dyn FrameSource>,
    appsrc: gst_app::AppSrc,
    shared: Arc<Shared>,
    first: Image,
    size: (u32, u32),
    format: gst_video::VideoFormat,
    fps: u32,
) -> Option<Image> {
    let interval = Duration::from_secs_f64(1.0 / fps as f64);
    let mut last = first;
    let mut last_push = Instant::now();
    let mut pending: Option<Image> = None;
    while !shared.stop.load(Ordering::Relaxed) {
        let wait = match pending {
            Some(_) => interval.saturating_sub(last_push.elapsed()),
            None => Duration::from_millis(100),
        };
        match source.next_frame(wait.max(Duration::from_millis(1))) {
            Ok(Some(frame)) => pending = Some(frame),
            Ok(None) => {}
            Err(e) => {
                shared.fail(format!("screen capture stopped: {e}"));
                break;
            }
        }
        if shared.paused.load(Ordering::Relaxed) || last_push.elapsed() < interval {
            continue;
        }
        if let Some(frame) = pending.take() {
            // Damage elsewhere on the output (or a compositor that doesn't track damage)
            // yields identical frames; skipping them keeps static screens nearly free.
            if same_pixels(&frame, &last, size) {
                continue;
            }
            push(&appsrc, &frame, size, format);
            last = frame;
            last_push = Instant::now();
        }
    }
    Some(last)
}

/// Whether two frames show the same pixels within the recorded `size`.
fn same_pixels(a: &Image, b: &Image, (width, height): (u32, u32)) -> bool {
    if a.format() != b.format() || a.width() < width || b.width() < width || a.height() < height || b.height() < height {
        return false;
    }
    let row = width as usize * 4;
    (0..height as usize).all(|y| a.data()[y * a.stride()..][..row] == b.data()[y * b.stride()..][..row])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_file_sits_next_to_the_output() {
        assert_eq!(temp_path(Path::new("/v/Rec 1.mp4")), PathBuf::from("/v/.Rec 1.mp4.part"));
    }

    #[test]
    fn identical_frames_are_detected_within_the_recorded_area() {
        let a = Image::new(5, 3, PixelFormat::Bgrx);
        let mut b = Image::new(5, 3, PixelFormat::Bgrx);
        assert!(same_pixels(&a, &b, (4, 2)));
        // A change in the cropped-off column doesn't count.
        b.blit(&Image::from_raw(1, 1, 4, PixelFormat::Bgrx, vec![9, 9, 9, 255]), 4, 0);
        assert!(same_pixels(&a, &b, (4, 2)));
        b.blit(&Image::from_raw(1, 1, 4, PixelFormat::Bgrx, vec![9, 9, 9, 255]), 1, 1);
        assert!(!same_pixels(&a, &b, (4, 2)));
    }

    #[test]
    fn clock_excludes_pauses() {
        let now = Instant::now();
        let clock = Clock {
            started: now - Duration::from_secs(10),
            paused_at: Some(now - Duration::from_secs(2)),
            paused_total: Duration::from_secs(3),
        };
        assert_eq!(clock.elapsed().as_secs(), 5);
    }
}
