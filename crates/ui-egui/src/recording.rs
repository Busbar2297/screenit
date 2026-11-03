//! Video recording UX: options on the capture bar (microphone, system audio, webcam, cursor),
//! area pick through the capture overlay, a 3-2-1 countdown, and a floating always-on-top toolbar
//! (hidden from the capture itself) with the timer, pause/resume, stop and a webcam preview.

use crate::icons::{self, Icon};
use crate::library_panel::VideoPlayer;
use crate::theme::{self, BORDER, MUTED, SURFACE, SURFACE2, SURFACE3, TEXT};
use crate::{UiApp, View, default_output};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2, ViewportBuilder, ViewportId};
use screenit_geom::Rect as IRect;
use serde_json::json;
use std::time::{Duration, Instant};

pub(crate) const TOOLBAR_TITLE: &str = "screenIt recording";
pub(crate) const REC_RED: Color32 = Color32::from_rgb(239, 68, 68);
const COUNTDOWN: Duration = Duration::from_secs(3);
const BAR_H: f32 = 64.0;

/// What goes into the next recording; toggled on the capture bar.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RecOptions {
    pub mic: bool,
    pub system: bool,
    pub webcam: bool,
    pub cursor: bool,
}

impl Default for RecOptions {
    fn default() -> Self {
        Self {
            mic: true,
            system: true,
            webcam: false,
            cursor: true,
        }
    }
}

pub(crate) struct RecUi {
    region: IRect,
    screen: IRect,
    /// Still counting down until this instant; `None` once the recorder runs.
    countdown_end: Option<Instant>,
    opts: RecOptions,
    output: String,
    paused: bool,
    elapsed_ms: u64,
    cam_tex: Option<egui::TextureHandle>,
    cam_at: Instant,
    excluded: bool,
}

enum Action {
    None,
    Pause,
    Resume,
    Stop,
    Cancel,
}

/// Toolbar placement (physical px): centred under the area, else above it, else inside its
/// bottom edge (the toolbar is excluded from capture, so overlapping the area is harmless).
pub(crate) fn toolbar_pos(region: IRect, screen: IRect, w: i32, h: i32) -> (i32, i32) {
    let gap = 12;
    let x = (region.x + region.w as i32 / 2 - w / 2)
        .clamp(screen.x, (screen.x + screen.w as i32 - w).max(screen.x));
    let below = region.y + region.h as i32 + gap;
    let above = region.y - gap - h;
    let y = if below + h <= screen.y + screen.h as i32 {
        below
    } else if above >= screen.y {
        above
    } else {
        region.y + region.h as i32 - h - 2 * gap
    };
    (x, y)
}

