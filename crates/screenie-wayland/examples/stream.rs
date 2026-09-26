//! Stream an output for a while and report the frame rate and time per frame, to measure
//! capture alone (no encoding).
//! Usage: cargo run --release -p screenie-wayland --example stream -- OUTPUT [SECONDS] [ext|wlr]
use std::time::{Duration, Instant};

use screenie_wayland::{Backend, Capturer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let output = args.first().ok_or("usage: stream OUTPUT [SECONDS] [ext|wlr]")?;
    let seconds: f64 = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(5.0);
    let backend = match args.get(2).map(String::as_str) {
        Some("wlr") => Some(Backend::WlrScreencopy),
        Some("ext") => Some(Backend::ExtImageCopyCapture),
        _ => None,
    };
    let capturer = Capturer::connect_with(backend)?;
    println!("backend: {}", capturer.backend().name());
    let mut stream = capturer.into_stream(output, None, false)?;
    let start = Instant::now();
    let (mut frames, mut waited, mut size) = (0u32, Duration::ZERO, (0, 0));
    while start.elapsed().as_secs_f64() < seconds {
        let t = Instant::now();
        if let Some(f) = stream.next_frame(Duration::from_millis(500))? {
            waited += t.elapsed();
            frames += 1;
            size = (f.image.width(), f.image.height());
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "{frames} frames of {}x{} in {elapsed:.2}s: {:.1} fps, {:.1} ms per frame",
        size.0,
        size.1,
        frames as f64 / elapsed,
        waited.as_secs_f64() * 1000.0 / frames.max(1) as f64
    );
    Ok(())
}
