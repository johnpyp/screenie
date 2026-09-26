//! Choosing the H.264 encoder, and how frames get into it.
//!
//! Frames are encoded on the GPU that holds them: the compositor renders them into GPU
//! buffers on its own GPU, and an encoder on that GPU takes them from there without a
//! copy. So the order is:
//!
//! 1. hardware encoders on the frames' GPU,
//! 2. other hardware encoders (frames get there through memory),
//! 3. software encoders.
//!
//! Hardware encoders are found in the GStreamer registry, whatever the vendor: VA-API
//! (AMD, Intel, and any other GPU with a VA driver, one element set per GPU), NVENC
//! (NVIDIA), the older gstreamer-vaapi, and V4L2 (SoCs). Software: x264, OpenH264.
//!
//! How frames get into an encoder is its [`Chain`]: VA-API's own converter, GL, or
//! scaling and conversion on the CPU. The first two take GPU buffers as well as memory,
//! and do the scaling and colour conversion on the GPU.
//!
//! An installed element is no proof: VA-API encoders exist whenever the plugin is
//! installed, even without a capable driver. So each candidate encodes a few test frames
//! once, and the verdict is cached for the life of the process. Encoders also have size
//! limits (VA-API: 128 to 4096 pixels a side), read from their pad templates, so a tiny
//! region or a native ultrawide goes to one that takes it.
//!
//! `SCREENIE_ENCODER=factory[:va|gl|cpu]` forces one, for troubleshooting.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use gst::prelude::*;
use screenie_config::{EncoderPreference, Quality};
use screenie_core::gpu::{MODIFIER_LINEAR, VENDOR_NVIDIA, fourcc, fourcc_name};
use screenie_core::{DmabufFormat, GpuDevice, GpuOffer};

/// An H.264 encoder element and how frames reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoder {
    pub factory: String,
    family: Family,
    pub chain: Chain,
    /// The GPU it encodes on, where known.
    pub device: Option<GpuDevice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Family {
    /// GStreamer's `va` plugin. `prefix` names the GPU's elements: `va` for the first,
    /// `varenderD129` for another.
    Va {
        prefix: String,
    },
    Nvenc,
    /// The older gstreamer-vaapi plugin.
    Vaapi,
    V4l2,
    X264,
    OpenH264,
}

/// How frames get from the capture into the encoder, scaled to the video's size and
/// converted to 4:2:0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Chain {
    /// VA-API's converter, on the encoder's GPU. Takes memory and GPU buffers.
    Va,
    /// GL, on the GPU GStreamer's GL runs on. Takes memory and GPU buffers.
    Gl,
    /// On the CPU.
    Cpu,
}

/// The test encode's size: within every encoder's limits.
const PROBE: (u32, u32) = (320, 180);

/// DRM formats frames can come in, best first: 8-bit RGB, as screens are.
const GPU_FORMATS: [[u8; 4]; 4] = [*b"XR24", *b"AR24", *b"XB24", *b"AB24"];

impl Encoder {
    /// Working encoders allowed by `preference` that take `size` frames, best first for
    /// frames on `gpu`.
    pub fn candidates(
        preference: EncoderPreference,
        size: (u32, u32),
        gpu: Option<&GpuDevice>,
    ) -> Vec<Encoder> {
        let forced = std::env::var("SCREENIE_ENCODER")
            .ok()
            .filter(|s| !s.is_empty());
        let mut all: Vec<Encoder> = discover()
            .into_iter()
            .filter(|e| match &forced {
                Some(f) => f == &e.factory || f == &format!("{}:{}", e.factory, e.chain.name()),
                None => match preference {
                    EncoderPreference::Auto => true,
                    EncoderPreference::Hardware => e.hardware(),
                    EncoderPreference::Software => !e.hardware(),
                },
            })
            .collect();
        all.sort_by_key(|e| e.tier(gpu));
        all.into_iter()
            .filter(|e| e.fits(size) && e.works())
            .collect()
    }

    /// Every encoder found, with whether it works here, for diagnostics.
    pub fn survey() -> Vec<(Encoder, bool)> {
        discover()
            .into_iter()
            .map(|e| {
                let ok = e.works();
                (e, ok)
            })
            .collect()
    }

    pub fn hardware(&self) -> bool {
        !matches!(self.family, Family::X264 | Family::OpenH264)
    }

