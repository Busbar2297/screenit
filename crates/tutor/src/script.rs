//! The tutorial script: chapters of steps, each with an action, narration and expected result.
//! Plain serde data so a person, the planner or an MCP agent can write it.

use crate::error::{TutorError, TutorResult};
use crate::traits::{UiElement, UiSnapshot};
use screenit_geom::Rect;
use serde::{Deserialize, Serialize};

pub const SCRIPT_VERSION: u32 = 1;
pub const MAX_CHAPTERS: usize = 60;
pub const MAX_STEPS: usize = 200;
pub const MAX_TEXT: usize = 2000;

/// How to find an element again. `automation_id` wins; otherwise role+name (+ancestor hint).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub automation_id: String,
    /// Text the ancestor path must contain.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub within: String,
    /// Which match to take when several fit (0 = first).
    #[serde(default)]
    pub nth: u32,
    /// Last resort: where the element was during rehearsal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<Rect>,
}

impl Selector {
    pub fn named(role: &str, name: &str) -> Self {
        Self {
            role: role.into(),
            name: name.into(),
            ..Self::default()
        }
    }

    fn is_blank(&self) -> bool {
        self.role.is_empty()
            && self.name.is_empty()
            && self.automation_id.is_empty()
            && self.rect.is_none()
    }

    /// The matching element, or `None`.
    pub fn resolve<'a>(&self, snap: &'a UiSnapshot) -> Option<&'a UiElement> {
        if self.is_blank() {
            return None;
        }
        let usable = |e: &&UiElement| !e.rect.is_empty();
        let within = |e: &&UiElement| {
            self.within.is_empty() || lower(&e.path).contains(&lower(&self.within))
        };
        let nth = self.nth as usize;
        if !self.automation_id.is_empty() {
            let hits: Vec<_> = snap
                .elements
                .iter()
                .filter(usable)
                .filter(within)
                .filter(|e| e.automation_id == self.automation_id)
                .collect();
            if let Some(e) = hits.get(nth).or(hits.first()) {
                return Some(e);
            }
        }
        if !self.role.is_empty() || !self.name.is_empty() {
            let role_ok = |e: &&UiElement| self.role.is_empty() || eq(&e.role, &self.role);
            let pool: Vec<_> = snap
                .elements
                .iter()
                .filter(usable)
                .filter(within)
                .filter(role_ok)
                .collect();
            if self.name.is_empty() {
                if let Some(e) = pool.get(nth) {
                    return Some(e);
                }
            } else {
                let exact: Vec<_> = pool.iter().filter(|e| eq(&e.name, &self.name)).collect();
                if let Some(e) = exact.get(nth).or(exact.first()) {
                    return Some(**e);
                }
                let part: Vec<_> = pool
                    .iter()
                    .filter(|e| lower(&e.name).contains(&lower(&self.name)))
                    .collect();
                if let Some(e) = part.get(nth).or(part.first()) {
                    return Some(**e);
                }
            }
        }
        let want = self.rect?;
        let (cx, cy) = center(want);
        snap.elements
            .iter()
            .filter(usable)
            .filter(|e| {
                e.rect.w <= want.w.saturating_mul(2) && e.rect.h <= want.h.saturating_mul(2)
            })
            .map(|e| {
                let (x, y) = center(e.rect);
                (i64::from(x - cx).abs() + i64::from(y - cy).abs(), e)
            })
            .filter(|(d, _)| *d <= 40)
            .min_by_key(|(d, _)| *d)
            .map(|(_, e)| e)
    }
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}
fn eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b) || lower(a) == lower(b)
}

/// Centre point of a rect (saturating).
pub fn center(r: Rect) -> (i32, i32) {
    (
        r.x.saturating_add((r.w / 2) as i32),
        r.y.saturating_add((r.h / 2) as i32),
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Click {
        target: Selector,
    },
    DoubleClick {
        target: Selector,
    },
    RightClick {
        target: Selector,
    },
    Hover {
        target: Selector,
    },
    Type {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<Selector>,
        text: String,
    },
    Key {
        keys: String,
    },
    Scroll {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<Selector>,
        dy: i32,
    },
    Drag {
        from: Selector,
        to: Selector,
    },
    Wait {
        ms: u32,
    },
    /// Start or focus the app (normally done once by `AppTarget`).
    Focus,
}

impl Action {
    pub fn kind(&self) -> &'static str {
        match self {
            Action::Click { .. } => "click",
            Action::DoubleClick { .. } => "double_click",
            Action::RightClick { .. } => "right_click",
            Action::Hover { .. } => "hover",
            Action::Type { .. } => "type",
            Action::Key { .. } => "key",
            Action::Scroll { .. } => "scroll",
            Action::Drag { .. } => "drag",
            Action::Wait { .. } => "wait",
            Action::Focus => "focus",
        }
    }
}

/// What must be true after a step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expect {
    Appears { target: Selector },
    Disappears { target: Selector },
    ValueIs { target: Selector, value: String },
    ValueContains { target: Selector, value: String },
    Focused { target: Selector },
    TitleContains { text: String },
}

