//! Text layout and rasterization for text shapes and step numbers, via cosmic-text.
//!
//! Annotation text is set in bundled Inter Bold so it looks the same everywhere. System
//! fonts are loaded too, for glyphs Inter lacks (emoji, CJK…). Loading them takes a
//! moment, so call [`warm_up`] early.

use std::sync::{LazyLock, Mutex, MutexGuard};

use cosmic_text::{
    Attrs, Buffer, Cursor, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, fontdb,
};
use tiny_skia::{Pixmap, PremultipliedColorU8};

use crate::Color;

const FAMILY: &str = "Inter";
const LINE_HEIGHT: f32 = 1.25;

struct Fonts {
    system: FontSystem,
    swash: SwashCache,
}

static FONTS: LazyLock<Mutex<Fonts>> = LazyLock::new(|| {
    let started = std::time::Instant::now();
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    db.load_font_data(include_bytes!("../../../assets/fonts/Inter-Bold.otf").to_vec());
    db.set_sans_serif_family(FAMILY);
    let locale = std::env::var("LANG")
        .ok()
        .and_then(|l| l.split('.').next().map(|l| l.replace('_', "-")));
    let system = FontSystem::new_with_locale_and_db(locale.unwrap_or_else(|| "en-US".into()), db);
    tracing::debug!(elapsed = ?started.elapsed(), "fonts loaded");
    Mutex::new(Fonts {
        system,
        swash: SwashCache::new(),
    })
});

fn fonts() -> MutexGuard<'static, Fonts> {
    FONTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Load fonts on a background thread so the first text shape doesn't stall.
pub fn warm_up() {
    std::thread::spawn(|| drop(fonts()));
}

/// A laid-out block of text, possibly several lines. Positions are relative to its
/// top-left corner.
pub struct TextBlock {
    buffer: Buffer,
    text: String,
    width: f32,
    height: f32,
}

impl TextBlock {
    pub fn new(text: &str, font_size: f32) -> Self {
        let mut guard = fonts();
        let fonts = &mut *guard;
        let metrics = Metrics::relative(font_size.max(1.0), LINE_HEIGHT);
        let mut buffer = Buffer::new(&mut fonts.system, metrics);
        let attrs = Attrs::new()
            .family(Family::Name(FAMILY))
            .weight(Weight::BOLD);
        {
            let mut b = buffer.borrow_with(&mut fonts.system);
            b.set_size(None, None);
            b.set_text(text, &attrs, Shaping::Advanced, None);
            b.shape_until_scroll(false);
        }
        let mut width: f32 = 0.0;
        let mut height: f32 = 0.0;
        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
            height = height.max(run.line_top + run.line_height);
        }
        let height = height.max(metrics.line_height);
        Self {
            buffer,
            text: text.to_string(),
            width,
            height,
        }
    }

    pub fn width(&self) -> f32 {
        self.width
    }

    pub fn height(&self) -> f32 {
        self.height
    }

    /// The first line's baseline, from the top.
    pub fn baseline(&self) -> f32 {
        self.buffer
            .layout_runs()
            .next()
            .map_or(self.line_height() * 0.8, |run| run.line_y)
    }

    pub fn line_height(&self) -> f32 {
        self.buffer.metrics().line_height
    }

    /// Where a caret before byte `index` goes: its x, and the top of its line.
    pub fn caret(&self, index: usize) -> (f32, f32) {
        let cursor = self.cursor_for(index);
        self.buffer
            .cursor_position(&cursor)
            .unwrap_or((0.0, cursor.line as f32 * self.line_height()))
    }

    /// The byte index nearest to a point.
    pub fn hit(&self, x: f32, y: f32) -> usize {
        let cursor = self
            .buffer
            .hit(x, y.clamp(0.0, self.height - 0.5))
            .unwrap_or(Cursor::new(0, 0));
        self.index_of(cursor)
    }

    fn cursor_for(&self, index: usize) -> Cursor {
        let index = index.min(self.text.len());
        let before = &self.text[..index];
        let line = before.matches('\n').count();
        let start = before.rfind('\n').map_or(0, |i| i + 1);
        Cursor::new(line, index - start)
    }

    fn index_of(&self, cursor: Cursor) -> usize {
        let start: usize = self
            .text
            .split('\n')
            .take(cursor.line)
            .map(|l| l.len() + 1)
            .sum();
        (start + cursor.index).min(self.text.len())
    }

    /// Draw the text with its top-left at `(x, y)` in pixmap coordinates.
    pub fn draw(&self, pixmap: &mut Pixmap, x: f32, y: f32, color: Color) {
        let mut guard = fonts();
        let fonts = &mut *guard;
        let (width, height) = (pixmap.width() as i32, pixmap.height() as i32);
        let pixels = pixmap.pixels_mut();
        let base = cosmic_text::Color::rgba(color.r, color.g, color.b, color.a);
        for run in self.buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical = glyph.physical((x, y + run.line_y), 1.0);
                let glyph_color = glyph.color_opt.unwrap_or(base);
                fonts.swash.with_pixels(
                    &mut fonts.system,
                    physical.cache_key,
                    glyph_color,
                    |gx, gy, c| {
                        let (px, py) = (physical.x + gx, physical.y + gy);
                        if px < 0 || py < 0 || px >= width || py >= height || c.a() == 0 {
                            return;
                        }
                        let dst = &mut pixels[(py * width + px) as usize];
                        *dst = blend(*dst, c.r(), c.g(), c.b(), c.a());
                    },
                );
            }
        }
    }
}

