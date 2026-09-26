//! Pixel effects on premultiplied RGBA pixmaps: the redaction tools and soft shadows.

use tiny_skia::{IntRect, Pixmap, PremultipliedColorU8};

/// Replace `area` with blocks of `block` pixels, each the average of what it covers.
/// Blocks are aligned to `area`'s top-left so a shape's blocks don't shimmer as it moves.
pub fn pixelate(pixmap: &mut Pixmap, area: IntRect, block: u32) {
    let Some(area) = clip(pixmap, area) else { return };
    let block = block.max(2);
    let width = pixmap.width() as usize;
    let pixels = pixmap.pixels_mut();
    let mut by = area.top();
    while by < area.bottom() {
        let bh = (block as i32).min(area.bottom() - by);
        let mut bx = area.left();
        while bx < area.right() {
            let bw = (block as i32).min(area.right() - bx);
            let mut sum = [0u32; 4];
            for y in by..by + bh {
                for p in &pixels[y as usize * width + bx as usize..][..bw as usize] {
                    sum[0] += p.red() as u32;
                    sum[1] += p.green() as u32;
                    sum[2] += p.blue() as u32;
                    sum[3] += p.alpha() as u32;
                }
            }
            let n = (bw * bh) as u32;
            let avg = sum.map(|s| ((s + n / 2) / n) as u8);
            let color = PremultipliedColorU8::from_rgba(avg[0], avg[1], avg[2], avg[3]).unwrap_or(PremultipliedColorU8::TRANSPARENT);
            for y in by..by + bh {
                pixels[y as usize * width + bx as usize..][..bw as usize].fill(color);
            }
            bx += bw;
        }
        by += bh;
    }
}

/// Blur `area` in place with an approximately Gaussian kernel of `radius`, reading pixels
/// just outside it so edges don't darken.
pub fn blur(pixmap: &mut Pixmap, area: IntRect, radius: f32) {
    let Some(area) = clip(pixmap, area) else { return };
    let r = radius.round().max(1.0) as i32;
    // Read a margin around the area so the blur takes in the surroundings.
    let Some(src) = clip(pixmap, IntRect::from_ltrb(area.left() - r * 3, area.top() - r * 3, area.right() + r * 3, area.bottom() + r * 3).unwrap_or(area)) else {
        return;
    };
    let (w, h) = (src.width() as usize, src.height() as usize);
    let width = pixmap.width() as usize;
    let mut buf: Vec<[f32; 4]> = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = &pixmap.pixels()[(src.top() as usize + y) * width + src.left() as usize..][..w];
        buf.extend(row.iter().map(|p| [p.red() as f32, p.green() as f32, p.blue() as f32, p.alpha() as f32]));
    }
    for radius in box_radii(r as f32) {
        box_blur(&mut buf, w, h, radius);
    }
    let pixels = pixmap.pixels_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let v = buf[(y - src.top()) as usize * w + (x - src.left()) as usize];
            let [r, g, b, a] = v.map(|c| c.round().clamp(0.0, 255.0) as u8);
            pixels[y as usize * width + x as usize] =
                PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a).unwrap_or(PremultipliedColorU8::TRANSPARENT);
        }
    }
}

/// Blur an alpha-only mask (for shadows) in place.
pub fn blur_mask(mask: &mut [f32], width: usize, height: usize, radius: f32) {
    if radius < 0.5 {
        return;
    }
    let mut buf: Vec<[f32; 4]> = mask.iter().map(|&a| [a, 0.0, 0.0, 0.0]).collect();
    for r in box_radii(radius) {
        box_blur(&mut buf, width, height, r);
    }
    for (m, v) in mask.iter_mut().zip(buf) {
        *m = v[0];
    }
}

/// Three box blurs approximate a Gaussian with standard deviation `sigma`.
fn box_radii(sigma: f32) -> [usize; 3] {
    // http://blog.ivank.net/fastest-gaussian-blur.html
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let mut lower = ideal.floor() as i32;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let upper = lower + 2;
    let m = ((12.0 * sigma * sigma - 3.0 * (lower * lower) as f32 - 12.0 * lower as f32 - 9.0) / (-4.0 * lower as f32 - 4.0)).round() as i32;
    let size = |i: i32| (if i < m { lower } else { upper }).max(1) as usize / 2;
    [size(0), size(1), size(2)]
}

/// A horizontal then vertical running-sum box blur with clamped edges.
fn box_blur(buf: &mut [[f32; 4]], w: usize, h: usize, r: usize) {
    if r == 0 || w == 0 || h == 0 {
        return;
    }
    let mut line = vec![[0f32; 4]; w.max(h)];
    let norm = 1.0 / (2 * r + 1) as f32;
    let mut pass = |len: usize, count: usize, at: &dyn Fn(usize, usize) -> usize, buf: &mut [[f32; 4]]| {
        for n in 0..count {
            let get = |i: isize| buf[at(n, i.clamp(0, len as isize - 1) as usize)];
            let mut acc = [0f32; 4];
            for i in -(r as isize)..=(r as isize) {
                add(&mut acc, get(i), 1.0);
            }
            for (i, out) in line.iter_mut().take(len).enumerate() {
                *out = acc.map(|c| c * norm);
                add(&mut acc, get(i as isize + r as isize + 1), 1.0);
                add(&mut acc, get(i as isize - r as isize), -1.0);
            }
            for (i, v) in line.iter().take(len).enumerate() {
                buf[at(n, i)] = *v;
            }
        }
    };
    pass(w, h, &|row, i| row * w + i, buf);
    pass(h, w, &|col, i| i * w + col, buf);
}

fn add(acc: &mut [f32; 4], v: [f32; 4], sign: f32) {
    for (a, v) in acc.iter_mut().zip(v) {
        *a += v * sign;
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
        assert_eq!((px.red(), px.green(), px.blue(), px.alpha()), (10, 200, 30, 255));
    }
}