    /// For logs: `vah264enc`, `nvh264enc (gl)`.
    pub fn name(&self) -> String {
        let chain = match (&self.family, self.chain) {
            (Family::Va { .. }, Chain::Va) | (_, Chain::Cpu) => String::new(),
            (_, chain) => format!(" ({})", chain.name()),
        };
        format!("{}{chain}", self.factory)
    }

    /// Lower is better: on the frames' GPU, then other hardware, then software; then
    /// by family, GPU chains first.
    fn tier(&self, gpu: Option<&GpuDevice>) -> (u8, u8) {
        let tier = match (self.hardware(), gpu) {
            (true, Some(gpu)) if self.runs_on(gpu) => 0,
            (true, _) => 1,
            (false, _) => 2,
        };
        let family = match (&self.family, self.chain) {
            (Family::Va { .. }, _) => 0,
            (Family::Nvenc, Chain::Gl) => 1,
            (Family::Nvenc, _) => 2,
            (Family::Vaapi, _) => 3,
            (Family::V4l2, _) => 4,
            (Family::X264, _) => 5,
            (Family::OpenH264, _) => 6,
        };
        (tier, family)
    }

    /// Whether it encodes on `gpu`.
    fn runs_on(&self, gpu: &GpuDevice) -> bool {
        match (&self.family, &self.device) {
            (_, Some(device)) => device.same_as(gpu),
            // GStreamer doesn't say which NVIDIA GPU NVENC is on; one is the usual case.
            (Family::Nvenc, None) => gpu.vendor == Some(VENDOR_NVIDIA),
            _ => false,
        }
    }

    /// The GPU buffer format to take frames in from `offer`, if this encoder can: it runs
    /// on the offer's GPU, and its chain imports one of the formats offered. Only the
    /// layouts (modifiers) both sides take are kept.
    pub fn gpu_format(&self, offer: &GpuOffer) -> Option<DmabufFormat> {
        if !self.runs_on(&offer.device) || gst::version() < (1, 24, 0, 0) {
            return None;
        }
        let importer = self.importer()?;
        let importable = importable(&importer);
        GPU_FORMATS.iter().map(fourcc).find_map(|code| {
            let offered = offer.formats.iter().find(|f| f.fourcc == code)?;
            let modifiers: Vec<u64> = offered
                .modifiers
                .iter()
                .copied()
                .filter(|m| importable.contains(&(code, *m)))
                .collect();
            (!modifiers.is_empty()).then_some(DmabufFormat {
                fourcc: code,
                modifiers,
            })
        })
    }

    /// The first element of its chain, if it can import GPU buffers.
    fn importer(&self) -> Option<String> {
        match (&self.family, self.chain) {
            (Family::Va { prefix }, Chain::Va) => Some(format!("{prefix}postproc")),
            (_, Chain::Gl) => Some("glupload".into()),
            _ => None,
        }
    }

    /// Whether its sink pad takes `width`×`height` frames. Unknown means yes (the
    /// pipeline will say otherwise).
    fn fits(&self, (width, height): (u32, u32)) -> bool {
        let Some(factory) = gst::ElementFactory::find(&self.factory) else {
            return false;
        };
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
            .all(|t| {
                t.caps()
                    .iter()
                    .any(|s| within(s, "width", width) && within(s, "height", height))
            })
    }

    fn works(&self) -> bool {
        static VERDICTS: OnceLock<Mutex<HashMap<(String, Chain), bool>>> = OnceLock::new();
        let verdicts = VERDICTS.get_or_init(Default::default);
        let key = (self.factory.clone(), self.chain);
        if let Some(&ok) = verdicts.lock().unwrap().get(&key) {
            return ok;
        }
        let ok = self.test_encode();
        tracing::debug!(encoder = self.name(), ok, "probed encoder");
        verdicts.lock().unwrap().insert(key, ok);
        ok
    }

