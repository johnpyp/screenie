//! Capture every output to `<dir>/<output>.png` and print what was found.
//! Usage: cargo run -p screenie-wayland --example snap -- [dir] [ext|wlr] [--stream N]
use screenie_wayland::{Backend, Capturer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(args.first().map(String::as_str).unwrap_or(".cache/snap"));
    let backend = match args.get(1).map(String::as_str) {
        Some("ext") => Some(Backend::ExtImageCopyCapture),
        Some("wlr") => Some(Backend::WlrScreencopy),
        _ => None,
    };
    std::fs::create_dir_all(&dir)?;
    let mut capturer = Capturer::connect_with(backend)?;
    println!("backend: {}", capturer.backend().name());
    let t = std::time::Instant::now();
    let captures = capturer.capture_outputs(None, false)?;
    println!("captured {} outputs in {:?}", captures.len(), t.elapsed());
    for c in &captures {
        println!("  {:?} -> {}x{} {:?} (scale {:.3})", c.output, c.image.width(), c.image.height(), c.image.format(), c.scale());
        c.image.save_png(&dir.join(format!("{}.png", c.output.name)))?;
    }
    if let Some(pos) = args.iter().position(|a| a == "--stream") {
        let n: u64 = args[pos + 1].parse()?;
        let name = captures[0].output.name.clone();
        let mut stream = capturer.into_stream(&name, Some(screenie_core::Rect::new(10.0, 10.0, 300.0, 200.0)), true)?;
        let t = std::time::Instant::now();
        let mut got = 0;
        while got < n && t.elapsed().as_secs() < 10 {
            if let Some(f) = stream.next_frame(std::time::Duration::from_millis(500))? {
                println!("  frame {} {}x{} presented {:?} at {:?}", f.sequence, f.image.width(), f.image.height(), f.presented, t.elapsed());
                got += 1;
            } else {
                println!("  (no new frame)");
            }
        }
    }
    Ok(())
}
