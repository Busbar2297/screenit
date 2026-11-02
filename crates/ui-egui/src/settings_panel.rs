//! The Settings view: category list on the left, form on the right. Edits apply live and are
//! saved to `settings.json` by the engine.

use crate::icons::Icon;
use crate::theme::{self, MUTED, TEXT};
use crate::{UiApp, View};
use eframe::egui;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    General,
    Recording,
    Ocr,
}

fn combo(ui: &mut egui::Ui, id: &str, value: &mut String, options: &[(&str, &str)]) {
    let shown = options
        .iter()
        .find(|(v, _)| v == value)
        .map(|(_, l)| *l)
        .unwrap_or("?");
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for (v, label) in options {
                ui.selectable_value(value, (*v).to_string(), *label);
            }
        });
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(220.0, 22.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| ui.label(egui::RichText::new(label).color(TEXT)),
        );
        add(ui);
    });
    ui.add_space(4.0);
}

impl UiApp {
    pub(crate) fn settings_view(&mut self, ui: &mut egui::Ui) {
        let mut tab = self.settings_tab;
        egui::Panel::left("settings_tabs")
            .frame(theme::panel_frame(theme::SURFACE, 12))
            .exact_size(170.0)
            .resizable(false)
            .show(ui, |ui| {
                for (t, label) in [
                    (Tab::General, "General"),
                    (Tab::Recording, "Recording"),
                    (Tab::Ocr, "Text recognition"),
                ] {
                    if ui.selectable_label(tab == t, label).clicked() {
                        tab = t;
                    }
                }
            });
        self.settings_tab = tab;

        let mut s = self.session.settings.clone();
        egui::CentralPanel::default()
            .frame(theme::panel_frame(theme::BG, 22))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| match tab {
                    Tab::General => self.general_form(ui, &mut s),
                    Tab::Recording => recording_form(ui, &mut s),
                    Tab::Ocr => self.ocr_form(ui, &mut s),
                });
            });
        if s != self.session.settings
            && let Err(e) = self.session.replace_settings(s)
        {
            self.status = format!("settings: {e}");
        }
    }

    fn general_form(&mut self, ui: &mut egui::Ui, s: &mut screenit_engine::settings::Settings) {
        ui.heading("General");
        ui.add_space(8.0);
        let g = &mut s.general;
        row(ui, "After a capture", |ui| {
            combo(
                ui,
                "after_capture",
                &mut g.after_capture,
                &[
                    ("editor", "Open in the editor"),
                    ("silent", "Save silently and stay out of the way"),
                ],
            );
        });
        row(ui, "Copy captures to the clipboard", |ui| {
            ui.checkbox(&mut g.copy_to_clipboard, "");
        });
        row(ui, "Include the mouse cursor", |ui| {
            ui.checkbox(&mut g.include_cursor, "");
        });
        row(ui, "Save captures to", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut g.save_dir)
                    .hint_text("Default: Pictures\\screenIt")
                    .desired_width(320.0),
            );
        });
        row(ui, "Start with Windows", |ui| {
            ui.checkbox(&mut g.start_with_windows, "");
        });
        row(ui, "Start minimized to the tray", |ui| {
            ui.checkbox(&mut g.start_minimized, "");
        });
        ui.add_space(10.0);
        if let Some(p) = &self.session.settings_path {
            ui.label(
                egui::RichText::new(format!("Settings file: {}", p.display()))
                    .size(11.0)
                    .color(MUTED),
            );
        }
        ui.add_space(6.0);
        if theme::pill(ui, Icon::Trash, "Reset to defaults", false).clicked() {
            *s = screenit_engine::settings::Settings {
                presets: s.presets.clone(),
                ..Default::default()
            };
        }
    }
}

fn recording_form(ui: &mut egui::Ui, s: &mut screenit_engine::settings::Settings) {
    ui.heading("Recording");
    ui.add_space(8.0);
    let r = &mut s.recording;
    row(ui, "Frame rate", |ui| {
        ui.add(
            egui::DragValue::new(&mut r.fps)
                .range(1..=120)
                .suffix(" fps"),
        );
    });
    row(ui, "Codec", |ui| {
        combo(
            ui,
            "codec",
            &mut r.codec,
            &[("h264", "H.264"), ("h265", "H.265 (HEVC)")],
        );
    });
    row(ui, "Output size", |ui| {
        combo(
            ui,
            "output",
            &mut r.output,
            &[
                ("native", "Native (as captured)"),
                ("1080p", "1080p"),
                ("1440p", "1440p (2K)"),
                ("2160p", "2160p (4K)"),
            ],
        );
    });
    row(ui, "Record the microphone", |ui| {
        ui.checkbox(&mut r.microphone, "");
    });
    row(ui, "Record system audio", |ui| {
        ui.checkbox(&mut r.system_audio, "");
    });
}

