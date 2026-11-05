//! Explore: the model gets a task, not a script. It first splits the task into chapters (titles
//! and goals only, no controls), then works each chapter one step at a time: look at the window
//! in front (control list with real positions, plus a screenshot), choose exactly one action on a
//! listed control, do it, and keep it only when the screen changed. When it cannot see how to go
//! on, it asks the user. The script is written from the steps that worked.

use crate::error::{TutorError, TutorResult};
use crate::plan::extract_any;
use crate::player::{Mode, Player};
use crate::rehearse::{Event, follow_front, pin, wait_for};
use crate::safety::Guard;
use crate::script::{Action, AppTarget, Chapter, Selector, Step, TutorialScript};
use crate::traits::{ChatMessage, ChatOptions, LlmChat, Rig, UiSnapshot, WindowInfo};
use serde::Deserialize;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// What the user said when the model asked for help.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// A hint the model reads from now on ("yes" also allows a risky step).
    Hint(String),
    /// The user did the step by hand; look again and carry on.
    DidIt,
    /// Leave this chapter as it is and go to the next one.
    SkipChapter,
    /// Stop exploring and keep what is there.
    Finish,
}

pub struct ExploreOpts {
    pub max_steps: u32,
    /// Failed or useless moves in a row before the user is asked.
    pub max_misses: u32,
    pub chat: Arc<dyn LlmChat>,
    pub copts: ChatOptions,
}

const MOVE_RULES: &str = r#"Reply with ONE JSON object and nothing else. The first character is {.
Choose exactly one of:
 {"narration":str,"do":"click"|"double_click"|"right_click","id":int}
 {"narration":str,"do":"type","id":int?,"text":str}
 {"narration":str,"do":"key","keys":"ctrl+n"}
 {"narration":str,"do":"scroll","id":int?,"dy":int}
 {"done":true}                      when this chapter's goal is reached
 {"ask":str}                        when you cannot see how to go on: a short question for the user
"id" is the #number of a control in the list. Only use listed ids; never guess a position.
"narration" is one short sentence a viewer reads. Add "risky":true for delete, send, buy or close-without-saving."#;

#[derive(Deserialize)]
struct Board {
    #[serde(default)]
    title: String,
    chapters: Vec<BoardChapter>,
}

#[derive(Deserialize)]
struct BoardChapter {
    title: String,
    #[serde(default)]
    goal: String,
}

