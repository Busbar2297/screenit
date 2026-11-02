//! Library view (captures grouped by day with thumbnails) and the video transport bar.

use crate::icons::{self, Icon};
use crate::{UiApp, View, theme};
use eframe::egui;
use serde_json::{Value, json};
use std::time::Instant;

pub(crate) struct VideoPlayer {
    pub path: String,
    pub duration_ms: u64,
    pub pos_ms: u64,
    pub playing: bool,
    pub last_tick: Instant,
    shown_ms: Option<u64>,
}

impl VideoPlayer {
    pub(crate) fn new(path: String, duration_ms: u64) -> Self {
        Self {
            path,
            duration_ms: duration_ms.max(1),
            pos_ms: 0,
            playing: false,
            last_tick: Instant::now(),
            shown_ms: None,
        }
    }

    /// Advance the playhead by wall-clock time while playing; stops at the end.
    pub(crate) fn tick(&mut self, now: Instant) {
        if self.playing {
            let dt = now.saturating_duration_since(self.last_tick).as_millis() as u64;
            self.pos_ms += dt;
            if self.pos_ms >= self.duration_ms {
                self.pos_ms = self.duration_ms - 1;
                self.playing = false;
            }
        }
        self.last_tick = now;
    }
}

/// Thumbnail box in logical points.
const THUMB_W: f32 = 220.0;
const THUMB_H: f32 = 138.0;

fn load_thumb(app: &UiApp, e: &Value) -> Option<egui::ColorImage> {
    if e["kind"].as_str() == Some("text") {
        return None;
    }
    let path = std::path::PathBuf::from(e["path"].as_str()?);
    let img = if e["kind"].as_str() == Some("video") {
        app.session
            .services
            .video
            .clone()?
            .frame_at(&path, 0)
            .ok()?
    } else if screenit_format::is_project(&path) {
        screenit_compose::flatten(&screenit_format::load(&path).ok()?)
    } else {
        screenit_codecs::decode(&std::fs::read(&path).ok()?).ok()?
    };
    let k = (THUMB_W * 2.0 / img.width().max(1) as f32)
        .min(THUMB_H * 2.0 / img.height().max(1) as f32)
        .min(1.0);
    let (w, h) = (
        ((img.width() as f32 * k) as u32).max(1),
        ((img.height() as f32 * k) as u32).max(1),
    );
    let small = screenit_algo::resize(&img, w, h).ok()?;
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        small.data(),
    ))
}

/// Local calendar day and clock time of a unix timestamp.
pub(crate) fn day_and_time(secs: i64) -> (String, String) {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|d| d.with_timezone(&chrono::Local))
        .map(|d| {
            (
                d.format("%A, %d %B %Y").to_string(),
                d.format("%H:%M:%S").to_string(),
            )
        })
        .unwrap_or_else(|| ("Unknown date".into(), String::new()))
}

/// Group entries (already newest first) by local day, keeping order.
pub(crate) fn group_by_day(entries: &[Value]) -> Vec<(String, Vec<&Value>)> {
    let mut groups: Vec<(String, Vec<&Value>)> = Vec::new();
    for e in entries {
        let (day, _) = day_and_time(e["created"].as_i64().unwrap_or(0));
        match groups.last_mut() {
            Some((d, v)) if *d == day => v.push(e),
            _ => groups.push((day, vec![e])),
        }
    }
    groups
}

