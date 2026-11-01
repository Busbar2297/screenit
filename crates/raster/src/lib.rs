//! RGBA8 raster surface. Pixels are straight (non-premultiplied) alpha, row-major.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use screenit_geom::Rect;

/// Largest surface edge accepted (guards allocations sized by hostile input).
pub const MAX_EDGE: u32 = 65_536;
/// Largest surface pixel count accepted (about 1 GiB of RGBA).
pub const MAX_PIXELS: u64 = 268_435_456;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RasterError {
    #[error("invalid raster size {0}x{1}")]
    BadSize(u32, u32),
    #[error("pixel buffer length {got} does not match {w}x{h}")]
    BadLength { got: usize, w: u32, h: u32 },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Raster {
    w: u32,
    h: u32,
    data: Vec<u8>,
}

fn check(w: u32, h: u32) -> Result<usize, RasterError> {
    if w > MAX_EDGE || h > MAX_EDGE || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(RasterError::BadSize(w, h));
    }
    Ok(w as usize * h as usize * 4)
}

impl Raster {
    pub fn new(w: u32, h: u32, fill: [u8; 4]) -> Result<Self, RasterError> {
        let len = check(w, h)?;
        let mut data = Vec::with_capacity(len);
        for _ in 0..(len / 4) {
            data.extend_from_slice(&fill);
        }
        Ok(Self { w, h, data })
    }

    pub fn from_rgba(w: u32, h: u32, data: Vec<u8>) -> Result<Self, RasterError> {
        let len = check(w, h)?;
        if data.len() != len {
            return Err(RasterError::BadLength {
                got: data.len(),
                w,
                h,
            });
        }
        Ok(Self { w, h, data })
    }

    /// Build from BGRA with a row stride in bytes (as delivered by GDI/DXGI). Alpha is forced opaque
    /// when `opaque` is set (screen grabs often carry garbage alpha).
    pub fn from_bgra(
        w: u32,
        h: u32,
        stride: usize,
        src: &[u8],
        opaque: bool,
    ) -> Result<Self, RasterError> {
        let len = check(w, h)?;
        let row = w as usize * 4;
        if stride < row
            || src.len()
                < stride
                    .saturating_mul(h as usize)
                    .saturating_sub(stride - row)
        {
            return Err(RasterError::BadLength {
                got: src.len(),
                w,
                h,
            });
        }
        let mut data = Vec::with_capacity(len);
        for y in 0..h as usize {
            let Some(line) = src.get(y * stride..y * stride + row) else {
                return Err(RasterError::BadLength {
                    got: src.len(),
                    w,
                    h,
                });
            };
            for px in line.chunks_exact(4) {
                data.extend_from_slice(&[px[2], px[1], px[0], if opaque { 255 } else { px[3] }]);
            }
        }
        Ok(Self { w, h, data })
    }

    pub fn width(&self) -> u32 {
        self.w
    }
    pub fn height(&self) -> u32 {
        self.h
    }
    pub fn rect(&self) -> Rect {
        Rect::new(0, 0, self.w, self.h)
    }
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
    pub fn data(&self) -> &[u8] {
        &self.data
    }
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    fn idx(&self, x: i64, y: i64) -> Option<usize> {
        if x < 0 || y < 0 || x >= i64::from(self.w) || y >= i64::from(self.h) {
            return None;
        }
        Some((y as usize * self.w as usize + x as usize) * 4)
    }

    pub fn get(&self, x: i32, y: i32) -> Option<[u8; 4]> {
        let i = self.idx(i64::from(x), i64::from(y))?;
        let s = self.data.get(i..i + 4)?;
        Some([s[0], s[1], s[2], s[3]])
    }

    /// Write a pixel; out-of-bounds writes are ignored.
    pub fn put(&mut self, x: i32, y: i32, px: [u8; 4]) {
        if let Some(i) = self.idx(i64::from(x), i64::from(y)) {
            if let Some(d) = self.data.get_mut(i..i + 4) {
                d.copy_from_slice(&px);
            }
        }
    }

    /// Source-over blend of a straight-alpha pixel with extra coverage 0..=255.
    pub fn blend(&mut self, x: i32, y: i32, src: [u8; 4], coverage: u8) {
        let Some(i) = self.idx(i64::from(x), i64::from(y)) else {
            return;
        };
        let Some(d) = self.data.get_mut(i..i + 4) else {
            return;
        };
        let sa = u32::from(src[3]) * u32::from(coverage) / 255;
        if sa == 0 {
            return;
        }
        let da = u32::from(d[3]);
        let oa = sa + da * (255 - sa) / 255;
        if oa == 0 {
            return;
        }
        for c in 0..3 {
            let s = u32::from(src[c]) * sa;
            let dd = u32::from(d[c]) * da * (255 - sa) / 255;
            d[c] = ((s + dd) / oa).min(255) as u8;
        }
        d[3] = oa.min(255) as u8;
    }

