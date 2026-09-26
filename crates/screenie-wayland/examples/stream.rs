//! Stream an output for a while and report the frame rate and time per frame, to measure
//! capture alone (no encoding). `gpu` takes frames in GPU buffers (linear XRGB8888, as
//! VA-API imports them) instead of shared memory; `offer` lists what the compositor offers.
//! Usage: cargo run --release -p screenie-wayland --example stream -- OUTPUT [SECONDS] [ext|wlr] [gpu|offer] [FPS]
use std::time::{Duration, Instant};

use screenie_core::gpu::{MODIFIER_LINEAR, fourcc, fourcc_name};
use screenie_core::{DmabufFormat, Pixels};
use screenie_wayland::{Backend, Capturer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // RUST_LOG=debug shows what the compositor says about GPU buffers.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let output = args
        .first()
        .ok_or("usage: stream OUTPUT [SECONDS] [ext|wlr] [gpu|offer] [FPS]")?;
    let seconds: f64 = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(5.0);
    let backend = match args.get(2).map(String::as_str) {
        Some("wlr") => Some(Backend::WlrScreencopy),
        Some("ext") => Some(Backend::ExtImageCopyCapture),
        _ => None,
    };
    let mode = args.get(3).map(String::as_str).unwrap_or("shm");
    let capturer = Capturer::connect_with(backend)?;
    println!("backend: {}", capturer.backend().name());
    let mut stream = capturer.into_stream(output, None, false)?;
    if let Some(fps) = args.get(4) {
        stream.set_max_rate(fps.parse()?);
    }
    // The first frame brings the compositor's buffer constraints.
    while stream.next_frame(Duration::from_millis(500))?.is_none() {}
    if mode != "shm" {
        let offer = stream
            .gpu_offer()
            .ok_or("the compositor offers no GPU buffers")?;
        println!("GPU: {:?}", offer.device);
        for f in &offer.formats {
            let mods: Vec<String> = f.modifiers.iter().map(|m| format!("{m:#x}")).collect();
            println!("  {}: {}", fourcc_name(f.fourcc), mods.join(" "));
        }
        if mode == "offer" {
            return Ok(());
        }
        stream.use_gpu(Some(DmabufFormat {
            fourcc: fourcc(b"XR24"),
            modifiers: vec![MODIFIER_LINEAR],
        }))?;
    }
    let start = Instant::now();
    let (mut frames, mut waited, mut size) = (0u32, Duration::ZERO, (0, 0));
    while start.elapsed().as_secs_f64() < seconds {
        let t = Instant::now();
        if let Some(f) = stream.next_frame(Duration::from_millis(500))? {
            waited += t.elapsed();
            frames += 1;
            size = f.size();
            // Hand GPU buffers straight back, as a consumer that's done with them would.
            if let Pixels::Gpu(buf) = &f.pixels {
                assert!(buf.width > 0);
            }
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
