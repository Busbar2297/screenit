//! Anti-aliased vector drawing of annotation objects onto rasters, plus text via system fonts.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use screenit_doc::{Color, Object, ObjectKind};
use screenit_geom::{PointF, RectF};
use screenit_raster::Raster;
use std::sync::OnceLock;

fn font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let dirs = [std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into())];
        for d in dirs {
            for f in ["segoeui.ttf", "arial.ttf", "calibri.ttf", "tahoma.ttf"] {
                if let Ok(bytes) = std::fs::read(format!("{d}\\Fonts\\{f}")) {
                    if let Ok(font) = FontVec::try_from_vec(bytes) {
                        return Some(font);
                    }
                }
            }
        }
        None
    })
    .as_ref()
}

/// Panic-free clamp: NaN maps to lo, and hi < lo collapses to lo.
fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_nan() {
        lo
    } else {
        v.max(lo).min(hi.max(lo))
    }
}

fn cov(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0) as u8
}

/// Distance from p to segment ab.
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

fn bounds_px(r: &Raster, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<(i32, i32, i32, i32)> {
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        return None;
    }
    let lim = |v: f32, max: u32| clampf(v, 0.0, max as f32 - 1.0) as i32;
    let (ax, ay) = (
        lim(x0.floor() - 1.0, r.width()),
        lim(y0.floor() - 1.0, r.height()),
    );
    let (bx, by) = (
        lim(x1.ceil() + 1.0, r.width()),
        lim(y1.ceil() + 1.0, r.height()),
    );
    if r.is_empty() {
        None
    } else {
        Some((ax, ay, bx, by))
    }
}

pub fn draw_line(r: &mut Raster, a: PointF, b: PointF, width: f32, c: Color) {
    let w = width.max(1.0);
    let Some((x0, y0, x1, y1)) = bounds_px(
        r,
        a.x.min(b.x) - w,
        a.y.min(b.y) - w,
        a.x.max(b.x) + w,
        a.y.max(b.y) + w,
    ) else {
        return;
    };
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = seg_dist(PointF::new(x as f32 + 0.5, y as f32 + 0.5), a, b);
            let v = cov(w / 2.0 + 0.5 - d);
            if v > 0 {
                r.blend(x, y, c.0, v);
            }
        }
    }
}

pub fn draw_polyline(r: &mut Raster, pts: &[PointF], width: f32, c: Color) {
    if let [only] = pts {
        draw_line(r, *only, *only, width, c);
    }
    for w in pts.windows(2) {
        if let [a, b] = w {
            draw_line(r, *a, *b, width, c);
        }
    }
}

/// Signed distance to a rounded rectangle (negative inside).
fn sd_round_rect(p: PointF, rc: RectF, radius: f32) -> f32 {
    let (cx, cy) = (rc.x + rc.w / 2.0, rc.y + rc.h / 2.0);
    let rad = clampf(radius, 0.0, rc.w.min(rc.h) / 2.0);
    let (qx, qy) = (
        (p.x - cx).abs() - (rc.w / 2.0 - rad),
        (p.y - cy).abs() - (rc.h / 2.0 - rad),
    );
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - rad
}

fn sd_ellipse(p: PointF, rc: RectF) -> f32 {
    let (rx, ry) = ((rc.w / 2.0).max(0.01), (rc.h / 2.0).max(0.01));
    let (dx, dy) = ((p.x - rc.x - rx) / rx, (p.y - rc.y - ry) / ry);
    ((dx * dx + dy * dy).sqrt() - 1.0) * rx.min(ry)
}

