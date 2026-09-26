//! The recording engine: frames from a [`FrameSource`] and optional audio are encoded to
//! an H.264/AAC MP4 with GStreamer.
//!
//! ```text
//! FrameSource ─ capture thread ─▶ appsrc ─▶ scale + convert ─▶ H.264 ─┐
//! pulsesrc (speakers) ─┐                                              ├─▶ mp4mux ─▶ file
//! pulsesrc (mic) ──────┴─▶ audiomixer ─▶ AAC ─────────────────────────┘
//! ```
//!
//! Frames stay on the GPU where they can: the compositor renders them into GPU buffers,
//! and an encoder on that same GPU scales, converts and encodes them there (see
//! `encoder`). Otherwise they come through memory. Either way, frames arrive only when
//! the screen changes and are capped at the configured frame rate, so a static screen
//! costs almost nothing. Each is stamped with when the compositor presented it, on the
//! pipeline's clock, which also keeps audio in sync across pauses.
//!
//! The video's size is the first frame's, capped by `recording.resolution` (scaled down
//! before colour conversion, so a 4K screen recorded at 1080p costs about what a 1080p
//! one does). A source whose frames change size (a recorded window being resized) is
//! scaled to fit it, letterboxed: frames in memory are padded with black to the video's
//! aspect ratio, since GPU scalers' own borders vary by driver (green on radeonsi), and
//! GPU frames switch to memory once the size changes. `recording.framerate` paces the
//! source itself, so the compositor only copies the frames that get recorded.

mod encoder;
mod feed;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gst::prelude::*;
use screenie_config::{EncoderPreference, Framerate, Quality, Resolution};
use screenie_core::{DmabufFormat, Frame, FrameSource, Image, Next, Pacer, Pixels};