    /// Encode a few frames the way a recording does: screen pixels in, scaled.
    fn test_encode(&self) -> bool {
        let desc = format!(
            "videotestsrc num-buffers=3 ! video/x-raw,format=BGRx,width=640,height=360,framerate=30/1 \
             ! {} ! {} ! h264parse ! fakesink",
            self.prepare(PROBE),
            self.element()
        );
        let Ok(pipeline) = gst::parse::launch(&desc) else {
            return false;
        };
        if let Some(encoder) = pipeline
            .downcast_ref::<gst::Bin>()
            .and_then(|b| b.by_name("encoder"))
        {
            self.configure(&encoder, Quality::Medium, 30, PROBE);
        }
        let ok = runs(&pipeline);
        if !ok {
            tracing::debug!(encoder = self.name(), %desc, "test encode failed");
        }
        ok
    }

    /// The raw format to feed it: always 4:2:0, which every player decodes (left to
    /// negotiate, videoconvert would pick 4:4:4 for RGB input).
    fn input_format(&self) -> &'static str {
        if self.family == Family::OpenH264 {
            "I420"
        } else {
            "NV12"
        }
    }

    /// The elements between the frames (screen pixels in memory or GPU buffers, of any
    /// size and the video's aspect ratio) and the encoder: scaled to `width`×`height` and
    /// converted to its input format. On the CPU, scaling comes first so the conversion
    /// has fewer pixels to do.
    pub(crate) fn prepare(&self, (width, height): (u32, u32)) -> String {
        let format = self.input_format();
        match (&self.family, self.chain) {
            (Family::Va { prefix }, Chain::Va) => format!(
                "{prefix}postproc \
                 ! video/x-raw(memory:VAMemory),format={format},width={width},height={height},pixel-aspect-ratio=1/1"
            ),
            // GL scales RGB only: scale, then convert. Encoders other than NVENC take
            // the result from memory.
            (family, Chain::Gl) => format!(
                "glupload ! glcolorscale \
                 ! video/x-raw(memory:GLMemory),width={width},height={height},pixel-aspect-ratio=1/1 \
                 ! glcolorconvert ! video/x-raw(memory:GLMemory),format={format}{}",
                if *family == Family::Nvenc {
                    ""
                } else {
                    " ! gldownload"
                }
            ),
            _ => format!(
                "videoscale add-borders=false n-threads=0 \
                 ! video/x-raw,width={width},height={height},pixel-aspect-ratio=1/1 \
                 ! videoconvert n-threads=0 ! video/x-raw,format={format}"
            ),
        }
    }

    /// The encoder's element for a `gst::parse::launch` pipeline, named `encoder`, to be
    /// set up with [`Encoder::configure`].
    ///
    /// Hardware encoders are held to the High profile, which every player decodes: left
    /// to choose, NVENC takes the first profile downstream allows (alphabetically, so
    /// Baseline) when there are no B-frames, and Baseline's entropy coding costs a
    /// fifth more bits for the same picture. x264 picks High itself; OpenH264 only does
    /// Baseline, whatever it says.
    pub(crate) fn element(&self) -> String {
        let profile = match self.family {
            Family::Va { .. } | Family::Vaapi | Family::Nvenc => " ! video/x-h264,profile=high",
            _ => "",
        };
        format!("{} name=encoder{profile}", self.factory)
    }

    /// Set the encoder up: constant quality, no B-frames, a keyframe every two seconds.
    /// Properties are set by name where the element has them, since they differ between
    /// GStreamer versions (NVENC's changed in 1.26).
    pub(crate) fn configure(
        &self,
        element: &gst::Element,
        quality: Quality,
        fps: u32,
        (width, height): (u32, u32),
    ) {
        // Keyframes every two seconds keep seeking snappy in players and chat apps. No
        // B-frames: with variable frame rate they skew decode timestamps (and durations
        // in some players) for little gain on screen content.
        let gop = (fps.max(1) * 2).to_string();
        // Constant-quality QP for hardware encoders and CRF for x264: lower is better.
        let (qp, crf) = match quality {
            Quality::Low => (30, 28),
            Quality::Medium => (26, 23),
            Quality::High => (22, 19),
            Quality::Lossless => (16, 12),
        };
        let qp = qp.to_string();
        let set = |name: &str, value: &str| {
            if element.find_property(name).is_some() {
                element.set_property_from_str(name, value);
            }
        };
        match self.family {
            Family::Va { .. } => {
                set("rate-control", "cqp");
                set("qpi", &qp);
                set("qpp", &qp);
                set("qpb", &qp);
                set("key-int-max", &gop);
                set("b-frames", "0");
            }
            Family::Vaapi => {
                set("rate-control", "cqp");
                set("init-qp", &qp);
                set("keyframe-period", &gop);
            }
            Family::Nvenc => {
                // 1.26+ (and the old nvh264enc): rc-mode, qp-const, bframes.
                set("rc-mode", "constqp");
                set("qp-const", &qp);
                set("bframes", "0");
                set("zerolatency", "true");
                // 1.22–1.24's nvcudah264enc: rate-control, qp-i/p, b-frames.
                set("rate-control", "cqp");
                set("qp-i", &qp);
                set("qp-p", &qp);
                set("b-frames", "0");
                set("zero-reorder-delay", "true");
                set("gop-size", &gop);
                if enum_has(element, "preset", "p4") {
                    set("preset", "p4");
                }
                if enum_has(element, "tune", "low-latency") {
                    set("tune", "low-latency");
                }
            }
            Family::V4l2 => {
                // V4L2 encoders take controls, and most only do bitrate control.
                let bitrate = bits_per_second(quality, fps, (width, height));
                element.set_property_from_str(
                    "extra-controls",
                    &format!("controls,video_bitrate={bitrate},h264_i_frame_period={gop}"),
                );
            }
            Family::X264 => {
                // Faster presets above 1080p60 keep a software encode real-time.
                let heavy = width as u64 * height as u64 * fps as u64 > 1920 * 1080 * 60;
                set("speed-preset", if heavy { "superfast" } else { "veryfast" });
                set("pass", "qual");
                set("quantizer", &crf.to_string());
                // In quality mode x264enc turns `bitrate` (default 2 Mbit/s) into a VBV
                // ceiling, which starves anything that moves. At its maximum, CRF decides.
                set("bitrate", "2048000");
                set("key-int-max", &gop);
                set("bframes", "0");
                set("threads", "0");
            }
            Family::OpenH264 => {
                // openh264 only does bitrate control.
                set("rate-control", "bitrate");
                set(
                    "bitrate",
                    &bits_per_second(quality, fps, (width, height)).to_string(),
                );
                set("gop-size", &gop);
            }
        }
    }
}