    pub fn fill_rect(&mut self, r: Rect, px: [u8; 4]) {
        let Some(r) = r.intersect(&self.rect()) else {
            return;
        };
        for y in r.y..r.y + r.h as i32 {
            for x in r.x..r.x + r.w as i32 {
                self.put(x, y, px);
            }
        }
    }

    /// Copy of the sub-rectangle, clipped to the surface. `None` when nothing remains.
    pub fn crop(&self, r: Rect) -> Option<Raster> {
        let r = r.intersect(&self.rect())?;
        let mut data = Vec::with_capacity(r.w as usize * r.h as usize * 4);
        for y in r.y..r.y + r.h as i32 {
            let i = self.idx(i64::from(r.x), i64::from(y))?;
            data.extend_from_slice(self.data.get(i..i + r.w as usize * 4)?);
        }
        Some(Raster {
            w: r.w,
            h: r.h,
            data,
        })
    }

    /// Composite `src` onto self at (x, y) with source-over.
    pub fn draw(&mut self, src: &Raster, x: i32, y: i32) {
        for sy in 0..src.h as i32 {
            for sx in 0..src.w as i32 {
                if let Some(p) = src.get(sx, sy) {
                    self.blend(x.saturating_add(sx), y.saturating_add(sy), p, 255);
                }
            }
        }
    }

    /// Copy `src` over self at (x, y) without blending.
    pub fn copy_from(&mut self, src: &Raster, x: i32, y: i32) {
        for sy in 0..src.h as i32 {
            for sx in 0..src.w as i32 {
                if let Some(p) = src.get(sx, sy) {
                    self.put(x.saturating_add(sx), y.saturating_add(sy), p);
                }
            }
        }
    }

    /// Mean absolute per-channel difference (0..255), `None` when sizes differ.
    pub fn mean_abs_diff(&self, o: &Raster) -> Option<f64> {
        if self.w != o.w || self.h != o.h || self.data.is_empty() {
            return if self.w == o.w && self.h == o.h {
                Some(0.0)
            } else {
                None
            };
        }
        let sum: u64 = self
            .data
            .iter()
            .zip(&o.data)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        Some(sum as f64 / self.data.len() as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_huge_and_mismatched() {
        assert!(Raster::new(u32::MAX, 1, [0; 4]).is_err());
        assert!(Raster::new(60_000, 60_000, [0; 4]).is_err());
        assert!(Raster::from_rgba(2, 2, vec![0; 3]).is_err());
    }

    #[test]
    fn put_get_and_oob() {
        let mut r = Raster::new(2, 2, [0, 0, 0, 255]).unwrap();
        r.put(1, 1, [1, 2, 3, 4]);
        r.put(-5, 9, [9; 4]);
        assert_eq!(r.get(1, 1), Some([1, 2, 3, 4]));
        assert_eq!(r.get(2, 0), None);
    }

    #[test]
    fn bgra_converts_with_stride() {
        let src = [10u8, 20, 30, 0, 99, 99, 40, 50, 60, 0, 99, 99];
        let r = Raster::from_bgra(1, 2, 6, &src, true).unwrap();
        assert_eq!(r.get(0, 0), Some([30, 20, 10, 255]));
        assert_eq!(r.get(0, 1), Some([60, 50, 40, 255]));
        assert!(Raster::from_bgra(1, 2, 2, &src, true).is_err());
    }

    #[test]
    fn crop_clips() {
        let mut r = Raster::new(4, 4, [0, 0, 0, 255]).unwrap();
        r.put(3, 3, [7, 7, 7, 255]);
        let c = r.crop(Rect::new(2, 2, 10, 10)).unwrap();
        assert_eq!((c.width(), c.height()), (2, 2));
        assert_eq!(c.get(1, 1), Some([7, 7, 7, 255]));
        assert!(r.crop(Rect::new(10, 10, 2, 2)).is_none());
    }

    #[test]
    fn blend_half_alpha() {
        let mut r = Raster::new(1, 1, [0, 0, 0, 255]).unwrap();
        r.blend(0, 0, [255, 255, 255, 255], 128);
        let p = r.get(0, 0).unwrap();
        assert!(p[0] > 120 && p[0] < 136);
    }
}
