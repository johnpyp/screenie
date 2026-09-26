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
//!
//! The video's size is the first frame's, capped by `recording.resolution` (scaled down
//! before colour conversion, so a 4K screen recorded at 1080p costs about what a 1080p
//! one does). A source whose frames change size (a recorded window being resized) is
//! scaled to fit it, letterboxed. `recording.framerate` paces the source itself, so the
//! compositor only copies the frames that get recorded.

mod encoder;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gst::prelude::*;
use screenie_config::{EncoderPreference, Framerate, Quality, Resolution};
use screenie_core::{FrameSource, Image, Next, Pacer, PixelFormat};

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
    #[error("the window closed")]
    Ended,
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
    pub framerate: Framerate,
    /// The most the video's size may be.
    pub resolution: Resolution,
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
    /// The source ended (a recorded window closed): time to stop and save.
    ended: AtomicBool,
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
        let cap = spec.framerate.fps().map(|fps| fps.clamp(1, 240));
        if let Some(fps) = cap {
            source.pace(fps);
        }
        let first = first_frame(source.as_mut())?;
        let native = even_size(&first);
        if native.0 < 16 || native.1 < 16 {
            return Err(Error::TooSmall);
        }
        let size = fit(native, spec.resolution.bounds(native.0, native.1));
        let encoder = Encoder::select(spec.encoder).ok_or(Error::NoEncoder)?;
        // What the stream says it is; frames are timestamped as they come either way.
        let fps = cap.unwrap_or(60);

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

        let appsrc = by_name("video")?
            .downcast::<gst_app::AppSrc>()
            .map_err(|_| Error::Gst("video source is not an appsrc".into()))?;
        appsrc.set_format(gst::Format::Time);
        appsrc.set_is_live(true);
        appsrc.set_do_timestamp(true);
        // The capture loop checks for room before pushing: when the pipeline falls
        // behind, frames are skipped there (and counted), not queued by the gigabyte.
        appsrc.set_property("max-buffers", PIPELINE_FRAMES);

        let shared = Arc::new(Shared::default());
        let eos = watch_bus(&pipeline, shared.clone());
        pipeline.set_state(gst::State::Playing)?;
        let clock = Clock { started: Instant::now(), paused_at: None, paused_total: Duration::ZERO };
        let mut feed = Feed { appsrc, fps, caps: None, pushed: 0 };
        feed.push(&first);

        let capture = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("screenie-record".into())
                .spawn(move || capture_loop(source, feed, shared, first, cap))?
        };
        tracing::info!(
            path = %spec.path.display(),
            encoder = encoder.factory,
            ?native,
            ?size,
            framerate = ?spec.framerate,
            audio = ?spec.audio,
            "recording"
        );
        Ok(Recording {
            pipeline,
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

    /// Whether the source ended (a recorded window closed), so the recording should be
    /// stopped and saved.
    pub fn ended(&self) -> bool {
        self.shared.ended.load(Ordering::Relaxed)
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
        match source.next_frame(Duration::from_millis(250)).map_err(|e| Error::Source(e.to_string()))? {
            Next::Frame(image) => return Ok(image),
            Next::Unchanged => {}
            Next::Ended => return Err(Error::Ended),
        }
    }
    Err(Error::NoFrames)
}

/// 4:2:0 chroma needs even dimensions; an odd last row or column is dropped.
fn even_size(image: &Image) -> (u32, u32) {
    (image.width() & !1, image.height() & !1)
}

/// `size` scaled down to fit `bounds` (never up), keeping its aspect ratio, in even
/// dimensions.
fn fit((width, height): (u32, u32), bounds: Option<(u32, u32)>) -> (u32, u32) {
    let Some((max_w, max_h)) = bounds else { return (width, height) };
    let scale = (max_w as f64 / width as f64).min(max_h as f64 / height as f64);
    if scale >= 1.0 {
        return (width, height);
    }
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    (even(width as f64 * scale), even(height as f64 * scale))
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
    // Raw frames are big (33 MB at 4K): a few decouple the capture thread from scaling
    // and conversion, and the appsrc holds a couple more before the capture loop waits.
    const FRAMES: &str = "queue max-size-buffers=3 max-size-bytes=0 max-size-time=0";
    let mut desc = format!(
        "mp4mux name=mux faststart=true ! filesink name=sink \
         appsrc name=video ! {FRAMES} ! {} ! {} ! h264parse ! {QUEUE} ! mux.",
        encoder.prepare(size),
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

/// Frames on their way into the pipeline. The appsrc's caps follow each frame's size and
/// format, and the pipeline scales them to the video's.
struct Feed {
    appsrc: gst_app::AppSrc,
    fps: u32,
    caps: Option<(gst_video::VideoFormat, u32, u32)>,
    pushed: u64,
}

/// Raw frames the appsrc holds before the pipeline counts as behind.
const PIPELINE_FRAMES: u64 = 2;

impl Feed {
    /// Whether the pipeline can take another frame now.
    fn has_room(&self) -> bool {
        self.appsrc.property::<u64>("current-level-buffers") < PIPELINE_FRAMES
    }

    fn push(&mut self, image: &Image) {
        let format = video_format(image.format());
        let (width, height) = even_size(image);
        if width < 2 || height < 2 {
            return;
        }
        if self.caps != Some((format, width, height)) {
            let caps = gst_video::VideoInfo::builder(format, width, height)
                .fps(gst::Fraction::new(self.fps as i32, 1))
                .build()
                .and_then(|info| info.to_caps());
            match caps {
                Ok(caps) => self.appsrc.set_caps(Some(&caps)),
                Err(e) => {
                    tracing::warn!("cannot describe a {width}x{height} frame: {e}");
                    return;
                }
            }
            if self.caps.is_some() {
                tracing::debug!(width, height, "recorded frames changed size");
            }
            self.caps = Some((format, width, height));
        }
        self.pushed += 1;
        push(&self.appsrc, image, (width, height), format);
    }
}

/// Hand a frame to the pipeline, cropped to `width`×`height` by describing its layout with
/// a video meta.
fn push(appsrc: &gst_app::AppSrc, image: &Image, (width, height): (u32, u32), format: gst_video::VideoFormat) {
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

/// Pull frames from the source until stopped, pushing at most `fps` per second (or all
/// of them). The newest frame always wins, and one that arrives early (or while the
/// pipeline is busy) is held rather than dropped, so the video never ends on a stale
/// frame. Returns the last frame, after pushing it once more (unless paused) so a still
/// ending lasts until the stop.
///
/// Frames replaced before they could be pushed are counted and logged at the end: with
/// a paced source there should be none, and many mean the pipeline couldn't keep up.
fn capture_loop(
    mut source: Box<dyn FrameSource>,
    mut feed: Feed,
    shared: Arc<Shared>,
    first: Image,
    fps: Option<u32>,
) -> Option<Image> {
    let mut pacer = Pacer::new(fps);
    let started = Instant::now();
    pacer.tick(started); // the first frame, pushed by the caller
    let (mut received, mut skipped, mut longest_gap) = (1u64, 0u64, Duration::ZERO);
    let mut last = first;
    let mut last_push = started;
    let mut pending: Option<Image> = None;
    while !shared.stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let wait = match (&pending, pacer.wait_until(now)) {
            (Some(_), Some(due)) => due - now,
            // Due, but the pipeline is busy: look again shortly.
            (Some(_), None) if !feed.has_room() => Duration::from_millis(2),
            (Some(_), None) => Duration::ZERO,
            (None, _) => Duration::from_millis(100),
        };
        match source.next_frame(wait.max(Duration::from_millis(1))) {
            Ok(Next::Frame(frame)) => {
                received += 1;
                skipped += pending.replace(frame).is_some() as u64;
            }
            Ok(Next::Unchanged) => {}
            Ok(Next::Ended) => {
                tracing::info!("the recorded source ended");
                shared.ended.store(true, Ordering::Relaxed);
                break;
            }
            Err(e) => {
                shared.fail(format!("screen capture stopped: {e}"));
                break;
            }
        }
        if shared.paused.load(Ordering::Relaxed) || pacer.wait_until(Instant::now()).is_some() || !feed.has_room() {
            continue;
        }
        if let Some(frame) = pending.take() {
            // Damage elsewhere on the output (or a compositor that doesn't track damage)
            // yields identical frames; skipping them keeps static screens nearly free.
            if same_pixels(&frame, &last) {
                continue;
            }
            let now = Instant::now();
            longest_gap = longest_gap.max(now - last_push);
            pacer.tick(now);
            feed.push(&frame);
            last = frame;
            last_push = now;
        }
    }
    if !shared.paused.load(Ordering::Relaxed) {
        feed.push(&last);
    }
    let seconds = started.elapsed().as_secs_f64().max(1e-3);
    tracing::info!(
        received,
        pushed = feed.pushed,
        skipped,
        fps = format!("{:.1}", feed.pushed as f64 / seconds),
        ?longest_gap,
        "recorded frames"
    );
    if skipped * 20 > received {
        tracing::warn!("the encoder couldn't keep up: {skipped} of {received} frames were skipped");
    }
    Some(last)
}

/// Whether two frames show the same pixels, within what's recorded of them.
fn same_pixels(a: &Image, b: &Image) -> bool {
    let (width, height) = even_size(a);
    if a.format() != b.format() || even_size(b) != (width, height) {
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
        assert!(same_pixels(&a, &b));
        // A change in the cropped-off column doesn't count.
        b.blit(&Image::from_raw(1, 1, 4, PixelFormat::Bgrx, vec![9, 9, 9, 255]), 4, 0);
        assert!(same_pixels(&a, &b));
        b.blit(&Image::from_raw(1, 1, 4, PixelFormat::Bgrx, vec![9, 9, 9, 255]), 1, 1);
        assert!(!same_pixels(&a, &b));
        // Nor are frames of another size the same.
        assert!(!same_pixels(&a, &Image::new(7, 3, PixelFormat::Bgrx)));
    }

    #[test]
    fn video_size_fits_the_resolution_cap() {
        let box1080 = Some((1920, 1080));
        assert_eq!(fit((3840, 2160), box1080), (1920, 1080));
        assert_eq!(fit((1280, 720), box1080), (1280, 720)); // never up
        assert_eq!(fit((3000, 1000), box1080), (1920, 640));
        assert_eq!(fit((1001, 3002), Some((1080, 1920))), (640, 1920));
        assert_eq!(fit((3840, 2160), None), (3840, 2160));
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
