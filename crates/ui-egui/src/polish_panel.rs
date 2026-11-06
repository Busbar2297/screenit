//! Auto-polish bar shown under a loaded video: pick what the polish does and render a new MP4
//! next to the recording. Everything goes through `video.events.get`, `video.polish` and the
//! shared job status (`tutor.status`).

use crate::UiApp;
use eframe::egui;
use serde_json::{Value, json};
use std::path::Path;

const ASPECTS: [(&str, &str); 4] = [
    ("native", "Original"),
    ("square", "Square 1:1"),
    ("vertical", "Vertical 9:16"),
    ("wide", "Wide 16:9"),
];
const CURSORS: [(&str, &str); 3] = [
    ("native", "Cursor: as recorded"),
    ("disc", "Cursor: glow"),
    ("ring", "Cursor: ring"),
];

pub(crate) struct PolishUi {
    /// Video the event summary below belongs to.
    checked: String,
    /// `None`: the recording has no event log.
    events: Option<String>,
    aspect: usize,
    cursor: usize,
    zoom: bool,
    trim: bool,
    typing: bool,
    badges: bool,
    padding: u8,
    corner: u8,
    running: bool,
}

impl Default for PolishUi {
    fn default() -> Self {
        Self {
            checked: String::new(),
            events: None,
            aspect: 0,
            cursor: 0,
            zoom: true,
            trim: true,
            typing: true,
            badges: true,
            padding: 0,
            corner: 0,
            running: false,
        }
    }
}

impl PolishUi {
    /// Parameters for `video.polish` / `video.polish.plan`.
    fn params(&self) -> Value {
        json!({
            "zoom": self.zoom,
            "trim_idle": self.trim,
            "typing_speedup": self.typing,
            "shortcut_badges": self.badges,
            "click_rings": self.zoom,
            "aspect": ASPECTS[self.aspect.min(ASPECTS.len() - 1)].0,
            "cursor": CURSORS[self.cursor.min(CURSORS.len() - 1)].0,
            "padding_pct": self.padding,
            "corner_pct": self.corner,
        })
    }
}

fn polished_path(video: &str) -> String {
    let p = Path::new(video);
    let stem = p
        .file_stem()
        .map_or_else(|| "video".to_string(), |s| s.to_string_lossy().into_owned());
    p.with_file_name(format!("{stem}-polished.mp4"))
        .to_string_lossy()
        .into_owned()
}

fn summary(v: &Value) -> String {
    format!(
        "{} clicks, {} shortcuts",
        v["clicks"].as_u64().unwrap_or(0),
        v["keys"].as_u64().unwrap_or(0)
    )
}

impl UiApp {
    /// Whether the polish bar has anything to show for the loaded video.
    pub(crate) fn polish_wanted(&self) -> bool {
        self.video.is_some()
    }

    pub(crate) fn polish_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(path) = self.video.as_ref().map(|v| v.path.clone()) else {
            return;
        };
        if self.polish.checked != path {
            self.polish.checked = path.clone();
            self.polish.events = self
                .session
                .dispatch("video.events.get", &json!({"path": path}))
                .ok()
                .map(|v| summary(&v));
        }
        if self.polish.running {
            self.poll_polish(ctx);
        }
        let mut render = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label("Auto-polish");
            let Some(info) = self.polish.events.clone() else {
                ui.weak("No event log for this recording. Record with Auto-polish on.");
                return;
            };
            ui.weak(info);
            let p = &mut self.polish;
            ui.checkbox(&mut p.zoom, "Zoom on clicks");
            ui.checkbox(&mut p.trim, "Cut idle");
            ui.checkbox(&mut p.typing, "Speed up typing");
            ui.checkbox(&mut p.badges, "Shortcut badges");
            egui::ComboBox::from_id_salt("polish_aspect")
                .selected_text(ASPECTS[p.aspect.min(ASPECTS.len() - 1)].1)
                .show_ui(ui, |ui| {
                    for (i, (_, label)) in ASPECTS.iter().enumerate() {
                        ui.selectable_value(&mut p.aspect, i, *label);
                    }
                });
            egui::ComboBox::from_id_salt("polish_cursor")
                .selected_text(CURSORS[p.cursor.min(CURSORS.len() - 1)].1)
                .show_ui(ui, |ui| {
                    for (i, (_, label)) in CURSORS.iter().enumerate() {
                        ui.selectable_value(&mut p.cursor, i, *label);
                    }
                });
            ui.add(egui::Slider::new(&mut p.padding, 0..=25).text("Padding %"));
            ui.add(egui::Slider::new(&mut p.corner, 0..=25).text("Corners %"));
            let label = if p.running { "Rendering..." } else { "Render" };
            if ui
                .add_enabled(!p.running, egui::Button::new(label))
                .clicked()
            {
                render = true;
            }
        });
        if render {
            let mut params = self.polish.params();
            params["path"] = json!(path);
            params["output"] = json!(polished_path(&path));
            if self.run("video.polish", params).is_some() {
                self.polish.running = true;
                self.status = "Polishing video...".into();
            }
        }
    }

    /// Follow the render job; tell the user when it ends.
    fn poll_polish(&mut self, ctx: &egui::Context) {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        let Ok(st) = self.session.dispatch("tutor.status", &json!({})) else {
            return;
        };
        if st["running"].as_bool().unwrap_or(false) {
            let (d, t) = (
                st["progress_done"].as_u64().unwrap_or(0),
                st["progress_total"].as_u64().unwrap_or(0),
            );
            if t > 0 {
                self.status = format!("Polishing video... {}%", d * 100 / t);
            }
            return;
        }
        self.polish.running = false;
        self.status = match st["error"].as_str() {
            Some(e) => format!("Polish failed: {e}"),
            None => format!(
                "Polished video saved: {}",
                st["result"]["video"].as_str().unwrap_or("")
            ),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_carry_the_choices() {
        let mut p = PolishUi {
            aspect: 2,
            cursor: 1,
            padding: 8,
            ..PolishUi::default()
        };
        let v = p.params();
        assert_eq!(v["aspect"], "vertical");
        assert_eq!(v["cursor"], "disc");
        assert_eq!(v["padding_pct"], 8);
        p.aspect = 99; // out of range picks the last entry instead of panicking
        assert_eq!(p.params()["aspect"], "wide");
    }

    #[test]
    fn output_sits_next_to_the_recording() {
        let o = polished_path("C:\\v\\clip.mp4");
        assert!(o.ends_with("clip-polished.mp4"), "{o}");
        assert!(
            polished_path("").ends_with("video-polished.mp4")
                || polished_path("").ends_with(".mp4")
        );
    }

    #[test]
    fn summary_reads_counts() {
        assert_eq!(
            summary(&json!({"clicks": 3, "keys": 1})),
            "3 clicks, 1 shortcuts"
        );
    }
}
