//! Time what the editor does per pointer move while a shape is dragged: draw it over the
//! canvas tiles it touches, in parallel (as `screenie-editor`'s raster does), and the
//! export.
//!
//! cargo run --release -p screenie-annotate --example bench -- [WIDTH HEIGHT SCALE]

use std::time::Instant;

use rayon::prelude::*;

use screenie_annotate::render::{draw_over, render_area};
use screenie_annotate::tiny_skia::IntRect;
use screenie_annotate::{Document, Kind, Style};
use screenie_core::{Image, PixelFormat, Point, Rect};

/// The editor's tile size.
const TILE: u32 = 256;

fn main() {
    let args: Vec<f64> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let (w, h, scale) = (
        args.first().copied().unwrap_or(3840.0),
        args.get(1).copied().unwrap_or(2160.0),
        args.get(2).copied().unwrap_or(1.0),
    );
    let base = Image::from_raw(
        w as u32,
        h as u32,
        w as usize * 4,
        PixelFormat::Rgba,
        vec![200; (w * h * 4.0) as usize],
    );
    let mut doc = Document::new(&base, scale as f32);
    let composite = render_area(
        &doc,
        IntRect::from_xywh(0, 0, w as u32, h as u32).unwrap(),
        |_| true,
    );
    let s = |f: f64| f * scale;
    let cases = [
        (
            "short arrow",
            Kind::Arrow {
                from: Point::new(w * 0.4, h * 0.4),
                to: Point::new(w * 0.5, h * 0.5),
            },
        ),
        (
            "long flat arrow",
            Kind::Arrow {
                from: Point::new(w * 0.05, h * 0.5),
                to: Point::new(w * 0.95, h * 0.52),
            },
        ),
        (
            "long diagonal arrow",
            Kind::Arrow {
                from: Point::new(w * 0.05, h * 0.05),
                to: Point::new(w * 0.95, h * 0.95),
            },
        ),
        (
            "big rectangle",
            Kind::Rectangle {
                rect: Rect::new(w * 0.05, h * 0.05, w * 0.9, h * 0.9),
            },
        ),
        (
            "big ellipse",
            Kind::Ellipse {
                rect: Rect::new(w * 0.05, h * 0.05, w * 0.9, h * 0.9),
            },
        ),
        (
            "long diagonal pen",
            Kind::Pen {
                points: (0..400)
                    .map(|i| {
                        Point::new(
                            w * (0.05 + 0.9 * i as f64 / 400.0),
                            h * (0.05 + 0.9 * i as f64 / 400.0) + s((i % 7) as f64),
                        )
                    })
                    .collect(),
            },
        ),
    ];
    let tiles: Vec<IntRect> = (0..h as u32)
        .step_by(TILE as usize - 2)
        .flat_map(|y| {
            (0..w as u32)
                .step_by(TILE as usize - 2)
                .map(move |x| (x, y))
        })
        .filter_map(|(x, y)| {
            IntRect::from_xywh(
                x as i32,
                y as i32,
                TILE.min(w as u32 - x),
                TILE.min(h as u32 - y),
            )
        })
        .collect();
    println!("{w}x{h} at scale {scale}, {TILE}px tiles:");
    for (name, kind) in cases {
        let shape = doc.make(kind, Style::default());
        doc.add(shape.clone());
        let runs = 5;
        let started = Instant::now();
        let mut touched = 0;
        for _ in 0..runs {
            let hit: Vec<&IntRect> = tiles
                .iter()
                .filter(|r| {
                    let area = Rect::new(
                        r.x() as f64,
                        r.y() as f64,
                        r.width() as f64,
                        r.height() as f64,
                    );
                    shape.touches(&area, doc.scale())
                })
                .collect();
            touched = hit.len();
            hit.par_iter().for_each(|rect| {
                let mut tile = composite.clone_rect(**rect).unwrap();
                draw_over(&doc, &[&shape], &mut tile, (rect.x(), rect.y()));
                std::hint::black_box(&tile);
            });
        }
        let frame = started.elapsed().as_secs_f64() * 1000.0 / runs as f64;
        let started = Instant::now();
        if std::env::var_os("NO_EXPORT").is_none() {
            std::hint::black_box(doc.export());
        }
        let export = started.elapsed().as_secs_f64() * 1000.0;
        println!(
            "{name:>20}: {frame:>5.1} ms per move ({touched:>3} of {} tiles), export {export:>5.1} ms",
            tiles.len()
        );
        doc.undo();
    }
}
