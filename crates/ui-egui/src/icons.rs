//! Vector icons drawn with the painter (no icon font or bitmap assets to license or bundle).
//! Every icon is laid out on a unit square and scaled into the target rect.

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use std::f32::consts::TAU;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Capture,
    Fullscreen,
    Clipboard,
    Library,
    Editor,
    Select,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Text,
    Step,
    Blur,
    Highlight,
    Pen,
    Crop,
    Undo,
    Redo,
    Copy,
    Save,
    Export,
    Search,
    Play,
    Pause,
    Close,
    Back,
    ZoomIn,
    ZoomOut,
    Fit,
    Compact,
    Video,
    Trash,
    Record,
    Stop,
    Mic,
    Camera,
    Speaker,
    Cursor,
    Folder,
}

pub fn paint(p: &Painter, icon: Icon, rect: Rect, c: Color32) {
    let s = rect.width().min(rect.height());
    let o = rect.center() - Vec2::splat(s / 2.0);
    let at = |x: f32, y: f32| o + Vec2::new(x * s, y * s);
    let w = (s * 0.09).clamp(1.3, 2.4);
    let st = Stroke::new(w, c);
    let line = |pts: &[(f32, f32)]| {
        p.add(Shape::line(
            pts.iter().map(|&(x, y)| at(x, y)).collect(),
            st,
        ));
    };
    let closed = |pts: &[(f32, f32)]| {
        p.add(Shape::closed_line(
            pts.iter().map(|&(x, y)| at(x, y)).collect(),
            st,
        ));
    };
    let rrect = |x0: f32, y0: f32, x1: f32, y1: f32, r: f32| {
        p.rect_stroke(
            Rect::from_min_max(at(x0, y0), at(x1, y1)),
            r * s,
            st,
            eframe::egui::StrokeKind::Middle,
        );
    };
    let arc = |cx: f32, cy: f32, r: f32, a0: f32, a1: f32| -> Vec<Pos2> {
        (0..=16)
            .map(|i| {
                let t = a0 + (a1 - a0) * i as f32 / 16.0;
                at(cx + r * t.cos(), cy + r * t.sin())
            })
            .collect()
    };
    match icon {
        Icon::Capture => {
            for (a, b, d) in [
                ((0.08, 0.32), (0.08, 0.08), (0.32, 0.08)),
                ((0.68, 0.08), (0.92, 0.08), (0.92, 0.32)),
                ((0.92, 0.68), (0.92, 0.92), (0.68, 0.92)),
                ((0.32, 0.92), (0.08, 0.92), (0.08, 0.68)),
            ] {
                line(&[a, b, d]);
            }
            p.circle_stroke(at(0.5, 0.5), s * 0.16, st);
            p.circle_filled(at(0.5, 0.5), s * 0.05, c);
        }
        Icon::Fullscreen => {
            rrect(0.06, 0.14, 0.94, 0.74, 0.08);
            line(&[(0.36, 0.9), (0.64, 0.9)]);
            line(&[(0.5, 0.74), (0.5, 0.9)]);
        }
        Icon::Clipboard => {
            rrect(0.16, 0.14, 0.84, 0.94, 0.08);
            rrect(0.34, 0.06, 0.66, 0.22, 0.05);
            line(&[(0.32, 0.46), (0.68, 0.46)]);
            line(&[(0.32, 0.64), (0.58, 0.64)]);
        }
        Icon::Library => {
            rrect(0.06, 0.06, 0.44, 0.44, 0.08);
            rrect(0.56, 0.06, 0.94, 0.44, 0.08);
            rrect(0.06, 0.56, 0.44, 0.94, 0.08);
            rrect(0.56, 0.56, 0.94, 0.94, 0.08);
        }
        Icon::Editor => {
            rrect(0.06, 0.12, 0.94, 0.88, 0.08);
            line(&[
                (0.06, 0.66),
                (0.34, 0.42),
                (0.56, 0.62),
                (0.7, 0.5),
                (0.94, 0.72),
            ]);
            p.circle_filled(at(0.7, 0.3), s * 0.07, c);
        }
        Icon::Select => {
            p.add(Shape::convex_polygon(
                [
                    (0.2, 0.08),
                    (0.2, 0.82),
                    (0.4, 0.64),
                    (0.54, 0.94),
                    (0.66, 0.88),
                    (0.52, 0.58),
                    (0.78, 0.58),
                ]
                .iter()
                .map(|&(x, y)| at(x, y))
                .collect(),
                Color32::TRANSPARENT,
                st,
            ));
        }
        Icon::Arrow => {
            line(&[(0.12, 0.88), (0.84, 0.16)]);
            line(&[(0.46, 0.14), (0.86, 0.14), (0.86, 0.54)]);
        }
        Icon::Line => line(&[(0.12, 0.88), (0.88, 0.12)]),
        Icon::Rect => rrect(0.08, 0.2, 0.92, 0.8, 0.04),
        Icon::Ellipse => {
            let pts: Vec<Pos2> = (0..40)
                .map(|i| {
                    let t = i as f32 / 40.0 * TAU;
                    at(0.5 + 0.42 * t.cos(), 0.5 + 0.32 * t.sin())
                })
                .collect();
            p.add(Shape::closed_line(pts, st));
        }
        Icon::Text => {
            line(&[(0.16, 0.22), (0.16, 0.12), (0.84, 0.12), (0.84, 0.22)]);
            line(&[(0.5, 0.12), (0.5, 0.88)]);
            line(&[(0.36, 0.88), (0.64, 0.88)]);
        }
        Icon::Step => {
            p.circle_stroke(at(0.5, 0.5), s * 0.42, st);
            line(&[(0.42, 0.36), (0.54, 0.26), (0.54, 0.74)]);
            line(&[(0.42, 0.74), (0.66, 0.74)]);
        }
        Icon::Blur => {
            for (i, y) in [0.2f32, 0.5, 0.8].iter().enumerate() {
                for (j, x) in [0.2f32, 0.5, 0.8].iter().enumerate() {
                    let r = if (i + j) % 2 == 0 { 0.1 } else { 0.06 };
                    p.circle_filled(
                        at(*x, *y),
                        s * r,
                        c.gamma_multiply(if r > 0.08 { 1.0 } else { 0.6 }),
                    );
                }
            }
        }
        Icon::Highlight => {
            closed(&[(0.56, 0.1), (0.9, 0.44), (0.5, 0.84), (0.16, 0.5)]);
            line(&[(0.16, 0.5), (0.06, 0.94), (0.5, 0.84)]);
            p.rect_filled(
                Rect::from_min_max(at(0.04, 0.9), at(0.96, 0.98)),
                1.0,
                c.gamma_multiply(0.5),
            );
        }
        Icon::Pen => {
            line(&[
                (0.1, 0.9),
                (0.18, 0.66),
                (0.7, 0.14),
                (0.86, 0.3),
                (0.34, 0.82),
                (0.1, 0.9),
            ]);
            line(&[(0.6, 0.24), (0.76, 0.4)]);
        }
        Icon::Crop => {
            line(&[(0.26, 0.04), (0.26, 0.74), (0.96, 0.74)]);
            line(&[(0.04, 0.26), (0.74, 0.26), (0.74, 0.96)]);
        }
        Icon::Undo => {
            p.add(Shape::line(arc(0.54, 0.56, 0.34, -2.6, 1.6), st));
            line(&[(0.1, 0.18), (0.24, 0.32), (0.4, 0.2)]);
        }
        Icon::Redo => {
            p.add(Shape::line(
                arc(
                    0.46,
                    0.56,
                    0.34,
                    std::f32::consts::PI + 2.6,
                    std::f32::consts::PI - 1.6,
                ),
                st,
            ));
            line(&[(0.9, 0.18), (0.76, 0.32), (0.6, 0.2)]);
        }
        Icon::Copy => {
            rrect(0.3, 0.3, 0.92, 0.92, 0.08);
            line(&[
                (0.7, 0.2),
                (0.7, 0.08),
                (0.08, 0.08),
                (0.08, 0.7),
                (0.2, 0.7),
            ]);
        }
        Icon::Save => {
            closed(&[
                (0.08, 0.08),
                (0.74, 0.08),
                (0.92, 0.26),
                (0.92, 0.92),
                (0.08, 0.92),
            ]);
            rrect(0.26, 0.56, 0.74, 0.92, 0.02);
            line(&[(0.26, 0.08), (0.26, 0.3), (0.62, 0.3), (0.62, 0.08)]);
        }
        Icon::Export => {
            line(&[(0.5, 0.06), (0.5, 0.62)]);
            line(&[(0.28, 0.28), (0.5, 0.06), (0.72, 0.28)]);
            line(&[(0.1, 0.56), (0.1, 0.92), (0.9, 0.92), (0.9, 0.56)]);
        }
        Icon::Search => {
            p.circle_stroke(at(0.42, 0.42), s * 0.3, st);
            line(&[(0.64, 0.64), (0.92, 0.92)]);
        }
        Icon::Play => {
            p.add(Shape::convex_polygon(
                vec![at(0.22, 0.1), at(0.88, 0.5), at(0.22, 0.9)],
                c,
                Stroke::NONE,
            ));
        }
        Icon::Pause => {
            p.rect_filled(Rect::from_min_max(at(0.2, 0.1), at(0.4, 0.9)), 2.0, c);
            p.rect_filled(Rect::from_min_max(at(0.6, 0.1), at(0.8, 0.9)), 2.0, c);
        }
        Icon::Close => {
            line(&[(0.18, 0.18), (0.82, 0.82)]);
            line(&[(0.82, 0.18), (0.18, 0.82)]);
        }
        Icon::Back => {
            line(&[(0.62, 0.14), (0.26, 0.5), (0.62, 0.86)]);
        }
        Icon::ZoomIn | Icon::ZoomOut => {
            p.circle_stroke(at(0.42, 0.42), s * 0.3, st);
            line(&[(0.64, 0.64), (0.92, 0.92)]);
            line(&[(0.28, 0.42), (0.56, 0.42)]);
            if icon == Icon::ZoomIn {
                line(&[(0.42, 0.28), (0.42, 0.56)]);
            }
        }
        Icon::Fit => {
            line(&[(0.08, 0.34), (0.08, 0.08), (0.34, 0.08)]);
            line(&[(0.66, 0.08), (0.92, 0.08), (0.92, 0.34)]);
            line(&[(0.92, 0.66), (0.92, 0.92), (0.66, 0.92)]);
            line(&[(0.34, 0.92), (0.08, 0.92), (0.08, 0.66)]);
            rrect(0.3, 0.3, 0.7, 0.7, 0.04);
        }
        Icon::Compact => {
            rrect(0.06, 0.3, 0.94, 0.7, 0.1);
            p.circle_filled(at(0.26, 0.5), s * 0.08, c);
            line(&[(0.44, 0.5), (0.8, 0.5)]);
        }
        Icon::Video => {
            rrect(0.06, 0.22, 0.68, 0.78, 0.08);
            closed(&[(0.68, 0.42), (0.94, 0.26), (0.94, 0.74), (0.68, 0.58)]);
        }
        Icon::Trash => {
            line(&[(0.08, 0.22), (0.92, 0.22)]);
            line(&[(0.36, 0.22), (0.36, 0.08), (0.64, 0.08), (0.64, 0.22)]);
            closed(&[(0.18, 0.22), (0.24, 0.94), (0.76, 0.94), (0.82, 0.22)]);
        }
        Icon::Folder => {
            closed(&[
                (0.06, 0.18),
                (0.38, 0.18),
                (0.48, 0.3),
                (0.94, 0.3),
                (0.94, 0.86),
                (0.06, 0.86),
            ]);
            line(&[(0.06, 0.42), (0.94, 0.42)]);
        }
        Icon::Record => {
            p.circle_stroke(at(0.5, 0.5), s * 0.42, st);
            p.circle_filled(at(0.5, 0.5), s * 0.26, c);
        }
        Icon::Stop => {
            p.rect_filled(
                Rect::from_min_max(at(0.18, 0.18), at(0.82, 0.82)),
                s * 0.1,
                c,
            );
        }
        Icon::Mic => {
            rrect(0.34, 0.06, 0.66, 0.6, 0.16);
            p.add(Shape::line(
                arc(0.5, 0.42, 0.28, 0.0, std::f32::consts::PI),
                st,
            ));
            line(&[(0.5, 0.7), (0.5, 0.9)]);
            line(&[(0.32, 0.92), (0.68, 0.92)]);
        }
        Icon::Camera => {
            rrect(0.06, 0.26, 0.94, 0.86, 0.1);
            line(&[(0.32, 0.26), (0.4, 0.12), (0.6, 0.12), (0.68, 0.26)]);
            p.circle_stroke(at(0.5, 0.56), s * 0.17, st);
        }
        Icon::Speaker => {
            closed(&[
                (0.08, 0.36),
                (0.28, 0.36),
                (0.52, 0.14),
                (0.52, 0.86),
                (0.28, 0.64),
                (0.08, 0.64),
            ]);
            p.add(Shape::line(arc(0.56, 0.5, 0.18, -0.9, 0.9), st));
            p.add(Shape::line(arc(0.56, 0.5, 0.34, -0.9, 0.9), st));
        }
        Icon::Cursor => {
            closed(&[
                (0.22, 0.08),
                (0.22, 0.8),
                (0.4, 0.64),
                (0.54, 0.92),
                (0.66, 0.86),
                (0.52, 0.58),
                (0.76, 0.58),
            ]);
        }
    }
}