#[derive(Deserialize, Default)]
struct Move {
    #[serde(default)]
    narration: String,
    #[serde(default, rename = "do")]
    act: String,
    #[serde(default)]
    id: Option<u32>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    keys: String,
    #[serde(default)]
    dy: i32,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    ask: Option<String>,
    #[serde(default)]
    risky: bool,
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub fn storyboard_messages(
    goal: &str,
    app: &str,
    max_chapters: usize,
    snap: &UiSnapshot,
) -> Vec<ChatMessage> {
    let system = "You split a software task into a short tutorial outline. Do not name buttons or \
                  positions: each chapter is a goal a person can check on screen. Reply with ONE \
                  JSON object only, first character {: \
                  {\"title\":str,\"chapters\":[{\"title\":str,\"goal\":str}]}";
    let user = format!(
        "Task: {}\nApp: {}\nAt most {} chapters.\nWhat the app shows now:\n{}",
        clip(goal, 600),
        clip(app, 120),
        max_chapters.clamp(1, 12),
        snap.compact_within(25, 1200, false)
    );
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

#[allow(clippy::too_many_arguments)]
pub fn move_messages(
    goal: &str,
    chapter: &str,
    chapter_goal: &str,
    done: &[String],
    hints: &[String],
    note: &str,
    picture: bool,
    snap: &UiSnapshot,
) -> Vec<ChatMessage> {
    let system =
        format!("You operate a Windows app one action at a time to reach a goal.\n{MOVE_RULES}");
    let recent: Vec<&String> = done.iter().rev().take(4).rev().collect();
    let mut user = format!(
        "Task: {}\nChapter: {} - {}\nDone in this chapter: {}\n",
        clip(goal, 300),
        clip(chapter, 100),
        clip(chapter_goal, 200),
        if recent.is_empty() {
            "nothing yet".to_string()
        } else {
            recent
                .iter()
                .map(|s| clip(s, 80))
                .collect::<Vec<_>>()
                .join(" | ")
        }
    );
    if !hints.is_empty() {
        user.push_str(&format!(
            "The user told you: {}\n",
            hints
                .iter()
                .rev()
                .take(3)
                .map(|h| clip(h, 200))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    if !note.is_empty() {
        user.push_str(&format!("Last try: {}\n", clip(note, 200)));
    }
    if picture {
        user.push_str("A screenshot of the window is attached.\n");
    }
    user.push_str(&format!(
        "Controls now:\n{}Your one move:",
        snap.compact_within(45, 2200, true)
    ));
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

fn parse_board(reply: &str) -> TutorResult<Board> {
    let json = extract_any(reply, &["chapters"]).ok_or_else(|| {
        TutorError::Llm(
            "the outline holds no `chapters` (the model may have run out of room)".into(),
        )
    })?;
    let b: Board = serde_json::from_str(&json)
        .map_err(|e| TutorError::Llm(format!("the outline does not parse: {e}")))?;
    if b.chapters.is_empty() {
        return Err(TutorError::Llm("the outline has no chapters".into()));
    }
    Ok(b)
}

fn parse_move(reply: &str) -> Option<Move> {
    let json = extract_any(reply, &["do", "done", "ask"])?;
    serde_json::from_str(&json).ok()
}

/// What changed on screen, cheaply: window, and every control's role, name, value and state.
fn fingerprint(snap: &UiSnapshot) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    snap.window.hwnd.hash(&mut h);
    snap.window.title.hash(&mut h);
    for e in &snap.elements {
        (&e.role, &e.name, &e.value, e.enabled, e.focused).hash(&mut h);
    }
    h.finish()
}

fn selector_of(snap: &UiSnapshot, id: u32) -> Option<Selector> {
    let e = snap.find(id)?;
    if e.rect.is_empty() {
        return None;
    }
    Some(Selector {
        role: e.role.clone(),
        name: e.name.clone(),
        automation_id: e.automation_id.clone(),
        rect: Some(e.rect),
        ..Selector::default()
    })
}

/// Turn the model's move into a step on a real control, or say why not.
fn step_of(m: &Move, snap: &UiSnapshot) -> Result<Step, String> {
    let target = |need: bool| -> Result<Option<Selector>, String> {
        match m.id {
            Some(id) => selector_of(snap, id)
                .map(Some)
                .ok_or_else(|| format!("#{id} is not in the control list")),
            None if need => Err(format!("`{}` needs an id from the list", m.act)),
            None => Ok(None),
        }
    };
    let action = match m.act.as_str() {
        "click" | "double_click" | "right_click" => {
            let t = target(true)?.ok_or("missing id")?;
            match m.act.as_str() {
                "click" => Action::Click { target: t },
                "double_click" => Action::DoubleClick { target: t },
                _ => Action::RightClick { target: t },
            }
        }
        "type" if !m.text.is_empty() => Action::Type {
            target: target(false)?,
            text: clip(&m.text, 500),
        },
        "key" if !m.keys.trim().is_empty() => Action::Key {
            keys: m.keys.trim().to_string(),
        },
        "scroll" => Action::Scroll {
            target: target(false)?,
            dy: m.dy.clamp(-20, 20),
        },
        other => return Err(format!("`{other}` is not a move I can do")),
    };
    Ok(Step {
        id: String::new(),
        narration: clip(m.narration.trim(), 300),
        action,
        expect: Vec::new(),
        destructive: m.risky,
        timeout_ms: 3000,
        verified_rect: None,
    })
}

fn is_yes(s: &str) -> bool {
    matches!(
        s.trim().to_lowercase().as_str(),
        "yes" | "y" | "ok" | "go" | "do it" | "allow"
    )
}

/// Explore `goal` on the live app. `script` holds what is known so far, also when this fails, so
/// a stopped run keeps its steps. `ask` blocks until the user answers.
#[allow(clippy::too_many_arguments)]
pub fn explore(
    script: &mut TutorialScript,
    rig: &Rig,
    guard: Guard,
    window: WindowInfo,
    opts: &ExploreOpts,
    on_event: &mut dyn FnMut(Event),
    ask: &mut dyn FnMut(&str) -> TutorResult<Answer>,
) -> TutorResult<()> {
    let abort = Arc::clone(&guard.abort);
    let home = window.clone();
    let mut player = Player::new(rig, guard, window, Mode::Rehearse);
    let goal = script.goal.clone();
    let app = if script.app.window_title.is_empty() {
        home.title.clone()
    } else {
        script.app.window_title.clone()
    };
    let chat = |msgs: Vec<ChatMessage>, shot: Option<crate::traits::Shot>| {
        let chat = Arc::clone(&opts.chat);
        let copts = opts.copts.clone();
        wait_for(&*rig.input, &abort, move || match &shot {
            Some(img) => chat
                .chat_image(&msgs, img, &copts)
                .or_else(|_| chat.chat(&msgs, &copts)),
            None => chat.chat(&msgs, &copts),
        })
        .and_then(|r| r)
    };

    let snap = rig.ui.snapshot(home.hwnd)?;
    let board = {
        let mut last = None;
        let mut got = None;
        for _ in 0..2 {
            let reply = chat(storyboard_messages(&goal, &app, 8, &snap), None)?;
            match parse_board(&reply) {
                Ok(b) => {
                    got = Some(b);
                    break;
                }
                Err(e) => last = Some(e),
            }
        }
        got.ok_or_else(|| last.unwrap_or_else(|| TutorError::Llm("no outline".into())))?
    };
    if !board.title.trim().is_empty() {
        script.title = clip(board.title.trim(), 200);
    }
    script.chapters.clear();
    let mut budget = opts.max_steps.clamp(1, 200);
    let mut hints: Vec<String> = Vec::new();

    'chapters: for (ci, bc) in board.chapters.iter().take(12).enumerate() {
        if budget == 0 {
            break;
        }
        on_event(Event::Chapter {
            index: ci,
            title: bc.title.clone(),
        });
        player.begin_chapter(ci);
        let mut chapter = Chapter {
            title: clip(&bc.title, 200),
            summary: clip(&bc.goal, 400),
            steps: Vec::new(),
            checked: true,
        };
        let mut done_lines: Vec<String> = Vec::new();
        let mut note = String::new();
        let mut misses = 0u32;
        let mut turns = 0u32;
        loop {
            turns += 1;
            if budget == 0 || turns > 40 {
                break;
            }
            let question = if misses >= opts.max_misses.max(1) {
                Some(format!(
                    "I am stuck on \"{}\". {} What should I do next?",
                    bc.title,
                    if note.is_empty() {
                        String::new()
                    } else {
                        format!("Last try: {note}.")
                    }
                ))
            } else {
                None
            };
            let m = match question {
                Some(q) => Move {
                    ask: Some(q),
                    ..Move::default()
                },
                None => {
                    follow_front(&mut player);
                    let snap = rig.ui.snapshot(player.window.hwnd)?;
                    let shot = rig.ui.capture(player.window.hwnd);
                    let msgs = move_messages(
                        &goal,
                        &bc.title,
                        &bc.goal,
                        &done_lines,
                        &hints,
                        &note,
                        shot.is_some(),
                        &snap,
                    );
                    let reply = chat(msgs, shot)?;
                    match parse_move(&reply) {
                        Some(m) => m,
                        None => {
                            misses += 1;
                            note = "your reply was not one JSON move".into();
                            continue;
                        }
                    }
                }
            };
            if m.done {
                break;
            }
            if let Some(q) = m.ask.clone().filter(|q| !q.trim().is_empty()) {
                on_event(Event::Ask {
                    question: clip(&q, 300),
                });
                let answer = ask(&clip(&q, 300))?;
                rig.input.begin();
                let _ = rig.launcher.focus(player.window.hwnd);
                rig.pace.sleep_ms(400);
                misses = 0;
                match answer {
                    Answer::Hint(h) => {
                        note.clear();
                        if !h.trim().is_empty() {
                            hints.push(clip(h.trim(), 300));
                        }
                    }
                    Answer::DidIt => note = "the user did the last step by hand".into(),
                    Answer::SkipChapter => break,
                    Answer::Finish => {
                        if !chapter.steps.is_empty() {
                            script.chapters.push(chapter);
                        }
                        break 'chapters;
                    }
                }
                continue;
            }
            let snap = rig.ui.snapshot(player.window.hwnd)?;
            let mut step = match step_of(&m, &snap) {
                Ok(s) => s,
                Err(why) => {
                    misses += 1;
                    note = why;
                    continue;
                }
            };
            if step.destructive {
                let q = format!(
                    "May I do this: \"{}\"? Answer yes, or do it yourself and press I did it.",
                    step.narration
                );
                on_event(Event::Ask {
                    question: q.clone(),
                });
                let answer = ask(&q)?;
                rig.input.begin();
                let _ = rig.launcher.focus(player.window.hwnd);
                rig.pace.sleep_ms(400);
                match answer {
                    Answer::Hint(h) if is_yes(&h) => {}
                    Answer::Hint(h) => {
                        hints.push(clip(h.trim(), 300));
                        note = "the user did not allow that step".into();
                        continue;
                    }
                    Answer::DidIt => {
                        note = "the user did the last step by hand".into();
                        continue;
                    }
                    Answer::SkipChapter => break,
                    Answer::Finish => {
                        if !chapter.steps.is_empty() {
                            script.chapters.push(chapter);
                        }
                        break 'chapters;
                    }
                }
            }
            let before = fingerprint(&snap);
            let allow = player.guard.allow_destructive;
            player.guard.allow_destructive = allow || step.destructive;
            let result = player.run_step(&step);
            player.guard.allow_destructive = allow;
            match result {
                Ok(marker) => {
                    rig.pace.sleep_ms(300);
                    follow_front(&mut player);
                    let changed = rig
                        .ui
                        .snapshot(player.window.hwnd)
                        .map(|s| fingerprint(&s) != before)
                        .unwrap_or(true);
                    if changed {
                        pin(&mut step, marker.rect);
                        step.id = format!("c{}s{}", ci + 1, chapter.steps.len() + 1);
                        on_event(Event::Step {
                            chapter: ci,
                            step_id: step.id.clone(),
                        });
                        done_lines.push(step.narration.clone());
                        chapter.steps.push(step);
                        budget = budget.saturating_sub(1);
                        misses = 0;
                        note.clear();
                    } else {
                        misses += 1;
                        note = format!("\"{}\" changed nothing on screen", step.narration);
                    }
                }
                Err(TutorError::Step(msg)) => {
                    misses += 1;
                    note = msg;
                }
                Err(e) => {
                    if !chapter.steps.is_empty() {
                        script.chapters.push(chapter);
                    }
                    return Err(e);
                }
            }
        }
        if !chapter.steps.is_empty() {
            script.chapters.push(chapter);
            on_event(Event::ChapterChecked { index: ci });
        }
    }
    if script.chapters.is_empty() {
        return Err(TutorError::Step(
            "no step worked; give a hint or check that the app is open".into(),
        ));
    }
    script.validate()
}

/// A fresh script shell for `explore`.
pub fn empty_script(goal: &str, app: AppTarget) -> TutorialScript {
    TutorialScript {
        version: crate::script::SCRIPT_VERSION,
        title: clip(goal, 80),
        goal: clip(goal, 2000),
        app,
        chapters: Vec::new(),
    }
}
