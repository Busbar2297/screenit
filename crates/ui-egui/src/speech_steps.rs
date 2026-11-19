//! Speech steps: several sound jobs on one recording, in the order the user picks. Each step
//! runs on the file the previous one produced; transcription is added on its own when a later
//! step needs subtitles of the current file. Only the last file is kept.

use crate::UiApp;
use eframe::egui;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Denoise,
    Transcribe,
    CutSilences,
    Burn,
}

pub(crate) const ALL: [Step; 4] = [
    Step::Denoise,
    Step::Transcribe,
    Step::CutSilences,
    Step::Burn,
];

impl Step {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Step::Denoise => "Reduce noise",
            Step::Transcribe => "Transcribe",
            Step::CutSilences => "Cut silences",
            Step::Burn => "Burn subtitles",
        }
    }

    /// Whether the step reads the transcript or subtitles of its input file.
    fn needs_transcript(self, fillers: bool, captions: bool) -> bool {
        match self {
            Step::Burn => true,
            Step::CutSilences => fillers || captions,
            _ => false,
        }
    }
}

/// A run in progress.
pub(crate) struct Chain {
    pub queue: VecDeque<Step>,
    /// File the next step works on.
    pub current: String,
    /// Files made by earlier steps (deleted at the end, except the last).
    pub made: Vec<String>,
    pub done: usize,
    pub total: usize,
}

fn has_transcript(video: &str) -> bool {
    let p = Path::new(video);
    p.with_extension("transcript.json").is_file() || p.with_extension("captions.json").is_file()
}

/// `{stem}-steps{n}.mp4` next to the original recording.
fn step_output(original: &str, n: usize) -> String {
    let p = Path::new(original);
    let stem = p
        .file_stem()
        .map_or_else(|| "video".to_string(), |s| s.to_string_lossy().into_owned());
    p.with_file_name(format!("{stem}-steps{n}.mp4"))
        .to_string_lossy()
        .into_owned()
}

/// Strip an earlier `-stepsN` so outputs stay next to the original name.
fn original_of(video: &str) -> String {
    let p = Path::new(video);
    let stem = p
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    match stem.rsplit_once("-steps") {
        Some((base, n)) if !base.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => p
            .with_file_name(format!("{base}.mp4"))
            .to_string_lossy()
            .into_owned(),
        _ => video.to_string(),
    }
}

/// Command and parameters for one step on `input`.
pub(crate) fn command(
    step: Step,
    input: &str,
    output: &str,
    fillers: bool,
    captions: bool,
) -> (&'static str, Value) {
    match step {
        Step::Denoise => (
            "video.audio.denoise",
            json!({"path": input, "output": output}),
        ),
        Step::Transcribe => ("video.transcribe", json!({"path": input})),
        Step::CutSilences => (
            "video.silence.cut",
            json!({"path": input, "output": output, "fillers": fillers, "captions": captions}),
        ),
        Step::Burn => (
            "video.captions.burn",
            json!({"path": input, "output": output}),
        ),
    }
}

fn remove_with_sidecars(video: &str) {
    let p = Path::new(video);
    let _ = std::fs::remove_file(p);
    let _ = std::fs::remove_file(p.with_extension("transcript.json"));
    let _ = std::fs::remove_file(p.with_extension("captions.json"));
}