impl Expect {
    pub fn holds(&self, snap: &UiSnapshot) -> bool {
        match self {
            Expect::Appears { target } => target.resolve(snap).is_some(),
            Expect::Disappears { target } => target.resolve(snap).is_none(),
            Expect::ValueIs { target, value } => target
                .resolve(snap)
                .is_some_and(|e| e.value.trim() == value.trim()),
            Expect::ValueContains { target, value } => target
                .resolve(snap)
                .is_some_and(|e| lower(&e.value).contains(&lower(value))),
            Expect::Focused { target } => target.resolve(snap).is_some_and(|e| e.focused),
            Expect::TitleContains { text } => lower(&snap.window.title).contains(&lower(text)),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Expect::Appears { target } => format!("`{}` appears", label(target)),
            Expect::Disappears { target } => format!("`{}` disappears", label(target)),
            Expect::ValueIs { target, value } => format!("`{}` value is `{value}`", label(target)),
            Expect::ValueContains { target, value } => {
                format!("`{}` value contains `{value}`", label(target))
            }
            Expect::Focused { target } => format!("`{}` has focus", label(target)),
            Expect::TitleContains { text } => format!("window title contains `{text}`"),
        }
    }
}

pub fn label(s: &Selector) -> String {
    if !s.name.is_empty() {
        format!("{} {}", s.role, s.name).trim().to_string()
    } else if !s.automation_id.is_empty() {
        s.automation_id.clone()
    } else if !s.role.is_empty() {
        s.role.clone()
    } else {
        "element".into()
    }
}

fn default_timeout() -> u32 {
    5000
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default)]
    pub id: String,
    /// Spoken/caption text shown with this step.
    #[serde(default)]
    pub narration: String,
    pub action: Action,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expect: Vec<Expect>,
    /// Delete, send, buy, close without saving: needs explicit confirmation to run.
    #[serde(default)]
    pub destructive: bool,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u32,
    /// Filled by rehearsal: where the target was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_rect: Option<Rect>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chapter {
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub steps: Vec<Step>,
    /// Set when every step passed rehearsal.
    #[serde(default)]
    pub checked: bool,
}

/// The application the tutorial is about.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppTarget {
    /// Exe path, document or URL to launch when no window matches.
    #[serde(default)]
    pub launch: String,
    #[serde(default)]
    pub args: String,
    /// Part of the main window title.
    #[serde(default)]
    pub window_title: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TutorialScript {
    #[serde(default = "version")]
    pub version: u32,
    pub title: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub app: AppTarget,
    pub chapters: Vec<Chapter>,
}

fn version() -> u32 {
    SCRIPT_VERSION
}

impl TutorialScript {
    pub fn step_count(&self) -> usize {
        self.chapters.iter().map(|c| c.steps.len()).sum()
    }

    /// Reject hostile or broken scripts and give steps ids (`c1s1`).
    pub fn validate(&mut self) -> TutorResult<()> {
        let bad = |m: String| Err(TutorError::Script(m));
        if self.version != SCRIPT_VERSION {
            return bad(format!("unsupported script version {}", self.version));
        }
        if self.chapters.is_empty() || self.chapters.len() > MAX_CHAPTERS {
            return bad(format!("needs 1..={MAX_CHAPTERS} chapters"));
        }
        if self.title.chars().count() > 200 || self.goal.chars().count() > MAX_TEXT {
            return bad("title or goal too long".into());
        }
        for (ci, c) in self.chapters.iter_mut().enumerate() {
            if c.steps.is_empty() || c.steps.len() > MAX_STEPS {
                return bad(format!("chapter {} needs 1..={MAX_STEPS} steps", ci + 1));
            }
            for (si, s) in c.steps.iter_mut().enumerate() {
                if s.id.is_empty() {
                    s.id = format!("c{}s{}", ci + 1, si + 1);
                }
                s.timeout_ms = s.timeout_ms.clamp(200, 60_000);
                if s.narration.chars().count() > MAX_TEXT {
                    return bad(format!("step {} narration too long", s.id));
                }
                check_action(&s.id, &s.action)?;
            }
        }
        Ok(())
    }
}

