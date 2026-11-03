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
    /// A text box: `pos` is its top-left corner. `width`/`height` are the box size; 0 means
    /// "not set" (the old unboxed text, sized from its characters).
    Text {
        pos: PointF,
        text: String,
        size: f32,
        #[serde(default)]
        width: f32,
        #[serde(default)]
        height: f32,
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
    /// Clockwise rotation in degrees about the centre of the object's box. Only box-like kinds
    /// (rect, ellipse, text, callout, stamp, step) use it; line-like kinds rotate their points.
    #[serde(default)]
    pub rotation: f32,
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
            ObjectKind::Text {
                pos,
                text,
                size,
                width,
                height,
            } => {
                if !ok_pt(pos) || !size.is_finite() || *size <= 0.0 || *size > 2000.0 {
                    return bad("bad text position or size");
                }
                if ![width, height]
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=MAX_COORD).contains(*v))
                {
                    return bad("bad text box");
                }
                if text.chars().count() > MAX_TEXT {
                    return bad("text too long");
                }
            }
            ObjectKind::Stamp {
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

/// Rotate `p` clockwise (screen space, y down) by `deg` degrees about `c`.
pub fn rotate_point(p: PointF, c: PointF, deg: f32) -> PointF {
    let (s, co) = deg.to_radians().sin_cos();
    let (dx, dy) = (p.x - c.x, p.y - c.y);
    PointF::new(c.x + dx * co - dy * s, c.y + dx * s + dy * co)
}

fn seg_dist(p: PointF, a: PointF, b: PointF) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l2 = dx * dx + dy * dy;
    let t = if l2 <= f32::EPSILON {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0)
    };
    p.dist(PointF::new(a.x + t * dx, a.y + t * dy))
}

fn norm_angle(a: f32) -> f32 {
    let r = a % 360.0;
    if r > 180.0 {
        r - 360.0
    } else if r <= -180.0 {
        r + 360.0
    } else {
        r
    }
}

impl Object {
    /// Whether this kind keeps its rotation in [`Object::rotation`] (box-like kinds).
    pub fn uses_rotation_field(&self) -> bool {
        matches!(
            self.kind,
            ObjectKind::Rect { .. }
                | ObjectKind::Ellipse { .. }
                | ObjectKind::Text { .. }
                | ObjectKind::Callout { .. }
                | ObjectKind::Stamp { .. }
                | ObjectKind::Step { .. }
        )
    }

    /// Whether rotating makes sense for this kind.
    pub fn can_rotate(&self) -> bool {
        self.uses_rotation_field()
            || matches!(
                self.kind,
                ObjectKind::Line { .. } | ObjectKind::Arrow { .. } | ObjectKind::Pen { .. }
            )
    }

