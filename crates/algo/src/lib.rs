//! Imaging algorithms on RGBA8 rasters: blur, pixelate, filters, flood fill, resize, borders, shadow.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod fx;
pub mod redact;
use screenit_geom::Rect;
use screenit_raster::{Raster, RasterError};

fn clip(r: &Raster, rect: Rect) -> Option<Rect> {
    rect.intersect(&r.rect())
}

/// Separable box blur of `rect` (clamped at the rectangle edge), `radius` in pixels (capped at 200).
pub fn box_blur(r: &mut Raster, rect: Rect, radius: u32) {
    let Some(rc) = clip(r, rect) else { return };
    let radius = radius.min(200) as i64;
    if radius == 0 {
        return;
    }
    let (w, h) = (rc.w as usize, rc.h as usize);
    let mut buf: Vec<[u8; 4]> = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            buf.push(r.get(rc.x + x as i32, rc.y + y as i32).unwrap_or([0; 4]));
        }
    }
    let pass = |src: &[[u8; 4]], horizontal: bool| -> Vec<[u8; 4]> {
        let mut out = vec![[0u8; 4]; src.len()];
        let (len, lines) = if horizontal { (w, h) } else { (h, w) };
        for line in 0..lines {
            let at = |i: usize| {
                if horizontal {
                    line * w + i
                } else {
                    i * w + line
                }
            };
            for i in 0..len {
                let lo = (i as i64 - radius).max(0) as usize;
                let hi = ((i as i64 + radius) as usize).min(len - 1);
                let n = (hi - lo + 1) as u32;
                let mut s = [0u32; 4];
                for j in lo..=hi {
                    if let Some(p) = src.get(at(j)) {
                        for c in 0..4 {
                            s[c] += u32::from(p[c]);
                        }
                    }
                }
                if let Some(o) = out.get_mut(at(i)) {
                    *o = [
                        (s[0] / n) as u8,
                        (s[1] / n) as u8,
                        (s[2] / n) as u8,
                        (s[3] / n) as u8,
                    ];
                }
            }
        }
        out
    };
    let a = pass(&buf, true);
    let b = pass(&a, false);
    for y in 0..h {
        for x in 0..w {
            if let Some(p) = b.get(y * w + x) {
                r.put(rc.x + x as i32, rc.y + y as i32, *p);
            }
        }
    }
}

/// Replace each `block`x`block` cell inside `rect` with its average colour.
pub fn pixelate(r: &mut Raster, rect: Rect, block: u32) {
    let Some(rc) = clip(r, rect) else { return };
    let block = block.clamp(1, 512) as i32;
    let mut y = rc.y;
    while y < rc.y + rc.h as i32 {
        let mut x = rc.x;
        while x < rc.x + rc.w as i32 {
            let bw = block.min(rc.x + rc.w as i32 - x);
            let bh = block.min(rc.y + rc.h as i32 - y);
            let mut s = [0u64; 4];
            for yy in y..y + bh {
                for xx in x..x + bw {
                    if let Some(p) = r.get(xx, yy) {
                        for c in 0..4 {
                            s[c] += u64::from(p[c]);
                        }
                    }
                }
            }
            let n = (bw * bh) as u64;
            let avg = [
                (s[0] / n) as u8,
                (s[1] / n) as u8,
                (s[2] / n) as u8,
                (s[3] / n) as u8,
            ];
            for yy in y..y + bh {
                for xx in x..x + bw {
                    r.put(xx, yy, avg);
                }
            }
            x += block;
        }
        y += block;
    }
}

fn map_pixels(r: &mut Raster, rect: Rect, f: impl Fn([u8; 4]) -> [u8; 4]) {
    let Some(rc) = clip(r, rect) else { return };
    for y in rc.y..rc.y + rc.h as i32 {
        for x in rc.x..rc.x + rc.w as i32 {
            if let Some(p) = r.get(x, y) {
                r.put(x, y, f(p));
            }
        }
    }
}

fn luma(p: [u8; 4]) -> u8 {
    ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000) as u8
}

pub fn grayscale(r: &mut Raster, rect: Rect) {
    map_pixels(r, rect, |p| {
        let l = luma(p);
        [l, l, l, p[3]]
    });
}

pub fn invert(r: &mut Raster, rect: Rect) {
    map_pixels(r, rect, |p| [255 - p[0], 255 - p[1], 255 - p[2], p[3]]);
}

pub fn sepia(r: &mut Raster, rect: Rect) {
    map_pixels(r, rect, |p| {
        let (rr, gg, bb) = (f32::from(p[0]), f32::from(p[1]), f32::from(p[2]));
        let c = |v: f32| v.clamp(0.0, 255.0) as u8;
        [
            c(0.393 * rr + 0.769 * gg + 0.189 * bb),
            c(0.349 * rr + 0.686 * gg + 0.168 * bb),
            c(0.272 * rr + 0.534 * gg + 0.131 * bb),
            p[3],
        ]
    });
}

