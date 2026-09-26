//! Choosing an H.264 encoder that actually works on this machine.
//!
//! An installed element is no proof: VA-API encoders exist whenever the plugin is
//! installed, even without a capable driver. So each candidate encodes a couple of test
//! frames once, and the verdict is cached for the life of the process. Encoders also
//! have size limits (VA-API: 128 to 4096 pixels a side), read from their pad templates,
//! so a tiny region or a native ultrawide goes to one that takes it.

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

/// The test encode's size: within every encoder's limits.
const PROBE: (u32, u32) = (320, 180);

impl Encoder {
    /// The first working encoder allowed by `preference` that takes `size` frames.
    pub fn select(preference: EncoderPreference, size: (u32, u32)) -> Option<Encoder> {
        CANDIDATES
            .iter()
            .filter(|e| match preference {
                EncoderPreference::Auto => true,
                EncoderPreference::Hardware => e.hardware,
                EncoderPreference::Software => !e.hardware,
            })
            .filter(|e| e.fits(size))
            .find(|e| e.works())
            .copied()
    }

    /// Whether its sink pad takes `width`×`height` frames. Unknown means yes (the
    /// pipeline will say otherwise).
    fn fits(&self, (width, height): (u32, u32)) -> bool {
        let Some(factory) = gst::ElementFactory::find(self.factory) else { return false };
        let within = |s: &gst::StructureRef, field: &str, v: u32| match s.value(field) {
            Ok(value) => match (value.get::<gst::IntRange<i32>>(), value.get::<i32>()) {
                (Ok(range), _) => (range.min()..=range.max()).contains(&(v as i32)),
                (_, Ok(exact)) => exact == v as i32,
                _ => true,
            },
            Err(_) => true,
        };
        factory
            .static_pad_templates()
            .iter()
            .filter(|t| t.direction() == gst::PadDirection::Sink)
            .all(|t| t.caps().iter().any(|s| within(s, "width", width) && within(s, "height", height)))
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

    /// Encode a few frames the way a recording does: screen pixels in, scaled.
    fn test_encode(&self) -> bool {
        let desc = format!(
            "videotestsrc num-buffers=3 ! video/x-raw,format=BGRx,width=640,height=360,framerate=30/1 \
             ! {} ! {} ! h264parse ! fakesink",
            self.prepare(PROBE),
            self.element(Quality::Medium, 30, PROBE)
        );
        let ok = runs(&desc);
        if !ok {
            tracing::debug!(encoder = self.factory, %desc, "test encode failed");
        }
        ok
    }


    /// The raw format to feed it: always 4:2:0, which every player decodes (left to
    /// negotiate, videoconvert would pick 4:4:4 for RGB input).
    fn input_format(&self) -> &'static str {
        if self.factory == "openh264enc" { "I420" } else { "NV12" }
    }

    /// The elements between screen pixels (BGRx and friends, of any size and the
    /// video's aspect ratio) and this encoder: scaled to `width`×`height` and converted
    /// to its input format. VA-API encoders get it done on the GPU along with
    /// the upload, which at 4K saves a whole CPU core; the rest on the CPU, scaling
    /// first so the conversion has fewer pixels to do.
    pub(crate) fn prepare(&self, (width, height): (u32, u32)) -> String {
        let format = self.input_format();
        match self.factory {
            "vah264enc" | "vah264lpenc" => format!(
                "vapostproc \
                 ! video/x-raw(memory:VAMemory),format={format},width={width},height={height},pixel-aspect-ratio=1/1"
            ),
            _ => format!(
                "videoscale add-borders=false n-threads=0 \
                 ! video/x-raw,width={width},height={height},pixel-aspect-ratio=1/1 \
                 ! videoconvert n-threads=0 ! video/x-raw,format={format}"
            ),
        }
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
                // In quality mode x264enc turns `bitrate` (default 2 Mbit/s) into a VBV
                // ceiling, which starves anything that moves. At its maximum, CRF decides.
                format!(
                    "x264enc speed-preset={preset} pass=qual quantizer={crf} bitrate=2048000 \
                     key-int-max={gop} bframes=0 threads=0"
                )
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

/// Whether a pipeline plays to its end without an error.
fn runs(desc: &str) -> bool {
    let Ok(pipeline) = gst::parse::launch(desc) else { return false };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_encode_fits_every_encoder() {
        gst::init().unwrap();
        for encoder in CANDIDATES.iter().filter(|e| gst::ElementFactory::find(e.factory).is_some()) {
            assert!(encoder.fits(PROBE), "{} can't take the test encode's size", encoder.factory);
        }
    }

    #[test]
    fn va_api_size_limits_are_read() {
        gst::init().unwrap();
        let va = Encoder { factory: "vah264enc", hardware: true };
        if gst::ElementFactory::find(va.factory).is_some() {
            assert!(va.fits((1920, 1080)));
            assert!(!va.fits((100, 300)));
            assert!(!va.fits((5120, 1440)));
        }
    }
}
