//! Capture effects: edges, reflection, spotlight, watermark, fade, perspective, page curl,
//! canvas size, trim and combine. Pure functions over `Raster` (straight RGBA).

use screenit_geom::Rect;
use screenit_raster::{Raster, RasterError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeStyle {
    Wave,
    Saw,
    Torn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

fn hash(n: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B1) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65_535.0
}

/// Edge displacement (0..=depth) at position `t` along an edge.
fn edge_offset(style: EdgeStyle, t: u32, depth: u32) -> f32 {
    let d = depth as f32;
    let period = (depth.max(1) * 4) as f32;
    match style {
        EdgeStyle::Wave => d * (0.5 + 0.5 * (t as f32 / period * std::f32::consts::TAU).sin()),
        EdgeStyle::Saw => {
            let ph = (t as f32 / period).fract();
            d * if ph < 0.5 { ph * 2.0 } else { 2.0 - ph * 2.0 }
        }
        EdgeStyle::Torn => {
            // Value noise: interpolate hashed lattice points every 3 px.
            let (i, f) = (t / 3, (t % 3) as f32 / 3.0);
            d * (hash(i) * (1.0 - f) + hash(i + 1) * f)
        }
    }
}

/// Make the chosen edges non-straight by clearing pixels to transparent.
pub fn edges(src: &Raster, style: EdgeStyle, depth: u32, sides: &[Side]) -> Raster {
    let mut out = src.clone();
    let (w, h) = (src.width(), src.height());
    let depth = depth.min(w.min(h) / 2);
    for y in 0..h {
        for x in 0..w {
            let mut clear = false;
            for s in sides {
                let (dist, t) = match s {
                    Side::Top => (y, x),
                    Side::Bottom => (h - 1 - y, x + 7),
                    Side::Left => (x, y + 13),
                    Side::Right => (w - 1 - x, y + 29),
                };
                if (dist as f32) < edge_offset(style, t, depth) {
                    clear = true;
                }
            }
            if clear {
                out.put(x as i32, y as i32, [0, 0, 0, 0]);
            }
        }
    }
    out
}

/// Mirror reflection below the image, fading from `start_alpha` to 0 over `height_frac` of the height.
pub fn reflection(
    src: &Raster,
    height_frac: f32,
    gap: u32,
    start_alpha: f32,
) -> Result<Raster, RasterError> {
    let (w, h) = (src.width(), src.height());
    let rh = ((h as f32 * height_frac.clamp(0.05, 1.0)) as u32).max(1);
    let mut out = Raster::new(w, h + gap + rh, [0, 0, 0, 0])?;
    out.copy_from(src, 0, 0);
    for ry in 0..rh {
        let a = start_alpha.clamp(0.0, 1.0) * (1.0 - ry as f32 / rh as f32);
        for x in 0..w {
            if let Some(mut px) = src.get(x as i32, (h - 1 - ry) as i32) {
                px[3] = (f32::from(px[3]) * a) as u8;
                out.put(x as i32, (h + gap + ry) as i32, px);
            }
        }
    }
    Ok(out)
}

/// Darken everything outside `rect` by `dim` (0..=1).
pub fn spotlight(r: &mut Raster, rect: Rect, dim: f32) {
    let k = 1.0 - dim.clamp(0.0, 1.0);
    let (w, h) = (r.width() as i32, r.height() as i32);
    for y in 0..h {
        for x in 0..w {
            let inside = x >= rect.x
                && y >= rect.y
                && x < rect.x + rect.w as i32
                && y < rect.y + rect.h as i32;
            if !inside {
                if let Some(mut px) = r.get(x, y) {
                    for c in &mut px[..3] {
                        *c = (f32::from(*c) * k) as u8;
                    }
                    r.put(x, y, px);
                }
            }
        }
    }
}

/// Fade the pixels near `side` to transparent over `len` pixels.
pub fn fade(r: &mut Raster, side: Side, len: u32) {
    let (w, h) = (r.width(), r.height());
    let len = len.max(1);
    for y in 0..h {
        for x in 0..w {
            let dist = match side {
                Side::Top => y,
                Side::Bottom => h - 1 - y,
                Side::Left => x,
                Side::Right => w - 1 - x,
            };
            if dist < len {
                if let Some(mut px) = r.get(x as i32, y as i32) {
                    px[3] = (f32::from(px[3]) * dist as f32 / len as f32) as u8;
                    r.put(x as i32, y as i32, px);
                }
            }
        }
    }
}

/// Alpha-blend `mark` into `r` at a corner with `margin` and `opacity`.
pub fn watermark(
    r: &mut Raster,
    mark: &Raster,
    right: bool,
    bottom: bool,
    margin: i32,
    opacity: f32,
) {
    let x0 = if right {
        r.width() as i32 - mark.width() as i32 - margin
    } else {
        margin
    };
    let y0 = if bottom {
        r.height() as i32 - mark.height() as i32 - margin
    } else {
        margin
    };
    let o = opacity.clamp(0.0, 1.0);
    for y in 0..mark.height() as i32 {
        for x in 0..mark.width() as i32 {
            if let Some(px) = mark.get(x, y) {
                r.blend(x0 + x, y0 + y, px, (f32::from(px[3]) * o) as u8);
            }
        }
    }
}

/// Perspective: horizontally converge the top/bottom rows by the given fractions (0..0.9) of the width.
pub fn perspective(src: &Raster, top_inset: f32, bottom_inset: f32) -> Result<Raster, RasterError> {
    let (w, h) = (src.width(), src.height());
    let mut out = Raster::new(w, h, [0, 0, 0, 0])?;
    for y in 0..h {
        let t = y as f32 / (h.max(2) - 1) as f32;
        let inset = (top_inset * (1.0 - t) + bottom_inset * t).clamp(0.0, 0.9) * w as f32 / 2.0;
        let span = w as f32 - 2.0 * inset;
        for x in 0..w {
            let fx = x as f32 - inset;
            if fx < 0.0 || fx >= span {
                continue;
            }
            let sx = (fx / span * w as f32) as i32;
            if let Some(px) = src.get(sx, y as i32) {
                out.put(x as i32, y as i32, px);
            }
        }
    }
    Ok(out)
}

/// Peel the bottom-right corner: the triangle beyond the fold is cleared and the area near the
/// fold gets a shadow gradient.
pub fn page_curl(src: &Raster, size: u32) -> Raster {
    let mut out = src.clone();
    let (w, h) = (src.width() as i32, src.height() as i32);
    let s = (size as i32).min(w.min(h)).max(1);
    for y in 0..h {
        for x in 0..w {
            let u = (x - (w - s)) + (y - (h - s));
            if u > s {
                out.put(x, y, [0, 0, 0, 0]);
            } else if u > s / 2 {
                if let Some(mut px) = src.get(x, y) {
                    let k = 1.0 - 0.5 * (u - s / 2) as f32 / (s - s / 2).max(1) as f32;
                    for c in &mut px[..3] {
                        *c = (f32::from(*c) * k) as u8;
                    }
                    out.put(x, y, px);
                }
            }
        }
    }
    out
}

/// New canvas `w`x`h` with the image placed by anchor fractions (0..=1) and `fill` background.
pub fn canvas(
    src: &Raster,
    w: u32,
    h: u32,
    ax: f32,
    ay: f32,
    fill: [u8; 4],
) -> Result<Raster, RasterError> {
    let mut out = Raster::new(w, h, fill)?;
    let x = ((w as f32 - src.width() as f32) * ax.clamp(0.0, 1.0)) as i32;
    let y = ((h as f32 - src.height() as f32) * ay.clamp(0.0, 1.0)) as i32;
    out.draw(src, x, y);
    Ok(out)
}

/// Bounding box of pixels that differ from the top-left pixel by more than `tolerance`.
pub fn trim_rect(src: &Raster, tolerance: u8) -> Option<Rect> {
    let bg = src.get(0, 0)?;
    let (w, h) = (src.width() as i32, src.height() as i32);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, -1, -1);
    for y in 0..h {
        for x in 0..w {
            let px = src.get(x, y)?;
            if px
                .iter()
                .zip(bg.iter())
                .any(|(a, b)| a.abs_diff(*b) > tolerance)
            {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x1 >= 0).then(|| Rect::new(x0, y0, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32))
}

/// Stack images vertically or horizontally with `spacing`, centred on the cross axis.
pub fn combine(
    images: &[Raster],
    vertical: bool,
    spacing: u32,
    fill: [u8; 4],
) -> Result<Raster, RasterError> {
    let gaps = spacing * images.len().saturating_sub(1) as u32;
    let (w, h) = if vertical {
        (
            images.iter().map(Raster::width).max().unwrap_or(1),
            images.iter().map(Raster::height).sum::<u32>() + gaps,
        )
    } else {
        (
            images.iter().map(Raster::width).sum::<u32>() + gaps,
            images.iter().map(Raster::height).max().unwrap_or(1),
        )
    };
    let mut out = Raster::new(w, h, fill)?;
    let mut pos = 0i32;
    for im in images {
        if vertical {
            out.draw(im, (w as i32 - im.width() as i32) / 2, pos);
            pos += (im.height() + spacing) as i32;
        } else {
            out.draw(im, pos, (h as i32 - im.height() as i32) / 2);
            pos += (im.width() + spacing) as i32;
        }
    }
    Ok(out)
}

/// Classical background removal: clear every pixel connected to a corner whose colour is within
/// `tolerance` of that corner. Returns the number of pixels cleared.
pub fn remove_background(r: &mut Raster, tolerance: u8) -> usize {
    let (w, h) = (r.width() as i32, r.height() as i32);
    let mut mask = vec![false; (w * h) as usize];
    for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
        for (m, s) in mask.iter_mut().zip(crate::flood_select(r, x, y, tolerance)) {
            *m |= s;
        }
    }
    let mut n = 0;
    for (i, m) in mask.iter().enumerate() {
        if *m {
            r.put(i as i32 % w, i as i32 / w, [0, 0, 0, 0]);
            n += 1;
        }
    }
    n
}
#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, px: [u8; 4]) -> Raster {
        Raster::new(w, h, px).unwrap()
    }

    #[test]
    fn edges_clear_only_near_chosen_sides() {
        let r = solid(100, 60, [9, 9, 9, 255]);
        let o = edges(&r, EdgeStyle::Saw, 10, &[Side::Bottom]);
        assert_eq!(o.get(50, 0), Some([9, 9, 9, 255]));
        let cleared = (0..100).filter(|x| o.get(*x, 59).unwrap()[3] == 0).count();
        assert!(
            cleared > 20,
            "bottom edge should be serrated, cleared {cleared}"
        );
        assert_eq!(o.get(50, 40), Some([9, 9, 9, 255]));
    }

    #[test]
    fn reflection_adds_faded_mirror() {
        let mut r = solid(10, 10, [0, 0, 0, 255]);
        r.fill_rect(Rect::new(0, 9, 10, 1), [255, 0, 0, 255]);
        let o = reflection(&r, 0.5, 2, 0.6).unwrap();
        assert_eq!(o.height(), 10 + 2 + 5);
        let top = o.get(0, 12).unwrap();
        assert_eq!(top[0], 255, "mirror starts with the last source row");
        assert!(top[3] > 100 && top[3] <= 153);
        assert!(o.get(0, 16).unwrap()[3] < top[3]);
        assert_eq!(o.get(0, 10).unwrap()[3], 0, "gap is transparent");
    }

    #[test]
    fn spotlight_dims_outside_only() {
        let mut r = solid(20, 20, [200, 200, 200, 255]);
        spotlight(&mut r, Rect::new(5, 5, 10, 10), 0.5);
        assert_eq!(r.get(10, 10), Some([200, 200, 200, 255]));
        assert_eq!(r.get(0, 0), Some([100, 100, 100, 255]));
    }

    #[test]
    fn fade_ramps_alpha() {
        let mut r = solid(10, 10, [1, 2, 3, 255]);
        fade(&mut r, Side::Left, 5);
        assert_eq!(r.get(0, 3).unwrap()[3], 0);
        assert!(r.get(2, 3).unwrap()[3] > 90);
        assert_eq!(r.get(7, 3).unwrap()[3], 255);
    }

    #[test]
    fn watermark_lands_in_corner_with_opacity() {
        let mut r = solid(20, 20, [0, 0, 0, 255]);
        watermark(
            &mut r,
            &solid(4, 4, [255, 255, 255, 255]),
            true,
            true,
            2,
            0.5,
        );
        let px = r.get(15, 15).unwrap();
        assert!((120..=135).contains(&px[0]), "{px:?}");
        assert_eq!(r.get(0, 0), Some([0, 0, 0, 255]));
    }

    #[test]
    fn perspective_narrows_top() {
        let r = solid(100, 50, [5, 5, 5, 255]);
        let o = perspective(&r, 0.4, 0.0).unwrap();
        assert_eq!(o.get(2, 0).unwrap()[3], 0);
        assert_eq!(o.get(50, 0).unwrap()[3], 255);
        assert_eq!(o.get(2, 49).unwrap()[3], 255);
    }

    #[test]
    fn page_curl_clears_corner() {
        let r = solid(50, 50, [100, 100, 100, 255]);
        let o = page_curl(&r, 20);
        assert_eq!(o.get(49, 49).unwrap()[3], 0);
        assert_eq!(o.get(10, 10), Some([100, 100, 100, 255]));
    }

    #[test]
    fn background_removal_keeps_the_subject() {
        let mut r = solid(20, 20, [250, 250, 250, 255]);
        r.fill_rect(Rect::new(5, 5, 10, 10), [200, 0, 0, 255]);
        r.fill_rect(Rect::new(8, 8, 2, 2), [250, 250, 250, 255]);
        let n = remove_background(&mut r, 10);
        assert_eq!(n, 400 - 100);
        assert_eq!(r.get(0, 0).unwrap()[3], 0);
        assert_eq!(r.get(10, 10).unwrap()[3], 255);
        assert_eq!(
            r.get(8, 8).unwrap()[3],
            255,
            "enclosed hole is not connected to a corner"
        );
    }

    #[test]
    fn canvas_trim_and_combine() {
        let r = solid(10, 10, [255, 0, 0, 255]);
        let c = canvas(&r, 30, 20, 0.5, 0.5, [0, 0, 0, 255]).unwrap();
        assert_eq!(c.get(15, 10), Some([255, 0, 0, 255]));
        assert_eq!(c.get(0, 0), Some([0, 0, 0, 255]));
        assert_eq!(trim_rect(&c, 0), Some(Rect::new(10, 5, 10, 10)));
        assert_eq!(trim_rect(&solid(5, 5, [1, 1, 1, 255]), 0), None);
        let m = combine(&[r.clone(), solid(4, 6, [0, 255, 0, 255])], true, 3, [0; 4]).unwrap();
        assert_eq!((m.width(), m.height()), (10, 19));
        assert_eq!(m.get(5, 14), Some([0, 255, 0, 255]));
    }
}