/// Multiply alpha by `opacity` (0..=1).
pub fn set_opacity(r: &mut Raster, rect: Rect, opacity: f32) {
    let o = if opacity.is_finite() {
        opacity.clamp(0.0, 1.0)
    } else {
        1.0
    };
    map_pixels(r, rect, |p| [p[0], p[1], p[2], (f32::from(p[3]) * o) as u8]);
}

/// Brightness (-1..=1) and contrast (-1..=1) adjustment.
pub fn adjust(r: &mut Raster, rect: Rect, brightness: f32, contrast: f32) {
    let b = if brightness.is_finite() {
        brightness.clamp(-1.0, 1.0)
    } else {
        0.0
    } * 255.0;
    let c = if contrast.is_finite() {
        contrast.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let k = (1.0 + c) / (1.0 - c.min(0.999));
    map_pixels(r, rect, |p| {
        let f = |v: u8| ((f32::from(v) - 128.0) * k + 128.0 + b).clamp(0.0, 255.0) as u8;
        [f(p[0]), f(p[1]), f(p[2]), p[3]]
    });
}

/// Unsharp-style sharpen: original + amount * (original - blurred).
pub fn sharpen(r: &mut Raster, rect: Rect, amount: f32) {
    let Some(rc) = clip(r, rect) else { return };
    let amount = if amount.is_finite() {
        amount.clamp(0.0, 5.0)
    } else {
        0.0
    };
    let mut blurred = r.clone();
    box_blur(&mut blurred, rc, 1);
    for y in rc.y..rc.y + rc.h as i32 {
        for x in rc.x..rc.x + rc.w as i32 {
            if let (Some(o), Some(b)) = (r.get(x, y), blurred.get(x, y)) {
                let f = |i: usize| {
                    (f32::from(o[i]) + amount * (f32::from(o[i]) - f32::from(b[i])))
                        .clamp(0.0, 255.0) as u8
                };
                r.put(x, y, [f(0), f(1), f(2), o[3]]);
            }
        }
    }
}

/// Replace every pixel close to `from` (per-channel tolerance) with `to`.
pub fn replace_color(r: &mut Raster, rect: Rect, from: [u8; 4], to: [u8; 4], tolerance: u8) {
    map_pixels(r, rect, |p| {
        if (0..3).all(|i| p[i].abs_diff(from[i]) <= tolerance) {
            [to[0], to[1], to[2], p[3].max(to[3])]
        } else {
            p
        }
    });
}

/// Contiguous region selection (Magic Wand): returns a w*h mask of selected pixels.
pub fn flood_select(r: &Raster, x: i32, y: i32, tolerance: u8) -> Vec<bool> {
    let (w, h) = (r.width() as usize, r.height() as usize);
    let mut mask = vec![false; w * h];
    let Some(seed) = r.get(x, y) else { return mask };
    let near = |p: [u8; 4]| (0..4).all(|i| p[i].abs_diff(seed[i]) <= tolerance);
    let mut stack = vec![(x as usize, y as usize)];
    while let Some((cx, cy)) = stack.pop() {
        let i = cy * w + cx;
        if mask.get(i).copied().unwrap_or(true) {
            continue;
        }
        let Some(p) = r.get(cx as i32, cy as i32) else {
            continue;
        };
        if !near(p) {
            continue;
        }
        if let Some(m) = mask.get_mut(i) {
            *m = true;
        }
        if cx > 0 {
            stack.push((cx - 1, cy));
        }
        if cy > 0 {
            stack.push((cx, cy - 1));
        }
        if cx + 1 < w {
            stack.push((cx + 1, cy));
        }
        if cy + 1 < h {
            stack.push((cx, cy + 1));
        }
    }
    mask
}

/// Paint bucket: fill the contiguous region at (x, y). Returns pixels changed.
pub fn flood_fill(r: &mut Raster, x: i32, y: i32, tolerance: u8, color: [u8; 4]) -> usize {
    let mask = flood_select(r, x, y, tolerance);
    let w = r.width() as usize;
    let mut n = 0;
    for (i, m) in mask.iter().enumerate() {
        if *m {
            r.put((i % w) as i32, (i / w) as i32, color);
            n += 1;
        }
    }
    n
}

/// Bilinear resize.
pub fn resize(src: &Raster, w: u32, h: u32) -> Result<Raster, RasterError> {
    let mut out = Raster::new(w, h, [0; 4])?;
    if src.is_empty() || w == 0 || h == 0 {
        return Ok(out);
    }
    let (sw, sh) = (src.width() as f32, src.height() as f32);
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * sh / h as f32 - 0.5).clamp(0.0, sh - 1.0);
        let y0 = fy.floor() as i32;
        let ty = fy - y0 as f32;
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * sw / w as f32 - 0.5).clamp(0.0, sw - 1.0);
            let x0 = fx.floor() as i32;
            let tx = fx - x0 as f32;
            let g = |xx: i32, yy: i32| {
                src.get(
                    xx.min(src.width() as i32 - 1),
                    yy.min(src.height() as i32 - 1),
                )
                .unwrap_or([0; 4])
            };
            let (a, b, c, d) = (g(x0, y0), g(x0 + 1, y0), g(x0, y0 + 1), g(x0 + 1, y0 + 1));
            let mut px = [0u8; 4];
            for i in 0..4 {
                let top = f32::from(a[i]) * (1.0 - tx) + f32::from(b[i]) * tx;
                let bot = f32::from(c[i]) * (1.0 - tx) + f32::from(d[i]) * tx;
                px[i] = (top * (1.0 - ty) + bot * ty).round().clamp(0.0, 255.0) as u8;
            }
            out.put(x as i32, y as i32, px);
        }
    }
    Ok(out)
}