impl Chain {
    pub fn name(self) -> &'static str {
        match self {
            Chain::Va => "va",
            Chain::Gl => "gl",
            Chain::Cpu => "cpu",
        }
    }
}

/// A bitrate for encoders without constant quality: a bits-per-pixel budget.
fn bits_per_second(quality: Quality, fps: u32, (width, height): (u32, u32)) -> u32 {
    let bpp = match quality {
        Quality::Low => 0.04,
        Quality::Medium => 0.07,
        Quality::High => 0.1,
        Quality::Lossless => 0.2,
    };
    (width as f64 * height as f64 * fps as f64 * bpp).min(u32::MAX as f64) as u32
}

fn enum_has(element: &gst::Element, property: &str, nick: &str) -> bool {
    element
        .find_property(property)
        .and_then(|p| p.downcast::<gst::glib::ParamSpecEnum>().ok())
        .is_some_and(|p| p.enum_class().value_by_nick(nick).is_some())
}

/// Every H.264 encoder in the registry that we know how to drive, with each way frames
/// can reach it.
fn discover() -> Vec<Encoder> {
    static FOUND: OnceLock<Vec<Encoder>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let factories = gst::ElementFactory::factories_with_type(
                gst::ElementFactoryType::ENCODER | gst::ElementFactoryType::MEDIA_VIDEO,
                gst::Rank::NONE,
            );
            let names: HashSet<String> = factories.iter().map(|f| f.name().to_string()).collect();
            let mut found = Vec::new();
            for name in &names {
                let Some(family) = family(name, &names) else {
                    continue;
                };
                let device = match &family {
                    Family::Va { .. } => va_device(name),
                    _ => None,
                };
                let chains: &[Chain] = match family {
                    Family::Va { .. } => &[Chain::Va],
                    Family::Nvenc => &[Chain::Gl, Chain::Cpu],
                    _ => &[Chain::Cpu],
                };
                for &chain in chains {
                    found.push(Encoder {
                        factory: name.clone(),
                        family: family.clone(),
                        chain,
                        device: device.clone(),
                    });
                }
            }
            found.sort_by(|a, b| a.factory.cmp(&b.factory));
            found
        })
        .clone()
}

