//! Full-screen capture overlay: shows the frozen desktop, dims it, highlights the selection or
//! the window under the cursor, and reports the chosen region.

use crate::UiApp;
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, ViewportBuilder, ViewportId};
use screenit_capture::{FrozenScreen, ScreenSource, Selection, window_at};
use screenit_geom::{Point, Rect as IRect};
use std::sync::Arc;

pub struct Overlay {
    pub frozen: Arc<FrozenScreen>,
    tex: egui::TextureHandle,
    lens: egui::TextureHandle,
    start: Option<Pos2>,
    focused: bool,
    /// Picking the area to record rather than to capture.
    pub record: bool,
    /// Picking the area to read text from.
    pub text: bool,
}

impl Overlay {
    pub fn new(ctx: &egui::Context, frozen: Arc<FrozenScreen>) -> Self {
        let img = frozen.image();
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            img.data(),
        );
        let tex = ctx.load_texture("frozen", ci.clone(), egui::TextureOptions::LINEAR);
        let lens = ctx.load_texture("frozen-lens", ci, egui::TextureOptions::NEAREST);
        Self {
            frozen,
            tex,
            lens,
            start: None,
            focused: false,
            record: false,
            text: false,
        }
    }
}

enum Outcome {
    None,
    Cancel,
    Pick(Selection),
}