/// Add a solid border around the image (canvas grows by `width` on every side).
pub fn border(src: &Raster, width: u32, color: [u8; 4]) -> Result<Raster, RasterError> {
    let width = width.min(1000);
    let mut out = Raster::new(
        src.width().saturating_add(width * 2),
        src.height().saturating_add(width * 2),
        color,
    )?;
    out.copy_from(src, width as i32, width as i32);
    Ok(out)
}

/// Drop shadow behind the image; canvas grows to fit.
pub fn drop_shadow(
    src: &Raster,
    offset: i32,
    blur: u32,
    color: [u8; 4],
) -> Result<Raster, RasterError> {
    let pad = blur.min(100) as i32 + offset.unsigned_abs().min(200) as i32;
    let (w, h) = (src.width() + pad as u32 * 2, src.height() + pad as u32 * 2);
    let mut out = Raster::new(w, h, [0; 4])?;
    let mut sh = Raster::new(w, h, [0; 4])?;
    for y in 0..src.height() as i32 {
        for x in 0..src.width() as i32 {
            sh.put(
                x + pad + offset,
                y + pad + offset,
                [color[0], color[1], color[2], color[3]],
            );
        }
    }
    box_blur(&mut sh, Rect::new(0, 0, w, h), blur.min(100));
    out.draw(&sh, 0, 0);
    out.draw(src, pad, pad);
    Ok(out)
}

pub fn rotate90(src: &Raster) -> Result<Raster, RasterError> {
    let mut out = Raster::new(src.height(), src.width(), [0; 4])?;
    for y in 0..src.height() as i32 {
        for x in 0..src.width() as i32 {
            if let Some(p) = src.get(x, y) {
                out.put(src.height() as i32 - 1 - y, x, p);
            }
        }
    }
    Ok(out)
}

pub fn flip_horizontal(r: &mut Raster) {
    let w = r.width() as i32;
    for y in 0..r.height() as i32 {
        for x in 0..w / 2 {
            if let (Some(a), Some(b)) = (r.get(x, y), r.get(w - 1 - x, y)) {
                r.put(x, y, b);
                r.put(w - 1 - x, y, a);
            }
        }
    }
}

pub fn flip_vertical(r: &mut Raster) {
    let h = r.height() as i32;
    for y in 0..h / 2 {
        for x in 0..r.width() as i32 {
            if let (Some(a), Some(b)) = (r.get(x, y), r.get(x, h - 1 - y)) {
                r.put(x, y, b);
                r.put(x, h - 1 - y, a);
            }
        }
    }
}