fn draw_sdf(
    r: &mut Raster,
    rc: RectF,
    pad: f32,
    fill: Option<Color>,
    stroke: Option<(Color, f32)>,
    sdf: impl Fn(PointF) -> f32,
) {
    let Some((x0, y0, x1, y1)) = bounds_px(
        r,
        rc.x - pad,
        rc.y - pad,
        rc.x + rc.w + pad,
        rc.y + rc.h + pad,
    ) else {
        return;
    };
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = sdf(PointF::new(x as f32 + 0.5, y as f32 + 0.5));
            if let Some(f) = fill {
                let v = cov(0.5 - d);
                if v > 0 {
                    r.blend(x, y, f.0, v);
                }
            }
            if let Some((c, w)) = stroke {
                let v = cov(w / 2.0 + 0.5 - d.abs());
                if v > 0 {
                    r.blend(x, y, c.0, v);
                }
            }
        }
    }
}

pub fn draw_rect(
    r: &mut Raster,
    rc: RectF,
    radius: f32,
    fill: Option<Color>,
    stroke: Option<(Color, f32)>,
) {
    let rc = rc.normalized();
    let pad = stroke.map_or(1.0, |s| s.1) + 1.0;
    draw_sdf(r, rc, pad, fill, stroke, |p| sd_round_rect(p, rc, radius));
}

pub fn draw_ellipse(r: &mut Raster, rc: RectF, fill: Option<Color>, stroke: Option<(Color, f32)>) {
    let rc = rc.normalized();
    let pad = stroke.map_or(1.0, |s| s.1) + 1.0;
    draw_sdf(r, rc, pad, fill, stroke, |p| sd_ellipse(p, rc));
}

/// Arrow: shaft plus a filled head at `to`.
pub fn draw_arrow(r: &mut Raster, from: PointF, to: PointF, width: f32, c: Color) {
    let len = from.dist(to);
    if len < 0.5 {
        return;
    }
    let w = width.max(1.0);
    let head = (w * 4.0).max(10.0).min(len);
    let (ux, uy) = ((to.x - from.x) / len, (to.y - from.y) / len);
    let base = PointF::new(to.x - ux * head, to.y - uy * head);
    draw_line(r, from, base, w, c);
    let half = head * 0.5;
    let (l, rr) = (
        PointF::new(base.x - uy * half, base.y + ux * half),
        PointF::new(base.x + uy * half, base.y - ux * half),
    );
    // Triangle fill by edge functions.
    let area =
        |p: PointF, q: PointF, s: PointF| (q.x - p.x) * (s.y - p.y) - (q.y - p.y) * (s.x - p.x);
    let Some((x0, y0, x1, y1)) = bounds_px(
        r,
        l.x.min(rr.x).min(to.x),
        l.y.min(rr.y).min(to.y),
        l.x.max(rr.x).max(to.x),
        l.y.max(rr.y).max(to.y),
    ) else {
        return;
    };
    let total = area(l, rr, to);
    if total.abs() < f32::EPSILON {
        return;
    }
    for y in y0..=y1 {
        for x in x0..=x1 {
            let p = PointF::new(x as f32 + 0.5, y as f32 + 0.5);
            let (w0, w1, w2) = (
                area(rr, to, p) / total,
                area(to, l, p) / total,
                area(l, rr, p) / total,
            );
            let m = w0.min(w1).min(w2);
            let v = cov(m * head * 2.0 + 0.5);
            if v > 0 {
                r.blend(x, y, c.0, v);
            }
        }
    }
}

/// Width in pixels of a text run at `size` (px height). Falls back to a rough estimate without fonts.
pub fn text_width(text: &str, size: f32) -> f32 {
    let Some(f) = font() else {
        return text.chars().count() as f32 * size * 0.55;
    };
    let sf = f.as_scaled(PxScale::from(size));
    text.chars().map(|ch| sf.h_advance(f.glyph_id(ch))).sum()
}

