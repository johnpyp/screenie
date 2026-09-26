//! Pixel effects on premultiplied RGBA pixmaps: the redaction tools and soft shadows.

use tiny_skia::{IntRect, Pixmap, PremultipliedColorU8};

/// Replace the pixmap's part of `rect` with blocks of `block` pixels, each the average of
/// what it covers. Blocks are aligned to `rect`'s top-left, which may lie outside the
/// pixmap, so they sit in the same place however the image is tiled, and don't shimmer
/// as the shape moves.
pub fn pixelate(pixmap: &mut Pixmap, rect: IntRect, block: u32) {
    let Some(area) = clip(pixmap, rect) else {
        return;
    };
    let block = block.max(2) as i32;
    // The first block that reaches into the pixmap.
    let first = |edge: i32, from: i32| edge + (from - edge) / block * block;
    let width = pixmap.width() as usize;
    let pixels = pixmap.pixels_mut();
    let mut by = first(rect.top(), area.top());
    while by < area.bottom() {
        let (y0, y1) = (by.max(area.top()), (by + block).min(area.bottom()));
        let mut bx = first(rect.left(), area.left());
        while bx < area.right() {
            let (x0, x1) = (
                bx.max(area.left()) as usize,
                (bx + block).min(area.right()) as usize,
            );
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                for p in &pixels[y as usize * width..][x0..x1] {
                    sum[0] += p.red() as u32;
                    sum[1] += p.green() as u32;
                    sum[2] += p.blue() as u32;
                    sum[3] += p.alpha() as u32;
                }
            }
            let n = (x1 - x0) as u32 * (y1 - y0) as u32;
            let avg = sum.map(|s| ((s + n / 2) / n) as u8);
            let color = PremultipliedColorU8::from_rgba(avg[0], avg[1], avg[2], avg[3])
                .unwrap_or(PremultipliedColorU8::TRANSPARENT);
            for y in y0..y1 {
                pixels[y as usize * width..][x0..x1].fill(color);
            }
            bx += block;
        }
        by += block;
    }
}

/// Blur `area` in place with an approximately Gaussian kernel of `radius`, reading pixels
/// around it (see [`blur_reach`]) so edges don't darken.
pub fn blur(pixmap: &mut Pixmap, area: IntRect, radius: f32) {
    let Some(area) = clip(pixmap, area) else {
        return;
    };
    let radius = blur_radius(radius);
    let r = gaussian_reach(radius) as i32;
    let around = IntRect::from_ltrb(
        area.left() - r,
        area.top() - r,
        area.right() + r,
        area.bottom() + r,
    );
    let Some(src) = around.and_then(|a| clip(pixmap, a)) else {
        return;
    };
    let (w, h) = (src.width() as usize, src.height() as usize);
    let width = pixmap.width() as usize;
    let mut buf: Vec<f32> = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &pixmap.pixels()[(src.top() as usize + y) * width + src.left() as usize..][..w];
        buf.extend(
            row.iter()
                .flat_map(|p| [p.red(), p.green(), p.blue(), p.alpha()].map(f32::from)),
        );
    }
    gaussian::<4>(&mut buf, w, h, box_radii(radius));
    let pixels = pixmap.pixels_mut();
    for y in area.top()..area.bottom() {
        let from = ((y - src.top()) as usize * w + (area.left() - src.left()) as usize) * 4;
        let row = &mut pixels[y as usize * width..][area.left() as usize..area.right() as usize];
        for (dst, v) in row.iter_mut().zip(buf[from..].as_chunks::<4>().0) {
            let [r, g, b, a] = v.map(|c| c.round().clamp(0.0, 255.0) as u8);
            *dst = PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a)
                .unwrap_or(PremultipliedColorU8::TRANSPARENT);
        }
    }
}

/// How far [`blur`] with `radius` reads around what it blurs.
pub fn blur_reach(radius: f32) -> i32 {
    gaussian_reach(blur_radius(radius)) as i32
}

fn blur_radius(radius: f32) -> f32 {
    radius.round().max(1.0)
}

/// How far [`draw_with_shadow`]'s shadow reaches beyond the layer's content.
pub fn shadow_reach(sigma: f32, dy: i32) -> i32 {
    gaussian_reach(sigma) as i32 + dy.abs()
}