fn fmt_time(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

impl UiApp {
    /// Toggle chips for the recording options (capture bar).
    pub(crate) fn record_options(&mut self, ui: &mut egui::Ui) {
        let before = (self.rec_opts.mic, self.rec_opts.system);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let o = &mut self.rec_opts;
            chip(ui, Icon::Mic, "Microphone", &mut o.mic);
            chip(ui, Icon::Speaker, "System audio", &mut o.system);
            chip(ui, Icon::Camera, "Webcam", &mut o.webcam);
            chip(ui, Icon::Cursor, "Cursor effects", &mut o.cursor);
        });
        if before != (self.rec_opts.mic, self.rec_opts.system) {
            let mut s = self.session.settings.clone();
            s.recording.microphone = self.rec_opts.mic;
            s.recording.system_audio = self.rec_opts.system;
            if let Err(e) = self.session.replace_settings(s) {
                self.status = format!("settings: {e}");
            }
        }
    }

    pub(crate) fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// The overlay picked `rect` (physical px) for recording: clamp it and start the countdown.
    pub(crate) fn begin_record(&mut self, ctx: &egui::Context, rect: IRect, screen: IRect) {
        let Some(region) = rect.intersect(&screen).filter(|r| r.w >= 16 && r.h >= 16) else {
            self.status = "That area is too small to record".into();
            self.settle_after_capture();
            return;
        };
        if self.rec_opts.webcam {
            // Open the camera during the countdown so the first frames are ready.
            if let Some(cam) = &self.session.services.camera {
                let _ = cam.frame();
            }
        }
        self.recording = Some(RecUi {
            region,
            screen,
            countdown_end: Some(Instant::now() + COUNTDOWN),
            opts: self.rec_opts,
            output: default_output("mp4").replace("capture-", "recording-"),
            paused: false,
            elapsed_ms: 0,
            cam_tex: None,
            cam_at: Instant::now(),
            excluded: false,
        });
        ctx.request_repaint();
    }

    fn start_recorder(&mut self) -> Result<(), String> {
        let Some(r) = &self.recording else {
            return Err("no recording".into());
        };
        let mut p = json!({
            "x": r.region.x, "y": r.region.y, "w": r.region.w, "h": r.region.h,
            "output": r.output,
            "fps": self.session.settings.recording.fps,
            "codec": self.session.settings.recording.codec,
            "system_audio": r.opts.system,
            "cursor_highlight": r.opts.cursor,
            "click_ripples": r.opts.cursor,
            "webcam": r.opts.webcam,
        });
        if r.opts.mic {
            p["mic"] = json!("default");
        }
        self.session
            .dispatch("record.start", &p)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn end_recording(&mut self, ctx: &egui::Context, keep: bool) {
        let started = self
            .recording
            .as_ref()
            .is_some_and(|r| r.countdown_end.is_none());
        let webcam = self.recording.as_ref().is_some_and(|r| r.opts.webcam);
        self.recording = None;
        if started {
            match self.session.dispatch("record.stop", &json!({})) {
                Ok(v) if keep => {
                    let res = &v["result"];
                    let path = res["path"].as_str().unwrap_or_default().to_string();
                    let ms = res["duration_ms"].as_u64().unwrap_or(0);
                    let audio = if res["has_audio"].as_bool() == Some(true) {
                        " with audio"
                    } else {
                        ""
                    };
                    self.saved_to(&format!("Recorded {}{audio} ·", fmt_time(ms)), &path);
                    self.video = Some(VideoPlayer::new(path, ms));
                    self.thumbs.clear();
                    self.enter_view(ctx, View::Editor);
                }
                Ok(_) => {}
                Err(e) => self.status = format!("record.stop: {e}"),
            }
        } else {
            self.status = "Recording cancelled".into();
            if webcam {
                if let Some(cam) = &self.session.services.camera {
                    cam.stop();
                }
            }
        }
        self.restore_window(ctx);
    }

    /// Drive the countdown / recorder and show the floating toolbar. Called every frame.
    pub(crate) fn drive_recording(&mut self, ctx: &egui::Context) {
        let Some(r) = &self.recording else {
            return;
        };
        if r.countdown_end.is_some_and(|t| Instant::now() >= t) {
            match self.start_recorder() {
                Ok(()) => {
                    if let Some(r) = &mut self.recording {
                        r.countdown_end = None;
                    }
                }
                Err(e) => {
                    self.recording = None;
                    self.status = format!("Could not start recording: {e}");
                    if let Some(cam) = &self.session.services.camera {
                        cam.stop();
                    }
                    self.restore_window(ctx);
                    return;
                }
            }
        }
        let status = self.session.dispatch("record.status", &json!({})).ok();
        let cam = self.session.services.camera.clone();
        let Some(r) = &mut self.recording else {
            return;
        };
        if let Some(s) = status {
            r.elapsed_ms = s["elapsed_ms"].as_u64().unwrap_or(r.elapsed_ms);
            r.paused = s["state"].as_str() == Some("paused");
        }
        if r.opts.webcam && r.cam_at.elapsed() >= Duration::from_millis(66) {
            r.cam_at = Instant::now();
            if let Some(f) = cam.as_ref().and_then(|c| c.frame()) {
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [f.width() as usize, f.height() as usize],
                    f.data(),
                );
                match &mut r.cam_tex {
                    Some(t) => t.set(ci, egui::TextureOptions::LINEAR),
                    None => {
                        r.cam_tex =
                            Some(ctx.load_texture("rec-cam", ci, egui::TextureOptions::LINEAR))
                    }
                }
            }
        }

        let ppp = ctx.pixels_per_point().max(0.1);
        let w = if r.opts.webcam { 470.0 } else { 360.0 };
        let (px, py) = toolbar_pos(r.region, r.screen, (w * ppp) as i32, (BAR_H * ppp) as i32);
        let builder = ViewportBuilder::default()
            .with_title(TOOLBAR_TITLE)
            .with_decorations(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false)
            .with_position(Pos2::new(px as f32 / ppp, py as f32 / ppp))
            .with_inner_size([w, BAR_H]);
        let mut action = Action::None;
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("scit-rec-toolbar"),
            builder,
            |ui, _| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(theme::BG))
                    .show(ui, |ui| action = toolbar(ui, r));
            },
        );
        if !r.excluded {
            // SCREENIT_TOOLBAR_IN_CAPTURE=1 keeps the toolbar capturable (screenshots of the UI).
            r.excluded = std::env::var_os("SCREENIT_TOOLBAR_IN_CAPTURE").is_some()
                || screenit_platform::window::exclude_from_capture(TOOLBAR_TITLE);
        }
        ctx.request_repaint_after(Duration::from_millis(if r.opts.webcam { 66 } else { 200 }));
        match action {
            Action::None => {}
            Action::Pause => {
                self.run("record.pause", json!({}));
            }
            Action::Resume => {
                self.run("record.resume", json!({}));
            }
            Action::Stop => self.end_recording(ctx, true),
            Action::Cancel => self.end_recording(ctx, false),
        }
    }
}

