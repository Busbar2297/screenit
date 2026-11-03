//! The document canvas: shows the flattened document and turns drags into engine commands.

use crate::selection::{self, Gesture};
use crate::{Tool, UiApp};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use serde_json::{Value, json};

/// Pointer drags shorter than this (image pixels) are clicks, not boxes.
const CLICK_SLOP: f32 = 6.0;
/// Width of a text box made by a plain click, in image pixels.
const DEFAULT_TEXT_W: f32 = 240.0;

pub fn show(app: &mut UiApp, ui: &mut egui::Ui) {
    let Some(tex) = app.tex.clone() else {
        crate::chrome::empty_state(ui);
        return;
    };
    // zoom 1.0 = one image pixel per physical screen pixel (crisp on HiDPI displays).
    let ppp = ui.ctx().pixels_per_point().max(0.1);
    let avail = ui.available_size();
    let margin = 32.0;
    if app.fit {
        let img = tex.size_vec2() / ppp;
        let k = ((avail.x - margin * 2.0) / img.x.max(1.0))
            .min((avail.y - margin * 2.0) / img.y.max(1.0));
        app.zoom = if k.is_finite() {
            k.clamp(0.02, 1.0)
        } else {
            1.0
        };
    }
    let (wheel, ctrl) = ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.command));
    if ctrl && wheel != 0.0 && ui.rect_contains_pointer(ui.max_rect()) {
        app.zoom = (app.zoom * (1.0 + wheel * 0.002)).clamp(0.02, 8.0);
        app.fit = false;
    }
    let scale = app.zoom / ppp;
    let size = tex.size_vec2() * scale;
    crate::chrome::canvas_backdrop(ui.painter(), ui.max_rect());
    let mut scroll = egui::ScrollArea::both().auto_shrink([false, false]);
    if ctrl {
        scroll = scroll.scroll_source(egui::scroll_area::ScrollSource::SCROLL_BAR);
    }
    scroll.show(ui, |ui| {
        let area = (size + Vec2::splat(margin * 2.0)).max(avail);
        let (outer, _) = ui.allocate_exact_size(area, Sense::hover());
        let img_rect = Rect::from_center_size(outer.center(), size);
        let resp = ui.interact(img_rect, ui.scope_id().with("doc"), Sense::click_and_drag());
        let painter = ui.painter_at(outer);
        crate::chrome::document_frame(&painter, img_rect);
        tex.paint(&painter, img_rect);
        if resp.hovered() && app.tool != Tool::Select {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        let ctx = ui.ctx().clone();
        let geo = Geo {
            origin: img_rect.min,
            zoom: scale,
        };
        let pointer = resp.interact_pointer_pos();
        // A drag is reported once the pointer has moved a little; it began at the press.
        let press = ctx.input(|i| i.pointer.press_origin()).or(pointer);

        // Typing into the text box being edited.
        if let Some(id) = app.editing {
            if selection::text_input(app, &ctx, id) {
                end_edit(app);
            }
            ctx.request_repaint();
        }
        click_away(app, &resp, &geo, press, pointer);
        if resp.drag_started() {
            drag_start(app, &geo, press);
        }
        if resp.dragged() {
            dragging(app, &resp, &geo, &painter, &ctx, pointer);
        }
        released(app, &resp, &geo, &ctx, pointer);

        if let Some(sel) = selection::snapshot(app) {
            selection::paint(
                &painter,
                &sel,
                &|p| geo.to_screen(p),
                app.editing == Some(sel.id),
            );
        }
        if let Some(id) = app.editing {
            selection::paint_caret(app, &painter, &ctx, id, &|p| geo.to_screen(p), geo.zoom);
        }
        app.sync_selected_style();
        if app.tex_rev != app.doc_rev {
            // A command ran this frame: draw once more so the new pixels appear at once.
            ctx.request_repaint();
        }
    });
}

/// Where the image sits on screen and how big it is drawn.
struct Geo {
    origin: Pos2,
    zoom: f32,
}

impl Geo {
    fn to_img(&self, p: Pos2) -> Pos2 {
        ((p - self.origin) / self.zoom).to_pos2()
    }
    fn to_screen(&self, p: Pos2) -> Pos2 {
        self.origin + p.to_vec2() * self.zoom
    }
}

/// Clicking away from the box being edited (and its handles) ends the edit.
fn click_away(
    app: &mut UiApp,
    resp: &egui::Response,
    geo: &Geo,
    press: Option<Pos2>,
    pointer: Option<Pos2>,
) {
    if !(resp.drag_started() || resp.clicked() || resp.double_clicked()) {
        return;
    }
    let at = if resp.drag_started() { press } else { pointer };
    let (Some(p), Some(id)) = (at, app.editing) else {
        return;
    };
    let on_handle = selection::snapshot(app)
        .and_then(|s| selection::grab(&s, p, &|q| geo.to_screen(q)))
        .is_some();
    let q = geo.to_img(p);
    let inside = app
        .session
        .doc_ref()
        .ok()
        .and_then(|d| d.get(id))
        .is_some_and(|o| o.contains(screenit_geom::PointF::new(q.x, q.y)));
    if !inside && !on_handle {
        end_edit(app);
    }
}

