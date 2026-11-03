//! Selecting annotation objects on the canvas: the outline with resize / rotate handles
//! (drawn only for the selected object), the drag gestures they start, and typing into a
//! text box. Everything changes the document through engine commands.

use crate::UiApp;
use crate::theme::ACCENT2;
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use screenit_doc::{ObjectKind, rotate_point};
use screenit_geom::PointF;
use serde_json::json;

/// Screen distance within which a handle is grabbed.
const GRAB: f32 = 10.0;
/// Smallest box a resize can make, in image pixels.
const MIN_BOX: f32 = 6.0;
/// Distance of the rotate handle from the box edge, in screen pixels.
const ROTATE_GAP: f32 = 28.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum End {
    From,
    To,
    Tail,
}

/// What a drag on the canvas is currently doing to the selected object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Gesture {
    Move,
    /// Handle at fraction (hx, hy) of the box: 0, 0.5 or 1 on each axis.
    Resize {
        hx: f32,
        hy: f32,
    },
    Rotate {
        last: f32,
    },
    End(End),
}

/// A copy of what the overlay needs to know about the selected object.
#[derive(Clone, Debug)]
pub(crate) struct Sel {
    pub id: u64,
    /// The unrotated box in image space.
    pub geom: Rect,
    pub rotation: f32,
    /// Rotation is kept in the object (box-like kinds) rather than baked into points.
    pub field_rot: bool,
    pub can_rotate: bool,
    /// Show the eight box handles (lines and arrows use end handles instead).
    pub boxy: bool,
    pub ends: Option<(Pos2, Pos2)>,
    pub tail: Option<Pos2>,
}

impl Sel {
    pub fn center(&self) -> Pos2 {
        self.geom.center()
    }

    /// A point in the object's own frame to image space.
    fn world(&self, p: Pos2) -> Pos2 {
        if self.field_rot && self.rotation != 0.0 {
            let c = self.center();
            let r = rotate_point(PointF::new(p.x, p.y), PointF::new(c.x, c.y), self.rotation);
            Pos2::new(r.x, r.y)
        } else {
            p
        }
    }

    /// An image-space point into the object's own frame.
    fn local(&self, p: Pos2) -> Pos2 {
        if self.field_rot && self.rotation != 0.0 {
            let c = self.center();
            let r = rotate_point(PointF::new(p.x, p.y), PointF::new(c.x, c.y), -self.rotation);
            Pos2::new(r.x, r.y)
        } else {
            p
        }
    }

    fn at(&self, hx: f32, hy: f32) -> Pos2 {
        self.world(Pos2::new(
            self.geom.min.x + hx * self.geom.width(),
            self.geom.min.y + hy * self.geom.height(),
        ))
    }

