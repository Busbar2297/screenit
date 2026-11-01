//! Capture document: one base raster plus an ordered list of editable annotation objects.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use screenit_geom::{PointF, RectF};
use screenit_raster::Raster;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const MAX_OBJECTS: usize = 10_000;
pub const MAX_TEXT: usize = 10_000;
pub const MAX_PEN_POINTS: usize = 100_000;
const MAX_COORD: f32 = 1.0e6;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DocError {
    #[error("invalid object: {0}")]
    Invalid(String),
    #[error("no object with id {0}")]
    NoObject(u64),
    #[error("too many objects")]
    TooMany,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color(pub [u8; 4]);

impl Color {
    pub const RED: Color = Color([230, 30, 30, 255]);
    pub const BLACK: Color = Color([0, 0, 0, 255]);
    pub const WHITE: Color = Color([255, 255, 255, 255]);
    pub const YELLOW: Color = Color([255, 235, 0, 255]);
    pub const TRANSPARENT: Color = Color([0, 0, 0, 0]);

    /// Parse `#rrggbb` or `#rrggbbaa`.
    pub fn from_hex(s: &str) -> Option<Color> {
        let h = s.strip_prefix('#')?;
        if !h.is_ascii() || (h.len() != 6 && h.len() != 8) {
            return None;
        }
        let b = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
        Some(Color([
            b(0)?,
            b(2)?,
            b(4)?,
            if h.len() == 8 { b(6)? } else { 255 },
        ]))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub stroke: Color,
    pub fill: Option<Color>,
    pub width: f32,
    pub shadow: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            stroke: Color::RED,
            fill: None,
            width: 3.0,
            shadow: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ObjectKind {
    Arrow {
        from: PointF,
        to: PointF,
    },
    Line {
        from: PointF,
        to: PointF,
    },
    Rect {
        rect: RectF,
        radius: f32,
    },
    Ellipse {
        rect: RectF,
    },
    Text {
        pos: PointF,
        text: String,
        size: f32,
    },
    Callout {
        rect: RectF,
        text: String,
        tail: PointF,
        size: f32,
    },
    Step {
        center: PointF,
        number: u32,
        radius: f32,
    },
    Blur {
        rect: RectF,
        radius: u32,
        pixelate: bool,
    },
    Highlight {
        rect: RectF,
    },
    Pen {
        points: Vec<PointF>,
    },
    Stamp {
        pos: PointF,
        name: String,
        size: f32,
    },
    Magnify {
        src: RectF,
        dst: RectF,
        factor: f32,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: u64,
    pub kind: ObjectKind,
    pub style: Style,
}

fn ok_pt(p: &PointF) -> bool {
    p.is_finite() && p.x.abs() <= MAX_COORD && p.y.abs() <= MAX_COORD
}
fn ok_rect(r: &RectF) -> bool {
    r.is_finite() && [r.x, r.y, r.w, r.h].iter().all(|v| v.abs() <= MAX_COORD)
}

impl ObjectKind {
    pub fn validate(&self) -> Result<(), DocError> {
        let bad = |m: &str| Err(DocError::Invalid(m.to_string()));
        match self {
            ObjectKind::Arrow { from, to } | ObjectKind::Line { from, to } => {
                if !ok_pt(from) || !ok_pt(to) {
                    return bad("non-finite or out-of-range point");
                }
            }
            ObjectKind::Rect { rect, radius } => {
                if !ok_rect(rect) || !radius.is_finite() || *radius < 0.0 {
                    return bad("bad rect");
                }
            }
            ObjectKind::Ellipse { rect }
            | ObjectKind::Highlight { rect }
            | ObjectKind::Blur { rect, .. } => {
                if !ok_rect(rect) {
                    return bad("bad rect");
                }
            }
            ObjectKind::Text { pos, text, size }
            | ObjectKind::Stamp {
                pos,
                name: text,
                size,
            } => {
                if !ok_pt(pos) || !size.is_finite() || *size <= 0.0 || *size > 2000.0 {
                    return bad("bad text position or size");
                }
                if text.chars().count() > MAX_TEXT {
                    return bad("text too long");
                }
            }
            ObjectKind::Callout {
                rect,
                text,
                tail,
                size,
            } => {
                if !ok_rect(rect)
                    || !ok_pt(tail)
                    || !size.is_finite()
                    || *size <= 0.0
                    || *size > 2000.0
                {
                    return bad("bad callout");
                }
                if text.chars().count() > MAX_TEXT {
                    return bad("text too long");
                }
            }
            ObjectKind::Step { center, radius, .. } => {
                if !ok_pt(center) || !radius.is_finite() || *radius <= 0.0 || *radius > 2000.0 {
                    return bad("bad step");
                }
            }
            ObjectKind::Pen { points } => {
                if points.is_empty() || points.len() > MAX_PEN_POINTS || !points.iter().all(ok_pt) {
                    return bad("bad pen stroke");
                }
            }
            ObjectKind::Magnify { src, dst, factor } => {
                if !ok_rect(src)
                    || !ok_rect(dst)
                    || !factor.is_finite()
                    || *factor < 1.0
                    || *factor > 32.0
                {
                    return bad("bad magnify");
                }
            }
        }
        Ok(())
    }
}

impl Object {
    /// Axis-aligned bounds in image coordinates (stroke width included).
    pub fn bounds(&self) -> RectF {
        let pad = self.style.width.max(1.0) / 2.0 + 1.0;
        let line = |a: PointF, b: PointF| {
            RectF::new(
                a.x.min(b.x) - pad,
                a.y.min(b.y) - pad,
                (a.x - b.x).abs() + 2.0 * pad,
                (a.y - b.y).abs() + 2.0 * pad,
            )
        };
        match &self.kind {
            ObjectKind::Arrow { from, to } | ObjectKind::Line { from, to } => line(*from, *to),
            ObjectKind::Rect { rect, .. }
            | ObjectKind::Ellipse { rect }
            | ObjectKind::Highlight { rect }
            | ObjectKind::Blur { rect, .. } => rect.normalized(),
            ObjectKind::Callout { rect, tail, .. } => {
                let r = rect.normalized();
                let x0 = r.x.min(tail.x);
                let y0 = r.y.min(tail.y);
                let x1 = (r.x + r.w).max(tail.x);
                let y1 = (r.y + r.h).max(tail.y);
                RectF::new(x0, y0, x1 - x0, y1 - y0)
            }
            ObjectKind::Text { pos, text, size } => {
                let w = text.chars().count() as f32 * size * 0.6;
                RectF::new(pos.x, pos.y, w.max(*size), *size * 1.3)
            }
            ObjectKind::Stamp { pos, size, .. } => RectF::new(pos.x, pos.y, *size, *size),
            ObjectKind::Step { center, radius, .. } => RectF::new(
                center.x - radius,
                center.y - radius,
                radius * 2.0,
                radius * 2.0,
            ),
            ObjectKind::Pen { points } => {
                let mut r = RectF::default();
                if let Some(f) = points.first() {
                    let (mut x0, mut y0, mut x1, mut y1) = (f.x, f.y, f.x, f.y);
                    for p in points {
                        x0 = x0.min(p.x);
                        y0 = y0.min(p.y);
                        x1 = x1.max(p.x);
                        y1 = y1.max(p.y);
                    }
                    r = RectF::new(x0 - pad, y0 - pad, x1 - x0 + 2.0 * pad, y1 - y0 + 2.0 * pad);
                }
                r
            }
            ObjectKind::Magnify { dst, .. } => dst.normalized(),
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        let mv = |p: &mut PointF| {
            p.x += dx;
            p.y += dy;
        };
        let mr = |r: &mut RectF| {
            r.x += dx;
            r.y += dy;
        };
        match &mut self.kind {
            ObjectKind::Arrow { from, to } | ObjectKind::Line { from, to } => {
                mv(from);
                mv(to);
            }
            ObjectKind::Rect { rect, .. }
            | ObjectKind::Ellipse { rect }
            | ObjectKind::Highlight { rect }
            | ObjectKind::Blur { rect, .. } => mr(rect),
            ObjectKind::Callout { rect, tail, .. } => {
                mr(rect);
                mv(tail);
            }
            ObjectKind::Text { pos, .. } | ObjectKind::Stamp { pos, .. } => mv(pos),
            ObjectKind::Step { center, .. } => mv(center),
            ObjectKind::Pen { points } => points.iter_mut().for_each(mv),
            ObjectKind::Magnify { dst, .. } => mr(dst),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    pub app: String,
    pub title: String,
    pub url: Option<String>,
    pub captured_at: String,
    pub monitor: u32,
    pub dpi: u32,
    pub source: String,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CaptureDoc {
    #[serde(skip)]
    base: Arc<Raster>,
    pub objects: Vec<Object>,
    pub metadata: Metadata,
    pub name: String,
    next_id: u64,
}

impl CaptureDoc {
    pub fn new(base: Raster) -> Self {
        Self {
            base: Arc::new(base),
            next_id: 1,
            ..Default::default()
        }
    }

    pub fn base(&self) -> &Raster {
        &self.base
    }
    pub fn base_arc(&self) -> Arc<Raster> {
        Arc::clone(&self.base)
    }
    pub fn set_base(&mut self, r: Raster) {
        self.base = Arc::new(r);
    }
    /// Restore the base after deserialising a manifest.
    pub fn with_base(mut self, r: Raster) -> Self {
        self.base = Arc::new(r);
        self.next_id = self
            .next_id
            .max(self.objects.iter().map(|o| o.id + 1).max().unwrap_or(1));
        self
    }

    pub fn add(&mut self, kind: ObjectKind, style: Style) -> Result<u64, DocError> {
        kind.validate()?;
        if !style.width.is_finite() || !(0.0..=500.0).contains(&style.width) {
            return Err(DocError::Invalid("bad stroke width".into()));
        }
        if self.objects.len() >= MAX_OBJECTS {
            return Err(DocError::TooMany);
        }
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        self.objects.push(Object { id, kind, style });
        Ok(id)
    }

    pub fn get(&self, id: u64) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn remove(&mut self, id: u64) -> Result<Object, DocError> {
        let i = self
            .objects
            .iter()
            .position(|o| o.id == id)
            .ok_or(DocError::NoObject(id))?;
        Ok(self.objects.remove(i))
    }

    pub fn translate(&mut self, id: u64, dx: f32, dy: f32) -> Result<(), DocError> {
        if !dx.is_finite() || !dy.is_finite() || dx.abs() > MAX_COORD || dy.abs() > MAX_COORD {
            return Err(DocError::Invalid("bad offset".into()));
        }
        let o = self
            .objects
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or(DocError::NoObject(id))?;
        o.translate(dx, dy);
        Ok(())
    }

    /// Replace an object's kind (validated), keeping its id and style.
    pub fn replace_kind(&mut self, id: u64, kind: ObjectKind) -> Result<(), DocError> {
        kind.validate()?;
        let o = self
            .objects
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or(DocError::NoObject(id))?;
        o.kind = kind;
        Ok(())
    }

    pub fn set_style(&mut self, id: u64, style: Style) -> Result<(), DocError> {
        if !style.width.is_finite() || !(0.0..=500.0).contains(&style.width) {
            return Err(DocError::Invalid("bad stroke width".into()));
        }
        let o = self
            .objects
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or(DocError::NoObject(id))?;
        o.style = style;
        Ok(())
    }

    /// Next number for the Step tool (highest existing + 1).
    pub fn next_step_number(&self) -> u32 {
        self.objects
            .iter()
            .filter_map(|o| match o.kind {
                ObjectKind::Step { number, .. } => Some(number),
                _ => None,
            })
            .max()
            .map_or(1, |n| n.saturating_add(1))
    }

    /// Topmost object under the point.
    pub fn hit_test(&self, p: PointF) -> Option<u64> {
        self.objects
            .iter()
            .rev()
            .find(|o| o.bounds().contains(p))
            .map(|o| o.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> CaptureDoc {
        CaptureDoc::new(Raster::new(100, 100, [255; 4]).unwrap())
    }

    #[test]
    fn add_validates() {
        let mut d = doc();
        let bad = ObjectKind::Arrow {
            from: PointF::new(f32::NAN, 0.0),
            to: PointF::default(),
        };
        assert!(d.add(bad, Style::default()).is_err());
        let id = d
            .add(
                ObjectKind::Line {
                    from: PointF::new(0.0, 0.0),
                    to: PointF::new(5.0, 5.0),
                },
                Style::default(),
            )
            .unwrap();
        assert_eq!(id, 1);
        assert!(
            d.add(ObjectKind::Pen { points: vec![] }, Style::default())
                .is_err()
        );
    }

    #[test]
    fn step_numbers_increment() {
        let mut d = doc();
        for _ in 0..3 {
            let n = d.next_step_number();
            d.add(
                ObjectKind::Step {
                    center: PointF::new(10.0, 10.0),
                    number: n,
                    radius: 12.0,
                },
                Style::default(),
            )
            .unwrap();
        }
        assert_eq!(d.next_step_number(), 4);
    }

    #[test]
    fn translate_and_hit_test() {
        let mut d = doc();
        let id = d
            .add(
                ObjectKind::Rect {
                    rect: RectF::new(10.0, 10.0, 20.0, 20.0),
                    radius: 0.0,
                },
                Style::default(),
            )
            .unwrap();
        assert_eq!(d.hit_test(PointF::new(15.0, 15.0)), Some(id));
        d.translate(id, 50.0, 0.0).unwrap();
        assert_eq!(d.hit_test(PointF::new(15.0, 15.0)), None);
        assert_eq!(d.hit_test(PointF::new(65.0, 15.0)), Some(id));
        assert!(d.translate(99, 1.0, 1.0).is_err());
        assert!(d.translate(id, f32::INFINITY, 0.0).is_err());
    }

    #[test]
    fn color_hex() {
        assert_eq!(Color::from_hex("#ff0080"), Some(Color([255, 0, 128, 255])));
        assert_eq!(Color::from_hex("ff0080"), None);
        assert_eq!(Color::from_hex("#é"), None);
        assert_eq!(Color::from_hex("#ggggggg"), None);
    }

    #[test]
    fn serde_manifest_skips_base() {
        let mut d = doc();
        d.add(
            ObjectKind::Highlight {
                rect: RectF::new(1.0, 1.0, 5.0, 5.0),
            },
            Style::default(),
        )
        .unwrap();
        let json = serde_json::to_string(&d).unwrap();
        let back: CaptureDoc = serde_json::from_str(&json).unwrap();
        let back = back.with_base(Raster::new(100, 100, [255; 4]).unwrap());
        assert_eq!(back.objects, d.objects);
    }
}
