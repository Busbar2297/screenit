//! Runs one step against a live app: resolve the target through UI Automation, act, then check
//! that the expected result happened. Used for rehearsal (fast) and performance (human pace).

use crate::error::{TutorError, TutorResult};
use crate::path::cursor_path;
use crate::safety::Guard;
use crate::script::{Action, Selector, Step, center, label};
use crate::traits::{MouseButton, Rig, UiElement, UiSnapshot, WindowInfo};
use screenit_geom::Rect;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Fast, no cursor theatrics. Used while checking a script.
    Rehearse,
    /// Human pace, for the recording.
    Perform,
}

/// What happened at one step, timed from the start of its chapter take.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub step_id: String,
    pub chapter: usize,
    pub kind: String,
    pub narration: String,
    pub start_ms: u64,
    /// When the click, keystroke or drag happened.
    pub act_ms: u64,
    pub end_ms: u64,
    /// Target in screen pixels.
    pub rect: Option<Rect>,
    /// Typed text (never recorded for password fields: those are refused).
    pub text: String,
}

pub struct Player<'a> {
    pub rig: &'a Rig,
    pub guard: Guard,
    pub window: WindowInfo,
    pub mode: Mode,
    /// Index of the chapter being played (copied into markers).
    pub chapter: usize,
    /// Clock value that counts as time zero for markers.
    pub origin_ms: u64,
    last_pos: (i32, i32),
}

const POLL_MS: u32 = 150;

impl<'a> Player<'a> {
    pub fn new(rig: &'a Rig, guard: Guard, window: WindowInfo, mode: Mode) -> Self {
        rig.input.bind_abort(&guard.abort);
        rig.input.begin();
        let last_pos = center(window.rect);
        Self {
            rig,
            guard,
            window,
            mode,
            chapter: 0,
            origin_ms: rig.pace.now_ms(),
            last_pos,
        }
    }

    pub fn begin_chapter(&mut self, chapter: usize) {
        self.chapter = chapter;
        self.origin_ms = self.rig.pace.now_ms();
    }

    fn rel(&self) -> u64 {
        self.rig.pace.now_ms().saturating_sub(self.origin_ms)
    }

    fn snap(&self) -> TutorResult<UiSnapshot> {
        self.rig.ui.snapshot(self.window.hwnd)
    }

    /// Poll until `sel` resolves to an enabled element.
    fn find(&mut self, sel: &Selector, timeout_ms: u32) -> TutorResult<(UiElement, UiSnapshot)> {
        let deadline = self.rig.pace.now_ms().saturating_add(u64::from(timeout_ms));
        let mut why = "not found";
        loop {
            self.check_alive()?;
            let snap = self.snap()?;
            if let Some(e) = sel.resolve(&snap) {
                if e.enabled {
                    return Ok((e.clone(), snap));
                }
                why = "disabled";
            }
            if self.rig.pace.now_ms() >= deadline {
                return Err(TutorError::Step(format!("`{}` is {why}", label(sel))));
            }
            self.rig.pace.sleep_ms(POLL_MS);
        }
    }

    /// Sleep in short slices so Stop and a mouse takeover land within a fraction of a second.
    fn pause(&self, ms: u32) -> TutorResult<()> {
        let mut left = ms;
        while left > 0 {
            self.check_alive()?;
            let slice = left.min(40);
            self.rig.pace.sleep_ms(slice);
            left -= slice;
        }
        self.check_alive()
    }