/// Draw text with its top-left at `pos`. Returns the width drawn.
pub fn draw_text(r: &mut Raster, text: &str, pos: PointF, size: f32, c: Color) -> f32 {
    if !size.is_finite() || size <= 0.0 || !pos.is_finite() {
        return 0.0;
    }
    let Some(f) = font() else { return 0.0 };
    let scale = PxScale::from(size.min(2000.0));
    let sf = f.as_scaled(scale);
    let mut x = pos.x;
    let mut y = pos.y + sf.ascent();
    let line_h = sf.height() + sf.line_gap();
    let mut max_w: f32 = 0.0;
    for ch in text.chars() {
        if ch == '\n' {
            max_w = max_w.max(x - pos.x);
            x = pos.x;
            y += line_h;
            continue;
        }
        let g = f
            .glyph_id(ch)
            .with_scale_and_position(scale, ab_glyph::point(x, y));
        x += sf.h_advance(g.id);
        if let Some(og) = f.outline_glyph(g) {
            let b = og.px_bounds();
            og.draw(|gx, gy, v| {
                r.blend(
                    b.min.x as i32 + gx as i32,
                    b.min.y as i32 + gy as i32,
                    c.0,
                    cov(v),
                );
            });
        }
    }
    max_w.max(x - pos.x)
}

fn shadow_color() -> Color {
    Color([0, 0, 0, 90])
}

