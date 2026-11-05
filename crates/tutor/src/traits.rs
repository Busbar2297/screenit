//! Seams the tutorial engine drives: UI Automation, synthetic input, app launching, a chat
//! model and a clock. `screenit-platform` implements the first three, `screenit-llm` the chat
//! model; `screenit-testkit` fakes all of them.

use crate::error::{TutorError, TutorResult};
use screenit_geom::Rect;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// A top-level window.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub hwnd: u64,
    pub pid: u32,
    pub title: String,
    pub rect: Rect,
}

/// One UI Automation element, flattened in document (pre-)order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiElement {
    /// Short stable id inside one snapshot (index), what the model refers to.
    pub id: u32,
    pub role: String,
    pub name: String,
    pub automation_id: String,
    pub rect: Rect,
    pub enabled: bool,
    pub focused: bool,
    pub value: String,
    pub password: bool,
    pub depth: u8,
    /// Ancestor chain, `Window[App]/Pane/ToolBar[Main]`; filled by `UiSnapshot::fill_paths`.
    #[serde(default)]
    pub path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSnapshot {
    pub window: WindowInfo,
    pub elements: Vec<UiElement>,
}

impl UiSnapshot {
    /// Assign ids (index) and ancestor paths from the depth-ordered element list.
    pub fn finish(mut self) -> Self {
        let mut stack: Vec<String> = Vec::new();
        for (i, e) in self.elements.iter_mut().enumerate() {
            e.id = i as u32;
            stack.truncate(usize::from(e.depth));
            e.path = stack.join("/");
            let label = if e.name.is_empty() {
                e.role.clone()
            } else {
                format!(
                    "{}[{}]",
                    e.role,
                    e.name.chars().take(40).collect::<String>()
                )
            };
            stack.push(label);
        }
        self
    }

    pub fn find(&self, id: u32) -> Option<&UiElement> {
        self.elements.get(id as usize).filter(|e| e.id == id)
    }

    pub fn focused(&self) -> Option<&UiElement> {
        self.elements.iter().find(|e| e.focused)
    }

    /// Compact text for a language model: one line per useful element, capped.
    pub fn compact(&self, max_lines: usize) -> String {
        let mut out = format!("window \"{}\"\n", self.window.title);
        let mut n = 0;
        for e in &self.elements {
            if e.rect.is_empty() || (e.name.is_empty() && e.automation_id.is_empty()) {
                continue;
            }
            if n >= max_lines {
                out.push_str("...\n");
                break;
            }
            n += 1;
            let indent = " ".repeat(usize::from(e.depth.min(8)));
            out.push_str(&format!("{indent}#{} {}", e.id, e.role));
            if !e.name.is_empty() {
                out.push_str(&format!(
                    " \"{}\"",
                    e.name.chars().take(60).collect::<String>()
                ));
            }
            if !e.automation_id.is_empty() {
                out.push_str(&format!(" id={}", e.automation_id));
            }
            if !e.enabled {
                out.push_str(" disabled");
            }
            if e.focused {
                out.push_str(" focused");
            }
            if !e.password && !e.value.is_empty() {
                out.push_str(&format!(
                    " value=\"{}\"",
                    e.value.chars().take(40).collect::<String>()
                ));
            }
            out.push_str(&format!(
                " at {},{} {}x{}",
                e.rect.x, e.rect.y, e.rect.w, e.rect.h
            ));
            out.push('\n');
        }
        out
    }
}

pub trait UiTree: Send + Sync {
    /// Snapshot a window (bounded by element count, depth and a deadline).
    fn snapshot(&self, window: u64) -> TutorResult<UiSnapshot>;
    /// The window that currently has keyboard focus.
    fn foreground(&self) -> Option<WindowInfo>;
    /// Other apps' windows, front to back. The first is the one left open behind screenIt.
    fn other_windows(&self) -> Vec<WindowInfo> {
        Vec::new()
    }
    /// Whether `window` still exists.
    fn is_alive(&self, window: u64) -> bool;
    /// A screenshot of `window`, for a vision model. `None` when a picture is not available.
    fn capture(&self, _window: u64) -> Option<Shot> {
        None
    }
}

