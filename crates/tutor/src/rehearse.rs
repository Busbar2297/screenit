//! Rehearsal: run every step against the live app without recording, ask the language model to
//! repair steps that fail, and pin down where each target really was.

use crate::error::{TutorError, TutorResult};
use crate::plan::{parse_step, repair_messages};
use crate::player::{Mode, Player};
use crate::safety::Guard;
use crate::script::{Action, Selector, Step, TutorialScript};
use crate::traits::{ChatOptions, InputDriver, LlmChat, Rig, WindowInfo};
use screenit_geom::Rect;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub struct RehearseOpts {
    pub from_chapter: usize,
    pub max_repairs: u32,
    pub chat: Option<(Arc<dyn LlmChat>, ChatOptions)>,
}

/// Run `work` on another thread and return as soon as it finishes, the user hits Stop, or the
/// mouse watcher trips. The worker is left to finish on its own after a stop.
pub fn wait_for<T: Send + 'static>(
    input: &dyn InputDriver,
    abort: &AtomicBool,
    work: impl FnOnce() -> T + Send + 'static,
) -> TutorResult<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    loop {
        if abort.load(Ordering::SeqCst) || input.interrupted() {
            return Err(TutorError::Aborted("stopped by the user".into()));
        }
        match rx.recv_timeout(Duration::from_millis(40)) {
            Ok(v) => return Ok(v),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(TutorError::Llm("the model call ended".into()));
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Chapter {
        index: usize,
        title: String,
    },
    Step {
        chapter: usize,
        step_id: String,
    },
    Repair {
        step_id: String,
        attempt: u32,
        error: String,
    },
    ChapterChecked {
        index: usize,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub steps_ok: usize,
    pub repairs: u32,
    pub chapters_checked: usize,
}

fn pin(step: &mut Step, rect: Option<Rect>) {
    step.verified_rect = rect;
    let Some(r) = rect else { return };
    let fix = |s: &mut Selector| {
        if s.rect.is_none() {
            s.rect = Some(r);
        }
    };
    match &mut step.action {
        Action::Click { target }
        | Action::DoubleClick { target }
        | Action::RightClick { target }
        | Action::Hover { target } => fix(target),
        Action::Type {
            target: Some(target),
            ..
        }
        | Action::Scroll {
            target: Some(target),
            ..
        } => fix(target),
        _ => {}
    }
}

/// Run chapters `from_chapter..`. On failure the script keeps the progress made so far.
pub fn rehearse(
    script: &mut TutorialScript,
    rig: &Rig,
    guard: Guard,
    window: WindowInfo,
    opts: &RehearseOpts,
    on_event: &mut dyn FnMut(Event),
) -> TutorResult<Report> {
    script.validate()?;
    if opts.from_chapter >= script.chapters.len() {
        return Err(TutorError::Script(
            "from_chapter is past the last chapter".into(),
        ));
    }
    let mut report = Report::default();
    let abort = Arc::clone(&guard.abort);
    let mut player = Player::new(rig, guard, window, Mode::Rehearse);
    let goal = script.goal.clone();
    for ci in opts.from_chapter..script.chapters.len() {
        script.chapters[ci].checked = false;
        on_event(Event::Chapter {
            index: ci,
            title: script.chapters[ci].title.clone(),
        });
        player.begin_chapter(ci);
        let mut si = 0;
        while si < script.chapters[ci].steps.len() {
            let mut attempt = 0;
            loop {
                let step = script.chapters[ci].steps[si].clone();
                match player.run_step(&step) {
                    Ok(marker) => {
                        pin(&mut script.chapters[ci].steps[si], marker.rect);
                        report.steps_ok += 1;
                        on_event(Event::Step {
                            chapter: ci,
                            step_id: step.id.clone(),
                        });
                        break;
                    }
                    Err(TutorError::Step(msg)) => {
                        let ctx = format!("chapter {} step {}: {msg}", ci + 1, step.id);
                        let Some((chat, copts)) = opts.chat.as_ref() else {
                            return Err(TutorError::Step(ctx));
                        };
                        if attempt >= opts.max_repairs {
                            return Err(TutorError::Step(format!(
                                "{ctx} (gave up after {attempt} repairs)"
                            )));
                        }
                        attempt += 1;
                        report.repairs += 1;
                        on_event(Event::Repair {
                            step_id: step.id.clone(),
                            attempt,
                            error: msg.clone(),
                        });
                        let snap = rig.ui.snapshot(player.window.hwnd)?;
                        let done = &script.chapters[ci].steps[..si];
                        let msgs = repair_messages(
                            &goal,
                            &script.chapters[ci].title,
                            done,
                            &step,
                            &msg,
                            &snap,
                        );
                        let reply = wait_for(&*rig.input, &abort, {
                            let chat = Arc::clone(chat);
                            let msgs = msgs.clone();
                            let copts = copts.clone();
                            move || chat.chat(&msgs, &copts)
                        })??;
                        script.chapters[ci].steps[si] = parse_step(&reply, &step.id)?;
                    }
                    Err(e) => return Err(e),
                }
            }
            si += 1;
        }
        script.chapters[ci].checked = true;
        report.chapters_checked += 1;
        on_event(Event::ChapterChecked { index: ci });
    }
    Ok(report)
}