fn check_action(id: &str, a: &Action) -> TutorResult<()> {
    let bad = |m: &str| Err(TutorError::Script(format!("step {id}: {m}")));
    let blank = |s: &Selector| s.is_blank();
    match a {
        Action::Click { target }
        | Action::DoubleClick { target }
        | Action::RightClick { target }
        | Action::Hover { target } => {
            if blank(target) {
                return bad("target selector is empty");
            }
        }
        Action::Drag { from, to } => {
            if blank(from) || blank(to) {
                return bad("drag needs two targets");
            }
        }
        Action::Type { text, .. } => {
            if text.is_empty() || text.chars().count() > MAX_TEXT {
                return bad("text must be 1..=2000 characters");
            }
        }
        Action::Key { keys } => {
            if keys.trim().is_empty() || keys.len() > 60 {
                return bad("keys must be a short chord such as ctrl+s");
            }
        }
        Action::Wait { ms } => {
            if *ms > 60_000 {
                return bad("wait is capped at 60000 ms");
            }
        }
        Action::Scroll { dy, .. } => {
            if dy.unsigned_abs() > 100 {
                return bad("scroll is capped at 100 notches");
            }
        }
        Action::Focus => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(role: &str, name: &str, x: i32, depth: u8) -> UiElement {
        UiElement {
            role: role.into(),
            name: name.into(),
            rect: Rect::new(x, 10, 50, 20),
            enabled: true,
            depth,
            ..UiElement::default()
        }
    }

    fn snap() -> UiSnapshot {
        UiSnapshot {
            window: Default::default(),
            elements: vec![
                el("Window", "Demo", 0, 0),
                el("ToolBar", "Main", 0, 1),
                el("Button", "Save", 10, 2),
                el("Button", "Save As", 70, 2),
                el("Pane", "Dialog", 0, 1),
                el("Button", "Save", 200, 2),
            ],
        }
        .finish()
    }

    #[test]
    fn finish_assigns_ids_and_paths() {
        let s = snap();
        assert_eq!(s.elements[2].id, 2);
        assert_eq!(s.elements[2].path, "Window[Demo]/ToolBar[Main]");
        assert_eq!(s.elements[5].path, "Window[Demo]/Pane[Dialog]");
    }

    #[test]
    fn exact_name_beats_partial_and_nth_picks() {
        let s = snap();
        let sel = Selector::named("button", "save");
        assert_eq!(sel.resolve(&s).map(|e| e.id), Some(2));
        let mut second = sel.clone();
        second.nth = 1;
        assert_eq!(second.resolve(&s).map(|e| e.id), Some(5));
        let mut within = sel;
        within.within = "dialog".into();
        assert_eq!(within.resolve(&s).map(|e| e.id), Some(5));
        assert!(Selector::named("Button", "Cancel").resolve(&s).is_none());
        assert!(Selector::default().resolve(&s).is_none());
    }

    #[test]
    fn partial_and_rect_fallback() {
        let s = snap();
        assert_eq!(
            Selector::named("Button", "as").resolve(&s).map(|e| e.id),
            Some(3)
        );
        let sel = Selector {
            rect: Some(Rect::new(72, 12, 50, 20)),
            ..Selector::default()
        };
        assert_eq!(sel.resolve(&s).map(|e| e.id), Some(3));
        let far = Selector {
            rect: Some(Rect::new(900, 900, 5, 5)),
            ..Selector::default()
        };
        assert!(far.resolve(&s).is_none());
    }

    #[test]
    fn expectations_evaluate() {
        let s = snap();
        let t = Selector::named("Button", "Save");
        assert!(Expect::Appears { target: t.clone() }.holds(&s));
        assert!(!Expect::Disappears { target: t }.holds(&s));
        assert!(
            Expect::TitleContains { text: "x".into() }.holds(&UiSnapshot {
                window: crate::traits::WindowInfo {
                    title: "Box".into(),
                    ..Default::default()
                },
                elements: vec![]
            })
        );
    }

    fn script(json: &str) -> TutorResult<TutorialScript> {
        let mut s: TutorialScript =
            serde_json::from_str(json).map_err(|e| TutorError::Script(e.to_string()))?;
        s.validate()?;
        Ok(s)
    }

    #[test]
    fn validates_and_numbers_steps() {
        let s = script(
            r#"{"title":"t","chapters":[{"title":"c","steps":[
            {"action":{"type":"click","target":{"role":"Button","name":"Save"}}},
            {"action":{"type":"key","keys":"ctrl+s"},"timeout_ms":1}]}]}"#,
        )
        .unwrap();
        assert_eq!(s.chapters[0].steps[1].id, "c1s2");
        assert_eq!(s.chapters[0].steps[1].timeout_ms, 200);
        assert_eq!(s.step_count(), 2);
    }

    #[test]
    fn rejects_bad_scripts() {
        assert!(script(r#"{"title":"t","chapters":[]}"#).is_err());
        assert!(script(r#"{"title":"t","chapters":[{"title":"c","steps":[]}]}"#).is_err());
        assert!(script(r#"{"title":"t","version":9,"chapters":[{"title":"c","steps":[{"action":{"type":"focus"}}]}]}"#).is_err());
        let blank = r#"{"title":"t","chapters":[{"title":"c","steps":[{"action":{"type":"click","target":{}}}]}]}"#;
        assert!(script(blank).is_err());
        let unknown =
            r#"{"title":"t","chapters":[{"title":"c","steps":[{"action":{"type":"nope"}}]}]}"#;
        assert!(script(unknown).is_err());
        let scroll = r#"{"title":"t","chapters":[{"title":"c","steps":[{"action":{"type":"scroll","dy":9999}}]}]}"#;
        assert!(script(scroll).is_err());
    }
}