impl UiApp {
    /// The "Steps" row: an ordered list with add / move / remove, and a run button.
    pub(crate) fn steps_row(&mut self, ui: &mut egui::Ui, path: &str) {
        let idle = !self.polish.running;
        let mut start = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(egui::RichText::new("Steps").strong())
                .on_hover_text(
                    "Run several jobs on this recording in this order; each works on the result of \
                 the one before. Only the final video is kept.",
                );
            let steps = &mut self.polish.steps;
            let mut action: Option<(usize, i32)> = None;
            for (i, s) in steps.iter().enumerate() {
                ui.group(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(format!("{}. {}", i + 1, s.label()));
                    if ui
                        .add_enabled(idle && i > 0, egui::Button::new("<").small())
                        .on_hover_text("Earlier")
                        .clicked()
                    {
                        action = Some((i, -1));
                    }
                    if ui
                        .add_enabled(idle && i + 1 < steps.len(), egui::Button::new(">").small())
                        .on_hover_text("Later")
                        .clicked()
                    {
                        action = Some((i, 1));
                    }
                    if ui
                        .add_enabled(idle, egui::Button::new("x").small())
                        .on_hover_text("Remove")
                        .clicked()
                    {
                        action = Some((i, 0));
                    }
                });
            }
            match action {
                Some((i, 0)) => {
                    steps.remove(i);
                }
                Some((i, d)) => {
                    let j = if d < 0 { i.saturating_sub(1) } else { i + 1 };
                    if j < steps.len() {
                        steps.swap(i, j);
                    }
                }
                None => {}
            }
            ui.add_enabled_ui(idle && steps.len() < 8, |ui| {
                ui.menu_button("+ Add step", |ui| {
                    for s in ALL {
                        if ui.button(s.label()).clicked() {
                            steps.push(s);
                            ui.close();
                        }
                    }
                });
            });
            if ui
                .add_enabled(idle && !steps.is_empty(), egui::Button::new("Run steps"))
                .on_hover_text("Cut silences uses the options in the row above")
                .clicked()
            {
                start = true;
            }
        });
        if start {
            let steps = self.polish.steps.clone();
            self.polish.chain = Some(Chain {
                total: steps.len(),
                queue: steps.into(),
                current: path.to_string(),
                made: Vec::new(),
                done: 0,
            });
            self.next_step();
        }
    }

    /// Start the next queued step, or finish the run.
    pub(crate) fn next_step(&mut self) {
        let (fillers, captions) = (self.polish.fillers, self.polish.captions);
        let Some(chain) = self.polish.chain.as_mut() else {
            return;
        };
        let Some(step) = chain.queue.pop_front() else {
            let keep = chain.current.clone();
            for f in chain.made.iter().filter(|f| **f != keep) {
                remove_with_sidecars(f);
            }
            self.polish.chain = None;
            self.status = format!("All steps done: {keep}");
            return;
        };
        let step = if step.needs_transcript(fillers, captions) && !has_transcript(&chain.current) {
            chain.queue.push_front(step);
            chain.total += 1;
            Step::Transcribe
        } else {
            step
        };
        chain.done += 1;
        let n = chain.done;
        let out = step_output(&original_of(&chain.current), n);
        let (cmd, params) = command(step, &chain.current, &out, fillers, captions);
        let msg = format!("Step {n} of {}: {}...", chain.total, step.label());
        if self.run(cmd, params).is_some() {
            self.polish.running = true;
            self.status = msg;
        } else {
            self.polish.chain = None;
        }
    }

    /// A job of a step run ended: move on, or stop on an error. False when no run is active.
    pub(crate) fn step_finished(&mut self, st: &Value) -> bool {
        let Some(chain) = self.polish.chain.as_mut() else {
            return false;
        };
        if let Some(e) = st["error"].as_str() {
            let at = chain.done;
            self.polish.chain = None;
            self.status = format!("Step {at} failed: {e}");
            return true;
        }
        if let Some(v) = st["result"]["video"].as_str() {
            chain.made.push(v.to_string());
            chain.current = v.to_string();
        }
        self.next_step();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputs_sit_next_to_the_original_name() {
        let o = step_output("C:\\v\\clip.mp4", 2);
        assert!(o.ends_with("clip-steps2.mp4"), "{o}");
        assert!(original_of("C:\\v\\clip-steps2.mp4").ends_with("clip.mp4"));
        assert!(original_of("C:\\v\\my-steps-x.mp4").ends_with("my-steps-x.mp4"));
        assert!(original_of("").is_empty() || original_of("").ends_with(".mp4"));
    }

    #[test]
    fn commands_chain_input_to_output() {
        let (c, p) = command(Step::CutSilences, "a.mp4", "b.mp4", true, false);
        assert_eq!(c, "video.silence.cut");
        assert_eq!(p["path"], "a.mp4");
        assert_eq!(p["output"], "b.mp4");
        assert_eq!(p["fillers"], true);
        let (c, p) = command(Step::Transcribe, "a.mp4", "b.mp4", false, false);
        assert_eq!(c, "video.transcribe");
        assert!(p.get("output").is_none());
    }

    #[test]
    fn only_steps_reading_subtitles_need_a_transcript() {
        assert!(Step::Burn.needs_transcript(false, false));
        assert!(Step::CutSilences.needs_transcript(true, false));
        assert!(!Step::CutSilences.needs_transcript(false, false));
        assert!(!Step::Denoise.needs_transcript(true, true));
    }
}