impl UiApp {
    /// Full library view: captures grouped by day, newest first, with thumbnails and capture time.
    pub(crate) fn library_view(&mut self, ui: &mut egui::Ui) {
        let params = if self.lib_query.is_empty() {
            json!({})
        } else {
            json!({"text": self.lib_query})
        };
        let entries = self
            .session
            .dispatch("library.list", &params)
            .ok()
            .and_then(|v| v.get("entries").cloned())
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(screenit_i18n::t("lib.title"))
                    .size(24.0)
                    .strong(),
            );
            ui.label(
                egui::RichText::new(screenit_i18n::tf(
                    "lib.count",
                    &[("n", &entries.len().to_string())],
                ))
                .size(13.0)
                .color(theme::MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::Frame::default()
                    .fill(theme::SURFACE2)
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (r, _) = ui
                                .allocate_exact_size(egui::Vec2::splat(16.0), egui::Sense::hover());
                            icons::paint(ui.painter(), Icon::Search, r, theme::MUTED);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.lib_query)
                                    .hint_text(screenit_i18n::t("lib.search"))
                                    .frame(egui::Frame::NONE)
                                    .desired_width(240.0),
                            );
                        });
                    });
            });
        });
        ui.add_space(10.0);
        if entries.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(screenit_i18n::t("lib.empty"))
                        .size(18.0)
                        .strong(),
                );
                ui.label(
                    egui::RichText::new(
                        "Captures are filed here by day and time as you take them.",
                    )
                    .color(theme::MUTED),
                );
            });
        }
        let mut action: Option<(&'static str, u64)> = None;
        let mut budget = 4; // thumbnails decoded per frame, to keep the UI responsive
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (day, items) in group_by_day(&entries) {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(day).size(15.0).strong());
                        ui.label(
                            egui::RichText::new(format!("{}", items.len()))
                                .size(12.0)
                                .color(theme::MUTED),
                        );
                    });
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(14.0, 14.0);
                        for e in items {
                            let id = e["id"].as_u64().unwrap_or(0);
                            if !self.thumbs.contains_key(&id) {
                                if budget == 0 {
                                    ctx.request_repaint();
                                } else {
                                    budget -= 1;
                                    let tex = load_thumb(self, e).map(|ci| {
                                        ctx.load_texture(
                                            format!("thumb-{id}"),
                                            ci,
                                            egui::TextureOptions::LINEAR,
                                        )
                                    });
                                    self.thumbs.insert(id, tex);
                                }
                            }
                            let resp = self.card(ui, e, id);
                            if resp.clicked() {
                                action = Some(("open", id));
                            }
                            resp.context_menu(|ui| {
                                if ui.button(screenit_i18n::t("lib.open")).clicked() {
                                    action = Some(("open", id));
                                    ui.close();
                                }
                                if ui.button(screenit_i18n::t("lib.copy")).clicked() {
                                    action = Some(("copy", id));
                                    ui.close();
                                }
                                if ui.button(screenit_i18n::t("lib.remove")).clicked() {
                                    action = Some(("delete", id));
                                    ui.close();
                                }
                            });
                        }
                    });
                }
            });
        if let Some((what, id)) = action {
            match what {
                "open" => self.open_library_entry(id, &entries, ui.ctx()),
                "copy" => {
                    if self
                        .run("library.share", json!({"id": id, "mode": "clipboard"}))
                        .is_some()
                    {
                        self.status = screenit_i18n::t("status.copied");
                    }
                }
                _ => {
                    self.thumbs.remove(&id);
                    self.run("library.delete", json!({"id": id}));
                }
            }
        }
    }

    fn open_library_entry(&mut self, id: u64, entries: &[Value], ctx: &egui::Context) {
        let Some(e) = entries.iter().find(|e| e["id"].as_u64() == Some(id)) else {
            return;
        };
        if e["kind"].as_str() == Some("text") {
            self.video = None;
            let body = e["body"].as_str().unwrap_or("").to_string();
            self.reading = Some(screenit_i18n::display(&body));
            self.enter_view(ctx, View::Editor);
            self.status = screenit_i18n::t("status.opened_text");
            return;
        }
        self.reading = None;
        if e["kind"].as_str() == Some("video") {
            let path = e["path"].as_str().unwrap_or_default().to_string();
            match self.session.dispatch("video.probe", &json!({"path": path})) {
                Ok(info) => {
                    let d = info["duration_ms"].as_u64().unwrap_or(0);
                    self.video = Some(VideoPlayer::new(path, d));
                    self.view = View::Editor;
                    self.status = "Video loaded: use the transport bar to scrub or play".into();
                }
                Err(err) => self.status = format!("video.probe: {err}"),
            }
        } else if self.run("library.open", json!({"id": id})).is_some() {
            self.video = None;
            self.view = View::Editor;
            self.status = "Opened from library".into();
        }
    }

    /// Saved grab, shown in place of the canvas. The text is already shaped for display.
    pub(crate) fn text_view(&mut self, ui: &mut egui::Ui) {
        let Some(text) = &mut self.reading else {
            return;
        };
        egui::Frame::default().inner_margin(18).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(text)
                        .desired_width(f32::INFINITY)
                        .desired_rows(24)
                        .font(egui::TextStyle::Body),
                );
            });
        });
    }

    /// One library card: rounded thumbnail, capture time and title; accent border on hover.
    fn card(&self, ui: &mut egui::Ui, e: &Value, id: u64) -> egui::Response {
        let size = egui::vec2(THUMB_W, THUMB_H + 48.0);
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let p = ui.painter();
        let hovered = resp.hovered();
        p.rect_filled(
            rect,
            12.0,
            if hovered {
                theme::SURFACE2
            } else {
                theme::SURFACE
            },
        );
        p.rect_stroke(
            rect,
            12.0,
            egui::Stroke::new(
                1.0,
                if hovered {
                    theme::ACCENT
                } else {
                    theme::BORDER
                },
            ),
            egui::StrokeKind::Inside,
        );
        let thumb = egui::Rect::from_min_size(rect.min, egui::vec2(THUMB_W, THUMB_H)).shrink(6.0);
        p.rect_filled(thumb, 8.0, theme::CANVAS);
        if let Some(Some(t)) = self.thumbs.get(&id) {
            let ts = t.size_vec2();
            let k = (thumb.width() / ts.x).min(thumb.height() / ts.y);
            let r = egui::Rect::from_center_size(thumb.center(), ts * k);
            p.image(
                t.id(),
                r,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        if e["kind"].as_str() == Some("text") {
            let body = e["body"]
                .as_str()
                .unwrap_or(e["title"].as_str().unwrap_or(""));
            let shown = screenit_i18n::display(body);
            let galley = p.layout(
                shown,
                egui::FontId::proportional(13.0),
                theme::TEXT,
                thumb.width() - 16.0,
            );
            p.galley(thumb.min + egui::vec2(8.0, 8.0), galley, theme::TEXT);
        } else if e["kind"].as_str() == Some("video") {
            let c = thumb.center();
            p.circle_filled(c, 20.0, egui::Color32::from_black_alpha(150));
            icons::paint(
                p,
                Icon::Play,
                egui::Rect::from_center_size(c + egui::vec2(2.0, 0.0), egui::Vec2::splat(16.0)),
                egui::Color32::WHITE,
            );
        }
        let (_, time) = day_and_time(e["created"].as_i64().unwrap_or(0));
        let title = e["title"].as_str().unwrap_or("(untitled)");
        let short: String = if title.chars().count() > 30 {
            title.chars().take(29).chain(std::iter::once('…')).collect()
        } else {
            title.to_owned()
        };
        p.text(
            egui::pos2(rect.min.x + 12.0, thumb.max.y + 14.0),
            egui::Align2::LEFT_CENTER,
            time,
            egui::FontId::proportional(13.5),
            theme::TEXT,
        );
        p.text(
            egui::pos2(rect.min.x + 12.0, thumb.max.y + 32.0),
            egui::Align2::LEFT_CENTER,
            short,
            egui::FontId::proportional(11.5),
            theme::MUTED,
        );
        resp.on_hover_text(title)
    }

    /// Transport bar for the loaded video: play/pause, scrubber, frame export.
    pub(crate) fn video_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(v) = self.video.as_mut() else { return };
        v.tick(Instant::now());
        let mut pos = v.pos_ms;
        let mut close = false;
        ui.horizontal(|ui| {
            let (icon, tip) = if v.playing {
                (Icon::Pause, "Pause")
            } else {
                (Icon::Play, "Play")
            };
            if theme::icon_button(ui, icon, tip, true, 32.0).clicked() {
                if v.pos_ms + 1 >= v.duration_ms {
                    v.pos_ms = 0;
                }
                v.playing = !v.playing;
            }
            let max = v.duration_ms - 1;
            if ui
                .add(egui::Slider::new(&mut pos, 0..=max).suffix(" ms"))
                .changed()
            {
                v.pos_ms = pos;
                v.playing = false;
            }
            if theme::icon_button(ui, Icon::Close, "Close video", false, 32.0).clicked() {
                close = true;
            }
        });
        let (path, want) = (v.path.clone(), v.pos_ms);
        let stale = v.shown_ms.is_none_or(|s| s.abs_diff(want) >= 33);
        if v.playing {
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        }
        if close {
            self.video = None;
            return;
        }
        if stale {
            if self
                .run("video.frame", json!({"path": path, "ms": want}))
                .is_some()
            {
                if let Some(v) = self.video.as_mut() {
                    v.shown_ms = Some(want);
                }
            } else if let Some(v) = self.video.as_mut() {
                v.playing = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn playhead_advances_and_stops_at_end() {
        let mut v = VideoPlayer::new("x.mp4".into(), 1000);
        let t0 = v.last_tick;
        v.playing = true;
        v.tick(t0 + Duration::from_millis(400));
        assert_eq!(v.pos_ms, 400);
        v.tick(t0 + Duration::from_millis(5000));
        assert_eq!(v.pos_ms, 999);
        assert!(!v.playing);
        v.playing = false;
        v.tick(t0 + Duration::from_millis(9000));
        assert_eq!(v.pos_ms, 999, "paused playhead does not move");
    }

    #[test]
    fn entries_group_by_local_day_in_order() {
        let day = 86_400;
        let base = 1_800_000_000i64 + 4 * 3600 + 1800; // 12:30 UTC, far from any local midnight
        let e = |t: i64| json!({"created": t});
        let entries = vec![e(base + day * 2), e(base + day * 2 - 60), e(base)];
        let g = group_by_day(&entries);
        assert_eq!(g.len(), 2, "two distinct days");
        assert_eq!(g[0].1.len(), 2);
        assert_eq!(g[1].1.len(), 1);
    }
}