/// Draw `layer` onto `target` with its top-left at `at`, over a soft black shadow of it:
/// its alpha blurred by `sigma`, `dy` pixels lower, at `opacity`. Only the blocks near the
/// layer's content are blurred and drawn, so a thin shape across a large layer (a long
/// diagonal arrow) costs about what a short one does.
pub fn draw_with_shadow(
    target: &mut Pixmap,
    layer: &Pixmap,
    at: (i32, i32),
    sigma: f32,
    dy: i32,
    opacity: f32,
) {
    const BLOCK: usize = 16;
    let (lw, lh) = (layer.width() as usize, layer.height() as usize);
    let (tw, th) = (target.width() as isize, target.height() as isize);
    let radii = box_radii(sigma);
    let reach = radii.iter().sum::<usize>();
    let (cols, rows) = (lw.div_ceil(BLOCK), lh.div_ceil(BLOCK));

    // The blocks with anything in them.
    let mut filled = vec![false; cols * rows];
    for (y, line) in layer.pixels().chunks_exact(lw).enumerate() {
        for (bx, chunk) in line.chunks(BLOCK).enumerate() {
            let cell = &mut filled[y / BLOCK * cols + bx];
            *cell = *cell || chunk.iter().any(|p| p.alpha() != 0);
        }
    }
    // The blocks the layer or its shadow draws on: those within reach of a filled one,
    // `dy` higher.
    let blocks = |from: isize, to: isize, n: usize| {
        let at = |v: isize| v.div_euclid(BLOCK as isize).clamp(0, n as isize - 1) as usize;
        at(from)..=at(to)
    };
    let active = |bx: usize, by: usize| {
        let (x, y) = ((bx * BLOCK) as isize, (by * BLOCK) as isize);
        let (r, b) = (reach as isize, BLOCK as isize - 1);
        filled[by * cols + bx]
            || blocks(y - dy as isize - r, y + b - dy as isize + r, rows)
                .any(|sy| blocks(x - r, x + b + r, cols).any(|sx| filled[sy * cols + sx]))
    };

    // Only what lands on the target.
    let (x_min, x_max) = (
        (-at.0 as isize).max(0) as usize,
        (tw - at.0 as isize).clamp(0, lw as isize) as usize,
    );
    let (y_min, y_max) = (
        (-at.1 as isize).max(0) as usize,
        (th - at.1 as isize).clamp(0, lh as isize) as usize,
    );
    let source = layer.pixels();
    let pixels = target.pixels_mut();
    let mut window = Vec::new();
    for by in 0..rows {
        let mut bx = 0;
        while bx < cols {
            if !active(bx, by) {
                bx += 1;
                continue;
            }
            let start = bx;
            while bx < cols && active(bx, by) {
                bx += 1;
            }
            // A run of active blocks: blur the alpha around it in one go.
            let (x0, x1) = ((start * BLOCK).max(x_min), (bx * BLOCK).min(x_max));
            let (y0, y1) = ((by * BLOCK).max(y_min), ((by + 1) * BLOCK).min(y_max));
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            let (wx, wy) = (
                x0 as isize - reach as isize,
                y0 as isize - dy as isize - reach as isize,
            );
            let (ww, wh) = (x1 - x0 + 2 * reach, y1 - y0 + 2 * reach);
            window.clear();
            window.resize(ww * wh, 0.0);
            for row in 0..wh {
                let ly = wy + row as isize;
                if !(0..lh as isize).contains(&ly) {
                    continue;
                }
                let (lx0, lx1) = (
                    wx.max(0) as usize,
                    (wx + ww as isize).min(lw as isize).max(0) as usize,
                );
                let line = &source[ly as usize * lw..][lx0..lx1.max(lx0)];
                let out = &mut window[row * ww + (lx0 as isize - wx) as usize..];
                for (o, p) in out.iter_mut().zip(line) {
                    *o = p.alpha() as f32;
                }
            }
            gaussian::<1>(&mut window, ww, wh, radii);
            for y in y0..y1 {
                let shadow = &window[(y - y0 + reach) * ww + reach..][..x1 - x0];
                let line = &source[y * lw..][x0..x1];
                let dst = &mut pixels[(y as isize + at.1 as isize) as usize * tw as usize..]
                    [(x0 as isize + at.0 as isize) as usize..][..x1 - x0];
                for ((d, s), p) in dst.iter_mut().zip(shadow).zip(line) {
                    // Casting saturates, so float drift below zero is none.
                    let a = (s * opacity + 0.5).min(255.0) as u8;
                    if a != 0 {
                        *d = over(
                            *d,
                            PremultipliedColorU8::from_rgba(0, 0, 0, a)
                                .expect("black is premultiplied"),
                        );
                    }
                    if p.alpha() != 0 {
                        *d = over(*d, *p);
                    }
                }
            }
        }
    }
}

/// Source-over of premultiplied `src` onto `dst`.
fn over(dst: PremultipliedColorU8, src: PremultipliedColorU8) -> PremultipliedColorU8 {
    let keep = 255 - src.alpha() as u32;
    let mix = |s: u8, d: u8| s + div255(d as u32 * keep) as u8;
    PremultipliedColorU8::from_rgba(
        mix(src.red(), dst.red()),
        mix(src.green(), dst.green()),
        mix(src.blue(), dst.blue()),
        mix(src.alpha(), dst.alpha()),
    )
    .unwrap_or(src)
}

/// `v / 255`, rounded.
fn div255(v: u32) -> u32 {
    let v = v + 128;
    (v + (v >> 8)) >> 8
}

/// The three box blur radii that approximate a Gaussian with standard deviation `sigma`
/// (none below half a pixel).
fn box_radii(sigma: f32) -> [usize; 3] {
    if sigma < 0.5 {
        return [0; 3];
    }
    // http://blog.ivank.net/fastest-gaussian-blur.html
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let mut lower = ideal.floor() as i32;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let upper = lower + 2;
    let m = ((12.0 * sigma * sigma - 3.0 * (lower * lower) as f32 - 12.0 * lower as f32 - 9.0)
        / (-4.0 * lower as f32 - 4.0))
        .round() as i32;
    let size = |i: i32| (if i < m { lower } else { upper }).max(1) as usize / 2;
    [size(0), size(1), size(2)]
}

