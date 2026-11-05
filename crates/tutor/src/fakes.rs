//! In-memory stand-ins for the platform seams, used by tests across the workspace. `FakeWorld`
//! is a tiny scriptable "app": elements with rects, rules that fire on clicks and keys.

use crate::error::{TutorError, TutorResult};
use crate::perform::{ChapterRecorder, Segment};
use crate::traits::{
    AppLauncher, ChatMessage, ChatOptions, InputDriver, LlmChat, MouseButton, Pace, Shot,
    UiElement, UiSnapshot, UiTree, WindowInfo,
};
use screenit_geom::Rect;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone, Debug)]
pub enum Effect {
    Show(UiElement),
    Hide(String),
    SetValue { name: String, value: String },
    Retitle(String),
}

#[derive(Clone, Debug)]
pub enum Trigger {
    Click(String),
    Key(String),
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub when: Trigger,
    pub effects: Vec<Effect>,
}

struct World {
    window: WindowInfo,
    elements: Vec<UiElement>,
    rules: Vec<Rule>,
    log: Vec<String>,
    foreground_pid: u32,
    alive: bool,
    interrupted: bool,
    launched: Vec<String>,
    shot: Option<Shot>,
    front: Option<WindowInfo>,
    front_elements: Vec<UiElement>,
}

pub struct FakeWorld {
    w: Mutex<World>,
}

/// An element with sensible defaults.
pub fn element(role: &str, name: &str, rect: Rect) -> UiElement {
    UiElement {
        role: role.into(),
        name: name.into(),
        rect,
        enabled: true,
        depth: 1,
        ..UiElement::default()
    }
}

impl FakeWorld {
    pub const PID: u32 = 4242;

    pub fn new(title: &str) -> Arc<Self> {
        let window = WindowInfo {
            hwnd: 7,
            pid: Self::PID,
            title: title.into(),
            rect: Rect::new(100, 100, 800, 600),
        };
        Arc::new(Self {
            w: Mutex::new(World {
                elements: vec![UiElement {
                    depth: 0,
                    ..element("Window", title, window.rect)
                }],
                window,
                rules: Vec::new(),
                log: Vec::new(),
                foreground_pid: Self::PID,
                alive: true,
                interrupted: false,
                launched: Vec::new(),
                shot: None,
                front: None,
                front_elements: Vec::new(),
            }),
        })
    }

    pub fn add(&self, e: UiElement) {
        lock(&self.w).elements.push(e);
    }

    pub fn rule(&self, when: Trigger, effects: Vec<Effect>) {
        lock(&self.w).rules.push(Rule { when, effects });
    }

    pub fn log(&self) -> Vec<String> {
        lock(&self.w).log.clone()
    }

    pub fn value_of(&self, name: &str) -> Option<String> {
        lock(&self.w)
            .elements
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.value.clone())
    }

    pub fn has(&self, name: &str) -> bool {
        lock(&self.w).elements.iter().any(|e| e.name == name)
    }

    pub fn set_foreground_pid(&self, pid: u32) {
        lock(&self.w).foreground_pid = pid;
    }

    pub fn set_alive(&self, alive: bool) {
        lock(&self.w).alive = alive;
    }

    pub fn set_interrupted(&self, v: bool) {
        lock(&self.w).interrupted = v;
    }

    pub fn launched(&self) -> Vec<String> {
        lock(&self.w).launched.clone()
    }

    /// Screenshot returned for the main window and for a window set with `set_front`.
    pub fn set_shot(&self, shot: Shot) {
        lock(&self.w).shot = Some(shot);
    }

    /// A dialog of the same app, now in front. Snapshots of its handle see `elements`.
    pub fn set_front(&self, title: &str, elements: Vec<UiElement>) {
        let mut w = lock(&self.w);
        w.front = Some(WindowInfo {
            hwnd: 99,
            pid: Self::PID,
            title: title.into(),
            rect: Rect::new(200, 200, 400, 300),
        });
        w.front_elements = elements;
    }

    fn apply(w: &mut World, trig: &Trigger) {
        let effects: Vec<Effect> = w
            .rules
            .iter()
            .filter(|r| match (&r.when, trig) {
                (Trigger::Click(a), Trigger::Click(b)) => a == b,
                (Trigger::Key(a), Trigger::Key(b)) => a == b,
                _ => false,
            })
            .flat_map(|r| r.effects.clone())
            .collect();
        for e in effects {
            match e {
                Effect::Show(el) => w.elements.push(el),
                Effect::Hide(n) => w.elements.retain(|x| x.name != n),
                Effect::SetValue { name, value } => {
                    for x in w.elements.iter_mut().filter(|x| x.name == name) {
                        x.value = value.clone();
                    }
                }
                Effect::Retitle(t) => w.window.title = t,
            }
        }
    }
}