    fn check_alive(&self) -> TutorResult<()> {
        if self.rig.input.interrupted() {
            self.guard
                .abort
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if self.guard.aborted() {
            return Err(TutorError::Aborted("stopped by the user".into()));
        }
        Ok(())
    }

    fn glide(&mut self, to: (i32, i32)) -> TutorResult<()> {
        match self.mode {
            Mode::Rehearse => self.rig.input.move_to(to.0, to.1)?,
            Mode::Perform => {
                for p in cursor_path(self.last_pos, to) {
                    self.check_alive()?;
                    self.rig.pace.sleep_ms(p.dt_ms);
                    self.rig.input.move_to(p.x, p.y)?;
                }
                self.pause(280)?;
            }
        }
        self.last_pos = to;
        Ok(())
    }

    /// Play one step. Returns its marker. `Step` errors can be repaired; `Safety` and
    /// `Aborted` errors end the run.
    pub fn run_step(&mut self, step: &Step) -> TutorResult<Marker> {
        let start = self.rel();
        self.guard.check(step, &*self.rig.ui, &*self.rig.input)?;
        let (rect, act_ms, text) = self.act(step)?;
        self.pause(if self.mode == Mode::Perform { 600 } else { 120 })?;
        self.verify(step)?;
        if self.mode == Mode::Perform {
            let want = narration_ms(&step.narration);
            let spent = self.rel().saturating_sub(start);
            if want > spent {
                self.pause((want - spent).min(15_000) as u32)?;
            }
        }
        Ok(Marker {
            step_id: step.id.clone(),
            chapter: self.chapter,
            kind: step.action.kind().to_string(),
            narration: step.narration.clone(),
            start_ms: start,
            act_ms,
            end_ms: self.rel(),
            rect,
            text,
        })
    }

    fn act(&mut self, step: &Step) -> TutorResult<(Option<Rect>, u64, String)> {
        let t = step.timeout_ms;
        let input = self.rig.input.clone();
        let click = |this: &mut Self, sel: &Selector, b: MouseButton, n: u8| {
            let (e, _) = this.find(sel, t)?;
            let c = center(e.rect);
            this.glide(c)?;
            let at = this.rel();
            input.click(c.0, c.1, b, n)?;
            Ok::<_, TutorError>((Some(e.rect), at, String::new()))
        };
        match &step.action {
            Action::Click { target } => click(self, target, MouseButton::Left, 1),
            Action::DoubleClick { target } => click(self, target, MouseButton::Left, 2),
            Action::RightClick { target } => click(self, target, MouseButton::Right, 1),
            Action::Hover { target } => {
                let (e, _) = self.find(target, t)?;
                self.glide(center(e.rect))?;
                let at = self.rel();
                Ok((Some(e.rect), at, String::new()))
            }
            Action::Type { target, text } => {
                let mut rect = None;
                let mut id = None;
                if let Some(sel) = target {
                    let (e, _) = self.find(sel, t)?;
                    self.guard.check_typing(&self.snap()?, Some(e.id))?;
                    if !e.focused {
                        let c = center(e.rect);
                        self.glide(c)?;
                        input.click(c.0, c.1, MouseButton::Left, 1)?;
                    }
                    rect = Some(e.rect);
                    id = Some(e.id);
                }
                self.guard.check_typing(&self.snap()?, id)?;
                let at = self.rel();
                let delay = if self.mode == Mode::Perform { 65 } else { 4 };
                input.type_text(text, delay)?;
                Ok((rect, at, text.clone()))
            }
            Action::Key { keys } => {
                let at = self.rel();
                input.key(keys)?;
                Ok((None, at, keys.clone()))
            }
            Action::Scroll { target, dy } => {
                let (rect, c) = match target {
                    Some(sel) => {
                        let (e, _) = self.find(sel, t)?;
                        (Some(e.rect), center(e.rect))
                    }
                    None => (None, center(self.window.rect)),
                };
                self.glide(c)?;
                let at = self.rel();
                input.scroll(c.0, c.1, *dy)?;
                Ok((rect, at, String::new()))
            }
            Action::Drag { from, to } => {
                let (a, _) = self.find(from, t)?;
                let (b, _) = self.find(to, t)?;
                let (ca, cb) = (center(a.rect), center(b.rect));
                self.glide(ca)?;
                let at = self.rel();
                input.drag(ca, cb)?;
                self.last_pos = cb;
                Ok((Some(a.rect.union(&b.rect)), at, String::new()))
            }
            Action::Wait { ms } => {
                self.pause(*ms)?;
                Ok((None, self.rel(), String::new()))
            }
            Action::Focus => {
                self.rig.launcher.focus(self.window.hwnd)?;
                Ok((None, self.rel(), String::new()))
            }
        }
    }

    fn verify(&mut self, step: &Step) -> TutorResult<()> {
        if step.expect.is_empty() {
            return Ok(());
        }
        let deadline = self
            .rig
            .pace
            .now_ms()
            .saturating_add(u64::from(step.timeout_ms));
        loop {
            self.check_alive()?;
            let snap = self.snap()?;
            let unmet = step.expect.iter().find(|e| !e.holds(&snap));
            let Some(unmet) = unmet else {
                return Ok(());
            };
            if self.rig.pace.now_ms() >= deadline {
                return Err(TutorError::Step(format!(
                    "expected {} but it did not happen",
                    unmet.describe()
                )));
            }
            self.rig.pace.sleep_ms(POLL_MS);
        }
    }
}

/// Time a viewer needs to read the narration, about 22 characters per second.
pub fn narration_ms(text: &str) -> u64 {
    let n = text.chars().count() as u64;
    if n == 0 {
        0
    } else {
        (800 + n * 45).min(12_000)
    }
}
