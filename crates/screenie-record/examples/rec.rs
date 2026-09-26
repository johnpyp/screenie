//! Record an output for a few seconds.
//! Usage: cargo run -p screenie-record --example rec -- OUTPUT SECONDS FILE [x,y wxh] [audio] [pause]
//!        cargo run -p screenie-record --example rec -- encoders
use std::time::Duration;

use screenie_capture::CaptureContext;
use screenie_config::{CaptureBackend, EncoderPreference, Framerate, Quality, Resolution};
use screenie_record::{AudioSources, Encoder, RecordSpec, Recording};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("encoders") {
        gst::init()?;
        for (encoder, ok) in Encoder::survey() {
            println!("{:14} {:8} {}", encoder.factory, if encoder.hardware { "hardware" } else { "software" }, ok);
        }
        return Ok(());
    }
    let output = args.first().ok_or("usage: rec OUTPUT SECONDS FILE [region] [audio] [pause]")?;
    let seconds: f64 = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(3.0);
    let path = args.get(2).cloned().unwrap_or_else(|| ".cache/rec.mp4".into());
    if let Some(dir) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let region = args.get(3).filter(|a| a.contains('x')).map(|r| r.parse()).transpose()?;
    let flag = |f: &str| args.iter().any(|a| a == f);

    let capture = CaptureContext::new();
    let source = capture.stream(CaptureBackend::Auto, output, region, true)?;
    let spec = RecordSpec {
        path: std::fs::canonicalize(".")?.join(path),
        framerate: Framerate::default(),
        resolution: Resolution::Native,
        quality: Quality::High,
        encoder: std::env::var("ENCODER").map_or(EncoderPreference::Auto, |e| match e.as_str() {
            "hw" => EncoderPreference::Hardware,
            _ => EncoderPreference::Software,
        }),
        audio: AudioSources { system: flag("audio"), microphone: flag("mic") },
    };
    let recording = Recording::start(source, spec)?;
    println!("recording with {}", recording.encoder().factory);
    if flag("pause") {
        std::thread::sleep(Duration::from_secs_f64(seconds / 2.0));
        recording.set_paused(true)?;
        std::thread::sleep(Duration::from_secs(2));
        recording.set_paused(false)?;
        std::thread::sleep(Duration::from_secs_f64(seconds / 2.0));
    } else {
        std::thread::sleep(Duration::from_secs_f64(seconds));
    }
    let finished = recording.stop()?;
    println!("{finished:?}");
    Ok(())
}