/// A drag began: grab a handle, pick up an object, or start drawing.
fn drag_start(app: &mut UiApp, geo: &Geo, press: Option<Pos2>) {
    let Some(p) = press else { return };
    app.gesture_seq = app.gesture_seq.wrapping_add(1);
    app.pen.clear();
    let grabbed =
        selection::snapshot(app).and_then(|s| selection::grab(&s, p, &|q| geo.to_screen(q)));
    if let Some(g) = grabbed {
        app.gesture = Some(g);
    } else if app.tool == Tool::Select {
        let hit = hit_id(app, geo.to_img(p));
        if hit != app.selected {
            end_edit(app);
        }
        app.selected = hit;
        app.gesture = hit.map(|_| Gesture::Move);
    } else {
        app.drag = Some(geo.to_img(p));
    }
}

/// Mid-drag: drive the active gesture or draw the live preview.
fn dragging(
    app: &mut UiApp,
    resp: &egui::Response,
    geo: &Geo,
    painter: &egui::Painter,
    ctx: &egui::Context,
    pointer: Option<Pos2>,
) {
    let Some(p) = pointer else { return };
    let now = geo.to_img(p);
    if let Some(mut g) = app.gesture {
        if let Some(sel) = selection::snapshot(app) {
            let key = format!("ui-{}", app.gesture_seq);
            selection::drag(app, &mut g, &sel, now, resp.drag_delta() / geo.zoom, &key);
            app.gesture = Some(g);
            ctx.request_repaint();
        }
    } else if app.tool == Tool::Pen {
        app.pen.push([now.x, now.y]);
        let pts: Vec<Pos2> = app
            .pen
            .iter()
            .map(|q| geo.to_screen(Pos2::new(q[0], q[1])))
            .collect();
        painter.add(egui::Shape::line(
            pts,
            Stroke::new((app.width * geo.zoom).max(1.0), app.stroke),
        ));
    } else if let Some(start) = app.drag {
        draw_preview(
            painter,
            app.tool,
            app.stroke,
            (app.width * geo.zoom).max(1.0),
            geo.to_screen(start),
            geo.to_screen(now),
        );
    }
}

/// Drag finished or the pointer clicked.
fn released(
    app: &mut UiApp,
    resp: &egui::Response,
    geo: &Geo,
    ctx: &egui::Context,
    pointer: Option<Pos2>,
) {
    if resp.drag_stopped() {
        app.gesture = None;
        ctx.request_repaint();
        ctx.request_repaint_after(std::time::Duration::from_millis(60));
        if let (Some(start), Some(p)) = (app.drag.take(), pointer) {
            finish_drag(app, start, geo.to_img(p));
        }
        return;
    }
    let Some(p) = pointer else { return };
    let at = geo.to_img(p);
    if resp.double_clicked() {
        if let Some(id) = hit_id(app, at).filter(|id| is_text(app, *id)) {
            begin_edit(app, id);
        }
    } else if resp.clicked() {
        match app.tool {
            Tool::Text => match hit_id(app, at).filter(|id| is_text(app, *id)) {
                Some(id) => begin_edit(app, id),
                None => finish_drag(app, at, at),
            },
            Tool::Step => finish_drag(app, at, at),
            _ => {
                let hit = hit_id(app, at);
                if hit != app.selected {
                    end_edit(app);
                }
                app.selected = hit;
            }
        }
    }
}
fn hit_id(app: &mut UiApp, p: Pos2) -> Option<u64> {
    app.session
        .doc_ref()
        .ok()
        .and_then(|d| d.hit_test(screenit_geom::PointF::new(p.x, p.y)))
}

fn is_text(app: &UiApp, id: u64) -> bool {
    app.session
        .doc_ref()
        .ok()
        .and_then(|d| d.get(id))
        .is_some_and(|o| matches!(o.kind, screenit_doc::ObjectKind::Text { .. }))
}

/// Start typing into text box `id`.
pub(crate) fn begin_edit(app: &mut UiApp, id: u64) {
    app.gesture_seq = app.gesture_seq.wrapping_add(1);
    app.selected = Some(id);
    app.editing = Some(id);
}

/// Stop typing: the box outline disappears and only the text stays. A box left empty is removed.
pub(crate) fn end_edit(app: &mut UiApp) {
    let Some(id) = app.editing.take() else { return };
    let empty = app
        .session
        .doc_ref()
        .ok()
        .and_then(|d| d.get(id))
        .is_some_and(|o| {
            matches!(&o.kind, screenit_doc::ObjectKind::Text { text, .. } if text.trim().is_empty())
        });
    if empty {
        app.run("editor.object.delete", json!({"id": id}));
    }
    app.selected = None;
}