pub use encoder::{Chain, Encoder};
use feed::Feed;

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
    /// The last frame pushed, for a thumbnail before the file is finished.
    latest: Mutex<Option<Image>>,
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
    capture: Option<JoinHandle<()>>,
    eos: mpsc::Receiver<Result<(), String>>,
    clock: Mutex<Clock>,
    /// The video's length, once capture has stopped.
    length: Option<Duration>,
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
        let native = even_size(first.size().0, first.size().1);
        if native.0 < 16 || native.1 < 16 {
            return Err(Error::TooSmall);
        }
        let size = fit(native, spec.resolution.bounds(native.0, native.1));
        // What the stream says it is; frames are timestamped as they come either way.
        let fps = cap.unwrap_or(60);
        let offer = source.gpu_offer();
        let encoder = Encoder::candidates(spec.encoder, size, offer.as_ref().map(|o| &o.device))
            .into_iter()
            .next()
            .ok_or(Error::NoEncoder)?;
        if let Some(dir) = spec.path.parent() {
            std::fs::create_dir_all(dir)?;
        }

        let mut first = Some(first);
        if let Some(format) = offer.as_ref().and_then(|o| encoder.gpu_format(o)) {
            match Pipeline::launch_gpu(source.as_mut(), &spec, &encoder, format, size, fps) {
                Ok(pipeline) => return Ok(Self::run(source, spec, encoder, pipeline, native, cap)),
                Err(e) => {
                    tracing::warn!("GPU frames can't go into {} ({e}); taking them through memory", encoder.name());
                    source.use_gpu(None).map_err(|e| Error::Source(e.to_string()))?;
                    first = None;
                }
            }
        }
        let first = match first {
            Some(frame) => frame,
            None => first_frame(source.as_mut())?,
        };
        let pipeline = Pipeline::launch(&spec, &encoder, size, fps, first, false)?;
        Ok(Self::run(source, spec, encoder, pipeline, native, cap))
    }

    /// Hand the source to the capture thread, feeding `pipeline`.
    fn run(
        source: Box<dyn FrameSource>,
        spec: RecordSpec,
        encoder: Encoder,
        pipeline: Pipeline,
        native: (u32, u32),
        cap: Option<u32>,
    ) -> Recording {
        let Pipeline { pipeline, feed, shared, eos, temp, first, mut cleanup } = pipeline;
        // From here on, the Recording cleans up.
        cleanup.armed = false;
        let size = feed.size;
        let gpu = matches!(first.pixels, Pixels::Gpu(_));
        let clock = Clock { started: Instant::now(), paused_at: None, paused_total: Duration::ZERO };
        let capture = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("screenie-record".into())
                .spawn(move || capture_loop(source, feed, shared, first, cap))
                .expect("spawning the capture thread")
        };
        tracing::info!(
            path = %spec.path.display(),
            encoder = encoder.name(),
            gpu_frames = gpu,
            ?native,
            ?size,
            framerate = ?spec.framerate,
            audio = ?spec.audio,
            "recording"
        );
        Recording {
            pipeline,
            shared,
            capture: Some(capture),
            eos,
            clock: Mutex::new(clock),
            length: None,
            path: spec.path,
            temp,
            size,
            encoder,
            finished: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn encoder(&self) -> &Encoder {
        &self.encoder
    }

    /// Recorded time so far, excluding pauses.
    pub fn elapsed(&self) -> Duration {
        self.length.unwrap_or_else(|| self.clock.lock().unwrap().elapsed())
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// The newest recorded frame: what the video ends on if stopped now.
    pub fn latest_frame(&self) -> Option<Image> {
        self.shared.latest.lock().unwrap().clone()
    }

    /// The video's size.
    pub fn size(&self) -> (u32, u32) {
        self.size
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
        self.stop_capture();
        let last = self.latest_frame();
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

    /// End the video here: nothing after this is recorded (so a preview card shown next
    /// can't end up in it). [`Recording::stop`] then finishes the file. Blocks briefly
    /// while the capture thread hands over its last frame.
    pub fn stop_capture(&mut self) {
        let Some(capture) = self.capture.take() else { return };
        self.length = Some(self.elapsed());
        self.shared.stop.store(true, Ordering::Relaxed);
        let _ = capture.join();
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

/// A pipeline started with its first frame, not yet fed by the capture thread.
struct Pipeline {
    pipeline: gst::Pipeline,
    feed: Feed,
    shared: Arc<Shared>,
    eos: mpsc::Receiver<Result<(), String>>,
    temp: PathBuf,
    first: Frame,
    cleanup: Cleanup,
}

/// Stops a pipeline that never became a [`Recording`], and removes its files.
struct Cleanup {
    pipeline: gst::Pipeline,
    paths: [PathBuf; 2],
    armed: bool,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.pipeline.set_state(gst::State::Null);
            for path in &self.paths {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

impl Pipeline {
    /// Switch the source to GPU buffers of `format` and start a pipeline for them,
    /// making sure the first frame gets into the encoder.
    fn launch_gpu(
        source: &mut dyn FrameSource,
        spec: &RecordSpec,
        encoder: &Encoder,
        format: DmabufFormat,
        size: (u32, u32),
        fps: u32,
    ) -> Result<Pipeline> {
        source.use_gpu(Some(format)).map_err(|e| Error::Source(e.to_string()))?;
        let first = first_frame(source)?;
        if !matches!(first.pixels, Pixels::Gpu(_)) {
            return Err(Error::Source("the first frame wasn't a GPU buffer".into()));
        }
        Pipeline::launch(spec, encoder, size, fps, first, true)
    }

    /// Build and start the pipeline, and push the first frame. With `confirm`, wait for
    /// that frame to get through the chain into the encoder.
    fn launch(
        spec: &RecordSpec,
        encoder: &Encoder,
        size: (u32, u32),
        fps: u32,
        first: Frame,
        confirm: bool,
    ) -> Result<Pipeline> {
        let gpu = matches!(first.pixels, Pixels::Gpu(_));
        let temp = temp_path(&spec.path);
        let desc = pipeline_description(spec, encoder, size, gpu)?;
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
        let element = by_name("encoder")?;
        encoder.configure(&element, spec.quality, fps, size);
        let (arrived, arrival) = mpsc::channel();
        if confirm && let Some(pad) = element.static_pad("sink") {
            pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                let _ = arrived.send(());
                gst::PadProbeReturn::Remove
            });
        }

        let appsrc = by_name("video")?
            .downcast::<gst_app::AppSrc>()
            .map_err(|_| Error::Gst("video source is not an appsrc".into()))?;
        appsrc.set_format(gst::Format::Time);
        appsrc.set_is_live(true);
        // Frames are stamped with when they were presented (see `Feed`).
        appsrc.set_do_timestamp(false);
        // The capture loop checks for room before pushing: when the pipeline falls
        // behind, frames are skipped there (and counted), not queued by the gigabyte.
        appsrc.set_property("max-buffers", feed::PIPELINE_FRAMES);

        let shared = Arc::new(Shared::default());
        let eos = watch_bus(&pipeline, shared.clone());
        let cleanup = Cleanup { pipeline: pipeline.clone(), paths: [spec.path.clone(), temp.clone()], armed: true };
        let mut started = Pipeline { feed: Feed::new(appsrc, fps, size), pipeline, shared, eos, temp, first, cleanup };
        started.pipeline.set_state(gst::State::Playing)?;
        started.feed.push(&started.first);
        if let Some(image) = started.first.image() {
            *started.shared.latest.lock().unwrap() = Some(image.clone());
        }
        if confirm {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if arrival.try_recv().is_ok() {
                    break;
                }
                if let Some(e) = started.shared.failure.lock().unwrap().clone() {
                    return Err(Error::Gst(e));
                }
                if Instant::now() >= deadline {
                    return Err(Error::Gst("the first frame didn't reach the encoder".into()));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Ok(started)
    }
}

fn first_frame(source: &mut dyn FrameSource) -> Result<Frame> {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match source.next_frame(Duration::from_millis(250)).map_err(|e| Error::Source(e.to_string()))? {
            Next::Frame(frame) => return Ok(frame),
            Next::Unchanged => {}
            Next::Ended => return Err(Error::Ended),
        }
    }
    Err(Error::NoFrames)
}

/// 4:2:0 chroma needs even dimensions; an odd last row or column is dropped.
fn even_size(width: u32, height: u32) -> (u32, u32) {
    (width & !1, height & !1)
}

/// `image` padded with black to the aspect ratio of `video`, centred, or `None` if it's
/// already within a couple of pixels of it.
fn letterbox(image: &Image, video: (u32, u32)) -> Option<Image> {
    let (w, h) = even_size(image.width(), image.height());
    let (vw, vh) = (video.0 as u64, video.1 as u64);
    let even_up = |v: u64| (v.div_ceil(2) * 2) as u32;
    let (pw, ph) = if w as u64 * vh > h as u64 * vw {
        (w, even_up((w as u64 * vh).div_ceil(vw)))
    } else {
        (even_up((h as u64 * vw).div_ceil(vh)), h)
    };
    if pw - w <= 2 && ph - h <= 2 {
        return None;
    }
    let mut padded = Image::new(pw, ph, image.format());
    padded.blit(image, ((pw - w) / 2) as i32, ((ph - h) / 2) as i32);
    Some(padded)
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

fn pipeline_description(spec: &RecordSpec, encoder: &Encoder, size: (u32, u32), gpu: bool) -> Result<String> {
    // Generous queues: the muxer interleaves audio and video, and encoders have latency.
    const QUEUE: &str = "queue max-size-buffers=0 max-size-bytes=0 max-size-time=3000000000";
    // Frames in memory are big (33 MB at 4K): a few decouple the capture thread from
    // scaling and conversion, and the appsrc holds a couple more before the capture loop
    // waits. GPU frames are converted in a blink, and each holds a capture buffer.
    let frames = if gpu { 1 } else { 3 };
    let mut desc = format!(
        "mp4mux name=mux faststart=true ! filesink name=sink \
         appsrc name=video ! queue max-size-buffers={frames} max-size-bytes=0 max-size-time=0 \
         ! {} ! {} ! h264parse ! {QUEUE} ! mux.",
        encoder.prepare(size),
        encoder.element()
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

/// Pull frames from the source until stopped, pushing at most `fps` per second (or all
/// of them). The newest frame always wins, and one that arrives early (or while the
/// pipeline is busy) is held rather than dropped, so the video never ends on a stale
/// frame. The last frame is pushed once more at the end (unless paused), so a still
/// ending lasts until the stop.
///
/// Frames replaced before they could be pushed are counted and logged at the end: with
/// a paced source there should be none, and many mean the pipeline couldn't keep up.
fn capture_loop(mut source: Box<dyn FrameSource>, mut feed: Feed, shared: Arc<Shared>, first: Frame, fps: Option<u32>) {
    let mut pacer = Pacer::new(fps);
    let started = Instant::now();
    pacer.tick(started); // the first frame, pushed by the caller
    let (mut received, mut skipped, mut longest_gap) = (1u64, 0u64, Duration::ZERO);
    // GPU scalers stretch rather than letterbox: a GPU frame of another size switches
    // the source to memory.
    let mut gpu_size = match &first.pixels {
        Pixels::Gpu(buffer) => Some((buffer.width, buffer.height, buffer.crop)),
        Pixels::Cpu(_) => None,
    };
    let mut last = first;
    let mut last_push = started;
    let mut pending: Option<Frame> = None;
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
        let Some(frame) = pending.take() else { continue };
        match (&frame.pixels, &last.pixels) {
            (Pixels::Gpu(buffer), _) if gpu_size.is_some_and(|s| s != (buffer.width, buffer.height, buffer.crop)) => {
                tracing::info!("the recording changed size; frames come through memory from now on");
                gpu_size = None;
                if let Err(e) = source.use_gpu(None) {
                    shared.fail(format!("screen capture stopped: {e}"));
                    break;
                }
                continue;
            }
            // Damage elsewhere on the output (or a compositor that doesn't track damage)
            // yields identical frames; skipping them keeps static screens nearly free.
            (Pixels::Cpu(a), Pixels::Cpu(b)) if same_pixels(a, b) => continue,
            _ => {}
        }
        let now = Instant::now();
        longest_gap = longest_gap.max(now - last_push);
        pacer.tick(now);
        feed.push(&frame);
        if let Some(image) = frame.image() {
            *shared.latest.lock().unwrap() = Some(image.clone());
        }
        last = frame;
        last_push = now;
    }
    if !shared.paused.load(Ordering::Relaxed) {
        // Stamped now, so the video lasts until the stop.
        feed.push(&Frame { presented: None, ..last.clone() });
    }
    if matches!(last.pixels, Pixels::Gpu(_)) {
        // The thumbnail needs the last frame's pixels in memory.
        if let Some(image) = source.snapshot() {
            *shared.latest.lock().unwrap() = Some(image);
        }
    }
    let seconds = started.elapsed().as_secs_f64().max(1e-3);
    tracing::info!(
        received,
        pushed = feed.pushed,
        gpu = feed.pushed_gpu,
        skipped,
        fps = format!("{:.1}", feed.pushed as f64 / seconds),
        ?longest_gap,
        "recorded frames"
    );
    if skipped * 20 > received {
        tracing::warn!("the encoder couldn't keep up: {skipped} of {received} frames were skipped");
    }
}

/// Whether two frames show the same pixels, within what's recorded of them.
fn same_pixels(a: &Image, b: &Image) -> bool {
    let (width, height) = even_size(a.width(), a.height());
    if a.format() != b.format() || even_size(b.width(), b.height()) != (width, height) {
        return false;
    }
    let row = width as usize * 4;
    (0..height as usize).all(|y| a.data()[y * a.stride()..][..row] == b.data()[y * b.stride()..][..row])
}

#[cfg(test)]
mod tests {
    use screenie_core::PixelFormat;

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
    fn frames_of_another_shape_are_letterboxed() {
        let red = Image::from_raw(1, 1, 4, PixelFormat::Bgrx, vec![0, 0, 255, 255]);
        let mut wide = Image::new(100, 30, PixelFormat::Bgrx);
        for y in 0..30 {
            for x in 0..100 {
                wide.blit(&red, x, y);
            }
        }
        // Into a 4:3 video: black above and below, the frame in the middle.
        let boxed = letterbox(&wide, (400, 300)).unwrap();
        assert_eq!((boxed.width(), boxed.height()), (100, 76));
        assert_eq!(boxed.rgba_at(50, 5)[..3], [0, 0, 0]);
        assert_eq!(boxed.rgba_at(50, 38)[..3], [255, 0, 0]);
        assert_eq!(boxed.rgba_at(50, 70)[..3], [0, 0, 0]);
        // Tall into wide: bars at the sides.
        let boxed = letterbox(&Image::new(30, 100, PixelFormat::Bgrx), (1920, 1080)).unwrap();
        assert_eq!((boxed.width(), boxed.height()), (178, 100));
        // Already the right shape (give or take rounding): untouched.
        assert!(letterbox(&Image::new(1921, 1080, PixelFormat::Bgrx), (1920, 1080)).is_none());
        assert!(letterbox(&Image::new(640, 361, PixelFormat::Bgrx), (1920, 1080)).is_none());
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
