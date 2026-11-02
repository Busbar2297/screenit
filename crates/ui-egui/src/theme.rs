//! Visual identity: a dark, layered palette with a violet→cyan accent, plus the custom widgets
//! (icon buttons, gradient buttons, tiles) every view is built from.

use crate::icons::{self, Icon};
use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shadow, Stroke, TextStyle,
    Ui, Vec2, epaint::Mesh,
};

pub const BG: Color32 = Color32::from_rgb(13, 15, 21);
pub const CANVAS: Color32 = Color32::from_rgb(9, 10, 14);
pub const SURFACE: Color32 = Color32::from_rgb(21, 24, 33);
pub const SURFACE2: Color32 = Color32::from_rgb(30, 34, 46);
pub const SURFACE3: Color32 = Color32::from_rgb(40, 45, 60);
pub const BORDER: Color32 = Color32::from_rgb(43, 48, 64);
pub const TEXT: Color32 = Color32::from_rgb(232, 234, 242);
pub const MUTED: Color32 = Color32::from_rgb(138, 144, 164);
pub const ACCENT: Color32 = Color32::from_rgb(124, 92, 255);
pub const ACCENT2: Color32 = Color32::from_rgb(34, 211, 238);
pub const OK: Color32 = Color32::from_rgb(52, 211, 153);

/// Install the screenIt look on a context (dark only).
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        let v = &mut s.visuals;
        v.dark_mode = true;
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.extreme_bg_color = CANVAS;
        v.faint_bg_color = SURFACE;
        v.code_bg_color = SURFACE2;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        v.window_shadow = Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(140),
        };
        v.popup_shadow = Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(120),
        };
        v.selection.bg_fill = ACCENT.gamma_multiply(0.55);
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.hyperlink_color = ACCENT2;
        v.override_text_color = None;
        v.slider_trailing_fill = true;
        let w = &mut v.widgets;
        let r = CornerRadius::same(8);
        w.noninteractive.bg_fill = SURFACE;
        w.noninteractive.weak_bg_fill = SURFACE;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
        w.noninteractive.corner_radius = r;
        w.inactive.bg_fill = SURFACE2;
        w.inactive.weak_bg_fill = SURFACE2;
        w.inactive.bg_stroke = Stroke::NONE;
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = r;
        w.hovered.bg_fill = SURFACE3;
        w.hovered.weak_bg_fill = SURFACE3;
        w.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.6));
        w.hovered.fg_stroke = Stroke::new(1.5, TEXT);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;
        w.active.bg_fill = ACCENT;
        w.active.weak_bg_fill = ACCENT;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        w.active.corner_radius = r;
        w.active.expansion = 0.0;
        w.open = w.hovered;

        let sp = &mut s.spacing;
        sp.item_spacing = Vec2::new(8.0, 8.0);
        sp.button_padding = Vec2::new(12.0, 6.0);
        sp.interact_size = Vec2::new(36.0, 30.0);
        sp.slider_width = 150.0;
        sp.slider_rail_height = 4.0;
        sp.window_margin = egui::Margin::same(12);
        sp.menu_margin = egui::Margin::same(8);

        s.text_styles = [
            (TextStyle::Heading, FontId::proportional(20.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
    });
}

/// Frame for a panel: flat fill, a hairline border on the inner edge, generous padding.
pub fn panel_frame(fill: Color32, margin: i8) -> egui::Frame {
    egui::Frame::default()
        .fill(fill)
        .inner_margin(egui::Margin::same(margin))
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        m(a.r(), b.r()),
        m(a.g(), b.g()),
        m(a.b(), b.b()),
        m(a.a(), b.a()),
    )
}

/// Rounded rectangle filled with a diagonal two-colour gradient.
pub fn gradient_rect(painter: &egui::Painter, rect: Rect, radius: f32, a: Color32, b: Color32) {
    let r = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    let mut outline: Vec<Pos2> = Vec::with_capacity(4 * 9);
    let corners = [
        (Pos2::new(rect.max.x - r, rect.min.y + r), -90.0f32),
        (Pos2::new(rect.max.x - r, rect.max.y - r), 0.0),
        (Pos2::new(rect.min.x + r, rect.max.y - r), 90.0),
        (Pos2::new(rect.min.x + r, rect.min.y + r), 180.0),
    ];
    for (c, start) in corners {
        for i in 0..=8 {
            let t = (start + i as f32 * 90.0 / 8.0).to_radians();
            outline.push(c + Vec2::new(t.cos(), t.sin()) * r);
        }
    }
    let span = (rect.width() + rect.height()).max(1.0);
    let color_at = |p: Pos2| {
        lerp(
            a,
            b,
            ((p.x - rect.min.x) + (p.y - rect.min.y) * 0.6) / span * 1.25,
        )
    };
    let mut mesh = Mesh::default();
    let center = rect.center();
    mesh.colored_vertex(center, color_at(center));
    for p in &outline {
        mesh.colored_vertex(*p, color_at(*p));
    }
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(mesh);
    // Anti-aliased edge: the mesh itself has hard edges.
    painter.add(egui::Shape::closed_line(
        outline,
        Stroke::new(1.0, lerp(a, b, 0.5).gamma_multiply(0.6)),
    ));
}

