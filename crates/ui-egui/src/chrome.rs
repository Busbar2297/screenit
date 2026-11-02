//! Window chrome for every view: the compact capture launcher, the editor's top bar, tool palette,
//! properties panel and status bar.

use crate::icons::Icon;
use crate::theme::{self, ACCENT, ACCENT2, BORDER, MUTED, OK, SURFACE, SURFACE2, TEXT};
use crate::{Pending, Tool, UiApp, View, ask_save_path, default_output};

fn file_name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default()
}
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use screenit_i18n::t;
use serde_json::json;

const SWATCHES: [Color32; 10] = [
    Color32::from_rgb(239, 68, 68),
    Color32::from_rgb(249, 115, 22),
    Color32::from_rgb(250, 204, 21),
    Color32::from_rgb(34, 197, 94),
    Color32::from_rgb(34, 211, 238),
    Color32::from_rgb(59, 130, 246),
    Color32::from_rgb(124, 92, 255),
    Color32::from_rgb(236, 72, 153),
    Color32::from_rgb(255, 255, 255),
    Color32::from_rgb(17, 17, 17),
];

impl UiApp {
    /// Snagit-style launcher: one big gradient Capture button plus quick-action tiles.
    pub(crate) fn capture_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::hover());
            theme::brand_mark(ui.painter(), r);
            ui.label(
                egui::RichText::new("screenIt")
                    .size(15.0)
                    .strong()
                    .color(TEXT),
            );
            ui.label(
                egui::RichText::new(t("bar.capture"))
                    .size(13.0)
                    .color(MUTED),
            );
        });
        ui.add_space(6.0);
        crate::h_row(ui, |ui| {
            let size = Vec2::new(200.0, 104.0);
            let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
            let hovered = resp.hovered();
            let p = ui.painter();
            let (a, b) = if hovered {
                (ACCENT.gamma_multiply(1.0), ACCENT2)
            } else {
                (ACCENT, Color32::from_rgb(80, 150, 247))
            };
            p.add(
                egui::Shadow {
                    offset: [0, 6],
                    blur: 18,
                    spread: 0,
                    color: ACCENT.gamma_multiply(if hovered { 0.45 } else { 0.25 }),
                }
                .as_shape(rect, 14),
            );
            theme::gradient_rect(p, rect, 14.0, a, b);
            crate::icons::paint(
                p,
                Icon::Capture,
                Rect::from_center_size(
                    rect.left_center() + Vec2::new(40.0, -8.0),
                    Vec2::splat(34.0),
                ),
                Color32::WHITE,
            );
            p.text(
                rect.left_center() + Vec2::new(74.0, -12.0),
                egui::Align2::LEFT_CENTER,
                t("bar.capture"),
                egui::FontId::proportional(22.0),
                Color32::WHITE,
            );
            p.text(
                rect.left_center() + Vec2::new(75.0, 12.0),
                egui::Align2::LEFT_CENTER,
                t("bar.capture.sub"),
                egui::FontId::proportional(11.5),
                Color32::from_white_alpha(200),
            );
            theme::keycap(
                p,
                rect.right_bottom() + Vec2::new(-30.0, -16.0),
                "PrtSc",
                Color32::WHITE,
                Color32::from_black_alpha(70),
            );
            if resp.on_hover_text(t("bar.capture.tip")).clicked() {
                self.start_capture(ctx, Pending::Overlay, None);
            }
            let ts = Vec2::new(86.0, 104.0);
            let rec = theme::tile(
                ui,
                Icon::Record,
                &t("bar.record"),
                &t("bar.record.sub"),
                ts,
                true,
            );
            ui.painter().circle_filled(
                Pos2::new(rec.rect.center().x, rec.rect.min.y + ts.y * 0.36),
                6.0,
                crate::recording::REC_RED,
            );
            if rec.on_hover_text(t("bar.record.tip")).clicked() {
                self.start_capture(ctx, Pending::RecordOverlay, None);
            }
            if theme::tile(
                ui,
                Icon::Fullscreen,
                &t("bar.fullscreen"),
                "Ctrl+Shift+F",
                ts,
                true,
            )
            .clicked()
            {
                self.start_capture(ctx, Pending::Fullscreen, None);
            }
            if theme::tile(
                ui,
                Icon::Text,
                &t("bar.grab_text"),
                &t("bar.grab_text.sub"),
                ts,
                true,
            )
            .on_hover_text(t("bar.grab_text.tip"))
            .clicked()
            {
                self.start_capture(ctx, Pending::TextOverlay, None);
            }
            if theme::tile(
                ui,
                Icon::Clipboard,
                &t("bar.paste"),
                &t("bar.paste.sub"),
                ts,
                true,
            )
            .clicked()
                && self.run("capture.import.clipboard", json!({})).is_some()
            {
                self.fit = true;
                self.enter_view(ctx, View::Editor);
            }
            if theme::tile(
                ui,
                Icon::Library,
                &t("bar.library"),
                &t("bar.library.sub"),
                ts,
                true,
            )
            .clicked()
            {
                self.enter_view(ctx, View::Library);
            }
            if theme::tile(
                ui,
                Icon::Gear,
                &t("bar.settings"),
                &t("bar.settings.sub"),
                ts,
                true,
            )
            .clicked()
            {
                self.open_settings(ctx);
            }
            let has_doc = self.session.doc.is_some();
            if theme::tile(
                ui,
                Icon::Editor,
                &t("bar.editor"),
                &t("bar.editor.sub"),
                ts,
                has_doc,
            )
            .clicked()
            {
                self.enter_view(ctx, View::Editor);
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(t("bar.video")).size(11.5).color(MUTED));
            ui.add_space(2.0);
            self.record_options(ui);
        });
        ui.add_space(4.0);
        self.status_line(ui);
    }

    fn status_line(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 3.5, OK);
            ui.label(egui::RichText::new(&self.status).size(11.5).color(MUTED));
            self.reveal_link(ui);
        });
    }

    /// Editor / library top bar: brand, capture actions, history, output actions.
    pub(crate) fn top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        crate::h_row(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (r, _) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::hover());
            theme::brand_mark(ui.painter(), r);
            ui.label(egui::RichText::new("screenIt").size(15.0).strong());
            ui.add_space(10.0);
            if theme::pill(ui, Icon::Capture, &t("top.new_capture"), true)
                .on_hover_text("Print Screen")
                .clicked()
            {
                self.start_capture(ctx, Pending::Overlay, None);
            }
            if theme::icon_button(ui, Icon::Record, &t("top.record"), false, 32.0).clicked() {
                self.start_capture(ctx, Pending::RecordOverlay, None);
            }
            if theme::icon_button(ui, Icon::Fullscreen, &t("top.fullscreen"), false, 32.0).clicked()
            {
                self.start_capture(ctx, Pending::Fullscreen, None);
            }
            if theme::icon_button(ui, Icon::Text, &t("top.grab_text"), false, 32.0).clicked() {
                self.start_capture(ctx, Pending::TextOverlay, None);
            }
            if theme::icon_button(ui, Icon::Clipboard, &t("top.paste"), false, 32.0).clicked()
                && self.run("capture.import.clipboard", json!({})).is_some()
            {
                self.fit = true;
                self.status = t("status.imported");
                self.view = View::Editor;
            }
            separator(ui);
            let lib = self.view == View::Library;
            if theme::icon_button(ui, Icon::Library, &t("top.library"), lib, 32.0).clicked() {
                self.view = if lib && self.session.doc.is_some() {
                    View::Editor
                } else {
                    View::Library
                };
            }
            if theme::icon_button(
                ui,
                Icon::Editor,
                &t("top.editor"),
                self.view == View::Editor,
                32.0,
            )
            .clicked()
            {
                self.view = View::Editor;
            }
            if theme::icon_button(
                ui,
                Icon::Gear,
                &t("top.settings"),
                self.view == View::Settings,
                32.0,
            )
            .clicked()
            {
                self.open_settings(ctx);
            }
            if theme::pill(ui, Icon::Compact, &t("top.capture_bar"), false)
                .on_hover_text(t("top.capture_bar.tip"))
                .clicked()
            {
                self.enter_view(ctx, View::Compact);
            }
            if self.view != View::Editor {
                return;
            }
            separator(ui);
            if theme::icon_button(ui, Icon::Undo, &t("top.undo"), false, 32.0).clicked() {
                self.run("edit.undo", json!({}));
            }
            if theme::icon_button(ui, Icon::Redo, &t("top.redo"), false, 32.0).clicked() {
                self.run("edit.redo", json!({}));
            }
            if self.session.doc.is_some()
                && theme::pill(ui, Icon::Text, &t("top.grab_image"), false)
                    .on_hover_text(t("top.ocr"))
                    .clicked()
            {
                self.reading = None;
                self.grab_document_text(ctx);
            }
            ui.with_layout(crate::far_layout(), |ui| {
                let has_doc = self.session.doc.is_some();
                let video = self.video.as_ref().map(|v| v.path.clone());
                if let Some(src) = &video {
                    if theme::pill(ui, Icon::Export, &t("top.export_mp4"), true)
                        .on_hover_text(t("top.export_mp4.tip"))
                        .clicked()
                    {
                        self.export_mp4(src);
                    }
                }
                let png = if video.is_some() {
                    t("top.export_frame")
                } else {
                    t("top.export_png")
                };
                if theme::pill_enabled(ui, Icon::Export, &png, video.is_none(), has_doc)
                    .on_hover_text(t("top.export.tip"))
                    .clicked()
                {
                    self.save_png();
                }
                if video.is_none()
                    && theme::pill_enabled(ui, Icon::Save, &t("top.save"), false, has_doc)
                        .on_hover_text(t("top.save.tip"))
                        .clicked()
                {
                    let name = file_name_of(&default_output(screenit_format::EXT));
                    if let Some(path) =
                        ask_save_path("Save capture", &name, ("screenIt capture", "*.scit"))
                        && self.run("doc.save", json!({"path": path})).is_some()
                    {
                        self.saved_to("Capture", &path);
                    }
                }
                if theme::pill_enabled(ui, Icon::Copy, &t("top.copy"), false, has_doc)
                    .on_hover_text("Ctrl+C")
                    .clicked()
                {
                    self.copy_doc();
                }
            });
        });
    }

    pub(crate) fn save_png(&mut self) {
        let name = file_name_of(&default_output("png"));
        if let Some(path) = ask_save_path("Export PNG", &name, ("PNG image", "*.png"))
            && self.run("doc.export", json!({"path": path})).is_some()
        {
            self.saved_to("PNG", &path);
        }
    }

    fn export_mp4(&mut self, src: &str) {
        let name = file_name_of(src);
        if let Some(path) = ask_save_path("Export MP4", &name, ("MP4 video", "*.mp4"))
            && self
                .run("video.export", json!({"path": src, "output": path}))
                .is_some()
        {
            self.saved_to("MP4", &path);
        }
    }

    /// "Show in folder" link for the last written file.
    fn reveal_link(&self, ui: &mut egui::Ui) {
        let Some(p) = &self.last_saved else { return };
        if theme::icon_button(ui, Icon::Folder, &t("top.show_folder"), false, 22.0).clicked() {
            screenit_platform::shell::reveal(p);
        }
    }

    pub(crate) fn copy_doc(&mut self) {
        if self.run("edit.copy", json!({})).is_some() {
            self.status = t("status.copied");
        }
    }

    /// Vertical tool palette on the left of the editor.
    pub(crate) fn tool_palette(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.add_space(4.0);
            for (i, (t, name, icon, key)) in Tool::ALL.iter().enumerate() {
                if i == 1 || i == 5 || i == 7 || i == 10 {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(28.0, 9.0), egui::Sense::hover());
                    ui.painter()
                        .hline(r.x_range(), r.center().y, Stroke::new(1.0, BORDER));
                }
                if theme::icon_button(
                    ui,
                    *icon,
                    &format!("{name}  ({key})"),
                    self.tool == *t,
                    38.0,
                )
                .clicked()
                {
                    self.tool = *t;
                }
            }
        });
    }

    /// Properties for the active tool, plus view controls.
    pub(crate) fn properties(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let name = Tool::ALL
            .iter()
            .find(|t| t.0 == self.tool)
            .map(|t| t.1)
            .unwrap_or("");
        ui.label(egui::RichText::new(name).size(16.0).strong());
        ui.add_space(8.0);
        if self.tool.uses_color() {
            theme::caption(ui, "Color");
            let mut chosen = None;
            egui::Grid::new("swatches")
                .spacing([6.0, 6.0])
                .show(ui, |ui| {
                    for (i, c) in SWATCHES.iter().enumerate() {
                        let (r, resp) =
                            ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::click());
                        let p = ui.painter();
                        let selected = self.stroke.to_opaque() == *c;
                        p.circle_filled(r.center(), 10.0, *c);
                        p.circle_stroke(
                            r.center(),
                            10.0,
                            Stroke::new(1.0, Color32::from_white_alpha(40)),
                        );
                        if selected || resp.hovered() {
                            p.circle_stroke(
                                r.center(),
                                13.0,
                                Stroke::new(2.0, if selected { ACCENT2 } else { MUTED }),
                            );
                        }
                        if resp.clicked() {
                            chosen = Some(*c);
                        }
                        if i % 5 == 4 {
                            ui.end_row();
                        }
                    }
                });
            if let Some(c) = chosen {
                self.stroke = c;
            }
            ui.horizontal(|ui| {
                ui.color_edit_button_srgba(&mut self.stroke);
                ui.label(
                    egui::RichText::new(self.stroke_hex())
                        .monospace()
                        .color(MUTED),
                );
            });
            ui.add_space(10.0);
            theme::caption(
                ui,
                if self.tool == Tool::Text {
                    "Size"
                } else {
                    "Thickness"
                },
            );
            ui.add(egui::Slider::new(&mut self.width, 1.0..=40.0).suffix(" px"));
            ui.add_space(10.0);
        }
        if self.tool == Tool::Text {
            theme::caption(ui, "Text");
            ui.add(
                egui::TextEdit::multiline(&mut self.text)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(10.0);
        }
        ui.label(
            egui::RichText::new(self.tool.hint())
                .size(12.0)
                .color(MUTED),
        );
        ui.add_space(16.0);
        theme::caption(ui, "View");
        ui.horizontal(|ui| {
            if theme::icon_button(ui, Icon::ZoomOut, "Zoom out", false, 30.0).clicked() {
                self.zoom = (self.zoom / 1.25).max(0.1);
                self.fit = false;
            }
            ui.label(egui::RichText::new(format!("{:.0}%", self.zoom * 100.0)).strong());
            if theme::icon_button(ui, Icon::ZoomIn, "Zoom in", false, 30.0).clicked() {
                self.zoom = (self.zoom * 1.25).min(8.0);
                self.fit = false;
            }
            if theme::icon_button(ui, Icon::Fit, "Fit to window", false, 30.0).clicked() {
                self.fit = true;
            }
        });
        if ui.button("Actual size (100%)").clicked() {
            self.zoom = 1.0;
            self.fit = false;
        }
    }

    /// Bottom status bar: message on the left, image size on the right.
    pub(crate) fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 3.5, OK);
            ui.label(egui::RichText::new(&self.status).size(12.0).color(MUTED));
            self.reveal_link(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(t) = &self.tex {
                    let [w, h] = t.size();
                    ui.label(
                        egui::RichText::new(format!(
                            "{w} × {h} px   ·   {:.0}%",
                            self.zoom * 100.0
                        ))
                        .size(12.0)
                        .color(MUTED),
                    );
                }
            });
        });
    }

    /// Editor keyboard shortcuts (ignored while a text field has focus).
    pub(crate) fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let (keys, ctrl, shift) = ctx.input(|i| {
            let keys: Vec<egui::Key> = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key, pressed: true, ..
                    } => Some(*key),
                    _ => None,
                })
                .collect();
            (keys, i.modifiers.command, i.modifiers.shift)
        });
        for k in keys {
            if ctrl {
                match k {
                    egui::Key::Z if shift => {
                        self.run("edit.redo", json!({}));
                    }
                    egui::Key::Z => {
                        self.run("edit.undo", json!({}));
                    }
                    egui::Key::Y => {
                        self.run("edit.redo", json!({}));
                    }
                    egui::Key::S => self.save_png(),
                    egui::Key::Num0 => self.fit = true,
                    egui::Key::Num1 => {
                        self.zoom = 1.0;
                        self.fit = false;
                    }
                    _ => {}
                }
                continue;
            }
            if let Some(t) = Tool::ALL
                .iter()
                .find(|t| egui::Key::from_name(t.3) == Some(k))
            {
                self.tool = t.0;
            }
        }
        if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
            self.copy_doc();
        }
    }
}

