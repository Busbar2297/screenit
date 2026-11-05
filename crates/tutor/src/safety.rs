//! Rules that stop autonomous input before it does harm.

use crate::error::{TutorError, TutorResult};
use crate::script::{Action, Step};
use crate::traits::{InputDriver, UiSnapshot, UiTree};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Per-run guard shared with the abort command.
#[derive(Clone)]
pub struct Guard {
    pub abort: Arc<AtomicBool>,
    /// Process that owns the target window; input may only reach it.
    pub target_pid: u32,
    /// Main target window id.
    pub target_window: u64,
    pub allow_destructive: bool,
    /// Actions still allowed in this run.
    pub budget: u32,
    /// The app this run started on. Dialogs may change `target_pid`; this does not.
    pub home_pid: u32,
    pub home_window: u64,
}

impl Guard {
    pub fn new(target_pid: u32, target_window: u64, abort: Arc<AtomicBool>) -> Self {
        Self {
            abort,
            target_pid,
            target_window,
            allow_destructive: false,
            budget: 5000,
            home_pid: target_pid,
            home_window: target_window,
        }
    }

    pub fn aborted(&self) -> bool {
        self.abort.load(Ordering::SeqCst)
    }

    /// Run before every step that touches the keyboard or mouse.
    pub fn check(
        &mut self,
        step: &Step,
        ui: &dyn UiTree,
        input: &dyn InputDriver,
    ) -> TutorResult<()> {
        if input.interrupted() {
            self.abort.store(true, Ordering::SeqCst);
        }
        if self.aborted() {
            return Err(TutorError::Aborted(format!(
                "stopped before step {}",
                step.id
            )));
        }
        if self.budget == 0 {
            return Err(TutorError::Safety("action budget used up".into()));
        }
        self.budget -= 1;
        if step.destructive && !self.allow_destructive {
            return Err(TutorError::Safety(format!(
                "step {} is marked destructive; run with allow_destructive after reviewing it",
                step.id
            )));
        }
        if !self.moves_input(&step.action) {
            return Ok(());
        }
        if !ui.is_alive(self.target_window) {
            return Err(TutorError::Safety("the target window is gone".into()));
        }
        match ui.foreground() {
            Some(w) if w.pid == self.target_pid => Ok(()),
            Some(w) => Err(TutorError::Safety(format!(
                "foreground window `{}` does not belong to the target app",
                w.title.chars().take(60).collect::<String>()
            ))),
            None => Err(TutorError::Safety("no foreground window".into())),
        }
    }

    fn moves_input(&self, a: &Action) -> bool {
        !matches!(a, Action::Wait { .. })
    }

    /// Typing into a password field is never allowed.
    pub fn check_typing(&self, snap: &UiSnapshot, target_id: Option<u32>) -> TutorResult<()> {
        let el = target_id
            .and_then(|id| snap.find(id))
            .or_else(|| snap.focused());
        if el.is_some_and(|e| e.password) {
            return Err(TutorError::Safety(
                "refusing to type into a password field".into(),
            ));
        }
        Ok(())
    }
}