/// The family an encoder element belongs to, if we know how to drive it.
fn family(name: &str, all: &HashSet<String>) -> Option<Family> {
    match name {
        "x264enc" => return Some(Family::X264),
        "openh264enc" => return Some(Family::OpenH264),
        "vaapih264enc" => return Some(Family::Vaapi),
        _ => {}
    }
    // va: vah264enc, vah264lpenc (low power), varenderD129h264enc for another GPU.
    if let Some(prefix) = name
        .strip_suffix("h264enc")
        .or_else(|| name.strip_suffix("h264lpenc"))
        && prefix.starts_with("va")
        && !prefix.starts_with("vaapi")
    {
        return Some(Family::Va {
            prefix: prefix.to_string(),
        });
    }
    // NVENC: nvh264enc, nvcudah264enc, nvh264device1enc. On 1.22–1.24 nvh264enc is the
    // old implementation and nvcudah264enc the new one; take the new one where both are.
    // nvautogpuh264enc picks its GPU from CUDA input, which we don't give it. Jetson's
    // nvv4l2h264enc takes its own memory type.
    if name.starts_with("nv") && name.contains("h264") && name.ends_with("enc") {
        if name.starts_with("nvautogpu") || name.starts_with("nvv4l2") {
            return None;
        }
        if name == "nvh264enc" && all.contains("nvcudah264enc") {
            return None;
        }
        return Some(Family::Nvenc);
    }
    if name.starts_with("v4l2") && name.ends_with("h264enc") {
        return Some(Family::V4l2);
    }
    None
}

/// The GPU a `va` element works on.
fn va_device(factory: &str) -> Option<GpuDevice> {
    let element = gst::ElementFactory::make(factory).build().ok()?;
    let path: String = element
        .find_property("device-path")
        .map(|_| element.property("device-path"))?;
    GpuDevice::from_node(std::path::Path::new(&path))
}

/// The DMA-BUF formats (fourcc, modifier) `factory` imports, from its sink caps once it
/// has opened its device. Linear is written without a modifier.
fn importable(factory: &str) -> HashSet<(u32, u64)> {
    type Formats = HashSet<(u32, u64)>;
    static CACHE: OnceLock<Mutex<HashMap<String, Formats>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(found) = cache.lock().unwrap().get(factory) {
        return found.clone();
    }
    let mut found = HashSet::new();
    if let Ok(element) = gst::ElementFactory::make(factory).build()
        && element.set_state(gst::State::Ready).is_ok()
        && let Some(pad) = element.static_pad("sink")
    {
        let caps = pad.query_caps(None);
        for (s, features) in caps.iter_with_features() {
            if !features.contains("memory:DMABuf") {
                continue;
            }
            let Ok(value) = s.value("drm-format") else {
                continue;
            };
            let strings: Vec<String> = match (value.get::<String>(), value.get::<gst::List>()) {
                (Ok(one), _) => vec![one],
                (_, Ok(list)) => list.iter().filter_map(|v| v.get::<String>().ok()).collect(),
                _ => Vec::new(),
            };
            found.extend(strings.iter().filter_map(|s| parse_drm_format(s)));
        }
        let _ = element.set_state(gst::State::Null);
    }
    tracing::debug!(
        factory,
        formats = found.len(),
        "GPU buffer formats it imports"
    );
    cache
        .lock()
        .unwrap()
        .insert(factory.to_string(), found.clone());
    found
}

/// `XR24` (linear) or `XR24:0x0100000000000001`.
fn parse_drm_format(s: &str) -> Option<(u32, u64)> {
    let (code, modifier) = match s.split_once(':') {
        Some((code, m)) => (
            code,
            u64::from_str_radix(m.trim_start_matches("0x"), 16).ok()?,
        ),
        None => (s, MODIFIER_LINEAR),
    };
    let bytes: [u8; 4] = code.as_bytes().try_into().ok()?;
    Some((fourcc(&bytes), modifier))
}

/// The caps value for a GPU buffer format, as GStreamer writes it: linear bare.
pub(crate) fn drm_format_string(code: u32, modifier: u64) -> String {
    let name = fourcc_name(code);
    if modifier == MODIFIER_LINEAR {
        name
    } else {
        format!("{name}:0x{modifier:016x}")
    }
}