impl UiApp {
    fn ocr_form(&mut self, ui: &mut egui::Ui, s: &mut screenit_engine::settings::Settings) {
        ui.heading("Text recognition");
        ui.add_space(8.0);
        let o = &mut s.ocr;
        row(ui, "Engine", |ui| {
            combo(
                ui,
                "ocr_engine",
                &mut o.engine,
                &[
                    ("windows", "Windows (offline)"),
                    ("llm", "Language model (LM Studio)"),
                    ("windows_then_llm", "Windows, then the language model"),
                ],
            );
        });
        row(ui, "Language", |ui| {
            combo(
                ui,
                "ocr_lang",
                &mut o.language,
                &[("auto", "Automatic"), ("en", "English"), ("ar", "Arabic")],
            );
        });
        let langs = self
            .win_langs
            .get_or_insert_with(|| {
                self.session
                    .services
                    .ocr
                    .as_ref()
                    .map(|o| o.languages())
                    .unwrap_or_default()
            })
            .clone();
        let has_ar = langs
            .iter()
            .any(|l| l.to_ascii_lowercase().starts_with("ar"));
        ui.label(
            egui::RichText::new(format!(
                "Installed Windows OCR languages: {}",
                langs.join(", ")
            ))
            .size(11.5)
            .color(MUTED),
        );
        if !has_ar && o.language == "ar" {
            ui.colored_label(
                egui::Color32::from_rgb(250, 204, 21),
                "Arabic OCR is not installed. Add the Arabic language pack with the Optical character recognition feature.",
            );
        }
        if ui.small_button("Open Windows language settings").clicked() {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", "ms-settings:regionlanguage"])
                .spawn();
        }
        ui.add_space(14.0);
        ui.label(egui::RichText::new("Language model server").strong());
        ui.add_space(4.0);
        row(ui, "Server URL", |ui| {
            ui.add(egui::TextEdit::singleline(&mut o.llm_url).desired_width(320.0));
        });
        row(ui, "Model", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut o.llm_model)
                    .hint_text("Empty = the model loaded in LM Studio")
                    .desired_width(320.0),
            );
        });
        row(ui, "Timeout", |ui| {
            ui.add(
                egui::DragValue::new(&mut o.llm_timeout_secs)
                    .range(1..=600)
                    .suffix(" s"),
            );
        });
        let models = self.llm_models.clone();
        if !models.is_empty() {
            row(ui, "Available models", |ui| {
                egui::ComboBox::from_id_salt("llm_models")
                    .selected_text("Pick a model")
                    .show_ui(ui, |ui| {
                        for m in &models {
                            ui.selectable_value(&mut o.llm_model, m.clone(), m);
                        }
                    });
            });
        }
        let url = o.llm_url.clone();
        let model = o.llm_model.clone();
        let timeout = o.llm_timeout_secs;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.llm_rx.is_none(), egui::Button::new("Test connection"))
                .clicked()
                && let Some(llm) = self.session.services.llm.clone()
            {
                let (tx, rx) = std::sync::mpsc::channel();
                self.llm_rx = Some(rx);
                self.llm_msg = "Connecting...".into();
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    let cfg = screenit_capture::LlmConfig {
                        url,
                        model,
                        timeout_secs: timeout.min(10),
                    };
                    let _ = tx.send(llm.models(&cfg).map_err(|e| e.to_string()));
                    ctx.request_repaint();
                });
            }
            ui.label(egui::RichText::new(&self.llm_msg).size(11.5).color(MUTED));
        });
    }

    /// Collect finished background jobs (connection test, OCR).
    pub(crate) fn poll_ocr_jobs(&mut self) {
        if let Some(rx) = &self.llm_rx
            && let Ok(r) = rx.try_recv()
        {
            self.llm_rx = None;
            match r {
                Ok(models) => {
                    self.llm_msg = format!("Connected, {} model(s) available", models.len());
                    self.llm_models = models;
                }
                Err(e) => self.llm_msg = format!("Failed: {e}"),
            }
        }
        if let Some(rx) = &self.ocr_rx
            && let Ok(r) = rx.try_recv()
        {
            self.ocr_rx = None;
            match r {
                Ok(out) => {
                    let text = out.text();
                    let n = text.chars().count();
                    if let Some(cb) = &self.session.services.clipboard {
                        let _ = cb.set_text(&text);
                    }
                    self.status = if n == 0 {
                        "No text found".into()
                    } else {
                        format!("Copied {n} characters of text ({})", out.engine)
                    };
                }
                Err(e) => self.status = format!("Text recognition failed: {e}"),
            }
        }
    }

    /// Recognise the text in the open document on a worker thread and copy it to the clipboard.
    pub(crate) fn grab_document_text(&mut self, ctx: &egui::Context) {
        let Some(doc) = self.session.doc.as_ref() else {
            return;
        };
        if self.ocr_rx.is_some() {
            return;
        }
        let image = screenit_compose::flatten(doc);
        let job = self.session.ocr_job(None);
        let (tx, rx) = std::sync::mpsc::channel();
        self.ocr_rx = Some(rx);
        self.status = "Recognising text...".into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job.run(&image).map_err(|e| e.to_string()));
            ctx.request_repaint();
        });
    }
}
impl UiApp {
    pub(crate) fn open_settings(&mut self, ctx: &egui::Context) {
        self.enter_view(ctx, View::Settings);
    }
}