fn chip(ui: &mut egui::Ui, icon: Icon, label: &str, on: &mut bool) {
    let font = egui::FontId::proportional(12.0);
    let g = ui.painter().layout_no_wrap(label.to_owned(), font, TEXT);
    let size = Vec2::new(14.0 + 16.0 + 6.0 + g.size().x + 12.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    let p = ui.painter();
    let hovered = resp.hovered();
    let (fill, stroke, fg) = if *on {
        (
            theme::ACCENT.gamma_multiply(0.22),
            theme::ACCENT.gamma_multiply(0.9),
            TEXT,
        )
    } else {
        (if hovered { SURFACE3 } else { SURFACE2 }, BORDER, MUTED)
    };
    p.rect_filled(rect, 14.0, fill);
    p.rect_stroke(
        rect,
        14.0,
        Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );
    let ir = Rect::from_center_size(
        Pos2::new(rect.min.x + 14.0 + 8.0, rect.center().y),
        Vec2::splat(15.0),
    );
    icons::paint(p, icon, ir, if *on { theme::ACCENT2 } else { fg });
    if !*on {
        p.line_segment([ir.left_bottom(), ir.right_top()], Stroke::new(1.6, MUTED));
    }
    p.galley(
        Pos2::new(ir.max.x + 6.0, rect.center().y - g.size().y / 2.0),
        g,
        fg,
    );
    resp.on_hover_text(if *on {
        format!("{label}: on")
    } else {
        format!("{label}: off")
    });
}

fn toolbar(ui: &mut egui::Ui, r: &RecUi) -> Action {
    let full = ui.max_rect();
    let p = ui.painter().clone();
    p.rect_filled(full, 0.0, SURFACE);
    p.rect_stroke(
        full,
        0.0,
        Stroke::new(1.0, BORDER),
        egui::StrokeKind::Inside,
    );
    // Drag the toolbar by its background.
    let bg = ui.interact(full, ui.scope_id().with("rec-drag"), Sense::drag());
    if bg.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    let mut action = Action::None;
    let mut x = full.min.x + 12.0;
    let cy = full.center().y;
    if r.opts.webcam {
        let cam = Rect::from_min_size(Pos2::new(x, cy - 27.0), Vec2::new(96.0, 54.0));
        p.rect_filled(cam, 6.0, Color32::BLACK);
        if let Some(t) = &r.cam_tex {
            egui::Image::new((t.id(), cam.size()))
                .corner_radius(6.0)
                .paint_at(ui, cam);
        } else {
            icons::paint(&p, Icon::Camera, cam.shrink2(Vec2::new(36.0, 15.0)), MUTED);
        }
        x = cam.max.x + 12.0;
    }
    if let Some(end) = r.countdown_end {
        let left = end
            .saturating_duration_since(Instant::now())
            .as_secs_f32()
            .ceil() as u32;
        let c = Pos2::new(x + 20.0, cy);
        p.circle_filled(c, 19.0, REC_RED.gamma_multiply(0.25));
        p.circle_stroke(c, 19.0, Stroke::new(2.0, REC_RED));
        p.text(
            c,
            egui::Align2::CENTER_CENTER,
            left.max(1).to_string(),
            egui::FontId::proportional(20.0),
            Color32::WHITE,
        );
        p.text(
            Pos2::new(x + 50.0, cy - 9.0),
            egui::Align2::LEFT_CENTER,
            "Recording starts…",
            egui::FontId::proportional(14.0),
            TEXT,
        );
        p.text(
            Pos2::new(x + 50.0, cy + 10.0),
            egui::Align2::LEFT_CENTER,
            format!("{} × {}", r.region.w, r.region.h),
            egui::FontId::proportional(11.5),
            MUTED,
        );
        let b = Rect::from_center_size(Pos2::new(full.max.x - 30.0, cy), Vec2::splat(36.0));
        if button(ui, b, Icon::Close, "Cancel", false).clicked() {
            action = Action::Cancel;
        }
        return action;
    }
    // Live: blinking dot + timer.
    let blink = r.paused || (r.elapsed_ms / 600) % 2 == 0;
    let dot = Pos2::new(x + 7.0, cy);
    if blink {
        p.circle_filled(dot, 6.0, if r.paused { MUTED } else { REC_RED });
    } else {
        p.circle_stroke(dot, 6.0, Stroke::new(1.5, REC_RED));
    }
    p.text(
        Pos2::new(x + 20.0, cy - 7.0),
        egui::Align2::LEFT_CENTER,
        fmt_time(r.elapsed_ms),
        egui::FontId::monospace(20.0),
        TEXT,
    );
    p.text(
        Pos2::new(x + 21.0, cy + 13.0),
        egui::Align2::LEFT_CENTER,
        if r.paused { "Paused" } else { "Recording" },
        egui::FontId::proportional(11.0),
        MUTED,
    );
    // Source indicators.
    let mut ix = x + 110.0;
    for (on, icon, tip) in [
        (r.opts.mic, Icon::Mic, "Microphone"),
        (r.opts.system, Icon::Speaker, "System audio"),
        (r.opts.webcam, Icon::Camera, "Webcam"),
    ] {
        let ir = Rect::from_center_size(Pos2::new(ix, cy), Vec2::splat(16.0));
        icons::paint(
            &p,
            icon,
            ir,
            if on {
                theme::ACCENT2
            } else {
                MUTED.gamma_multiply(0.5)
            },
        );
        ui.interact(ir, ui.scope_id().with(("rec", tip)), Sense::hover())
            .on_hover_text(format!("{tip}: {}", if on { "on" } else { "off" }));
        ix += 24.0;
    }
    let stop = Rect::from_center_size(Pos2::new(full.max.x - 32.0, cy), Vec2::splat(40.0));
    let pause = Rect::from_center_size(Pos2::new(full.max.x - 80.0, cy), Vec2::splat(40.0));
    let (pi, pt) = if r.paused {
        (Icon::Play, "Resume")
    } else {
        (Icon::Pause, "Pause")
    };
    if button(ui, pause, pi, pt, false).clicked() {
        action = if r.paused {
            Action::Resume
        } else {
            Action::Pause
        };
    }
    if button(ui, stop, Icon::Stop, "Stop and open in the editor", true).clicked() {
        action = Action::Stop;
    }
    action
}

fn button(ui: &mut egui::Ui, rect: Rect, icon: Icon, tip: &str, danger: bool) -> egui::Response {
    let resp = ui.interact(rect, ui.scope_id().with(("rec", tip)), Sense::click());
    let p = ui.painter();
    let hovered = resp.hovered();
    if danger {
        p.rect_filled(
            rect,
            10.0,
            if hovered {
                REC_RED
            } else {
                REC_RED.gamma_multiply(0.85)
            },
        );
    } else {
        p.rect_filled(rect, 10.0, if hovered { SURFACE3 } else { SURFACE2 });
    }
    icons::paint(
        p,
        icon,
        rect.shrink(rect.width() * 0.3),
        if danger || hovered {
            Color32::WHITE
        } else {
            TEXT
        },
    );
    resp.on_hover_text(tip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_goes_below_then_above_then_inside() {
        let screen = IRect::new(0, 0, 1920, 1080);
        assert_eq!(
            toolbar_pos(IRect::new(100, 100, 400, 300), screen, 200, 60),
            (200, 412)
        );
        let (_, y) = toolbar_pos(IRect::new(100, 900, 400, 150), screen, 200, 60);
        assert_eq!(y, 900 - 12 - 60);
        let (x, y) = toolbar_pos(screen, screen, 200, 60);
        assert_eq!((x, y), (860, 1080 - 60 - 24));
        // Clamped horizontally to the screen.
        let (x, _) = toolbar_pos(IRect::new(-50, 100, 60, 60), screen, 200, 60);
        assert_eq!(x, 0);
    }

    #[test]
    fn time_format() {
        assert_eq!(fmt_time(0), "00:00");
        assert_eq!(fmt_time(65_400), "01:05");
    }
}
