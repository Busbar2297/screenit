//! AI tutorial panel: a goal box, the target app, the editable script (chapters and steps) and
//! the Plan, Rehearse, Record and Produce buttons. All work is done by the `tutor.*` commands;
//! this file only shows their state and sends the user's choices.

use crate::UiApp;
use crate::icons::Icon;
use crate::theme::{self, ACCENT, MUTED, OK, TEXT};
use eframe::egui;
use screenit_i18n::{t, tf};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const ERR: egui::Color32 = egui::Color32::from_rgb(244, 114, 114);

pub(crate) struct TutorUi {
    pub goal: String,
    pub launch: String,
    pub window: String,
    pub out_dir: String,
    pub script_path: String,
    pub max_steps: u32,
    script: Option<Value>,
    status: Value,
    polled: Option<Instant>,
    answer: String,
}

impl Default for TutorUi {
    fn default() -> Self {
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        Self {
            goal: String::new(),
            launch: String::new(),
            window: String::new(),
            out_dir: if home.is_empty() {
                String::new()
            } else {
                format!("{home}\\Videos\\screenIt tutorials")
            },
            script_path: String::new(),
            max_steps: 20,
            script: None,
            status: json!({}),
            polled: None,
            answer: String::new(),
        }
    }
}

enum Act {
    Plan,
    Rehearse,
    Perform,
    Produce,
    Abort,
    Save,
    Load,
    /// Push the edited script back, keeping the rehearsal marks.
    Edit,
    /// Reply to the model's question: `hint`, `did_it`, `skip` or `finish`.
    Answer(&'static str),
}

fn file_stem(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    if s.is_empty() { "tutorial".into() } else { s }
}

/// One-line summary of a step's action for the script tree.
fn describe(action: &Value) -> String {
    let kind = action["type"].as_str().unwrap_or("?");
    let sel = |v: &Value| {
        let n = v["name"].as_str().unwrap_or("");
        let id = v["automation_id"].as_str().unwrap_or("");
        let r = v["role"].as_str().unwrap_or("");
        let main = if !n.is_empty() { n } else { id };
        format!("{r} {main}").trim().to_string()
    };
    match kind {
        "type" => format!("type \"{}\"", action["text"].as_str().unwrap_or("")),
        "key" => format!("key {}", action["keys"].as_str().unwrap_or("")),
        "wait" => format!("wait {} ms", action["ms"]),
        "scroll" => format!("scroll {}", action["dy"]),
        "drag" => format!("drag {} -> {}", sel(&action["from"]), sel(&action["to"])),
        "focus" => "focus the app".into(),
        _ => format!("{kind} {}", sel(&action["target"])),
    }
}

/// A progress event as a sentence a user can read.
fn event_line(e: &Value) -> String {
    let s = |k: &str| match &e[k] {
        Value::String(v) => v.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    match e["event"].as_str().unwrap_or("") {
        "asking_model" => tf(
            "tutor.ev.asking",
            &[("model", &s("model")), ("n", &s("attempt"))],
        ),
        "chapter" => tf(
            "tutor.ev.chapter",
            &[
                ("n", &e["index"].as_u64().map_or(0, |i| i + 1).to_string()),
                ("title", &s("title")),
            ],
        ),
        "step" => tf("tutor.ev.step", &[("id", &s("step_id"))]),
        "ask" => tf("tutor.ev.ask", &[("q", &s("question"))]),
        "repair" => tf(
            "tutor.ev.repair",
            &[
                ("id", &s("step_id")),
                ("window", &s("window")),
                ("n", &s("attempt")),
            ],
        ),
        _ => e.to_string(),
    }
}

impl UiApp {
    fn tutor_poll(&mut self, ctx: &egui::Context, force: bool) {
        let due = force
            || self
                .tutor_ui
                .polled
                .is_none_or(|t| t.elapsed() > Duration::from_millis(400));
        if !due {
            return;
        }
        self.tutor_ui.polled = Some(Instant::now());
        if let Ok(s) = self.session.dispatch("tutor.status", &json!({})) {
            self.tutor_ui.status = s;
        }
        self.tutor_ui.script = if self.tutor_ui.status["title"].is_null() {
            None
        } else {
            self.session.dispatch("tutor.script.get", &json!({})).ok()
        };
        if self.tutor_ui.status["running"] == true {
            ctx.request_repaint_after(Duration::from_millis(400));
        }
    }

    fn tutor_run(&mut self, act: Act, ctx: &egui::Context) {
        let u = &self.tutor_ui;
        let title = u
            .script
            .as_ref()
            .and_then(|s| s["title"].as_str())
            .unwrap_or("tutorial")
            .to_string();
        let (id, params) = match act {
            Act::Plan => {
                let mut p = json!({"goal": u.goal.trim(), "max_steps": u.max_steps});
                if !u.launch.trim().is_empty() {
                    p["launch"] = json!(u.launch.trim());
                }
                if !u.window.trim().is_empty() {
                    p["window_title"] = json!(u.window.trim());
                }
                ("tutor.explore", p)
            }
            Act::Answer(action) => {
                let text = std::mem::take(&mut self.tutor_ui.answer);
                (
                    "tutor.answer",
                    json!({"action": action, "text": text.trim()}),
                )
            }
            Act::Rehearse => ("tutor.rehearse", json!({})),
            Act::Perform => (
                "tutor.perform",
                json!({"output_dir": u.out_dir.trim(), "name": file_stem(&title)}),
            ),
            Act::Produce => (
                "tutor.produce",
                json!({"output": format!("{}\\{}.mp4", u.out_dir.trim().trim_end_matches('\\'), file_stem(&title))}),
            ),
            Act::Abort => ("tutor.abort", json!({})),
            Act::Save => ("tutor.save", json!({"path": u.script_path.trim()})),
            Act::Load => ("tutor.load", json!({"path": u.script_path.trim()})),
            Act::Edit => match u.script.clone() {
                Some(script) => (
                    "tutor.script.set",
                    json!({"script": script, "keep_checked": true}),
                ),
                None => return,
            },
        };
        if self.run(id, params).is_some() {
            self.status = format!("{id}: ok");
        }
        self.tutor_poll(ctx, true);
    }

    pub(crate) fn tutor_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.tutor_poll(ctx, false);
        let running = self.tutor_ui.status["running"] == true;
        let has_script = self.tutor_ui.script.is_some();
        let mut act: Option<Act> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(egui::RichText::new(t("tutor.title")).size(24.0).strong());
                ui.label(egui::RichText::new(t("tutor.hint")).size(13.0).color(MUTED));
                let model = self.session.settings.ocr.llm_model.trim().to_string();
                let model_line = if model.is_empty() {
                    t("tutor.model_missing")
                } else {
                    tf("tutor.model", &[("model", &model)])
                };
                ui.label(egui::RichText::new(model_line).size(13.0).color(MUTED));
                ui.add_space(10.0);
                self.tutor_form(ui, running);
                if self.tutor_ui.launch.trim().is_empty() && self.tutor_ui.window.trim().is_empty()
                {
                    ui.label(
                        egui::RichText::new(t("tutor.open_target"))
                            .size(12.0)
                            .color(MUTED),
                    );
                }
                ui.add_space(8.0);
                crate::h_row(ui, |ui| {
                    let goal_ok = !self.tutor_ui.goal.trim().is_empty();
                    let open = self.tutor_ui.launch.trim().is_empty()
                        && self.tutor_ui.window.trim().is_empty();
                    let mut plan = theme::pill_enabled(
                        ui,
                        Icon::Step,
                        &t("tutor.plan"),
                        true,
                        goal_ok && !running,
                    );
                    if !goal_ok {
                        plan = plan.on_hover_text(t("tutor.need_goal"));
                    } else if open {
                        plan = plan.on_hover_text(t("tutor.open_target"));
                    }
                    if plan.clicked() {
                        act = Some(Act::Plan);
                    }
                    let mut rehearse = theme::pill_enabled(
                        ui,
                        Icon::Play,
                        &t("tutor.rehearse"),
                        false,
                        has_script && !running,
                    );
                    let mut record = theme::pill_enabled(
                        ui,
                        Icon::Record,
                        &t("tutor.perform"),
                        false,
                        has_script && !running,
                    );
                    let mut produce = theme::pill_enabled(
                        ui,
                        Icon::Video,
                        &t("tutor.produce"),
                        false,
                        has_script && !running,
                    );
                    if !has_script {
                        let why = t("tutor.need_script");
                        rehearse = rehearse.on_hover_text(why.clone());
                        record = record.on_hover_text(why.clone());
                        produce = produce.on_hover_text(why);
                    }
                    if rehearse.clicked() {
                        act = Some(Act::Rehearse);
                    }
                    if record.clicked() {
                        act = Some(Act::Perform);
                    }
                    if produce.clicked() {
                        act = Some(Act::Produce);
                    }
                    if running && theme::pill(ui, Icon::Stop, &t("tutor.abort"), true).clicked() {
                        act = Some(Act::Abort);
                    }
                });
                ui.label(
                    egui::RichText::new(t("tutor.stop_hint"))
                        .size(11.5)
                        .color(MUTED),
                );
                ui.add_space(6.0);
                if let Some(a) = self.tutor_question(ui) {
                    act = Some(a);
                }
                self.tutor_status_lines(ui);
                ui.add_space(10.0);
                ui.separator();
                if let Some(a) = self.tutor_script_tree(ui, running) {
                    act = Some(a);
                }
            });
        if let Some(a) = act {
            self.tutor_run(a, ctx);
        }
    }

    fn tutor_form(&mut self, ui: &mut egui::Ui, running: bool) {
        let u = &mut self.tutor_ui;
        ui.add_enabled_ui(!running, |ui| {
            ui.label(egui::RichText::new(t("tutor.goal")).color(MUTED));
            ui.add(
                egui::TextEdit::multiline(&mut u.goal)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("Show how to add a column in Excel"),
            );
            egui::Grid::new("tutor_form").num_columns(2).show(ui, |ui| {
                ui.label(egui::RichText::new(t("tutor.app")).color(MUTED));
                ui.add(
                    egui::TextEdit::singleline(&mut u.launch)
                        .hint_text(t("tutor.app_hint"))
                        .desired_width(420.0),
                );
                ui.end_row();
                ui.label(egui::RichText::new(t("tutor.window")).color(MUTED));
                ui.add(egui::TextEdit::singleline(&mut u.window).desired_width(420.0));
                ui.end_row();
                ui.label(egui::RichText::new(t("tutor.max_steps")).color(MUTED));
                ui.add(egui::DragValue::new(&mut u.max_steps).range(1..=200));
                ui.end_row();
                ui.label(egui::RichText::new(t("tutor.out")).color(MUTED));
                ui.add(egui::TextEdit::singleline(&mut u.out_dir).desired_width(420.0));
                ui.end_row();
            });
        });
    }

    /// The model's open question, with a box for the reply.
    fn tutor_question(&mut self, ui: &mut egui::Ui) -> Option<Act> {
        let q = self.tutor_ui.status["question"].as_str()?.to_string();
        let mut act = None;
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.5, OK))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(t("tutor.question_title"))
                        .strong()
                        .color(OK),
                );
                ui.label(egui::RichText::new(q).size(14.0).color(TEXT));
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.tutor_ui.answer)
                        .hint_text(t("tutor.answer_hint"))
                        .desired_width(f32::INFINITY),
                );
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                crate::h_row(ui, |ui| {
                    let has_text = !self.tutor_ui.answer.trim().is_empty();
                    if theme::pill_enabled(ui, Icon::Step, &t("tutor.answer"), true, has_text)
                        .clicked()
                        || (entered && has_text)
                    {
                        act = Some(Act::Answer("hint"));
                    }
                    if theme::pill(ui, Icon::Play, &t("tutor.did_it"), false).clicked() {
                        act = Some(Act::Answer("did_it"));
                    }
                    if theme::pill(ui, Icon::Step, &t("tutor.skip"), false).clicked() {
                        act = Some(Act::Answer("skip"));
                    }
                    if theme::pill(ui, Icon::Stop, &t("tutor.finish"), false).clicked() {
                        act = Some(Act::Answer("finish"));
                    }
                });
            });
        ui.add_space(6.0);
        act
    }

    fn tutor_status_lines(&self, ui: &mut egui::Ui) {
        let s = &self.tutor_ui.status;
        let secs = (s["elapsed_ms"].as_u64().unwrap_or(0) / 1000).to_string();
        let running = s["running"] == true;
        let failed = s["error"].as_str();
        let finished = !running && failed.is_none() && !s["result"].is_null();
        if !running && failed.is_none() && !finished {
            return;
        }
        let stroke = if running {
            ACCENT
        } else if failed.is_some() {
            ERR
        } else {
            OK
        };
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, stroke))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if running {
                    let phase = s["phase"].as_str().unwrap_or("");
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().color(ACCENT));
                        ui.label(
                            egui::RichText::new(tf("tutor.working", &[("phase", phase)]))
                                .color(ACCENT)
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new(tf("tutor.elapsed", &[("s", &secs)]))
                                .size(12.0)
                                .color(MUTED),
                        );
                    });
                    // An indeterminate bar: the model call has no measurable progress.
                    ui.add(
                        egui::ProgressBar::new(0.0)
                            .animate(true)
                            .desired_height(6.0),
                    );
                } else if let Some(e) = failed {
                    ui.label(
                        egui::RichText::new(tf("tutor.failed", &[("e", e)]))
                            .color(ERR)
                            .size(13.0),
                    );
                } else if s["last_phase"] == "planning" || s["last_phase"] == "exploring" {
                    let r = &s["result"];
                    let n = |k: &str| r[k].as_u64().unwrap_or(0).to_string();
                    ui.label(
                        egui::RichText::new(tf(
                            "tutor.plan_ready",
                            &[
                                ("chapters", &n("chapters")),
                                ("steps", &n("steps")),
                                ("s", &secs),
                            ],
                        ))
                        .color(OK)
                        .strong(),
                    );
                } else {
                    ui.label(egui::RichText::new(t("tutor.done")).color(OK).strong());
                    if let Some(v) = s["result"]["video"].as_str() {
                        ui.label(egui::RichText::new(v).size(12.0).color(TEXT));
                    }
                }
                if let Some(ev) = s["events"].as_array() {
                    for e in ev.iter().rev().take(5).rev() {
                        ui.label(egui::RichText::new(event_line(e)).size(11.5).color(MUTED));
                    }
                }
            });
    }

    fn tutor_script_tree(&mut self, ui: &mut egui::Ui, running: bool) -> Option<Act> {
        let mut act = None;
        crate::h_row(ui, |ui| {
            ui.label(egui::RichText::new(t("tutor.script")).size(16.0).strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.tutor_ui.script_path)
                    .hint_text(t("tutor.script_path"))
                    .desired_width(300.0),
            );
            let has_path = !self.tutor_ui.script_path.trim().is_empty();
            if theme::pill_enabled(
                ui,
                Icon::Save,
                &t("tutor.save"),
                false,
                has_path && !running,
            )
            .clicked()
            {
                act = Some(Act::Save);
            }
            if theme::pill_enabled(
                ui,
                Icon::Folder,
                &t("tutor.load"),
                false,
                has_path && !running,
            )
            .clicked()
            {
                act = Some(Act::Load);
            }
        });
        let Some(script) = self.tutor_ui.script.as_mut() else {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(t("tutor.no_script")).color(MUTED));
            return act;
        };
        let mut edited = false;
        if let Some(chapters) = script["chapters"].as_array_mut() {
            for (ci, ch) in chapters.iter_mut().enumerate() {
                let checked = ch["checked"] == true;
                let title = ch["title"].as_str().unwrap_or("").to_string();
                let head = format!(
                    "{}. {}{}",
                    ci + 1,
                    title,
                    if checked {
                        format!("   ({})", t("tutor.chapter_checked"))
                    } else {
                        String::new()
                    }
                );
                egui::CollapsingHeader::new(egui::RichText::new(head).strong().color(if checked {
                    OK
                } else {
                    TEXT
                }))
                .id_salt(("tutor_ch", ci))
                .default_open(true)
                .show(ui, |ui| {
                    let Some(steps) = ch["steps"].as_array_mut() else {
                        return;
                    };
                    for (si, st) in steps.iter_mut().enumerate() {
                        let line = describe(&st["action"]);
                        ui.label(
                            egui::RichText::new(format!("{}  {line}", si + 1))
                                .size(12.5)
                                .color(MUTED),
                        );
                        let mut text = st["narration"].as_str().unwrap_or("").to_string();
                        let r = ui.add_enabled(
                            !running,
                            egui::TextEdit::singleline(&mut text)
                                .id_salt(("tutor_nar", ci, si))
                                .desired_width(f32::INFINITY),
                        );
                        if r.changed() {
                            st["narration"] = json!(text);
                        }
                        if r.lost_focus() {
                            edited = true;
                        }
                    }
                });
            }
        }
        if edited {
            act = Some(Act::Edit);
        }
        act
    }
}
