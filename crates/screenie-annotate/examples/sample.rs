//! Render a sample document with every kind of shape, for eyeballing the renderer.
//!
//! cargo run -p screenie-annotate --example sample -- [BACKGROUND.png] [OUT.png] [SCALE]

use screenie_annotate::{Color, Document, Kind, Redaction, Style};
use screenie_core::{Image, PixelFormat, Point, Rect};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scale: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let base = match args.first().filter(|a| a.as_str() != "-") {
        Some(path) => Image::load_png(path.as_ref()).expect("background png"),
        None => stripes((900.0 * scale) as u32, (560.0 * scale) as u32),
    };
    let out = args.get(1).cloned().unwrap_or_else(|| "target/annotate-sample.png".into());

    let mut doc = Document::new(&base, scale);
    let s = |v: f64| v * scale as f64;
    let p = |x: f64, y: f64| Point::new(s(x), s(y));
    let r = |x: f64, y: f64, w: f64, h: f64| Rect::new(s(x), s(y), s(w), s(h));
    let color = |hex: &str| hex.parse::<Color>().unwrap();
    let red = Style::default();
    let mut shapes = vec![
        (Kind::Spotlight { rect: r(40.0, 40.0, 380.0, 220.0) }, red),
        (Kind::Redact { rect: r(470.0, 40.0, 180.0, 90.0), mode: Redaction::Pixelate }, red),
        (Kind::Redact { rect: r(680.0, 40.0, 180.0, 90.0), mode: Redaction::Blur }, red),
        (Kind::Arrow { from: p(120.0, 330.0), to: p(300.0, 200.0) }, red),
        (Kind::Arrow { from: p(80.0, 520.0), to: p(160.0, 460.0) }, Style { size: 2.0, ..red }),
        (Kind::Arrow { from: p(420.0, 520.0), to: p(260.0, 420.0) }, Style { size: 8.0, color: color("#0a84ff"), ..red }),
        (Kind::Line { from: p(480.0, 180.0), to: p(860.0, 180.0) }, Style { color: color("#34c759"), ..red }),
        (Kind::Rectangle { rect: r(60.0, 60.0, 200.0, 120.0) }, red),
        (Kind::Rectangle { rect: r(480.0, 220.0, 150.0, 80.0) }, Style { fill: true, color: color("#af52de"), ..red }),
        (Kind::Ellipse { rect: r(660.0, 210.0, 200.0, 100.0) }, Style { color: color("#ff9500"), size: 6.0, ..red }),
        (Kind::Pen { points: (0..60).map(|i| p(480.0 + i as f64 * 6.0, 380.0 + (i as f64 / 5.0).sin() * 30.0)).collect() }, Style { color: color("#0a84ff"), ..red }),
        (Kind::Highlighter { points: vec![p(470.0, 450.0), p(860.0, 450.0)] }, Style { color: color("#ffcc00"), ..red }),
        (Kind::Text { origin: p(500.0, 434.0), text: "Highlighted text".into() }, Style { color: Color::BLACK, size: 2.0, ..red }),
        (Kind::Text { origin: p(480.0, 490.0), text: "Plain bold text".into() }, red),
        (Kind::Text { origin: p(210.0, 290.0), text: "Label ✓ 日本".into() }, Style { fill: true, ..red }),
        (Kind::Step { center: p(330.0, 90.0) }, red),
        (Kind::Step { center: p(380.0, 90.0) }, Style { color: color("#ffcc00"), ..red }),
        (Kind::Step { center: p(380.0, 150.0) }, Style { color: color("#0a84ff"), size: 8.0, ..red }),
    ];
    for (kind, style) in shapes.drain(..) {
        let shape = doc.make(kind, style);
        doc.add(shape);
    }
    let started = std::time::Instant::now();
    let image = doc.export();
    eprintln!("exported {}x{} in {:?}", image.width(), image.height(), started.elapsed());
    image.save_png(out.as_ref()).expect("write png");
    println!("{out}");
}

/// A light UI-ish background: bands of text-like bars.
fn stripes(w: u32, h: u32) -> Image {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let band = (y / 28) % 2 == 0;
            let bar = (y % 28) > 8 && (y % 28) < 18 && (x / 11) % 7 != 0;
            let v: u8 = if bar { 120 } else if band { 250 } else { 236 };
            let tint = ((x * 255) / w.max(1)) as u8;
            data.extend_from_slice(&[v, v.saturating_sub(tint / 12), v.saturating_sub(tint / 6), 255]);
        }
    }
    Image::from_raw(w, h, w as usize * 4, PixelFormat::Rgba, data)
}