    /// The unpadded, unrotated box that resize handles act on.
    pub fn geom_rect(&self) -> RectF {
        let pts = match &self.kind {
            ObjectKind::Arrow { from, to } | ObjectKind::Line { from, to } => vec![*from, *to],
            ObjectKind::Pen { points } => points.clone(),
            _ => return self.local_bounds(),
        };
        let Some(f) = pts.first() else {
            return RectF::default();
        };
        let (mut x0, mut y0, mut x1, mut y1) = (f.x, f.y, f.x, f.y);
        for p in &pts {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        RectF::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Centre of rotation.
    pub fn center(&self) -> PointF {
        let b = self.geom_rect();
        PointF::new(b.x + b.w / 2.0, b.y + b.h / 2.0)
    }

    /// Axis-aligned bounds in image coordinates (stroke width and rotation included).
    pub fn bounds(&self) -> RectF {
        let b = self.local_bounds();
        if self.rotation == 0.0 || !self.uses_rotation_field() {
            return b;
        }
        let c = self.center();
        let corners = [
            PointF::new(b.x, b.y),
            PointF::new(b.x + b.w, b.y),
            PointF::new(b.x + b.w, b.y + b.h),
            PointF::new(b.x, b.y + b.h),
        ]
        .map(|p| rotate_point(p, c, self.rotation));
        let (mut x0, mut y0, mut x1, mut y1) =
            (corners[0].x, corners[0].y, corners[0].x, corners[0].y);
        for p in &corners {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        RectF::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Whether the image point `p` hits this object (rotation-aware; thin shapes use distance).
    pub fn contains(&self, p: PointF) -> bool {
        let tol = self.style.width.max(1.0) / 2.0 + 4.0;
        match &self.kind {
            ObjectKind::Arrow { from, to } | ObjectKind::Line { from, to } => {
                seg_dist(p, *from, *to) <= tol
            }
            ObjectKind::Pen { points } => match points.as_slice() {
                [] => false,
                [only] => only.dist(p) <= tol,
                pts => pts
                    .windows(2)
                    .any(|w| matches!(w, [a, b] if seg_dist(p, *a, *b) <= tol)),
            },
            _ => {
                let q = if self.rotation != 0.0 && self.uses_rotation_field() {
                    rotate_point(p, self.center(), -self.rotation)
                } else {
                    p
                };
                self.local_bounds().contains(q)
            }
        }
    }

    /// Rotate by `delta` degrees clockwise. Box-like kinds turn about their centre; lines and
    /// pen strokes turn their points. Blur, highlight and magnify cannot rotate.
    pub fn rotate(&mut self, delta: f32) -> Result<(), DocError> {
        if !delta.is_finite() {
            return Err(DocError::Invalid("bad angle".into()));
        }
        if self.uses_rotation_field() {
            self.rotation = norm_angle(self.rotation + delta);
            return Ok(());
        }
        let c = self.center();
        match &mut self.kind {
            ObjectKind::Arrow { from, to } | ObjectKind::Line { from, to } => {
                *from = rotate_point(*from, c, delta);
                *to = rotate_point(*to, c, delta);
            }
            ObjectKind::Pen { points } => {
                for p in points.iter_mut() {
                    *p = rotate_point(*p, c, delta);
                }
            }
            _ => return Err(DocError::Invalid("this object cannot be rotated".into())),
        }
        Ok(())
    }

    /// The kind with its box changed to `new`, points and rects mapped proportionally.
    fn fitted(&self, new: RectF) -> ObjectKind {
        let old = self.geom_rect();
        let (sx, sy) = (
            if old.w > 1e-3 { new.w / old.w } else { 1.0 },
            if old.h > 1e-3 { new.h / old.h } else { 1.0 },
        );
        let map = |p: PointF| PointF::new(new.x + (p.x - old.x) * sx, new.y + (p.y - old.y) * sy);
        let mut k = self.kind.clone();
        match &mut k {
            ObjectKind::Arrow { from, to } | ObjectKind::Line { from, to } => {
                *from = map(*from);
                *to = map(*to);
            }
            ObjectKind::Pen { points } => points.iter_mut().for_each(|p| *p = map(*p)),
            ObjectKind::Rect { rect, .. }
            | ObjectKind::Ellipse { rect }
            | ObjectKind::Highlight { rect }
            | ObjectKind::Blur { rect, .. } => *rect = new,
            ObjectKind::Magnify { dst, .. } => *dst = new,
            ObjectKind::Text {
                pos, width, height, ..
            } => {
                *pos = PointF::new(new.x, new.y);
                *width = new.w;
                *height = new.h;
            }
            ObjectKind::Callout { rect, tail, .. } => {
                let r = rect.normalized();
                let o = map(PointF::new(r.x, r.y));
                *rect = RectF::new(o.x, o.y, r.w * sx, r.h * sy);
                *tail = map(*tail);
            }
            ObjectKind::Stamp { pos, size, .. } => {
                *pos = PointF::new(new.x, new.y);
                *size = new.w.min(new.h);
            }
            ObjectKind::Step { center, radius, .. } => {
                *center = PointF::new(new.x + new.w / 2.0, new.y + new.h / 2.0);
                *radius = new.w.min(new.h) / 2.0;
            }
        }
        k
    }

    /// Resize the box to `new` (in the object's own, unrotated frame). The point at fraction
    /// `anchor` of the box (0..1, 0..1) stays where it is on screen, rotation included.
    pub fn resize(&mut self, new: RectF, anchor: (f32, f32)) -> Result<(), DocError> {
        if !ok_rect(&new) || !anchor.0.is_finite() || !anchor.1.is_finite() {
            return Err(DocError::Invalid("bad box".into()));
        }
        let new = new.normalized();
        let thin = matches!(
            self.kind,
            ObjectKind::Arrow { .. } | ObjectKind::Line { .. } | ObjectKind::Pen { .. }
        );
        let min = if thin { 0.0 } else { 1.0 };
        let new = RectF::new(new.x, new.y, new.w.max(min), new.h.max(min));
        let at = |b: RectF| PointF::new(b.x + anchor.0 * b.w, b.y + anchor.1 * b.h);
        let before = self.geom_rect();
        let world_old = if self.uses_rotation_field() {
            rotate_point(at(before), self.center(), self.rotation)
        } else {
            at(before)
        };
        let kind = self.fitted(new);
        kind.validate()?;
        self.kind = kind;
        let after = self.geom_rect();
        let world_new = if self.uses_rotation_field() {
            rotate_point(at(after), self.center(), self.rotation)
        } else {
            at(after)
        };
        self.translate(world_old.x - world_new.x, world_old.y - world_new.y);
        Ok(())
    }

    /// Unrotated bounds in image coordinates (stroke width included).
    pub fn local_bounds(&self) -> RectF {
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
            ObjectKind::Text {
                pos,
                text,
                size,
                width,
                height,
            } => {
                if *width > 0.0 {
                    RectF::new(pos.x, pos.y, *width, height.max(*size * 1.3))
                } else {
                    let w = text.chars().count() as f32 * size * 0.6;
                    RectF::new(pos.x, pos.y, w.max(*size), height.max(*size * 1.3))
                }
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
        self.objects.push(Object {
            id,
            kind,
            style,
            rotation: 0.0,
        });
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
            .find(|o| o.contains(p))
            .map(|o| o.id)
    }

    fn obj_mut(&mut self, id: u64) -> Result<&mut Object, DocError> {
        self.objects
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or(DocError::NoObject(id))
    }

    /// Rotate object `id` by `delta` degrees clockwise.
    pub fn rotate(&mut self, id: u64, delta: f32) -> Result<(), DocError> {
        self.obj_mut(id)?.rotate(delta)
    }

    /// Resize object `id`; see [`Object::resize`].
    pub fn resize(&mut self, id: u64, new: RectF, anchor: (f32, f32)) -> Result<(), DocError> {
        self.obj_mut(id)?.resize(new, anchor)
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
    fn old_manifests_without_rotation_or_text_box_still_load() {
        let o: Object = serde_json::from_str(
            r#"{"id":1,"kind":{"type":"text","pos":{"x":1.0,"y":2.0},"text":"a","size":12.0},
                "style":{"stroke":[0,0,0,255],"fill":null,"width":1.0,"shadow":false}}"#,
        )
        .unwrap();
        assert_eq!(o.rotation, 0.0);
        assert!(matches!(o.kind, ObjectKind::Text { width, .. } if width == 0.0));
    }

    #[test]
    fn rotation_changes_bounds_and_hits() {
        let mut d = doc();
        let id = d
            .add(
                ObjectKind::Rect {
                    rect: RectF::new(10.0, 40.0, 60.0, 10.0),
                    radius: 0.0,
                },
                Style::default(),
            )
            .unwrap();
        d.rotate(id, 90.0).unwrap();
        let b = d.get(id).unwrap().bounds();
        assert!(
            (b.w - 10.0).abs() < 0.01 && (b.h - 60.0).abs() < 0.01,
            "{b:?}"
        );
        assert_eq!(d.hit_test(PointF::new(40.0, 20.0)), Some(id));
        assert_eq!(d.hit_test(PointF::new(15.0, 45.0)), None);
        d.rotate(id, 450.0).unwrap();
        assert!(d.get(id).unwrap().rotation.abs() <= 180.0);
        assert!(d.rotate(id, f32::NAN).is_err());
        assert!(d.rotate(99, 1.0).is_err());
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