impl UiTree for FakeWorld {
    fn snapshot(&self, window: u64) -> TutorResult<UiSnapshot> {
        let w = lock(&self.w);
        if !w.alive {
            return Err(TutorError::Ui("no such window".into()));
        }
        if let Some(front) = w.front.clone().filter(|f| f.hwnd == window) {
            return Ok(UiSnapshot {
                window: front,
                elements: w.front_elements.clone(),
            }
            .finish());
        }
        if window != w.window.hwnd {
            return Err(TutorError::Ui("no such window".into()));
        }
        Ok(UiSnapshot {
            window: w.window.clone(),
            elements: w.elements.clone(),
        }
        .finish())
    }
    fn other_windows(&self) -> Vec<WindowInfo> {
        let w = lock(&self.w);
        let mut out = Vec::new();
        if let Some(front) = &w.front {
            out.push(front.clone());
        }
        if w.alive {
            out.push(w.window.clone());
        }
        out
    }
    fn foreground(&self) -> Option<WindowInfo> {
        let w = lock(&self.w);
        if let Some(front) = &w.front {
            return Some(front.clone());
        }
        Some(WindowInfo {
            pid: w.foreground_pid,
            ..w.window.clone()
        })
    }
    fn is_alive(&self, window: u64) -> bool {
        let w = lock(&self.w);
        w.alive && (window == w.window.hwnd || w.front.as_ref().is_some_and(|f| f.hwnd == window))
    }
    fn capture(&self, window: u64) -> Option<Shot> {
        let w = lock(&self.w);
        let known = window == w.window.hwnd || w.front.as_ref().is_some_and(|f| f.hwnd == window);
        if known { w.shot.clone() } else { None }
    }
}

impl InputDriver for FakeWorld {
    fn move_to(&self, x: i32, y: i32) -> TutorResult<()> {
        lock(&self.w).log.push(format!("move {x},{y}"));
        Ok(())
    }
    fn click(&self, x: i32, y: i32, button: MouseButton, count: u8) -> TutorResult<()> {
        let mut w = lock(&self.w);
        w.log.push(format!("click {x},{y} {button:?} x{count}"));
        let p = screenit_geom::Point::new(x, y);
        let hit = w
            .elements
            .iter()
            .rposition(|e| e.enabled && e.depth > 0 && e.rect.contains(p));
        if let Some(i) = hit {
            for e in &mut w.elements {
                e.focused = false;
            }
            w.elements[i].focused = true;
            let name = w.elements[i].name.clone();
            Self::apply(&mut w, &Trigger::Click(name));
        }
        Ok(())
    }
    fn drag(&self, from: (i32, i32), to: (i32, i32)) -> TutorResult<()> {
        lock(&self.w).log.push(format!("drag {from:?}->{to:?}"));
        Ok(())
    }
    fn type_text(&self, text: &str, _delay_ms: u32) -> TutorResult<()> {
        let mut w = lock(&self.w);
        w.log.push(format!("type {text}"));
        for e in w.elements.iter_mut().filter(|e| e.focused) {
            e.value.push_str(text);
        }
        Ok(())
    }
    fn key(&self, chord: &str) -> TutorResult<()> {
        let mut w = lock(&self.w);
        w.log.push(format!("key {chord}"));
        Self::apply(&mut w, &Trigger::Key(chord.to_string()));
        Ok(())
    }
    fn scroll(&self, x: i32, y: i32, dy: i32) -> TutorResult<()> {
        lock(&self.w).log.push(format!("scroll {x},{y} {dy}"));
        Ok(())
    }
    fn interrupted(&self) -> bool {
        lock(&self.w).interrupted
    }
}

