//! Choosing an H.264 encoder that actually works on this machine.
//!
//! An installed element is no proof: VA-API encoders exist whenever the plugin is
//! installed, even without a capable driver. So each candidate encodes a couple of test
//! frames once, and the verdict is cached for the life of the process.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use gst::prelude::*;

use screenie_config::{EncoderPreference, Quality};

/// An H.264 encoder element and how to configure it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoder {
    pub factory: &'static str,
    pub hardware: bool,
}

/// In order of preference.
const CANDIDATES: &[Encoder] = &[
    Encoder { factory: "vah264enc", hardware: true },
    Encoder { factory: "vah264lpenc", hardware: true },
    Encoder { factory: "vaapih264enc", hardware: true },
    Encoder { factory: "x264enc", hardware: false },
    Encoder { factory: "openh264enc", hardware: false },
];

impl Encoder {
    /// The first working encoder allowed by `preference`.
    pub fn select(preference: EncoderPreference) -> Option<Encoder> {
        CANDIDATES
            .iter()
            .filter(|e| match preference {
                EncoderPreference::Auto => true,
                EncoderPreference::Hardware => e.hardware,
                EncoderPreference::Software => !e.hardware,
            })
            .find(|e| e.works())
            .copied()
    }

    /// Every candidate with whether it works here, for diagnostics.
    pub fn survey() -> Vec<(Encoder, bool)> {
        CANDIDATES.iter().map(|e| (*e, e.works())).collect()
    }

    fn works(&self) -> bool {
        static VERDICTS: OnceLock<Mutex<HashMap<&'static str, bool>>> = OnceLock::new();
        let verdicts = VERDICTS.get_or_init(Default::default);
        if let Some(&ok) = verdicts.lock().unwrap().get(self.factory) {
            return ok;
        }
        let ok = gst::ElementFactory::find(self.factory).is_some() && self.test_encode();
        tracing::debug!(encoder = self.factory, ok, "probed encoder");
        verdicts.lock().unwrap().insert(self.factory, ok);
        ok
    }

    fn test_encode(&self) -> bool {
        let desc = format!(
            "videotestsrc num-buffers=3 ! video/x-raw,format={},width=320,height=240,framerate=30/1 \
             ! {} ! h264parse ! fakesink",
            self.input_format(),
            self.element(Quality::Medium, 30, (320, 240))
        );
        let Ok(pipeline) = gst::parse::launch(&desc) else { return false };
        let ok = pipeline.set_state(gst::State::Playing).is_ok()
            && pipeline.bus().is_some_and(|bus| {
                let msg = bus.timed_pop_filtered(
                    gst::ClockTime::from_mseconds(Duration::from_secs(5).as_millis() as u64),
                    &[gst::MessageType::Eos, gst::MessageType::Error],
                );
                matches!(msg.as_ref().map(|m| m.view()), Some(gst::MessageView::Eos(_)))
            });
        let _ = pipeline.set_state(gst::State::Null);
        ok
    }

    /// The raw format to feed it: always 4:2:0, which every player decodes (left to
    /// negotiate, videoconvert would pick 4:4:4 for RGB input).
    pub(crate) fn input_format(&self) -> &'static str {
        if self.factory == "openh264enc" { "I420" } else { "NV12" }
    }

    /// The element description for a `gst::parse::launch` pipeline.
    pub(crate) fn element(&self, quality: Quality, fps: u32, (width, height): (u32, u32)) -> String {
        // Keyframes every two seconds keep seeking snappy in players and chat apps. No
        // B-frames: with variable frame rate they skew decode timestamps (and durations
        // in some players) for little gain on screen content.
        let gop = fps.max(1) * 2;
        // Constant-quality QP for hardware encoders and CRF for x264: lower is better.
        let (qp, crf) = match quality {
            Quality::Low => (30, 28),
            Quality::Medium => (26, 23),
            Quality::High => (22, 19),
            Quality::Lossless => (16, 12),
        };
        match self.factory {
            "vah264enc" | "vah264lpenc" => {
                format!("{} rate-control=cqp qpi={qp} qpp={qp} qpb={qp} key-int-max={gop} b-frames=0", self.factory)
            }
            "vaapih264enc" => format!("vaapih264enc rate-control=cqp init-qp={qp} keyframe-period={gop}"),
            "x264enc" => {
                // Faster presets above 1080p60 keep a software encode real-time.
                let heavy = width as u64 * height as u64 * fps as u64 > 1920 * 1080 * 60;
                let preset = if heavy { "superfast" } else { "veryfast" };
                format!("x264enc speed-preset={preset} pass=qual quantizer={crf} key-int-max={gop} bframes=0 threads=0")
            }
            _ => {
                // openh264 only does bitrate control; aim for a bits-per-pixel budget.
                let bpp = match quality {
                    Quality::Low => 0.04,
                    Quality::Medium => 0.07,
                    Quality::High => 0.1,
                    Quality::Lossless => 0.2,
                };
                let bitrate = (width as f64 * height as f64 * fps as f64 * bpp).min(u32::MAX as f64) as u32;
                format!("openh264enc rate-control=bitrate bitrate={bitrate} gop-size={gop}")
            }
        }
    }
}

/// The first available AAC encoder, else Opus (which MP4 also carries).
pub(crate) fn audio_encoder() -> Option<&'static str> {
    [
        ("avenc_aac", "avenc_aac bitrate=192000"),
        ("fdkaacenc", "fdkaacenc bitrate=192000"),
        ("voaacenc", "voaacenc bitrate=192000"),
        ("opusenc", "opusenc bitrate=128000"),
    ]
    .into_iter()
    .find(|(factory, _)| gst::ElementFactory::find(factory).is_some())
    .map(|(_, desc)| desc)
}