/// Select what a drawing command just made.
fn select_new(app: &mut UiApp, made: Option<Value>) {
    if let Some(id) = made.and_then(|v| v["id"].as_u64()) {
        app.selected = Some(id);
    }
}

fn finish_drag(app: &mut UiApp, a: Pos2, b: Pos2) {
    let style = json!({"stroke": app.stroke_hex(), "width": app.width});
    let rect = {
        let r = Rect::from_two_pos(a, b);
        json!([r.min.x, r.min.y, r.width(), r.height()])
    };
    let moved = (b - a).length() > 2.0;
    let made = match app.tool {
        Tool::Arrow if moved => app.run(
            "editor.arrow.add",
            json!({"from": [a.x, a.y], "to": [b.x, b.y], "style": style}),
        ),
        Tool::Line if moved => app.run(
            "editor.line.add",
            json!({"from": [a.x, a.y], "to": [b.x, b.y], "style": style}),
        ),
        Tool::Rect if moved => app.run("editor.rect.add", json!({"rect": rect, "style": style})),
        Tool::Ellipse if moved => {
            app.run("editor.ellipse.add", json!({"rect": rect, "style": style}))
        }
        Tool::Text => {
            let size = (app.width * 6.0).max(12.0);
            let r = Rect::from_two_pos(a, b);
            let (w, h) = if (b - a).length() > CLICK_SLOP {
                (r.width().max(24.0), r.height().max(size * 1.3))
            } else {
                (DEFAULT_TEXT_W, size * 1.3)
            };
            let id = app
                .run(
                    "editor.text.add",
                    json!({
                        "pos": [r.min.x, r.min.y],
                        "text": "",
                        "size": size,
                        "width": w,
                        "height": h,
                        "style": {"stroke": app.stroke_hex()},
                    }),
                )
                .and_then(|v| v["id"].as_u64());
            if let Some(id) = id {
                begin_edit(app, id);
            }
            None
        }
        Tool::Step => app.run(
            "editor.step.add",
            json!({"center": [a.x, a.y], "style": style}),
        ),
        Tool::Blur if moved => app.run(
            "editor.blur.add",
            json!({"rect": rect, "radius": 10, "pixelate": false}),
        ),
        Tool::Highlight if moved => app.run("editor.highlight.add", json!({"rect": rect})),
        Tool::Crop if moved => {
            app.run("editor.crop", json!({"rect": rect}));
            None
        }
        Tool::Pen if app.pen.len() > 1 => {
            let pts = std::mem::take(&mut app.pen);
            app.run("editor.pen.add", json!({"points": pts, "style": style}))
        }
        _ => None,
    };
    select_new(app, made);
    app.pen.clear();
}

/// Live drag preview that matches what the tool will actually create (a line for line/arrow tools,
/// an ellipse for the ellipse tool, a rectangle for box tools).
fn draw_preview(painter: &egui::Painter, tool: Tool, color: Color32, width: f32, a: Pos2, b: Pos2) {
    let stroke = Stroke::new(width, color);
    match tool {
        Tool::Line => {
            painter.line_segment([a, b], stroke);
        }
        Tool::Arrow => {
            painter.line_segment([a, b], stroke);
            let d = b - a;
            if d.length() > 1.0 {
                let u = d / d.length();
                let n = Vec2::new(-u.y, u.x);
                let head = (width * 4.0).max(10.0);
                painter.line_segment([b, b - u * head + n * head * 0.5], stroke);
                painter.line_segment([b, b - u * head - n * head * 0.5], stroke);
            }
        }
        Tool::Ellipse => {
            let r = Rect::from_two_pos(a, b);
            let pts: Vec<Pos2> = (0..=48)
                .map(|i| {
                    let t = i as f32 / 48.0 * std::f32::consts::TAU;
                    r.center() + Vec2::new(t.cos() * r.width() / 2.0, t.sin() * r.height() / 2.0)
                })
                .collect();
            painter.add(egui::Shape::line(pts, stroke));
        }
        Tool::Rect => {
            painter.rect_stroke(
                Rect::from_two_pos(a, b),
                0.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Tool::Highlight => {
            painter.rect_filled(
                Rect::from_two_pos(a, b),
                0.0,
                Color32::from_rgba_unmultiplied(255, 230, 0, 90),
            );
        }
        _ => {
            let r = Rect::from_two_pos(a, b);
            painter.rect_filled(r, 0.0, crate::theme::ACCENT.gamma_multiply(0.15));
            painter.rect_stroke(
                r,
                0.0,
                Stroke::new(1.5, crate::theme::ACCENT2),
                egui::StrokeKind::Outside,
            );
        }
    }
}