/// How far a Gaussian blur of `sigma` reads: the box blurs' radii together.
fn gaussian_reach(sigma: f32) -> usize {
    box_radii(sigma).iter().sum()
}

/// Blur a `w`×`h` image of `C` interleaved channels in place with three box blurs each
/// way, edges clamped.
fn gaussian<const C: usize>(buf: &mut [f32], w: usize, h: usize, radii: [usize; 3]) {
    if w == 0 || h == 0 || radii == [0; 3] {
        return;
    }
    let mut tmp = vec![0f32; buf.len()];
    for r in radii.into_iter().filter(|r| *r > 0) {
        box_rows::<C>(buf, &mut tmp, w, r);
        box_columns(&tmp, buf, w * C, h, r);
    }
}

/// A box blur of radius `r` along each row of `src`, into `dst`.
fn box_rows<const C: usize>(src: &[f32], dst: &mut [f32], w: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f32;
    for (src, dst) in src.chunks_exact(w * C).zip(dst.chunks_exact_mut(w * C)) {
        let mut acc = [0f32; C];
        for i in -(r as isize)..=r as isize {
            let at = i.clamp(0, w as isize - 1) as usize * C;
            acc.iter_mut()
                .zip(&src[at..at + C])
                .for_each(|(a, v)| *a += v);
        }
        for x in 0..w {
            dst[x * C..x * C + C]
                .iter_mut()
                .zip(acc)
                .for_each(|(o, a)| *o = a * norm);
            let (add, sub) = ((x + r + 1).min(w - 1) * C, x.saturating_sub(r) * C);
            for c in 0..C {
                acc[c] += src[add + c] - src[sub + c];
            }
        }
    }
}

/// A box blur of radius `r` down each column of `src` (rows of `stride` values), into
/// `dst`. It goes a row at a time, keeping a running sum per column, so memory is read in
/// order.
fn box_columns(src: &[f32], dst: &mut [f32], stride: usize, h: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f32;
    let row = |y: isize| &src[y.clamp(0, h as isize - 1) as usize * stride..][..stride];
    let mut acc = vec![0f32; stride];
    for y in -(r as isize)..=r as isize {
        acc.iter_mut().zip(row(y)).for_each(|(a, v)| *a += v);
    }
    for (y, out) in dst.chunks_exact_mut(stride).enumerate() {
        out.iter_mut().zip(&acc).for_each(|(o, a)| *o = a * norm);
        let (add, sub) = (
            row(y as isize + r as isize + 1),
            row(y as isize - r as isize),
        );
        for ((a, p), m) in acc.iter_mut().zip(add).zip(sub) {
            *a += p - m;
        }
    }
}

fn clip(pixmap: &Pixmap, area: IntRect) -> Option<IntRect> {
    area.intersect(&IntRect::from_xywh(0, 0, pixmap.width(), pixmap.height())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Color;

    fn checker(w: u32, h: u32) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        for (i, px) in p.pixels_mut().iter_mut().enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            let v = if (x + y) % 2 == 0 { 255 } else { 0 };
            *px = PremultipliedColorU8::from_rgba(v, v, v, 255).unwrap();
        }
        p
    }

    #[test]
    fn pixelate_averages_blocks() {
        let mut p = checker(8, 8);
        pixelate(&mut p, IntRect::from_xywh(0, 0, 8, 8).unwrap(), 4);
        for px in p.pixels() {
            assert!((127..=128).contains(&px.red()), "{}", px.red());
            assert_eq!(px.alpha(), 255);
        }
    }

    #[test]
    fn pixelate_stays_inside_area() {
        let mut p = checker(8, 8);
        pixelate(&mut p, IntRect::from_xywh(4, 4, 4, 4).unwrap(), 4);
        assert_eq!(p.pixel(0, 0).unwrap().red(), 255);
        assert_eq!(p.pixel(1, 0).unwrap().red(), 0);
    }

    #[test]
    fn blur_smooths_and_keeps_opacity() {
        let mut p = checker(32, 32);
        blur(&mut p, IntRect::from_xywh(8, 8, 16, 16).unwrap(), 3.0);
        let mid = p.pixel(16, 16).unwrap();
        assert!((100..=155).contains(&mid.red()), "{}", mid.red());
        assert_eq!(mid.alpha(), 255);
        assert_eq!(p.pixel(0, 0).unwrap().red(), 255);
    }

    #[test]
    fn blur_of_flat_color_is_identity() {
        let mut p = Pixmap::new(20, 20).unwrap();
        p.fill(Color::from_rgba8(10, 200, 30, 255));
        blur(&mut p, IntRect::from_xywh(0, 0, 20, 20).unwrap(), 4.0);
        let px = p.pixel(10, 10).unwrap();
        assert_eq!(
            (px.red(), px.green(), px.blue(), px.alpha()),
            (10, 200, 30, 255)
        );
    }
}