/// Remove a vertical band (columns) or horizontal band (rows) and join the remainder (Cut Out).
pub fn cut_out(src: &Raster, vertical: bool, start: u32, len: u32) -> Result<Raster, RasterError> {
    let total = if vertical { src.width() } else { src.height() };
    let start = start.min(total);
    let len = len.min(total - start);
    if vertical {
        let mut out = Raster::new(src.width() - len, src.height(), [0; 4])?;
        for y in 0..src.height() as i32 {
            let mut ox = 0;
            for x in 0..src.width() {
                if x >= start && x < start + len {
                    continue;
                }
                if let Some(p) = src.get(x as i32, y) {
                    out.put(ox, y, p);
                }
                ox += 1;
            }
        }
        Ok(out)
    } else {
        let mut out = Raster::new(src.width(), src.height() - len, [0; 4])?;
        let mut oy = 0;
        for y in 0..src.height() {
            if y >= start && y < start + len {
                continue;
            }
            for x in 0..src.width() as i32 {
                if let Some(p) = src.get(x, y as i32) {
                    out.put(x, oy, p);
                }
            }
            oy += 1;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker() -> Raster {
        let mut r = Raster::new(8, 8, [0, 0, 0, 255]).unwrap();
        for y in 0..8 {
            for x in 0..8 {
                if (x + y) % 2 == 0 {
                    r.put(x, y, [255, 255, 255, 255]);
                }
            }
        }
        r
    }

    #[test]
    fn blur_averages_checker_to_gray() {
        let mut r = checker();
        let rc = r.rect();
        box_blur(&mut r, rc, 2);
        let p = r.get(4, 4).unwrap();
        assert!(p[0] > 90 && p[0] < 165, "{p:?}");
    }

    #[test]
    fn blur_out_of_range_is_noop() {
        let mut r = checker();
        let before = r.clone();
        box_blur(&mut r, Rect::new(100, 100, 5, 5), 3);
        let rc = r.rect();
        box_blur(&mut r, rc, 0);
        assert_eq!(r, before);
    }

    #[test]
    fn pixelate_makes_uniform_blocks() {
        let mut r = checker();
        let rc = r.rect();
        pixelate(&mut r, rc, 4);
        assert_eq!(r.get(0, 0), r.get(3, 3));
        assert_ne!(r.get(0, 0), r.get(7, 7).map(|_| [1, 2, 3, 4]));
    }

    #[test]
    fn filters_behave() {
        let mut r = Raster::new(2, 1, [200, 100, 50, 255]).unwrap();
        let rc = r.rect();
        grayscale(&mut r, rc);
        let p = r.get(0, 0).unwrap();
        assert!(p[0] == p[1] && p[1] == p[2]);
        let rc = r.rect();
        invert(&mut r, rc);
        assert_eq!(r.get(0, 0).unwrap()[0], 255 - p[0]);
        let rc = r.rect();
        set_opacity(&mut r, rc, f32::NAN);
        assert_eq!(r.get(0, 0).unwrap()[3], 255);
    }

    #[test]
    fn flood_fill_respects_boundaries() {
        let mut r = Raster::new(5, 5, [255; 4]).unwrap();
        for y in 0..5 {
            r.put(2, y, [0, 0, 0, 255]);
        }
        let n = flood_fill(&mut r, 0, 0, 0, [255, 0, 0, 255]);
        assert_eq!(n, 10);
        assert_eq!(r.get(4, 0), Some([255; 4]));
        assert_eq!(flood_fill(&mut r, -1, 0, 0, [1; 4]), 0);
    }

    #[test]
    fn resize_changes_dims_and_keeps_solid_colour() {
        let r = Raster::new(4, 4, [10, 20, 30, 255]).unwrap();
        let big = resize(&r, 9, 7).unwrap();
        assert_eq!((big.width(), big.height()), (9, 7));
        assert_eq!(big.get(4, 3), Some([10, 20, 30, 255]));
        assert!(resize(&r, 0, 0).unwrap().is_empty());
    }

    #[test]
    fn border_and_cutout_dimensions() {
        let r = Raster::new(10, 6, [1, 1, 1, 255]).unwrap();
        let b = border(&r, 3, [9, 9, 9, 255]).unwrap();
        assert_eq!((b.width(), b.height()), (16, 12));
        assert_eq!(b.get(0, 0), Some([9, 9, 9, 255]));
        let c = cut_out(&r, true, 2, 4).unwrap();
        assert_eq!((c.width(), c.height()), (6, 6));
        let c = cut_out(&r, false, 4, 100).unwrap();
        assert_eq!((c.width(), c.height()), (10, 4));
    }

    #[test]
    fn rotate_and_flip() {
        let mut r = Raster::new(3, 2, [0, 0, 0, 255]).unwrap();
        r.put(0, 0, [9, 0, 0, 255]);
        let rot = rotate90(&r).unwrap();
        assert_eq!((rot.width(), rot.height()), (2, 3));
        assert_eq!(rot.get(1, 0), Some([9, 0, 0, 255]));
        flip_horizontal(&mut r);
        assert_eq!(r.get(2, 0), Some([9, 0, 0, 255]));
        flip_vertical(&mut r);
        assert_eq!(r.get(2, 1), Some([9, 0, 0, 255]));
    }

    #[test]
    fn shadow_grows_canvas() {
        let r = Raster::new(10, 10, [255; 4]).unwrap();
        let s = drop_shadow(&r, 4, 3, [0, 0, 0, 200]).unwrap();
        assert!(s.width() > 10 && s.height() > 10);
    }
}
