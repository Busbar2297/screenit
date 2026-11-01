//! The document canvas: shows the flattened document and turns drags into engine commands.

use crate::{Tool, UiApp};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use serde_json::json;

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
            k.clamp(0.1, 1.0)
        } else {
            1.0
        };
    }
    let (wheel, ctrl) = ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.command));
    if ctrl && wheel != 0.0 && ui.rect_contains_pointer(ui.max_rect()) {
        app.zoom = (app.zoom * (1.0 + wheel * 0.002)).clamp(0.1, 8.0);
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
        let origin = img_rect.min;
        painter.image(
            tex.id(),
            img_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        if resp.hovered() && app.tool != Tool::Select {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        let zoom = scale;
        let to_img = |p: Pos2| -> Pos2 { ((p - origin) / zoom).to_pos2() };
        let to_screen = |p: Pos2| -> Pos2 { origin + p.to_vec2() * zoom };

        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                app.drag = Some(to_img(p));
                app.pen.clear();
                if app.tool == Tool::Select {
                    let hit = app.run(
                        "editor.object.hit_test",
                        json!({"point": [to_img(p).x, to_img(p).y]}),
                    );
                    app.selected = hit.and_then(|v| v["id"].as_u64());
                }
            }
        }
        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let now = to_img(p);
                match app.tool {
                    Tool::Pen => {
                        app.pen.push([now.x, now.y]);
                        let pts: Vec<Pos2> = app
                            .pen
                            .iter()
                            .map(|p| to_screen(Pos2::new(p[0], p[1])))
                            .collect();
                        painter.add(egui::Shape::line(
                            pts,
                            Stroke::new((app.width * zoom).max(1.0), app.stroke),
                        ));
                    }
                    Tool::Select => {
                        let d = resp.drag_delta() / zoom;
                        if let (Some(id), true) = (app.selected, d != Vec2::ZERO) {
                            app.run(
                                "editor.object.move",
                                json!({"id": id, "dx": d.x, "dy": d.y, "coalesce": "ui-drag"}),
                            );
                        }
                    }
                    _ => {
                        if let Some(start) = app.drag {
                            draw_preview(
                                &painter,
                                app.tool,
                                app.stroke,
                                (app.width * zoom).max(1.0),
                                to_screen(start),
                                to_screen(now),
                            );
                        }
                    }
                }
            }
        }
        if resp.drag_stopped() {
            if let (Some(start), Some(p)) = (app.drag.take(), resp.interact_pointer_pos()) {
                finish_drag(app, start, to_img(p));
            }
        } else if resp.clicked() && matches!(app.tool, Tool::Text | Tool::Step) {
            if let Some(p) = resp.interact_pointer_pos() {
                finish_drag(app, to_img(p), to_img(p));
            }
        }
    });
}

fn finish_drag(app: &mut UiApp, a: Pos2, b: Pos2) {
    let style = json!({"stroke": app.stroke_hex(), "width": app.width});
    let rect = {
        let r = Rect::from_two_pos(a, b);
        json!([r.min.x, r.min.y, r.width(), r.height()])
    };
    let moved = (b - a).length() > 2.0;
    match app.tool {
        Tool::Arrow if moved => {
            app.run(
                "editor.arrow.add",
                json!({"from": [a.x, a.y], "to": [b.x, b.y], "style": style}),
            );
        }
        Tool::Line if moved => {
            app.run(
                "editor.line.add",
                json!({"from": [a.x, a.y], "to": [b.x, b.y], "style": style}),
            );
        }
        Tool::Rect if moved => {
            app.run("editor.rect.add", json!({"rect": rect, "style": style}));
        }
        Tool::Ellipse if moved => {
            app.run("editor.ellipse.add", json!({"rect": rect, "style": style}));
        }
        Tool::Text => {
            let size = (app.width * 6.0).max(12.0);
            let text = app.text.clone();
            app.run(
                "editor.text.add",
                json!({"pos": [a.x, a.y], "text": text, "size": size, "style": style}),
            );
        }
        Tool::Step => {
            app.run(
                "editor.step.add",
                json!({"center": [a.x, a.y], "style": style}),
            );
        }
        Tool::Blur if moved => {
            app.run(
                "editor.blur.add",
                json!({"rect": rect, "radius": 10, "pixelate": false}),
            );
        }
        Tool::Highlight if moved => {
            app.run("editor.highlight.add", json!({"rect": rect}));
        }
        Tool::Crop if moved => {
            app.run("editor.crop", json!({"rect": rect}));
        }
        Tool::Pen if app.pen.len() > 1 => {
            let pts = std::mem::take(&mut app.pen);
            app.run("editor.pen.add", json!({"points": pts, "style": style}));
        }
        _ => {}
    }
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
