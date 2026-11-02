//! The Settings view: category list on the side, form next to it. Edits apply live and are
//! saved to `settings.json` by the engine.

use crate::icons::Icon;
use crate::theme::{self, MUTED, TEXT};
use crate::{UiApp, View};
use eframe::egui;
use screenit_engine::settings::Settings;
use screenit_i18n::{t, tf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    General,
    Recording,
    Ocr,
}

fn combo(ui: &mut egui::Ui, id: &str, value: &mut String, options: &[(String, String)]) {
    use eframe::egui::AtomExt as _;
    let shown = options
        .iter()
        .find(|(v, _)| v == value)
        .map(|(_, l)| l.as_str())
        .unwrap_or("?");
    // The closed combo paints its text from the left. Grow the label and pin it to the
    // right edge so a visual-order Arabic string starts beside the arrow.
    let mut combo = egui::ComboBox::from_id_salt(id);
    combo = if screenit_i18n::is_rtl() {
        combo.selected_text(shown.atom_grow(true).atom_align(egui::Align2::RIGHT_CENTER))
    } else {
        combo.selected_text(shown)
    };
    combo.show_ui(ui, |ui| {
        let mut fill = |ui: &mut egui::Ui| {
            for (v, label) in options {
                ui.selectable_value(value, v.clone(), label);
            }
        };
        if screenit_i18n::is_rtl() {
            ui.with_layout(egui::Layout::top_down(egui::Align::Max), fill);
        } else {
            fill(ui);
        }
    });
}

/// One line from the clipboard, or nothing when it is empty or not text.
fn clipboard_text(app: &UiApp) -> Option<String> {
    let text = app.session.services.clipboard.as_ref()?.get_text().ok()?;
    let text = text.trim().replace(['\r', '\n'], "");
    if text.is_empty() { None } else { Some(text) }
}

fn opts(list: &[(&str, String)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(v, l)| ((*v).to_string(), l.clone()))
        .collect()
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    let cell = |ui: &mut egui::Ui| {
        ui.allocate_ui_with_layout(egui::vec2(220.0, 22.0), crate::start_layout(), |ui| {
            ui.label(egui::RichText::new(label).color(TEXT))
        });
    };
    crate::h_row(ui, |ui| {
        cell(ui);
        add(ui);
    });
    ui.add_space(4.0);
}

impl UiApp {
    pub(crate) fn settings_view(&mut self, ui: &mut egui::Ui) {
        let mut tab = self.settings_tab;
        let rtl = screenit_i18n::is_rtl();
        let side = if rtl {
            egui::Panel::right("settings_tabs")
        } else {
            egui::Panel::left("settings_tabs")
        };
        side.frame(theme::panel_frame(theme::SURFACE, 12))
            .exact_size(170.0)
            .resizable(false)
            .show(ui, |ui| {
                ui.with_layout(crate::column_layout(), |ui| {
                    for (tb, key) in [
                        (Tab::General, "settings.cat.general"),
                        (Tab::Recording, "settings.cat.recording"),
                        (Tab::Ocr, "settings.cat.ocr"),
                    ] {
                        if ui.selectable_label(tab == tb, t(key)).clicked() {
                            tab = tb;
                        }
                    }
                });
            });
        self.settings_tab = tab;

        let mut s = self.session.settings.clone();
        egui::CentralPanel::default()
            .frame(theme::panel_frame(theme::BG, 22))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.with_layout(crate::column_layout(), |ui| match tab {
                        Tab::General => self.general_form(ui, &mut s),
                        Tab::Recording => recording_form(ui, &mut s),
                        Tab::Ocr => self.ocr_form(ui, &mut s),
                    });
                });
            });
        if s != self.session.settings
            && let Err(e) = self.session.replace_settings(s)
        {
            self.status = format!("settings: {e}");
        }
    }

    fn general_form(&mut self, ui: &mut egui::Ui, s: &mut Settings) {
        ui.heading(t("settings.cat.general"));
        ui.add_space(8.0);
        let g = &mut s.general;
        row(ui, &t("settings.language"), |ui| {
            let list: Vec<(String, String)> = screenit_i18n::locales()
                .into_iter()
                .map(|(c, n)| (c, screenit_i18n::display(&n)))
                .collect();
            combo(ui, "language", &mut g.language, &list);
        });
        row(ui, &t("settings.after_capture"), |ui| {
            combo(
                ui,
                "after_capture",
                &mut g.after_capture,
                &opts(&[
                    ("editor", t("settings.after_capture.editor")),
                    ("silent", t("settings.after_capture.silent")),
                ]),
            );
        });
        row(ui, &t("settings.copy_clipboard"), |ui| {
            ui.checkbox(&mut g.copy_to_clipboard, "");
        });
        row(ui, &t("settings.include_cursor"), |ui| {
            ui.checkbox(&mut g.include_cursor, "");
        });
        row(ui, &t("settings.save_to"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut g.save_dir)
                    .hint_text(t("settings.save_to.hint"))
                    .desired_width(320.0),
            );
        });
        row(ui, &t("settings.autostart"), |ui| {
            ui.checkbox(&mut g.start_with_windows, "");
        });
        row(ui, &t("settings.minimized"), |ui| {
            ui.checkbox(&mut g.start_minimized, "");
        });
        ui.add_space(10.0);
        crate::h_row(ui, |ui| {
            if ui.button(t("settings.reload")).clicked() {
                self.status = match screenit_i18n::reload() {
                    Ok(()) => t("settings.reload"),
                    Err(e) => tf("settings.translation_error", &[("error", &e)]),
                };
            }
            if ui.button(t("settings.open_translations")).clicked()
                && let Some(dir) = crate::user_locales_dir()
            {
                let _ = screenit_i18n::export_bundled(&dir);
                screenit_platform::shell::reveal(&dir);
            }
        });
        ui.label(
            egui::RichText::new(t("settings.translation_note"))
                .size(11.0)
                .color(MUTED),
        );
        ui.add_space(10.0);
        if let Some(p) = &self.session.settings_path {
            ui.label(
                egui::RichText::new(tf("settings.file", &[("path", &p.display().to_string())]))
                    .size(11.0)
                    .color(MUTED),
            );
        }
        ui.add_space(6.0);
        if theme::pill(ui, Icon::Trash, &t("settings.reset"), false).clicked() {
            *s = Settings {
                presets: s.presets.clone(),
                ..Default::default()
            };
        }
    }
}

