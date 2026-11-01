//! Geometry primitives, multi-monitor virtual-screen math.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PointF {
    pub x: f32,
    pub y: f32,
}

impl PointF {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub fn dist(self, o: PointF) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
    /// True when both coordinates are finite numbers.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

/// Integer rectangle; `w`/`h` are extents, so the right edge is exclusive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// Normalized rectangle spanning two corner points (any order), inclusive of both.
    pub fn from_points(a: Point, b: Point) -> Self {
        let x0 = a.x.min(b.x);
        let y0 = a.y.min(b.y);
        let w = (i64::from(a.x) - i64::from(b.x))
            .unsigned_abs()
            .min(u64::from(u32::MAX - 1));
        let h = (i64::from(a.y) - i64::from(b.y))
            .unsigned_abs()
            .min(u64::from(u32::MAX - 1));
        Self {
            x: x0,
            y: y0,
            w: w as u32 + 1,
            h: h as u32 + 1,
        }
    }

    pub fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.w)
    }
    pub fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.h)
    }
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
    pub fn area(&self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }

    pub fn contains(&self, p: Point) -> bool {
        i64::from(p.x) >= i64::from(self.x)
            && i64::from(p.y) >= i64::from(self.y)
            && i64::from(p.x) < self.right()
            && i64::from(p.y) < self.bottom()
    }

    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x0 = i64::from(self.x).max(i64::from(o.x));
        let y0 = i64::from(self.y).max(i64::from(o.y));
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Rect {
            x: i32::try_from(x0).ok()?,
            y: i32::try_from(y0).ok()?,
            w: u32::try_from(x1 - x0).ok()?,
            h: u32::try_from(y1 - y0).ok()?,
        })
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x0 = i64::from(self.x).min(i64::from(o.x));
        let y0 = i64::from(self.y).min(i64::from(o.y));
        let x1 = self.right().max(o.right());
        let y1 = self.bottom().max(o.bottom());
        Rect {
            x: x0 as i32,
            y: y0 as i32,
            w: (x1 - x0).clamp(0, i64::from(u32::MAX)) as u32,
            h: (y1 - y0).clamp(0, i64::from(u32::MAX)) as u32,
        }
    }

    pub fn translate(&self, dx: i32, dy: i32) -> Rect {
        Rect {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
            ..*self
        }
    }
}

/// Floating-point rectangle used by annotation objects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn normalized(self) -> Self {
        let (x, w) = if self.w < 0.0 {
            (self.x + self.w, -self.w)
        } else {
            (self.x, self.w)
        };
        let (y, h) = if self.h < 0.0 {
            (self.y + self.h, -self.h)
        } else {
            (self.y, self.h)
        };
        Self { x, y, w, h }
    }
    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.w.is_finite() && self.h.is_finite()
    }
    pub fn contains(&self, p: PointF) -> bool {
        let r = self.normalized();
        p.x >= r.x && p.y >= r.y && p.x <= r.x + r.w && p.y <= r.y + r.h
    }
    /// Integer bounds, rounded outwards.
    pub fn to_rect(&self) -> Rect {
        let r = self.normalized();
        let x0 = r.x.floor().clamp(-1.0e9, 1.0e9);
        let y0 = r.y.floor().clamp(-1.0e9, 1.0e9);
        let x1 = (r.x + r.w).ceil().clamp(-1.0e9, 1.0e9);
        let y1 = (r.y + r.h).ceil().clamp(-1.0e9, 1.0e9);
        Rect {
            x: x0 as i32,
            y: y0 as i32,
            w: (x1 - x0).max(0.0) as u32,
            h: (y1 - y0).max(0.0) as u32,
        }
    }
}

impl From<Rect> for RectF {
    fn from(r: Rect) -> Self {
        RectF {
            x: r.x as f32,
            y: r.y as f32,
            w: r.w as f32,
            h: r.h as f32,
        }
    }
}

/// Bounding rectangle of all monitors (the virtual screen).
pub fn virtual_screen(monitors: &[Rect]) -> Rect {
    monitors.iter().fold(Rect::default(), |acc, r| acc.union(r))
}

/// Scale a logical length by a DPI value (96 = 100%).
pub fn scale_dpi(v: i32, dpi: u32) -> i32 {
    let s = i64::from(v) * i64::from(dpi) / 96;
    s.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_points_is_normalized_and_inclusive() {
        let r = Rect::from_points(Point::new(10, 20), Point::new(5, 25));
        assert_eq!(r, Rect::new(5, 20, 6, 6));
    }

    #[test]
    fn intersect_and_union() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(&b), Some(Rect::new(5, 5, 5, 5)));
        assert_eq!(a.union(&b), Rect::new(0, 0, 15, 15));
        assert_eq!(a.intersect(&Rect::new(20, 20, 3, 3)), None);
    }

    #[test]
    fn virtual_screen_spans_negative_monitors() {
        let v = virtual_screen(&[Rect::new(-1920, 0, 1920, 1080), Rect::new(0, 0, 2560, 1440)]);
        assert_eq!(v, Rect::new(-1920, 0, 4480, 1440));
    }

    #[test]
    fn extreme_values_do_not_panic() {
        let a = Rect::new(i32::MAX, i32::MAX, u32::MAX, u32::MAX);
        let _ = a.intersect(&a);
        let _ = a.union(&a);
        let _ = a.translate(i32::MAX, i32::MAX);
        let r = RectF::new(f32::NAN, f32::INFINITY, -1.0, 1.0e30).to_rect();
        let _ = r;
    }
}