/// Source-over of a straight-alpha colour onto a premultiplied pixel.
fn blend(dst: PremultipliedColorU8, r: u8, g: u8, b: u8, a: u8) -> PremultipliedColorU8 {
    let a32 = a as u32;
    let inv = 255 - a32;
    let mix = |s: u8, d: u8| ((s as u32 * a32 + d as u32 * inv + 127) / 255) as u8;
    let alpha = (a32 + (dst.alpha() as u32 * inv + 127) / 255) as u8;
    let (r, g, b) = (mix(r, dst.red()), mix(g, dst.green()), mix(b, dst.blue()));
    PremultipliedColorU8::from_rgba(r.min(alpha), g.min(alpha), b.min(alpha), alpha).unwrap_or(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_grows_with_text() {
        let short = TextBlock::new("Hi", 20.0);
        let long = TextBlock::new("Hello there", 20.0);
        assert!(short.width() > 5.0);
        assert!(long.width() > short.width());
        let two = TextBlock::new("Hello\nthere", 20.0);
        assert!((two.height() - 2.0 * two.line_height()).abs() < 1.0);
        let empty = TextBlock::new("", 20.0);
        assert_eq!(empty.width(), 0.0);
        assert!(empty.height() > 20.0);
    }

    #[test]
    fn carets_and_hits_round_trip() {
        let block = TextBlock::new("ab\ncd", 20.0);
        let (x0, y0) = block.caret(0);
        let (x2, _) = block.caret(2);
        let (x3, y3) = block.caret(3);
        assert_eq!((x0, y0), (0.0, 0.0));
        assert!(x2 > 10.0);
        assert_eq!(x3, 0.0);
        assert!(y3 > 20.0);
        assert_eq!(block.hit(x2 + 5.0, 2.0), 2);
        assert_eq!(block.hit(0.0, y3 + 2.0), 3);
    }

    #[test]
    fn draws_ink() {
        let block = TextBlock::new("W", 30.0);
        let mut p = Pixmap::new(40, 50).unwrap();
        block.draw(&mut p, 2.0, 2.0, Color::rgb(255, 0, 0));
        let inked = p.pixels().iter().filter(|p| p.alpha() > 128).count();
        assert!(inked > 30, "{inked}");
        assert!(p.pixels().iter().all(|p| p.green() == 0));
    }
}