fn recording_form(ui: &mut egui::Ui, s: &mut Settings) {
    ui.heading(t("rec.title"));
    ui.add_space(8.0);
    let r = &mut s.recording;
    row(ui, &t("rec.fps"), |ui| {
        ui.add(
            egui::DragValue::new(&mut r.fps)
                .range(1..=120)
                .suffix(" fps"),
        );
    });
    row(ui, &t("rec.codec"), |ui| {
        combo(
            ui,
            "codec",
            &mut r.codec,
            &opts(&[("h264", "H.264".into()), ("h265", "H.265 (HEVC)".into())]),
        );
    });
    row(ui, &t("rec.output"), |ui| {
        combo(
            ui,
            "output",
            &mut r.output,
            &opts(&[
                ("native", t("rec.output.native")),
                ("1080p", "1080p".into()),
                ("1440p", "1440p (2K)".into()),
                ("2160p", "2160p (4K)".into()),
            ]),
        );
    });
    row(ui, &t("rec.mic"), |ui| {
        ui.checkbox(&mut r.microphone, "");
    });
    row(ui, &t("rec.sysaudio"), |ui| {
        ui.checkbox(&mut r.system_audio, "");
    });
}

impl UiApp {
    fn ocr_form(&mut self, ui: &mut egui::Ui, s: &mut Settings) {
        ui.heading(t("ocr.title"));
        ui.add_space(8.0);
        let o = &mut s.ocr;
        row(ui, &t("ocr.engine"), |ui| {
            combo(
                ui,
                "ocr_engine",
                &mut o.engine,
                &opts(&[
                    ("windows", t("ocr.engine.windows")),
                    ("llm", t("ocr.engine.llm")),
                    ("windows_then_llm", t("ocr.engine.both")),
                ]),
            );
        });
        row(ui, &t("ocr.language"), |ui| {
            combo(
                ui,
                "ocr_lang",
                &mut o.language,
                &opts(&[
                    ("auto", t("ocr.auto")),
                    ("en", t("ocr.english")),
                    ("ar", t("ocr.arabic")),
                ]),
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
            egui::RichText::new(tf("ocr.installed", &[("list", &langs.join(", "))]))
                .size(11.5)
                .color(MUTED),
        );
        if !has_ar && o.language == "ar" {
            ui.colored_label(egui::Color32::from_rgb(250, 204, 21), t("ocr.missing_ar"));
        }
        if ui.small_button(t("ocr.open_lang")).clicked() {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", "ms-settings:regionlanguage"])
                .spawn();
        }
        ui.add_space(14.0);
        ui.label(egui::RichText::new(t("ocr.server")).strong());
        ui.add_space(4.0);
        row(ui, &t("ocr.url"), |ui| {
            // Left to right even in an Arabic layout: a URL is Latin, and a right-to-left
            // row makes this field drop pasted text.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::TextEdit::singleline(&mut o.llm_url).desired_width(320.0));
                if ui.small_button(t("ocr.paste")).clicked()
                    && let Some(text) = clipboard_text(self)
                {
                    o.llm_url = text;
                }
            });
        });
        row(ui, &t("ocr.model"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut o.llm_model)
                    .hint_text(t("ocr.model.hint"))
                    .desired_width(320.0),
            );
        });
        row(ui, &t("ocr.api_key"), |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut o.llm_api_key)
                        .password(true)
                        .hint_text(t("ocr.api_key.hint"))
                        .desired_width(320.0),
                );
                if ui.small_button(t("ocr.paste")).clicked()
                    && let Some(text) = clipboard_text(self)
                {
                    o.llm_api_key = text;
                }
            });
        });
        row(ui, &t("ocr.timeout"), |ui| {
            ui.add(
                egui::DragValue::new(&mut o.llm_timeout_secs)
                    .range(1..=600)
                    .suffix(" s"),
            );
        });
        let models = self.llm_models.clone();
        if !models.is_empty() {
            row(ui, &t("ocr.available"), |ui| {
                egui::ComboBox::from_id_salt("llm_models")
                    .selected_text(t("ocr.pick"))
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
        let api_key = o.llm_api_key.clone();
        crate::h_row(ui, |ui| {
            if ui
                .add_enabled(self.llm_rx.is_none(), egui::Button::new(t("ocr.test")))
                .clicked()
                && let Some(llm) = self.session.services.llm.clone()
            {
                let (tx, rx) = std::sync::mpsc::channel();
                self.llm_rx = Some(rx);
                self.llm_msg = t("ocr.connecting");
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    let cfg = screenit_capture::LlmConfig {
                        url,
                        model,
                        api_key,
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
                    self.llm_msg = tf("ocr.connected", &[("n", &models.len().to_string())]);
                    self.llm_models = models;
                }
                Err(e) => self.llm_msg = tf("ocr.failed", &[("error", &e)]),
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
                    if n == 0 {
                        self.status = t("status.no_text");
                    } else {
                        let saved = self.session.store_text(&text).is_ok();
                        self.status = tf(
                            "status.copied_text",
                            &[("n", &n.to_string()), ("engine", out.engine)],
                        );
                        self.toast = Some(crate::toast::Notice::copied(&text, saved));
                    }
                }
                Err(e) => self.status = tf("status.ocr_failed", &[("error", &e)]),
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
        self.start_ocr(ctx, image);
    }

    /// The text overlay picked an area (or a window): read its text and copy it, no editor.
    pub(crate) fn finish_text_pick(
        &mut self,
        ctx: &egui::Context,
        frozen: &screenit_capture::FrozenScreen,
        sel: Option<screenit_capture::Selection>,
    ) {
        use screenit_capture::{ScreenSource, Selection};
        let rect = match sel {
            Some(Selection::Region { rect }) => Some(rect),
            Some(Selection::Window { id }) => frozen
                .windows_snapshot()
                .iter()
                .find(|w| w.id == id)
                .map(|w| w.rect),
            _ => None,
        };
        if let Some(r) = rect.filter(|r| !r.is_empty()) {
            match frozen.grab(r) {
                Ok(image) => self.start_ocr(ctx, image),
                Err(e) => self.status = tf("status.ocr_failed", &[("error", &e.to_string())]),
            }
        }
        if self.tray {
            self.settle_after_capture();
        } else {
            self.restore_window(ctx);
        }
    }

    fn start_ocr(&mut self, ctx: &egui::Context, image: screenit_raster::Raster) {
        if self.ocr_rx.is_some() {
            return;
        }
        let job = self.session.ocr_job(None);
        let (tx, rx) = std::sync::mpsc::channel();
        self.ocr_rx = Some(rx);
        self.status = t("status.recognising");
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job.run(&image).map_err(|e| e.to_string()));
            ctx.request_repaint();
        });
    }

    pub(crate) fn open_settings(&mut self, ctx: &egui::Context) {
        self.enter_view(ctx, View::Settings);
    }
}