    /// Direction "up" on screen for the rotated box.
    fn up(&self) -> Vec2 {
        let a = if self.field_rot { self.rotation } else { 0.0 }.to_radians();
        Vec2::new(a.sin(), -a.cos())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Handle {
    Rotate,
    End(End),
    Box(f32, f32),
}

const BOX_HANDLES: [(f32, f32); 8] = [
    (0.0, 0.0),
    (0.5, 0.0),
    (1.0, 0.0),
    (0.0, 0.5),
    (1.0, 0.5),
    (0.0, 1.0),
    (0.5, 1.0),
    (1.0, 1.0),
];

/// The selected object, if any, as the overlay sees it.
pub(crate) fn snapshot(app: &UiApp) -> Option<Sel> {
    let id = app.selected?;
    let o = app.session.doc_ref().ok()?.get(id)?;
    let g = o.geom_rect();
    let pt = |p: &PointF| Pos2::new(p.x, p.y);
    let (ends, tail) = match &o.kind {
        ObjectKind::Line { from, to } | ObjectKind::Arrow { from, to } => {
            (Some((pt(from), pt(to))), None)
        }
        ObjectKind::Callout { tail, .. } => (None, Some(pt(tail))),
        _ => (None, None),
    };
    let line = matches!(o.kind, ObjectKind::Line { .. } | ObjectKind::Arrow { .. });
    Some(Sel {
        id,
        geom: Rect::from_min_size(Pos2::new(g.x, g.y), Vec2::new(g.w, g.h)),
        rotation: o.rotation,
        field_rot: o.uses_rotation_field(),
        can_rotate: o.can_rotate() && !line,
        boxy: !line,
        ends,
        tail,
    })
}

fn handle_points(sel: &Sel, to_screen: &impl Fn(Pos2) -> Pos2) -> Vec<(Handle, Pos2)> {
    let mut v = Vec::new();
    if sel.can_rotate {
        let top = to_screen(sel.at(0.5, 0.0));
        v.push((Handle::Rotate, top + sel.up() * ROTATE_GAP));
    }
    if let Some((a, b)) = sel.ends {
        v.push((Handle::End(End::From), to_screen(a)));
        v.push((Handle::End(End::To), to_screen(b)));
    }
    if let Some(t) = sel.tail {
        v.push((Handle::End(End::Tail), to_screen(t)));
    }
    if sel.boxy {
        for (hx, hy) in BOX_HANDLES {
            v.push((Handle::Box(hx, hy), to_screen(sel.at(hx, hy))));
        }
    }
    v
}

/// Start a gesture if the pointer (screen space) is on one of the selected object's handles.
pub(crate) fn grab(sel: &Sel, p: Pos2, to_screen: &impl Fn(Pos2) -> Pos2) -> Option<Gesture> {
    handle_points(sel, to_screen)
        .into_iter()
        .find(|(_, s)| s.distance(p) <= GRAB)
        .map(|(h, _)| match h {
            Handle::Rotate => Gesture::Rotate { last: f32::NAN },
            Handle::End(e) => Gesture::End(e),
            Handle::Box(hx, hy) => Gesture::Resize { hx, hy },
        })
}

fn wrap_deg(a: f32) -> f32 {
    let r = a % 360.0;
    if r > 180.0 {
        r - 360.0
    } else if r < -180.0 {
        r + 360.0
    } else {
        r
    }
}

/// Apply one frame of a drag: `now` is the pointer in image space, `key` coalesces the frames
/// of this drag into one undo step.
pub(crate) fn drag(app: &mut UiApp, g: &mut Gesture, sel: &Sel, now: Pos2, delta: Vec2, key: &str) {
    let id = sel.id;
    match g {
        Gesture::Move => {
            if delta != Vec2::ZERO {
                app.run(
                    "editor.object.move",
                    json!({"id": id, "dx": delta.x, "dy": delta.y, "coalesce": key}),
                );
            }
        }
        Gesture::Resize { hx, hy } => {
            let q = sel.local(now);
            let (mut x0, mut y0, mut x1, mut y1) = (
                sel.geom.min.x,
                sel.geom.min.y,
                sel.geom.max.x,
                sel.geom.max.y,
            );
            if *hx == 0.0 {
                x0 = q.x.min(x1 - MIN_BOX);
            } else if *hx == 1.0 {
                x1 = q.x.max(x0 + MIN_BOX);
            }
            if *hy == 0.0 {
                y0 = q.y.min(y1 - MIN_BOX);
            } else if *hy == 1.0 {
                y1 = q.y.max(y0 + MIN_BOX);
            }
            app.run(
                "editor.object.resize",
                json!({
                    "id": id,
                    "rect": [x0, y0, x1 - x0, y1 - y0],
                    "anchor": [1.0 - *hx, 1.0 - *hy],
                    "coalesce": key,
                }),
            );
        }
        Gesture::Rotate { last } => {
            let c = sel.center();
            let ang = (now.y - c.y).atan2(now.x - c.x).to_degrees();
            if last.is_finite() {
                let d = wrap_deg(ang - *last);
                if d.abs() > 0.01 {
                    app.run(
                        "editor.object.rotate",
                        json!({"id": id, "delta": d, "coalesce": key}),
                    );
                }
            }
            *last = ang;
        }
        Gesture::End(e) => {
            let which = match e {
                End::From => "from",
                End::To => "to",
                End::Tail => "tail",
            };
            app.run(
                "editor.object.endpoint",
                json!({"id": id, "which": which, "point": [now.x, now.y], "coalesce": key}),
            );
        }
    }
}

/// Draw the outline and handles of the selected object.
pub(crate) fn paint(
    painter: &egui::Painter,
    sel: &Sel,
    to_screen: &impl Fn(Pos2) -> Pos2,
    editing: bool,
) {
    let accent = Stroke::new(1.5, ACCENT2);
    let halo = Stroke::new(3.5, Color32::from_black_alpha(110));
    if sel.boxy {
        let corners: Vec<Pos2> = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
            .iter()
            .map(|(x, y)| to_screen(sel.at(*x, *y)))
            .collect();
        painter.add(egui::Shape::closed_line(corners.clone(), halo));
        painter.add(egui::Shape::closed_line(corners, accent));
    }
    if let Some((a, b)) = sel.ends {
        painter.line_segment([to_screen(a), to_screen(b)], Stroke::new(1.0, ACCENT2));
    }
    if let Some(t) = sel.tail {
        let anchor = to_screen(sel.center());
        painter.line_segment([anchor, to_screen(t)], Stroke::new(1.0, ACCENT2));
    }
    let pts = handle_points(sel, to_screen);
    for (h, p) in &pts {
        match h {
            Handle::Rotate => {
                let top = to_screen(sel.at(0.5, 0.0));
                painter.line_segment([top, *p], accent);
                painter.circle_filled(*p, 6.0, Color32::WHITE);
                painter.circle_stroke(*p, 6.0, accent);
            }
            Handle::End(_) => {
                painter.circle_filled(*p, 6.0, ACCENT2);
                painter.circle_stroke(*p, 6.0, Stroke::new(1.5, Color32::WHITE));
            }
            Handle::Box(..) => {
                let r = Rect::from_center_size(*p, Vec2::splat(8.0));
                painter.rect_filled(r, 1.0, Color32::WHITE);
                painter.rect_stroke(r, 1.0, accent, egui::StrokeKind::Middle);
            }
        }
    }
    if editing {
        // A dotted second outline marks the text box as being typed into.
        let r = Rect::from_two_pos(to_screen(sel.geom.min), to_screen(sel.geom.max)).expand(3.0);
        painter.rect_stroke(
            r,
            2.0,
            Stroke::new(1.0, Color32::from_white_alpha(120)),
            egui::StrokeKind::Outside,
        );
    }
}

/// Type into the text box being edited: characters append, Backspace deletes, Enter starts a
/// new line, Escape finishes. The text is drawn by the engine, so the box always shows what
/// will be exported. Returns true when editing should stop.
pub(crate) fn text_input(app: &mut UiApp, ctx: &egui::Context, id: u64) -> bool {
    let events = ctx.input(|i| i.events.clone());
    let Some(mut text) = app
        .session
        .doc_ref()
        .ok()
        .and_then(|d| d.get(id))
        .and_then(|o| match &o.kind {
            ObjectKind::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
    else {
        return true;
    };
    let before = text.clone();
    let mut done = false;
    for e in events {
        match e {
            egui::Event::Text(s) | egui::Event::Paste(s) => {
                text.extend(s.chars().filter(|c| !c.is_control() || *c == '\n'));
            }
            egui::Event::Key {
                key, pressed: true, ..
            } => match key {
                egui::Key::Backspace => {
                    text.pop();
                }
                egui::Key::Enter => text.push('\n'),
                egui::Key::Escape => done = true,
                _ => {}
            },
            _ => {}
        }
    }
    if text != before {
        let key = format!("edit-text-{}-{}", id, app.gesture_seq);
        app.run(
            "editor.object.text",
            json!({"id": id, "text": text, "coalesce": key}),
        );
    }
    done
}

/// A blinking caret at the end of the text being typed.
pub(crate) fn paint_caret(
    app: &UiApp,
    painter: &egui::Painter,
    ctx: &egui::Context,
    id: u64,
    to_screen: &impl Fn(Pos2) -> Pos2,
    zoom: f32,
) {
    let Some(o) = app.session.doc_ref().ok().and_then(|d| d.get(id)) else {
        return;
    };
    let ObjectKind::Text {
        pos,
        text,
        size,
        width,
        ..
    } = &o.kind
    else {
        return;
    };
    ctx.request_repaint_after(std::time::Duration::from_millis(250));
    if (ctx.input(|i| i.time) * 2.0) as i64 % 2 != 0 {
        return;
    }
    let lay = screenit_vector::layout_text(text, *size, *width);
    let Some(last) = lay.lines.last() else { return };
    let row = (lay.lines.len() - 1) as f32;
    let x = if last.rtl {
        *width - last.width
    } else {
        last.width
    };
    let top = Pos2::new(pos.x + x, pos.y + row * lay.line_height);
    let bottom = Pos2::new(top.x, top.y + *size);
    let rot = |p: Pos2| {
        let local = Pos2::new(p.x, p.y);
        if o.rotation != 0.0 {
            let c = o.center();
            let r = rotate_point(
                PointF::new(local.x, local.y),
                PointF::new(c.x, c.y),
                o.rotation,
            );
            Pos2::new(r.x, r.y)
        } else {
            local
        }
    };
    painter.line_segment(
        [to_screen(rot(top)), to_screen(rot(bottom))],
        Stroke::new(
            (1.5 * zoom).clamp(1.0, 3.0),
            Color32::from_rgb(
                o.style.stroke.0[0],
                o.style.stroke.0[1],
                o.style.stroke.0[2],
            ),
        ),
    );
}