impl AppLauncher for FakeWorld {
    fn launch(&self, target: &str, _args: &str) -> TutorResult<WindowInfo> {
        let mut w = lock(&self.w);
        w.launched.push(target.to_string());
        Ok(w.window.clone())
    }
    fn find_window(&self, title: &str) -> Option<WindowInfo> {
        let w = lock(&self.w);
        w.window
            .title
            .to_lowercase()
            .contains(&title.to_lowercase())
            .then(|| w.window.clone())
    }
    fn focus(&self, _window: u64) -> TutorResult<()> {
        lock(&self.w).log.push("focus".into());
        Ok(())
    }
}

/// Virtual clock: sleeping just moves time forward.
#[derive(Default)]
pub struct FakePace {
    t: AtomicU64,
}

impl FakePace {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl Pace for FakePace {
    fn now_ms(&self) -> u64 {
        self.t.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, ms: u32) {
        self.t.fetch_add(u64::from(ms), Ordering::SeqCst);
    }
}

/// Canned model replies, in order.
pub struct FakeLlm {
    replies: Mutex<VecDeque<TutorResult<String>>>,
    calls: Mutex<Vec<Vec<ChatMessage>>>,
    images: AtomicU32,
}

impl FakeLlm {
    pub fn new(replies: Vec<TutorResult<String>>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            calls: Mutex::new(Vec::new()),
            images: AtomicU32::new(0),
        })
    }
    pub fn calls(&self) -> Vec<Vec<ChatMessage>> {
        lock(&self.calls).clone()
    }
    pub fn image_calls(&self) -> u32 {
        self.images.load(Ordering::SeqCst)
    }
}

impl LlmChat for FakeLlm {
    fn chat(&self, messages: &[ChatMessage], _opts: &ChatOptions) -> TutorResult<String> {
        lock(&self.calls).push(messages.to_vec());
        lock(&self.replies)
            .pop_front()
            .unwrap_or_else(|| Err(TutorError::Llm("no more canned replies".into())))
    }
    fn chat_image(
        &self,
        messages: &[ChatMessage],
        _image: &Shot,
        opts: &ChatOptions,
    ) -> TutorResult<String> {
        self.images.fetch_add(1, Ordering::SeqCst);
        self.chat(messages, opts)
    }
}

/// Recorder that notes chapters and "records" the virtual time that passes.
pub struct FakeChapterRecorder {
    pace: Arc<dyn Pace>,
    started: Option<(usize, u64)>,
    pub log: Vec<String>,
}

impl FakeChapterRecorder {
    pub fn new(pace: Arc<dyn Pace>) -> Self {
        Self {
            pace,
            started: None,
            log: Vec::new(),
        }
    }
}

impl ChapterRecorder for FakeChapterRecorder {
    fn start(&mut self, chapter: usize, region: Rect) -> TutorResult<()> {
        self.log
            .push(format!("start {chapter} {}x{}", region.w, region.h));
        self.started = Some((chapter, self.pace.now_ms()));
        Ok(())
    }
    fn stop(&mut self) -> TutorResult<Segment> {
        let (c, t0) = self
            .started
            .take()
            .ok_or_else(|| TutorError::Record("not recording".into()))?;
        self.log.push(format!("stop {c}"));
        Ok(Segment {
            path: PathBuf::from(format!("chapter{}.mp4", c + 1)),
            duration_ms: self.pace.now_ms().saturating_sub(t0),
        })
    }
}