/// Square icon button used in tool palettes and toolbars.
pub fn icon_button(ui: &mut Ui, icon: Icon, tip: &str, selected: bool, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hovered = resp.hovered();
        if selected {
            gradient_rect(p, rect, 8.0, ACCENT, lerp(ACCENT, ACCENT2, 0.55));
        } else if hovered {
            p.rect_filled(rect, 8.0, SURFACE3);
        }
        let fg = if selected {
            Color32::WHITE
        } else if hovered {
            TEXT
        } else {
            MUTED.gamma_multiply(1.15)
        };
        icons::paint(p, icon, rect.shrink(size * 0.24), fg);
    }
    resp.on_hover_text(tip)
}

/// Rounded button with an icon and a label. `primary` buttons get the accent gradient.
pub fn pill(ui: &mut Ui, icon: Icon, label: &str, primary: bool) -> Response {
    pill_enabled(ui, icon, label, primary, true)
}

pub fn pill_enabled(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    primary: bool,
    enabled: bool,
) -> Response {
    let font = FontId::proportional(13.5);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, TEXT);
    let h = 32.0;
    let icon_w = 16.0;
    let w = 12.0 + icon_w + 8.0 + galley.size().x + 14.0;
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), sense);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hovered = enabled && resp.hovered();
        if primary && enabled {
            let (a, b) = if hovered {
                (
                    lerp(ACCENT, Color32::WHITE, 0.12),
                    lerp(ACCENT2, Color32::WHITE, 0.1),
                )
            } else {
                (ACCENT, lerp(ACCENT, ACCENT2, 0.7))
            };
            gradient_rect(p, rect, 9.0, a, b);
        } else {
            let fill = if hovered { SURFACE3 } else { SURFACE2 };
            p.rect_filled(rect, 9.0, fill);
            if hovered {
                p.rect_stroke(
                    rect,
                    9.0,
                    Stroke::new(1.0, ACCENT.gamma_multiply(0.5)),
                    egui::StrokeKind::Inside,
                );
            }
        }
        let fg = if !enabled {
            MUTED.gamma_multiply(0.5)
        } else if primary {
            Color32::WHITE
        } else {
            TEXT
        };
        // Shaped Arabic is stored in visual order. The icon sits at the start of the label:
        // the left in LTR, the right in RTL, with the text growing away from it.
        let rtl = screenit_i18n::is_rtl();
        let icon_cx = if rtl {
            rect.max.x - 12.0 - icon_w / 2.0
        } else {
            rect.min.x + 12.0 + icon_w / 2.0
        };
        let ir = Rect::from_center_size(Pos2::new(icon_cx, rect.center().y), Vec2::splat(icon_w));
        icons::paint(p, icon, ir, fg);
        let text_x = if rtl {
            ir.min.x - 8.0 - galley.size().x
        } else {
            ir.max.x + 8.0
        };
        p.galley(
            Pos2::new(text_x, rect.center().y - galley.size().y / 2.0),
            galley,
            fg,
        );
    }
    resp
}

/// Large launcher tile: icon on top, label and a muted hint below.
pub fn tile(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    hint: &str,
    size: Vec2,
    enabled: bool,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hovered = enabled && resp.hovered();
        p.rect_filled(rect, 12.0, if hovered { SURFACE3 } else { SURFACE2 });
        p.rect_stroke(
            rect,
            12.0,
            Stroke::new(
                1.0,
                if hovered {
                    ACCENT.gamma_multiply(0.7)
                } else {
                    BORDER
                },
            ),
            egui::StrokeKind::Inside,
        );
        let fg = if enabled {
            TEXT
        } else {
            MUTED.gamma_multiply(0.5)
        };
        let ic = Rect::from_center_size(
            Pos2::new(rect.center().x, rect.min.y + size.y * 0.36),
            Vec2::splat(24.0),
        );
        icons::paint(p, icon, ic, if hovered { ACCENT2 } else { fg });
        p.text(
            Pos2::new(rect.center().x, rect.min.y + size.y * 0.66),
            egui::Align2::CENTER_CENTER,
            label,
            FontId::proportional(13.0),
            fg,
        );
        p.text(
            Pos2::new(rect.center().x, rect.min.y + size.y * 0.84),
            egui::Align2::CENTER_CENTER,
            hint,
            FontId::proportional(10.5),
            MUTED,
        );
    }
    resp
}

/// Small rounded "keycap" label, e.g. for showing a shortcut.
pub fn keycap(painter: &egui::Painter, center: Pos2, text: &str, fg: Color32, bg: Color32) {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(11.0), fg);
    let r = Rect::from_center_size(center, galley.size() + Vec2::new(12.0, 6.0));
    painter.rect_filled(r, 5.0, bg);
    painter.galley(r.center() - galley.size() / 2.0, galley, fg);
}

/// The screenIt brand mark: gradient rounded square with a capture reticle.
pub fn brand_mark(painter: &egui::Painter, rect: Rect) {
    gradient_rect(painter, rect, rect.width() * 0.28, ACCENT, ACCENT2);
    icons::paint(
        painter,
        Icon::Capture,
        rect.shrink(rect.width() * 0.22),
        Color32::WHITE,
    );
}

/// Section caption in small caps style.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .size(10.5)
            .color(MUTED)
            .strong(),
    );
}