/// Draw a pure-vector object. Pixel-dependent kinds (Blur, Magnify, Highlight) are handled by the compositor.
pub fn draw_object(r: &mut Raster, o: &Object) {
    let st = &o.style;
    let stroke = Some((st.stroke, st.width.max(0.5)));
    let shadow = |r: &mut Raster, obj: &Object| {
        if obj.style.shadow {
            let mut sh = obj.clone();
            sh.style.shadow = false;
            sh.style.stroke = shadow_color();
            sh.style.fill = sh.style.fill.map(|_| shadow_color());
            sh.translate(3.0, 3.0);
            draw_object(r, &sh);
        }
    };
    shadow(r, o);
    match &o.kind {
        ObjectKind::Arrow { from, to } => draw_arrow(r, *from, *to, st.width, st.stroke),
        ObjectKind::Line { from, to } => draw_line(r, *from, *to, st.width, st.stroke),
        ObjectKind::Rect { rect, radius } => draw_rect(r, *rect, *radius, st.fill, stroke),
        ObjectKind::Ellipse { rect } => draw_ellipse(r, *rect, st.fill, stroke),
        ObjectKind::Text { pos, text, size } => {
            draw_text(r, text, *pos, *size, st.stroke);
        }
        ObjectKind::Callout {
            rect,
            text,
            tail,
            size,
        } => {
            let rc = rect.normalized();
            let fill = st.fill.unwrap_or(Color::WHITE);
            let anchor = PointF::new(
                tail.x.clamp(rc.x, rc.x + rc.w),
                tail.y.clamp(rc.y, rc.y + rc.h),
            );
            draw_line(r, anchor, *tail, st.width.max(1.0), st.stroke);
            draw_rect(r, rc, 6.0, Some(fill), stroke);
            draw_text(
                r,
                text,
                PointF::new(rc.x + 6.0, rc.y + 4.0),
                *size,
                st.stroke,
            );
        }
        ObjectKind::Step {
            center,
            number,
            radius,
        } => {
            let rc = RectF::new(
                center.x - radius,
                center.y - radius,
                radius * 2.0,
                radius * 2.0,
            );
            draw_ellipse(r, rc, Some(st.fill.unwrap_or(st.stroke)), None);
            let label = number.to_string();
            let size = radius * 1.2;
            let w = text_width(&label, size);
            draw_text(
                r,
                &label,
                PointF::new(center.x - w / 2.0, center.y - size * 0.65),
                size,
                Color::WHITE,
            );
        }
        ObjectKind::Pen { points } => draw_polyline(r, points, st.width, st.stroke),
        ObjectKind::Stamp { pos, name, size } => {
            let rc = RectF::new(pos.x, pos.y, *size, *size);
            draw_ellipse(r, rc, Some(st.fill.unwrap_or(st.stroke)), None);
            let glyph: String = name
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default();
            let w = text_width(&glyph, size * 0.6);
            draw_text(
                r,
                &glyph,
                PointF::new(pos.x + (size - w) / 2.0, pos.y + size * 0.2),
                size * 0.6,
                Color::WHITE,
            );
        }
        ObjectKind::Blur { .. } | ObjectKind::Magnify { .. } | ObjectKind::Highlight { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenit_doc::Style;

    fn canvas() -> Raster {
        Raster::new(64, 64, [255, 255, 255, 255]).unwrap()
    }

    #[test]
    fn line_marks_pixels_along_path() {
        let mut r = canvas();
        draw_line(
            &mut r,
            PointF::new(5.0, 5.0),
            PointF::new(55.0, 5.0),
            3.0,
            Color::BLACK,
        );
        assert_eq!(r.get(30, 5).unwrap()[0], 0);
        assert_eq!(r.get(30, 30).unwrap()[0], 255);
    }

    #[test]
    fn rect_fill_and_stroke() {
        let mut r = canvas();
        draw_rect(
            &mut r,
            RectF::new(10.0, 10.0, 30.0, 30.0),
            0.0,
            Some(Color([0, 0, 255, 255])),
            Some((Color::BLACK, 2.0)),
        );
        assert_eq!(r.get(25, 25), Some([0, 0, 255, 255]));
        assert!(r.get(10, 25).unwrap()[0] < 80);
        assert_eq!(r.get(2, 2), Some([255; 4]));
    }

    #[test]
    fn ellipse_leaves_corners() {
        let mut r = canvas();
        draw_ellipse(
            &mut r,
            RectF::new(0.0, 0.0, 64.0, 64.0),
            Some(Color::BLACK),
            None,
        );
        assert_eq!(r.get(32, 32).unwrap()[0], 0);
        assert_eq!(r.get(1, 1).unwrap()[0], 255);
    }

    #[test]
    fn arrow_head_is_wider_than_shaft() {
        let mut r = canvas();
        draw_arrow(
            &mut r,
            PointF::new(5.0, 32.0),
            PointF::new(60.0, 32.0),
            2.0,
            Color::BLACK,
        );
        let col = |x: i32| (0..64).filter(|y| r.get(x, *y).unwrap()[0] < 128).count();
        assert!(col(54) > col(20), "{} vs {}", col(54), col(20));
    }

    #[test]
    fn hostile_objects_do_not_panic() {
        let mut r = canvas();
        for p in [f32::NAN, f32::INFINITY, -1.0e30, 1.0e30] {
            draw_line(
                &mut r,
                PointF::new(p, 0.0),
                PointF::new(0.0, p),
                p,
                Color::RED,
            );
            draw_rect(
                &mut r,
                RectF::new(p, p, p, p),
                p,
                Some(Color::RED),
                Some((Color::RED, p)),
            );
            draw_ellipse(
                &mut r,
                RectF::new(p, 0.0, 5.0, p),
                None,
                Some((Color::RED, p)),
            );
            draw_arrow(
                &mut r,
                PointF::new(p, p),
                PointF::new(1.0, 1.0),
                p,
                Color::RED,
            );
            draw_text(&mut r, "x", PointF::new(p, p), p, Color::RED);
        }
        draw_polyline(&mut r, &[], 3.0, Color::RED);
        let mut empty = Raster::default();
        draw_line(
            &mut empty,
            PointF::new(0.0, 0.0),
            PointF::new(5.0, 5.0),
            3.0,
            Color::RED,
        );
    }

    #[test]
    fn text_draws_when_a_font_exists() {
        let mut r = canvas();
        let w = draw_text(&mut r, "Hi", PointF::new(4.0, 4.0), 24.0, Color::BLACK);
        if font().is_some() {
            assert!(w > 5.0);
            assert!(r.data().chunks(4).any(|p| p[0] < 128));
        }
    }

    #[test]
    fn step_object_draws_disc() {
        let mut r = canvas();
        let o = Object {
            id: 1,
            kind: ObjectKind::Step {
                center: PointF::new(32.0, 32.0),
                number: 3,
                radius: 14.0,
            },
            style: Style::default(),
        };
        draw_object(&mut r, &o);
        assert_ne!(r.get(22, 32), Some([255; 4]));
    }
}
