//! CPU compositor: the reference ("oracle") renderer that flattens a document to pixels.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use screenit_algo as algo;
use screenit_doc::{CaptureDoc, Color, Object, ObjectKind};
use screenit_raster::Raster;
use screenit_vector as vector;

fn render_object(r: &mut Raster, o: &Object) {
    match &o.kind {
        ObjectKind::Blur {
            rect,
            radius,
            pixelate,
        } => {
            let rc = rect.to_rect();
            if *pixelate {
                algo::pixelate(r, rc, (*radius).max(2));
            } else {
                algo::box_blur(r, rc, *radius);
                algo::box_blur(r, rc, *radius);
            }
        }
        ObjectKind::Highlight { rect } => {
            let c = if o.style.stroke.0[3] == 255 {
                o.style.stroke
            } else {
                Color::YELLOW
            };
            let rc = rect.to_rect();
            let Some(rc) = rc.intersect(&r.rect()) else {
                return;
            };
            for y in rc.y..rc.y + rc.h as i32 {
                for x in rc.x..rc.x + rc.w as i32 {
                    // Multiply-style marker so dark text stays readable.
                    if let Some(p) = r.get(x, y) {
                        let m = |a: u8, b: u8| ((u32::from(a) * u32::from(b)) / 255) as u8;
                        r.put(
                            x,
                            y,
                            [m(p[0], c.0[0]), m(p[1], c.0[1]), m(p[2], c.0[2]), p[3]],
                        );
                    }
                }
            }
        }
        ObjectKind::Magnify { src, dst, factor } => {
            let s = src.to_rect();
            let Some(crop) = r.crop(s) else { return };
            let d = dst.to_rect();
            let (w, h) = (
                ((crop.width() as f32 * factor) as u32).min(d.w.max(1)),
                ((crop.height() as f32 * factor) as u32).min(d.h.max(1)),
            );
            if let Ok(big) = algo::resize(&crop, w.max(1), h.max(1)) {
                r.copy_from(&big, d.x, d.y);
                vector::draw_rect(
                    r,
                    (*dst).into_rect_f(w, h),
                    0.0,
                    None,
                    Some((o.style.stroke, o.style.width.max(1.0))),
                );
            }
        }
        _ => vector::draw_object(r, o),
    }
}

trait IntoRectF {
    fn into_rect_f(self, w: u32, h: u32) -> screenit_geom::RectF;
}

impl IntoRectF for screenit_geom::RectF {
    fn into_rect_f(self, w: u32, h: u32) -> screenit_geom::RectF {
        let n = self.normalized();
        screenit_geom::RectF::new(n.x, n.y, w as f32, h as f32)
    }
}

/// Render the base raster and all objects, bottom to top.
pub fn flatten(doc: &CaptureDoc) -> Raster {
    let mut out = doc.base().clone();
    for o in &doc.objects {
        render_object(&mut out, o);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenit_doc::Style;
    use screenit_geom::{PointF, RectF};

    fn doc() -> CaptureDoc {
        CaptureDoc::new(Raster::new(60, 40, [255, 255, 255, 255]).unwrap())
    }

    #[test]
    fn empty_doc_equals_base() {
        let d = doc();
        assert_eq!(&flatten(&d), d.base());
    }

    #[test]
    fn flatten_is_deterministic_and_non_destructive() {
        let mut d = doc();
        d.add(
            ObjectKind::Rect {
                rect: RectF::new(5.0, 5.0, 20.0, 20.0),
                radius: 3.0,
            },
            Style::default(),
        )
        .unwrap();
        let a = flatten(&d);
        let b = flatten(&d);
        assert_eq!(a, b);
        assert_ne!(&a, d.base());
        assert_eq!(d.base().get(5, 5), Some([255; 4]));
    }

    #[test]
    fn blur_object_changes_only_its_region() {
        let mut base = Raster::new(60, 40, [255; 4]).unwrap();
        for x in 0..60 {
            base.put(x, 20, [0, 0, 0, 255]);
        }
        let mut d = CaptureDoc::new(base);
        d.add(
            ObjectKind::Blur {
                rect: RectF::new(10.0, 10.0, 20.0, 20.0),
                radius: 4,
                pixelate: false,
            },
            Style::default(),
        )
        .unwrap();
        let out = flatten(&d);
        assert_ne!(out.get(20, 20), Some([0, 0, 0, 255]));
        assert_eq!(out.get(50, 20), Some([0, 0, 0, 255]));
    }

    #[test]
    fn highlight_keeps_dark_pixels_dark() {
        let mut base = Raster::new(10, 10, [255; 4]).unwrap();
        base.put(5, 5, [0, 0, 0, 255]);
        let mut d = CaptureDoc::new(base);
        d.add(
            ObjectKind::Highlight {
                rect: RectF::new(0.0, 0.0, 10.0, 10.0),
            },
            Style {
                stroke: Color::YELLOW,
                ..Style::default()
            },
        )
        .unwrap();
        let out = flatten(&d);
        assert_eq!(out.get(5, 5), Some([0, 0, 0, 255]));
        assert_eq!(out.get(0, 0).map(|p| p[2]), Some(0));
    }

    #[test]
    fn magnify_copies_enlarged_region() {
        let mut base = Raster::new(60, 40, [255; 4]).unwrap();
        base.fill_rect(screenit_geom::Rect::new(2, 2, 4, 4), [0, 0, 255, 255]);
        let mut d = CaptureDoc::new(base);
        d.add(
            ObjectKind::Magnify {
                src: RectF::new(2.0, 2.0, 4.0, 4.0),
                dst: RectF::new(30.0, 10.0, 20.0, 20.0),
                factor: 3.0,
            },
            Style::default(),
        )
        .unwrap();
        let out = flatten(&d);
        assert_eq!(out.get(36, 16), Some([0, 0, 255, 255]));
    }

    #[test]
    fn many_objects_with_hostile_geometry_do_not_panic() {
        let mut d = doc();
        for rect in [
            RectF::new(-100.0, -100.0, 1.0e5, 1.0e5),
            RectF::new(1.0, 1.0, 0.0, 0.0),
            RectF::new(70.0, 50.0, -30.0, -30.0),
        ] {
            d.add(
                ObjectKind::Blur {
                    rect,
                    radius: 1000,
                    pixelate: true,
                },
                Style::default(),
            )
            .unwrap();
            d.add(
                ObjectKind::Magnify {
                    src: rect,
                    dst: rect,
                    factor: 32.0,
                },
                Style::default(),
            )
            .unwrap();
            d.add(ObjectKind::Highlight { rect }, Style::default())
                .unwrap();
            d.add(ObjectKind::Ellipse { rect }, Style::default())
                .unwrap();
        }
        d.add(
            ObjectKind::Arrow {
                from: PointF::new(0.0, 0.0),
                to: PointF::new(0.0, 0.0),
            },
            Style::default(),
        )
        .unwrap();
        let _ = flatten(&d);
    }
}