/// One screenshot: straight RGBA8, row-major. Sent to the vision model when a step gets stuck.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

pub trait InputDriver: Send + Sync {
    fn move_to(&self, x: i32, y: i32) -> TutorResult<()>;
    fn click(&self, x: i32, y: i32, button: MouseButton, count: u8) -> TutorResult<()>;
    fn drag(&self, from: (i32, i32), to: (i32, i32)) -> TutorResult<()>;
    fn type_text(&self, text: &str, delay_ms: u32) -> TutorResult<()>;
    /// A chord such as `ctrl+s`, `enter`, `alt+f4`.
    fn key(&self, chord: &str) -> TutorResult<()>;
    fn scroll(&self, x: i32, y: i32, dy: i32) -> TutorResult<()>;
    /// True when the user pressed the kill hotkey or moved the mouse away from where this driver
    /// last put it. Checked before every action.
    fn interrupted(&self) -> bool;
    /// A run starts: forget the previous cursor position and clear any stop latch.
    fn begin(&self) {}
    /// Remember the run's abort flag so input calls stop as soon as Stop is pressed.
    fn bind_abort(&self, _flag: &Arc<AtomicBool>) {}
    /// The Stop button. Later `interrupted` checks stay true until `begin`.
    fn user_stop(&self) {}
}

pub trait LlmChat: Send + Sync {
    fn chat(&self, messages: &[ChatMessage], opts: &ChatOptions) -> TutorResult<String>;
    /// Same as `chat`, with a screenshot attached to the last user message.
    fn chat_image(
        &self,
        messages: &[ChatMessage],
        _image: &Shot,
        opts: &ChatOptions,
    ) -> TutorResult<String> {
        self.chat(messages, opts)
    }
}

pub trait AppLauncher: Send + Sync {
    /// Start `target` (an exe path, a document or a URL) and return its main window.
    fn launch(&self, target: &str, args: &str) -> TutorResult<WindowInfo>;
    /// First visible top-level window whose title contains `title` (case-insensitive).
    fn find_window(&self, title: &str) -> Option<WindowInfo>;
    /// Bring a window to the front.
    fn focus(&self, window: u64) -> TutorResult<()>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(c: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: c.into(),
        }
    }
    pub fn user(c: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: c.into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatOptions {
    pub url: String,
    pub model: String,
    pub api_key: String,
    pub timeout_secs: u32,
    /// Ask the server for a JSON object reply.
    pub json: bool,
    pub max_tokens: u32,
}

/// Time source and sleeper so scripts run in virtual time under test.
pub trait Pace: Send + Sync {
    fn now_ms(&self) -> u64;
    fn sleep_ms(&self, ms: u32);
}

/// Real clock.
pub struct RealPace;

impl Pace for RealPace {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
    fn sleep_ms(&self, ms: u32) {
        std::thread::sleep(std::time::Duration::from_millis(u64::from(ms.min(60_000))));
    }
}

/// Everything a run needs, bundled.
#[derive(Clone)]
pub struct Rig {
    pub ui: std::sync::Arc<dyn UiTree>,
    pub input: std::sync::Arc<dyn InputDriver>,
    pub launcher: std::sync::Arc<dyn AppLauncher>,
    pub pace: std::sync::Arc<dyn Pace>,
}

impl Rig {
    pub fn window(&self, app: &crate::script::AppTarget) -> TutorResult<WindowInfo> {
        if !app.window_title.is_empty() {
            if let Some(w) = self.launcher.find_window(&app.window_title) {
                return Ok(w);
            }
        }
        let launch = app.launch.trim();
        if launch.is_empty() {
            return Err(TutorError::Launch(format!(
                "no window titled `{}` and nothing to launch",
                app.window_title
            )));
        }
        // A bare name is the window title typed into the app box. Use that window. A path still launches.
        if !launch.contains('\\')
            && !launch.contains('/')
            && let Some(w) = self.launcher.find_window(launch)
        {
            return Ok(w);
        }
        self.launcher.launch(launch, &app.args)
    }
}