/// Whether a pipeline plays to its end without an error.
fn runs(pipeline: &gst::Element) -> bool {
    let ok = pipeline.set_state(gst::State::Playing).is_ok()
        && pipeline.bus().is_some_and(|bus| {
            let msg = bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(Duration::from_secs(5).as_millis() as u64),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            );
            matches!(
                msg.as_ref().map(|m| m.view()),
                Some(gst::MessageView::Eos(_))
            )
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

    fn names(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn families_by_name() {
        let all = names(&[
            "vah264enc",
            "varenderD129h264lpenc",
            "nvh264enc",
            "nvcudah264enc",
            "nvh264device1enc",
        ]);
        assert_eq!(
            family("vah264enc", &all),
            Some(Family::Va {
                prefix: "va".into()
            })
        );
        assert_eq!(
            family("varenderD129h264lpenc", &all),
            Some(Family::Va {
                prefix: "varenderD129".into()
            })
        );
        assert_eq!(family("vaapih264enc", &all), Some(Family::Vaapi));
        // The old nvh264enc gives way to the new one next to it (1.22–1.24)…
        assert_eq!(family("nvh264enc", &all), None);
        assert_eq!(family("nvcudah264enc", &all), Some(Family::Nvenc));
        assert_eq!(family("nvh264device1enc", &all), Some(Family::Nvenc));
        // …and is the new one where it's alone (1.26+).
        assert_eq!(
            family("nvh264enc", &names(&["nvh264enc"])),
            Some(Family::Nvenc)
        );
        assert_eq!(family("nvautogpuh264enc", &all), None);
        assert_eq!(family("v4l2h264enc", &all), Some(Family::V4l2));
        assert_eq!(family("vah265enc", &all), None);
    }

    #[test]
    fn drm_format_strings() {
        assert_eq!(
            parse_drm_format("XR24"),
            Some((fourcc(b"XR24"), MODIFIER_LINEAR))
        );
        assert_eq!(
            parse_drm_format("AR24:0x020000000056bb03"),
            Some((fourcc(b"AR24"), 0x020000000056bb03))
        );
        assert_eq!(drm_format_string(fourcc(b"XR24"), 0), "XR24");
        assert_eq!(
            drm_format_string(fourcc(b"XR24"), 0x0300000000606010),
            "XR24:0x0300000000606010"
        );
    }

    fn gpu(render: &str, vendor: u16) -> GpuDevice {
        GpuDevice {
            dev: 0,
            render_node: Some(render.into()),
            vendor: Some(vendor),
            driver: None,
            bus: None,
        }
    }

    #[test]
    fn the_frames_gpu_comes_first() {
        let amd = gpu("/dev/dri/renderD129", 0x1002);
        let nvidia = gpu("/dev/dri/renderD128", VENDOR_NVIDIA);
        let va = Encoder {
            factory: "varenderD129h264enc".into(),
            family: Family::Va {
                prefix: "varenderD129".into(),
            },
            chain: Chain::Va,
            device: Some(amd.clone()),
        };
        let nvenc = Encoder {
            factory: "nvh264enc".into(),
            family: Family::Nvenc,
            chain: Chain::Gl,
            device: None,
        };
        let x264 = Encoder {
            factory: "x264enc".into(),
            family: Family::X264,
            chain: Chain::Cpu,
            device: None,
        };
        let mut list = vec![x264.clone(), va.clone(), nvenc.clone()];
        list.sort_by_key(|e| e.tier(Some(&nvidia)));
        assert_eq!(list, [nvenc.clone(), va.clone(), x264.clone()]);
        list.sort_by_key(|e| e.tier(Some(&amd)));
        assert_eq!(list, [va, nvenc, x264]);
    }

    #[test]
    fn the_test_encode_fits_every_encoder() {
        gst::init().unwrap();
        for encoder in discover() {
            assert!(
                encoder.fits(PROBE),
                "{} can't take the test encode's size",
                encoder.factory
            );
        }
    }

    #[test]
    fn va_api_size_limits_are_read() {
        gst::init().unwrap();
        if let Some(va) = discover().into_iter().find(|e| e.factory == "vah264enc") {
            assert!(va.fits((1920, 1080)));
            assert!(!va.fits((100, 300)));
            assert!(!va.fits((5120, 1440)));
        }
    }
}
