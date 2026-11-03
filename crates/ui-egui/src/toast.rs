//! A short always-on-top notice. It stays visible while the main window is parked off-screen.

use crate::UiApp;
use eframe::egui::{self, Color32, Pos2, ViewportBuilder, ViewportId};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Notice {
    pub title: String,
    pub detail: String,
    pub until: Instant,
    /// Progress notice shown over a capture: excluded from screenshots, no Dismiss button.
    pub progress: bool,
}

/// Window title of the progress notice (unique so it can be found and hidden from capture).
const PROGRESS_TITLE: &str = "screenIt scrolling";

impl Notice {
    /// "Scrolling capture: page N" while a scrolling capture runs.
    pub fn scrolling(page: usize) -> Self {
        Self {
            title: screenit_i18n::tf("toast.scrolling", &[("n", &page.to_string())]),
            detail: screenit_i18n::t("toast.scrolling.detail"),
            until: Instant::now() + Duration::from_secs(3),
            progress: true,
        }
    }

    /// `saved` is true when the text was also written into the library.
    pub fn copied(detail: &str, saved: bool) -> Self {
        let key = if saved {
            "toast.copied_saved"
        } else {
            "toast.copied"
        };
        let preview: String = detail.chars().take(120).collect();
        Self {
            title: screenit_i18n::t(key),
            detail: screenit_i18n::display(&preview),
            until: Instant::now() + Duration::from_secs(6),
            progress: false,
        }
    }
}

pub fn drive(app: &mut UiApp, ctx: &egui::Context) {
    let Some(notice) = app.toast.clone() else {
        return;
    };
    if Instant::now() >= notice.until {
        app.toast = None;
        return;
    }
    ctx.request_repaint_after(Duration::from_millis(200));
    let (w, h) = ctx.input(|i| {
        i.viewport()
            .monitor_size
            .map(|s| (s.x, s.y))
            .unwrap_or((1280.0, 800.0))
    });
    let builder = ViewportBuilder::default()
        .with_title(if notice.progress {
            PROGRESS_TITLE
        } else {
            "screenIt"
        })
        .with_decorations(false)
        .with_always_on_top()
        .with_transparent(true)
        .with_taskbar(false)
        .with_active(false)
        .with_inner_size([420.0, 92.0])
        .with_position(Pos2::new((w - 448.0).max(16.0), (h - 150.0).max(16.0)));
    let mut dismiss = false;
    ctx.show_viewport_immediate(ViewportId::from_hash_of("scit-toast"), builder, |ui, _| {
        egui::Frame::default()
            .fill(Color32::from_rgb(30, 32, 44))
            .corner_radius(12)
            .inner_margin(14)
            .show(ui, |ui| {
                ui.with_layout(crate::column_layout(), |ui| {
                    ui.label(
                        egui::RichText::new(&notice.title)
                            .size(14.0)
                            .strong()
                            .color(Color32::WHITE),
                    );
                    if !notice.detail.is_empty() {
                        ui.label(
                            egui::RichText::new(&notice.detail)
                                .size(12.5)
                                .color(Color32::from_rgb(180, 186, 204)),
                        );
                    }
                    if !notice.progress {
                        dismiss = ui.button(screenit_i18n::t("toast.dismiss")).clicked();
                    }
                });
            });
    });
    if notice.progress {
        // Keep the notice out of the pictures being taken (no-op until the window exists).
        screenit_platform::window::exclude_from_capture(PROGRESS_TITLE);
    }
    if dismiss {
        app.toast = None;
    }
}
