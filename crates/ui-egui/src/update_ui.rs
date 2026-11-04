//! Update prompts: a strip above the status bar, a pill in the compact launcher, and the text
//! shown in About and Settings. All real work is the `app.update.*` commands.

use crate::UiApp;
use crate::icons::Icon;
use crate::theme::{self, MUTED, TEXT};
use eframe::egui;
use screenit_i18n::{t, tf};
use screenit_update::UpdateState;
use serde_json::json;
use std::time::Duration;

impl UiApp {
    pub(crate) fn update_state(&self) -> Option<UpdateState> {
        self.session.services.updater.as_ref().map(|u| u.state())
    }

    /// Keep repainting while the worker thread is busy so progress and results show up.
    pub(crate) fn poll_updates(&self, ctx: &egui::Context) {
        if matches!(
            self.update_state(),
            Some(UpdateState::Checking | UpdateState::Downloading { .. })
        ) {
            ctx.request_repaint_after(Duration::from_millis(400));
        }
    }

    /// A daily check, driven by the app's timer. Failures stay silent.
    pub(crate) fn auto_update_check(&mut self) {
        let due = self
            .session
            .dispatch("app.update.status", &json!({}))
            .map(|s| s["due"] == true)
            .unwrap_or(false);
        if due {
            let _ = self.session.dispatch("app.update.check", &json!({}));
        }
    }

    /// Does the strip have something to show?
    pub(crate) fn update_strip_wanted(&self) -> bool {
        match self.update_state() {
            Some(
                UpdateState::Ready { version, .. }
                | UpdateState::Available { version, .. }
                | UpdateState::Downloading { version, .. },
            ) => self.update_hidden.as_deref() != Some(version.as_str()),
            _ => false,
        }
    }

    pub(crate) fn update_strip(&mut self, ui: &mut egui::Ui) {
        crate::h_row(ui, |ui| self.update_actions(ui, true));
    }

    /// The message and buttons for the current update state. `full` adds the message, Later and
    /// Skip; the compact launcher only has room for the main button.
    pub(crate) fn update_actions(&mut self, ui: &mut egui::Ui, full: bool) {
        let Some(state) = self.update_state() else {
            return;
        };
        let msg = |ui: &mut egui::Ui, text: String| {
            ui.label(egui::RichText::new(text).size(13.0).color(TEXT));
        };
        match state {
            UpdateState::Ready { version, .. } => {
                if full {
                    msg(ui, tf("update.ready", &[("v", &version)]));
                }
                if theme::pill(ui, Icon::Info, &t("update.restart"), true).clicked() {
                    self.install_update();
                }
                self.later_and_skip(ui, full, &version);
            }
            UpdateState::Available {
                version,
                page_url,
                notify_only,
            } => {
                if full {
                    msg(ui, tf("update.available", &[("v", &version)]));
                }
                if notify_only {
                    if theme::pill(ui, Icon::Info, &t("update.open"), true).clicked() {
                        let _ = screenit_platform::shell::open_url(&page_url);
                    }
                } else if theme::pill(ui, Icon::Export, &t("update.download"), true).clicked() {
                    self.run("app.update.download", json!({}));
                }
                self.later_and_skip(ui, full, &version);
            }
            UpdateState::Downloading { .. } if full => msg(ui, self.update_status_line()),
            _ => {}
        }
    }

    fn later_and_skip(&mut self, ui: &mut egui::Ui, full: bool, version: &str) {
        if !full {
            return;
        }
        if ui.button(t("update.later")).clicked() {
            self.update_hidden = Some(version.to_string());
        }
        if ui.button(t("update.skip")).clicked() {
            self.run("app.update.skip", json!({"version": version}));
        }
    }

    /// Hand the downloaded installer over and leave, so it can replace the files.
    fn install_update(&mut self) {
        if self.run("app.update.install", json!({})).is_some() {
            self.quit = true;
        }
    }

    /// One line describing the update state for About and Settings.
    pub(crate) fn update_status_line(&self) -> String {
        match self.update_state() {
            None => String::new(),
            Some(UpdateState::Idle) => String::new(),
            Some(UpdateState::Checking) => t("update.checking"),
            Some(UpdateState::UpToDate) => t("update.uptodate"),
            Some(UpdateState::Available { version, .. }) => {
                tf("update.available", &[("v", &version)])
            }
            Some(UpdateState::Downloading { version, percent }) => tf(
                "update.downloading",
                &[("v", &version), ("p", &percent.to_string())],
            ),
            Some(UpdateState::Ready { version, .. }) => tf("update.ready", &[("v", &version)]),
            Some(UpdateState::Failed { message }) => tf("update.failed", &[("e", &message)]),
        }
    }

    /// "Check for updates" button plus the state line, shared by About and Settings.
    pub(crate) fn update_check_row(&mut self, ui: &mut egui::Ui) {
        if self.session.services.updater.is_none() {
            return;
        }
        crate::h_row(ui, |ui| {
            let busy = matches!(
                self.update_state(),
                Some(UpdateState::Checking | UpdateState::Downloading { .. })
            );
            if ui
                .add_enabled(!busy, egui::Button::new(t("update.check")))
                .clicked()
            {
                self.update_hidden = None;
                self.run("app.update.check", json!({}));
            }
            let line = self.update_status_line();
            if !line.is_empty() {
                ui.label(egui::RichText::new(line).size(12.5).color(MUTED));
            }
        });
        if matches!(
            self.update_state(),
            Some(UpdateState::Ready { .. } | UpdateState::Available { .. })
        ) {
            // The line above already says what is going on; show only the buttons.
            crate::h_row(ui, |ui| self.update_actions(ui, false));
        }
    }
}
