//! Auto-polish bar shown under a loaded video: pick what the polish does and render a new MP4
//! next to the recording. Everything goes through `video.events.get`, `video.polish` and the
//! shared job status (`tutor.status`).

use crate::{UiApp, theme};
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
    pub(crate) running: bool,
    pub(crate) fillers: bool,
    pub(crate) captions: bool,
    denoise: bool,
    silence: bool,
    pending: Option<Value>,
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
            fillers: false,
            captions: false,
            denoise: false,
            silence: false,
            pending: None,
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

impl PolishUi {
    /// What "Create video" will do, for the summary line and to know whether anything is ticked.
    fn todo(&self, has_log: bool) -> Vec<&'static str> {
        let mut v = Vec::new();
        if has_log {
            if self.zoom {
                v.push("zoom");
            }
            if self.trim {
                v.push("cut idle");
            }
            if self.typing {
                v.push("fast typing");
            }
            if self.badges {
                v.push("shortcuts");
            }
            if self.aspect != 0 || self.cursor != 0 || self.padding > 0 || self.corner > 0 {
                v.push("frame");
            }
        }
        if self.denoise {
            v.push("less noise");
        }
        if self.silence {
            v.push("no silences");
        }
        if self.fillers {
            v.push("no um/uh");
        }
        if self.captions {
            v.push("subtitles");
        }
        v
    }

    /// Filler cuts and burned subtitles read the transcript.
    fn needs_transcript(&self) -> bool {
        self.fillers || self.captions
    }

    /// `video.polish` parameters for everything ticked. Picture effects are off when the
    /// recording has no click log.
    fn create_params(&self, path: &str, has_log: bool) -> Value {
        let mut v = if has_log {
            self.params()
        } else {
            json!({
                "zoom": false, "trim_idle": false, "typing_speedup": false,
                "shortcut_badges": false, "click_rings": false,
            })
        };
        v["path"] = json!(path);
        v["output"] = json!(polished_path(path));
        v["silence"] = json!(self.silence);
        v["fillers"] = json!(self.fillers);
        v["captions"] = json!(self.captions);
        v["denoise"] = json!(self.denoise);
        v
    }
}