fn separator(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(9.0, 26.0), egui::Sense::hover());
    ui.painter()
        .vline(r.center().x, r.y_range(), Stroke::new(1.0, BORDER));
}

/// Paint a subtle drop shadow and border around the document on the canvas.
pub(crate) fn document_frame(painter: &egui::Painter, rect: Rect) {
    painter.add(
        egui::Shadow {
            offset: [0, 10],
            blur: 30,
            spread: 2,
            color: Color32::from_black_alpha(160),
        }
        .as_shape(rect, 2),
    );
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, Color32::from_white_alpha(18)),
        egui::StrokeKind::Outside,
    );
}

/// Dotted background for the empty canvas area.
pub(crate) fn canvas_backdrop(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(rect, 0.0, theme::CANVAS);
    let step = 22.0;
    let mut y = rect.min.y + step / 2.0;
    while y < rect.max.y {
        let mut x = rect.min.x + step / 2.0;
        while x < rect.max.x {
            painter.circle_filled(Pos2::new(x, y), 0.9, Color32::from_white_alpha(14));
            x += step;
        }
        y += step;
    }
}

/// Empty-state card shown when the editor has no image.
pub(crate) fn empty_state(ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    canvas_backdrop(ui.painter(), rect);
    let card = Rect::from_center_size(rect.center(), Vec2::new(380.0, 190.0));
    let p = ui.painter();
    p.rect_filled(card, 16.0, SURFACE);
    p.rect_stroke(
        card,
        16.0,
        Stroke::new(1.0, BORDER),
        egui::StrokeKind::Inside,
    );
    theme::brand_mark(
        p,
        Rect::from_center_size(card.center() - Vec2::new(0.0, 46.0), Vec2::splat(44.0)),
    );
    p.text(
        card.center() + Vec2::new(0.0, 6.0),
        egui::Align2::CENTER_CENTER,
        "No capture yet",
        egui::FontId::proportional(18.0),
        TEXT,
    );
    p.text(
        card.center() + Vec2::new(0.0, 32.0),
        egui::Align2::CENTER_CENTER,
        "Press Print Screen, or paste an image from the clipboard",
        egui::FontId::proportional(12.5),
        MUTED,
    );
    theme::keycap(
        p,
        card.center() + Vec2::new(0.0, 62.0),
        "PrtSc",
        TEXT,
        SURFACE2,
    );
}