/// Show the overlay viewport (if active) and apply its outcome.
pub fn drive(app: &mut UiApp, ctx: &egui::Context) {
    let Some(mut ov) = app.overlay.take() else {
        return;
    };
    let ppp = ctx.pixels_per_point().max(0.1);
    let b = ov.frozen.bounds();
    let builder = ViewportBuilder::default()
        .with_title("screenIt capture")
        .with_decorations(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_position(Pos2::new(b.x as f32 / ppp, b.y as f32 / ppp))
        .with_inner_size([b.w as f32 / ppp, b.h as f32 / ppp]);
    let mut outcome = Outcome::None;
    ctx.show_viewport_immediate(
        ViewportId::from_hash_of("scit-overlay"),
        builder,
        |ui, _class| {
            outcome = draw(&mut ov, ui);
        },
    );
    ctx.request_repaint();
    match outcome {
        Outcome::None => app.overlay = Some(ov),
        Outcome::Cancel => app.finish_overlay(ctx, ov.frozen, None, ov.record, ov.text),
        Outcome::Pick(sel) => app.finish_overlay(ctx, ov.frozen, Some(sel), ov.record, ov.text),
    }
}

fn draw(ov: &mut Overlay, ui: &mut egui::Ui) -> Outcome {
    let full = ui.max_rect();
    let b = ov.frozen.bounds();
    let to_phys = |p: Pos2| -> Point {
        let fx = (p.x - full.min.x) / full.width().max(1.0);
        let fy = (p.y - full.min.y) / full.height().max(1.0);
        Point::new(
            b.x + (fx * b.w as f32).round() as i32,
            b.y + (fy * b.h as f32).round() as i32,
        )
    };
    let to_screen = |r: IRect| -> Rect {
        let sx = full.width() / b.w as f32;
        let sy = full.height() / b.h as f32;
        Rect::from_min_size(
            Pos2::new(
                full.min.x + (r.x - b.x) as f32 * sx,
                full.min.y + (r.y - b.y) as f32 * sy,
            ),
            egui::vec2(r.w as f32 * sx, r.h as f32 * sy),
        )
    };
    let painter = ui.painter();
    let uv_all = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    painter.image(ov.tex.id(), full, uv_all, Color32::WHITE);
    painter.rect_filled(full, 0.0, Color32::from_rgba_unmultiplied(8, 6, 20, 140));

    if !ov.focused {
        ov.focused = true;
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
    }
    let (hover, pressed, released, esc, enter) = ui.input(|i| {
        (
            i.pointer.hover_pos().or(i.pointer.latest_pos()),
            i.pointer.primary_pressed(),
            i.pointer.primary_released(),
            i.key_pressed(egui::Key::Escape) || i.pointer.secondary_pressed(),
            i.key_pressed(egui::Key::Enter),
        )
    });
    // Escape works even when the overlay never received keyboard focus (global key state).
    if esc || screenit_platform::input::escape_down() {
        return Outcome::Cancel;
    }
    let Some(pos) = hover else {
        return Outcome::None;
    };
    if enter && ov.record {
        let at = to_phys(pos);
        let rect = ov
            .frozen
            .monitors()
            .ok()
            .and_then(|ms| ms.into_iter().find(|m| m.rect.contains(at)).map(|m| m.rect))
            .unwrap_or(b);
        return Outcome::Pick(Selection::Region { rect });
    }
    if pressed {
        ov.start = Some(pos);
    }

    // Selection rectangle (drag) or window under the cursor (hover).
    let dragging = ov.start.filter(|s| (pos - *s).length() > 4.0);
    let (hl, window_id): (Option<IRect>, Option<u64>) = if let Some(s) = dragging {
        let (a, c) = (to_phys(s), to_phys(pos));
        (Some(IRect::from_points(a, c)), None)
    } else {
        let wins = ov.frozen.windows_snapshot();
        match window_at(wins, to_phys(pos)) {
            Some(w) => (Some(w.rect), Some(w.id)),
            None => (None, None),
        }
    };
    let accent = if ov.record {
        crate::recording::REC_RED
    } else {
        crate::theme::ACCENT2
    };
    if let Some(r) = hl {
        let sr = to_screen(r);
        let sub = Rect::from_min_max(
            Pos2::new(
                (sr.min.x - full.min.x) / full.width(),
                (sr.min.y - full.min.y) / full.height(),
            ),
            Pos2::new(
                (sr.max.x - full.min.x) / full.width(),
                (sr.max.y - full.min.y) / full.height(),
            ),
        );
        painter.image(ov.tex.id(), sr, sub, Color32::WHITE);
        painter.rect_stroke(sr, 0.0, Stroke::new(2.0, accent), egui::StrokeKind::Outside);
        if dragging.is_some() {
            for c in [
                sr.left_top(),
                sr.right_top(),
                sr.left_bottom(),
                sr.right_bottom(),
            ] {
                painter.circle_filled(c, 5.0, Color32::WHITE);
                painter.circle_stroke(c, 5.0, Stroke::new(2.0, accent));
            }
        }
        let label = format!("{} × {}", r.w, r.h);
        let font = egui::FontId::proportional(12.5);
        let g = painter.layout_no_wrap(label, font, Color32::WHITE);
        let above = sr.min.y - full.min.y > 30.0;
        let at = if above {
            sr.min + egui::vec2(0.0, -28.0)
        } else {
            sr.min + egui::vec2(6.0, 6.0)
        };
        let pill = Rect::from_min_size(at, g.size() + egui::vec2(16.0, 8.0));
        painter.rect_filled(pill, 6.0, Color32::from_rgba_unmultiplied(15, 17, 24, 230));
        painter.galley(pill.min + egui::vec2(8.0, 4.0), g, Color32::WHITE);
    }
    // Crosshair.
    let line = Stroke::new(1.0, accent.gamma_multiply(0.75));
    painter.hline(full.x_range(), pos.y, line);
    painter.vline(pos.x, full.y_range(), line);

    // Hint pill at the top centre.
    {
        let text = if ov.record {
            "Record: drag an area  ·  Click a window  ·  Enter for the whole screen  ·  Esc to cancel"
        } else {
            "Drag to select a region  ·  Click a window  ·  Esc or right-click to cancel"
        };
        let g = painter.layout_no_wrap(
            text.into(),
            egui::FontId::proportional(13.0),
            Color32::WHITE,
        );
        let pill = Rect::from_center_size(
            Pos2::new(full.center().x, full.min.y + 34.0),
            g.size() + egui::vec2(28.0, 14.0),
        );
        if !pill.expand(30.0).contains(pos) {
            painter.rect_filled(pill, 10.0, Color32::from_rgba_unmultiplied(15, 17, 24, 225));
            painter.rect_stroke(
                pill,
                10.0,
                Stroke::new(1.0, crate::theme::ACCENT.gamma_multiply(0.7)),
                egui::StrokeKind::Inside,
            );
            painter.galley(pill.min + egui::vec2(14.0, 7.0), g, Color32::WHITE);
        }
    }

    // Magnifier next to the cursor: 8x zoom of the pixels under it, with the physical position.
    {
        let lens = 120.0;
        let px = 15.0; // source pixels across the lens
        let fx = (pos.x - full.min.x) / full.width().max(1.0);
        let fy = (pos.y - full.min.y) / full.height().max(1.0);
        let hw = px / 2.0 / b.w.max(1) as f32;
        let hh = px / 2.0 / b.h.max(1) as f32;
        let uv = Rect::from_min_max(Pos2::new(fx - hw, fy - hh), Pos2::new(fx + hw, fy + hh));
        let mut at = pos + egui::vec2(24.0, 24.0);
        if at.x + lens > full.max.x {
            at.x = pos.x - 24.0 - lens;
        }
        if at.y + lens + 26.0 > full.max.y {
            at.y = pos.y - 24.0 - lens - 26.0;
        }
        let r = Rect::from_min_size(at, egui::vec2(lens, lens));
        painter.rect_filled(
            r.expand(3.0),
            10.0,
            Color32::from_rgba_unmultiplied(15, 17, 24, 240),
        );
        painter.image(ov.lens.id(), r, uv, Color32::WHITE);
        let cell = lens / px;
        painter.rect_stroke(
            Rect::from_center_size(r.center(), egui::vec2(cell, cell)),
            0.0,
            Stroke::new(1.0, accent),
            egui::StrokeKind::Middle,
        );
        painter.rect_stroke(
            r.expand(3.0),
            10.0,
            Stroke::new(1.0, crate::theme::BORDER),
            egui::StrokeKind::Inside,
        );
        let p = to_phys(pos);
        painter.text(
            Pos2::new(r.center().x, r.max.y + 15.0),
            egui::Align2::CENTER_CENTER,
            format!("{}, {}", p.x, p.y),
            egui::FontId::proportional(11.5),
            Color32::WHITE,
        );
    }
    if released {
        let started = ov.start.take();
        if let Some(r) = hl {
            if started.is_some_and(|s| (pos - s).length() > 4.0) && r.w > 0 && r.h > 0 {
                return Outcome::Pick(Selection::Region { rect: r });
            }
            if let Some(id) = window_id {
                return Outcome::Pick(Selection::Window { id });
            }
        }
    }
    Outcome::None
}