/// Subtitles or a transcript already exist next to the recording.
fn has_transcript(video: &str) -> bool {
    let p = Path::new(video);
    p.with_extension("transcript.json").is_file() || p.with_extension("captions.json").is_file()
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

/// The bar shares the widgets' default fill, so boxes and buttons vanished into it.
fn contrast_widgets(ui: &mut egui::Ui) {
    let w = &mut ui.visuals_mut().widgets;
    w.inactive.bg_fill = theme::SURFACE3;
    w.inactive.weak_bg_fill = theme::SURFACE3;
    w.inactive.bg_stroke = egui::Stroke::new(1.0, theme::MUTED.gamma_multiply(0.7));
    w.hovered.bg_fill = theme::BORDER;
    w.hovered.weak_bg_fill = theme::BORDER;
    w.hovered.bg_stroke = egui::Stroke::new(1.0, theme::ACCENT2);
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
        contrast_widgets(ui);
        let has_log = self.polish.events.is_some();
        let idle = !self.polish.running;
        let mut create = false;
        let mut transcribe = false;
        let label = |ui: &mut egui::Ui, name: &str, tip: &str| {
            ui.add_sized(
                [74.0, 20.0],
                egui::Label::new(egui::RichText::new(name).strong()),
            )
            .on_hover_text(tip);
        };

        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Make video").strong().size(14.0));
            match &self.polish.events {
                Some(info) => ui.weak(format!("{info} recorded")),
                None => ui.weak(
                    "No click log: picture effects are unavailable, sound and subtitles still work",
                ),
            };
        });

        // Picture
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            label(ui, "Picture", "Effects drawn from your clicks and typing");
            let p = &mut self.polish;
            ui.add_enabled_ui(has_log && idle, |ui| {
                ui.checkbox(&mut p.zoom, "Zoom on clicks").on_hover_text(
                    "Glide in on each click and back out, with a ring where you clicked",
                );
                ui.checkbox(&mut p.trim, "Cut idle")
                    .on_hover_text("Remove stretches where nothing happens");
                ui.checkbox(&mut p.typing, "Speed up typing")
                    .on_hover_text("Play long typing runs at 3x");
                ui.checkbox(&mut p.badges, "Shortcut badges")
                    .on_hover_text("Show keyboard shortcuts such as Ctrl+S on screen");
                egui::ComboBox::from_id_salt("polish_aspect")
                    .selected_text(ASPECTS[p.aspect.min(ASPECTS.len() - 1)].1)
                    .show_ui(ui, |ui| {
                        for (i, (_, l)) in ASPECTS.iter().enumerate() {
                            ui.selectable_value(&mut p.aspect, i, *l);
                        }
                    })
                    .response
                    .on_hover_text("Shape of the finished video");
                egui::ComboBox::from_id_salt("polish_cursor")
                    .selected_text(CURSORS[p.cursor.min(CURSORS.len() - 1)].1)
                    .show_ui(ui, |ui| {
                        for (i, (_, l)) in CURSORS.iter().enumerate() {
                            ui.selectable_value(&mut p.cursor, i, *l);
                        }
                    });
                ui.weak("Padding");
                ui.add(egui::Slider::new(&mut p.padding, 0..=25).suffix(" %"))
                    .on_hover_text("Space around the picture");
                ui.weak("Corners");
                ui.add(egui::Slider::new(&mut p.corner, 0..=25).suffix(" %"))
                    .on_hover_text("Rounded corners on the picture");
            });
        });

        // Sound
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            label(ui, "Sound", "Clean up the audio and cut dead air");
            let p = &mut self.polish;
            ui.add_enabled_ui(idle, |ui| {
                ui.checkbox(&mut p.denoise, "Reduce noise")
                    .on_hover_text("Suppress steady background noise");
                ui.checkbox(&mut p.silence, "Cut silences")
                    .on_hover_text("Remove pauses longer than a second");
                ui.checkbox(&mut p.fillers, "Remove um / uh")
                    .on_hover_text("Cut filler words (needs a transcript; made automatically)");
            });
        });

        // Subtitles
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            label(
                ui,
                "Subtitles",
                "Words from the speech, drawn on the picture",
            );
            let p = &mut self.polish;
            ui.add_enabled_ui(idle, |ui| {
                ui.checkbox(&mut p.captions, "Burn in subtitles")
                    .on_hover_text("Draw the subtitles on the video (transcribed automatically)");
            });
            if ui
                .add_enabled(idle, egui::Button::new("Transcribe only"))
                .on_hover_text("Just write the subtitle files next to the recording")
                .clicked()
            {
                transcribe = true;
            }
        });

        // Create
        let todo = self.polish.todo(has_log);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (text, color) = if self.polish.running {
                ("Working...", egui::Color32::WHITE)
            } else {
                ("Create video", egui::Color32::WHITE)
            };
            let btn = egui::Button::new(egui::RichText::new(text).strong().color(color))
                .fill(theme::ACCENT)
                .min_size(egui::vec2(140.0, 30.0));
            if ui
                .add_enabled(idle && !todo.is_empty(), btn)
                .on_hover_text("Everything ticked above, in one new file next to the recording")
                .clicked()
            {
                create = true;
            }
            if todo.is_empty() {
                ui.weak("Tick something above to get started");
            } else {
                ui.weak(format!(
                    "{} -> {}",
                    todo.join(", "),
                    Path::new(&polished_path(&path))
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
            }
        });

        if transcribe
            && self
                .run("video.transcribe", json!({"path": path}))
                .is_some()
        {
            self.polish.running = true;
            self.status = "Transcribing...".into();
        }
        if create {
            let params = self.polish.create_params(&path, has_log);
            if self.polish.needs_transcript() && !has_transcript(&path) {
                if self
                    .run("video.transcribe", json!({"path": path}))
                    .is_some()
                {
                    self.polish.running = true;
                    self.polish.pending = Some(params);
                    self.status = "Step 1 of 2: transcribing...".into();
                }
            } else {
                self.start_render(params);
            }
        }
    }

    fn start_render(&mut self, params: Value) {
        if self.run("video.polish", params).is_some() {
            self.polish.running = true;
            self.status = "Creating video...".into();
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
        if let Some(params) = self.polish.pending.take() {
            if st["error"].is_null() {
                self.start_render(params);
                return;
            }
        }
        self.status = match st["error"].as_str() {
            Some(e) => format!("Polish failed: {e}"),
            None => match st["result"]["video"].as_str() {
                Some(v) => format!("Video saved: {v}"),
                None => {
                    let r = &st["result"];
                    if let Some(c) = r["chapters"].as_array() {
                        format!(
                            "{} chapters written: {}",
                            c.len(),
                            r["path"].as_str().unwrap_or("")
                        )
                    } else if r["description"].is_string() {
                        format!("Description written: {}", r["path"].as_str().unwrap_or(""))
                    } else if r["cleaned"].is_number() {
                        format!(
                            "Subtitles polished: {} lines cleaned, {} unchanged",
                            r["cleaned"].as_u64().unwrap_or(0),
                            r["unchanged"].as_u64().unwrap_or(0)
                        )
                    } else {
                        format!(
                            "Transcribed: {} words, {} subtitle lines",
                            r["words"].as_u64().unwrap_or(0),
                            r["lines"].as_u64().unwrap_or(0)
                        )
                    }
                }
            },
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
    fn one_render_carries_picture_sound_and_subtitle_choices() {
        let mut p = PolishUi {
            denoise: true,
            silence: true,
            fillers: true,
            captions: true,
            ..PolishUi::default()
        };
        let v = p.create_params("C:\\v\\clip.mp4", true);
        assert_eq!(v["zoom"], true);
        assert_eq!(v["denoise"], true);
        assert_eq!(v["silence"], true);
        assert_eq!(v["fillers"], true);
        assert_eq!(v["captions"], true);
        assert!(v["output"].as_str().unwrap().ends_with("clip-polished.mp4"));
        assert!(p.needs_transcript());
        // Without a click log the picture effects are switched off, the sound ones stay.
        let v = p.create_params("C:\\v\\clip.mp4", false);
        assert_eq!(v["zoom"], false);
        assert_eq!(v["trim_idle"], false);
        assert_eq!(v["denoise"], true);
        p.fillers = false;
        p.captions = false;
        assert!(!p.needs_transcript());
    }

    #[test]
    fn the_summary_lists_what_is_ticked() {
        let mut p = PolishUi {
            zoom: false,
            trim: false,
            typing: false,
            badges: false,
            ..PolishUi::default()
        };
        assert!(p.todo(true).is_empty());
        p.denoise = true;
        p.captions = true;
        p.aspect = 2;
        assert_eq!(p.todo(true), vec!["frame", "less noise", "subtitles"]);
        assert_eq!(p.todo(false), vec!["less noise", "subtitles"]);
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
