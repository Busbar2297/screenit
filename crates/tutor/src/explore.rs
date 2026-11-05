//! Explore: the model gets a task, not a script. It splits the task into chapters, then works one
//! move at a time from the picture in front of it. Each move is a mouse or keyboard instruction
//! (a point in that picture, a key, or text), carried out by the pointer. A move is kept only when
//! the screen changed. A new window title is expected, and a dialog is followed until it closes.
//! When the model cannot see how to go on, it asks the user.

use crate::error::{TutorError, TutorResult};
use crate::hand::{self, aim};
use crate::plan::extract_any;
use crate::player::{Mode, Player};
use crate::rehearse::{Event, pin, wait_for};
use crate::safety::Guard;
use crate::script::{Action, AppTarget, Chapter, Selector, Step, TutorialScript};
use crate::traits::{ChatMessage, ChatOptions, LlmChat, Rig, UiSnapshot, WindowInfo};
use serde::Deserialize;
use serde::de::Deserializer;
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

const MOVE_RULES: &str = r#"You see the app and drive the mouse and keyboard. Reply with ONE JSON object and nothing else. The first character is {.
Choose exactly one of:
 {"narration":str,"do":"click"|"double_click"|"right_click","x":0-1000,"y":0-1000}
 {"narration":str,"do":"type","text":str,"x":0-1000?,"y":0-1000?}
 {"narration":str,"do":"key","keys":"ctrl+n"}
 {"narration":str,"do":"scroll","dy":int,"x":0-1000?,"y":0-1000?}
 {"narration":str,"do":"drag","x":0-1000,"y":0-1000,"x2":0-1000,"y2":0-1000}
 {"done":true}     when this chapter's goal is already true on screen
 {"ask":str}       when you cannot see how to go on: a short question for the user
x and y are a point in the picture you were shown: 0,0 is the window's top-left, 1000,1000 its bottom-right. They are not screen pixels.
A window title change is normal. Act on the picture you were just shown, not on an earlier one.
"narration" is one short sentence a viewer reads. Add "risky":true for delete, send, buy or close-without-saving.
An "id" from the control list may be used instead of x,y when the picture is unclear."#;

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
    #[serde(default, deserialize_with = "coord")]
    x: Option<i32>,
    #[serde(default, deserialize_with = "coord")]
    y: Option<i32>,
    #[serde(default, deserialize_with = "coord")]
    x2: Option<i32>,
    #[serde(default, deserialize_with = "coord")]
    y2: Option<i32>,
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

/// A point from the model: an integer or a fractional picture coordinate.
fn coord<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.as_ref().and_then(|v| {
        v.as_i64().and_then(|i| i32::try_from(i).ok()).or_else(|| {
            v.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f.round().clamp(-10_000.0, 10_000.0) as i32)
        })
    }))
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
        user.push_str(
            "The attached picture is this window. x=0,y=0 is its top-left and x=1000,y=1000 its bottom-right.\n",
        );
    }
    user.push_str(&format!(
        "What you see now:\n{}Your one move:",
        hand::control_lines(snap, 24, 1100)
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

/// A picture point, or a listed id when the model did not point.
fn target_of(m: &Move, snap: &UiSnapshot, need: bool) -> Result<Option<Selector>, String> {
    match (m.x, m.y) {
        (Some(x), Some(y)) => aim(snap, x, y).map(Some),
        (Some(_), None) | (None, Some(_)) => {
            Err("a point needs both x and y, each from 0 to 1000".into())
        }
        (None, None) => match m.id {
            Some(id) => selector_of(snap, id)
                .map(Some)
                .ok_or_else(|| format!("#{id} is not in the control list")),
            None if need => {
                Err("point at the control with x and y (0 to 1000 in the picture)".into())
            }
            None => Ok(None),
        },
    }
}

/// Turn the model's move into a step, or say why not.
fn step_of(m: &Move, snap: &UiSnapshot) -> Result<Step, String> {
    let action = match m.act.as_str() {
        "click" | "double_click" | "right_click" => {
            let t = target_of(m, snap, true)?.ok_or("missing point")?;
            match m.act.as_str() {
                "click" => Action::Click { target: t },
                "double_click" => Action::DoubleClick { target: t },
                _ => Action::RightClick { target: t },
            }
        }
        "type" if !m.text.is_empty() => Action::Type {
            target: target_of(m, snap, false)?,
            text: clip(&m.text, 500),
        },
        "key" if !m.keys.trim().is_empty() => Action::Key {
            keys: m.keys.trim().to_string(),
        },
        "scroll" => Action::Scroll {
            target: target_of(m, snap, false)?,
            dy: m.dy.clamp(-20, 20),
        },
        "drag" => {
            let (Some(x), Some(y), Some(x2), Some(y2)) = (m.x, m.y, m.x2, m.y2) else {
                return Err("drag needs x, y, x2 and y2, each from 0 to 1000".into());
            };
            Action::Drag {
                from: aim(snap, x, y)?,
                to: aim(snap, x2, y2)?,
            }
        }
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

enum ChapterStop {
    /// This chapter is finished; go on to the next one.
    Next,
    /// The user ended the run. Chapters already stored stay stored.
    End,
}

/// One chapter: look, let the model point, do that one move, repeat.
#[allow(clippy::too_many_arguments)]
fn drive_chapter(
    script: &mut TutorialScript,
    player: &mut Player<'_>,
    rig: &Rig,
    goal: &str,
    chapter_title: &str,
    chapter_goal: &str,
    ci: usize,
    budget: &mut u32,
    hints: &mut Vec<String>,
    max_misses: u32,
    chat: &dyn Fn(Vec<ChatMessage>, Option<crate::traits::Shot>) -> TutorResult<String>,
    on_event: &mut dyn FnMut(Event),
    ask: &mut dyn FnMut(&str) -> TutorResult<Answer>,
) -> TutorResult<ChapterStop> {
    let mut chapter = Chapter {
        title: clip(chapter_title, 200),
        summary: clip(chapter_goal, 400),
        steps: Vec::new(),
        checked: true,
    };
    let mut done_lines: Vec<String> = Vec::new();
    let mut note = String::new();
    let mut misses = 0u32;
    let mut shifts = 0u32;
    let mut turns = 0u32;
    loop {
        turns += 1;
        if *budget == 0 || turns > 40 {
            break;
        }
        match drive_turn(
            script,
            player,
            rig,
            goal,
            chapter_title,
            chapter_goal,
            ci,
            budget,
            hints,
            max_misses,
            chat,
            on_event,
            ask,
            &mut chapter,
            &mut done_lines,
            &mut note,
            &mut misses,
            &mut shifts,
        )? {
            Some(ChapterStop::End) => return Ok(ChapterStop::End),
            Some(ChapterStop::Next) => break,
            None => {}
        }
    }
    if !chapter.steps.is_empty() {
        script.chapters.push(chapter);
        on_event(Event::ChapterChecked { index: ci });
    }
    Ok(ChapterStop::Next)
}

/// One look-and-act. `Ok(None)` means the chapter loop continues; `Ok(Some)` leaves the chapter.
#[allow(clippy::too_many_arguments)]
fn drive_turn(
    script: &mut TutorialScript,
    player: &mut Player<'_>,
    rig: &Rig,
    goal: &str,
    chapter_title: &str,
    chapter_goal: &str,
    ci: usize,
    budget: &mut u32,
    hints: &mut Vec<String>,
    max_misses: u32,
    chat: &dyn Fn(Vec<ChatMessage>, Option<crate::traits::Shot>) -> TutorResult<String>,
    on_event: &mut dyn FnMut(Event),
    ask: &mut dyn FnMut(&str) -> TutorResult<Answer>,
    chapter: &mut Chapter,
    done_lines: &mut Vec<String>,
    note: &mut String,
    misses: &mut u32,
    shifts: &mut u32,
) -> TutorResult<Option<ChapterStop>> {
    let question = if *misses >= max_misses.max(1) {
        Some(format!(
            "I am stuck on \"{chapter_title}\". {} What should I do next?",
            if note.is_empty() {
                String::new()
            } else {
                format!("Last try: {note}.")
            }
        ))
    } else {
        None
    };
    let (m, shown) = match question {
        Some(q) => (
            Move {
                ask: Some(q),
                ..Move::default()
            },
            None,
        ),
        None => match ask_model(
            player,
            goal,
            chapter_title,
            chapter_goal,
            done_lines,
            hints,
            note,
            chat,
        )? {
            ModelSay::Miss(msg) => {
                *misses += 1;
                *note = msg;
                return Ok(None);
            }
            ModelSay::Move(m, shown) => (m, shown),
        },
    };
    if m.done {
        return Ok(Some(ChapterStop::Next));
    }
    if let Some(q) = m.ask.clone().filter(|q| !q.trim().is_empty()) {
        let stop = user_answer(
            script, player, rig, chapter, hints, note, misses, on_event, ask, &q,
        )?;
        // The hint is kept in the hint list. It is not a failed try.
        if stop.is_none() && hints.last().is_some_and(|h| h == note) {
            note.clear();
        }
        return Ok(stop);
    }
    let fresh = match glance(player, false)? {
        Ok(s) => s,
        Err(msg) => {
            *misses += 1;
            *note = msg;
            return Ok(None);
        }
    };
    if shown.is_some_and(|h| h != fresh.snap.window.hwnd) {
        *shifts += 1;
        *note = format!(
            "The window changed to \"{}\" while you were deciding. Look at this picture and move again.",
            fresh.snap.window.title
        );
        if *shifts >= 3 {
            *misses += 1;
        }
        return Ok(None);
    }
    *shifts = 0;
    let snap = fresh.snap;
    let mut step = match step_of(&m, &snap) {
        Ok(s) => s,
        Err(why) => {
            *misses += 1;
            *note = why;
            return Ok(None);
        }
    };
    if step.destructive {
        let q = format!(
            "May I do this: \"{}\"? Answer yes, or do it yourself and press I did it.",
            step.narration
        );
        match user_answer(
            script, player, rig, chapter, hints, note, misses, on_event, ask, &q,
        )? {
            Some(stop) => return Ok(Some(stop)),
            // "yes" allows the step. Anything else is a hint, so the step is not done.
            None if is_yes(note) => note.clear(),
            None => return Ok(None),
        }
    }
    act_step(
        script, player, rig, ci, budget, chapter, done_lines, note, misses, on_event, &snap,
        &mut step,
    )
}

enum ModelSay {
    Move(Move, Option<u64>),
    Miss(String),
}

#[allow(clippy::too_many_arguments)]
fn ask_model(
    player: &mut Player<'_>,
    goal: &str,
    chapter_title: &str,
    chapter_goal: &str,
    done_lines: &[String],
    hints: &[String],
    note: &mut String,
    chat: &dyn Fn(Vec<ChatMessage>, Option<crate::traits::Shot>) -> TutorResult<String>,
) -> TutorResult<ModelSay> {
    let scene = match glance(player, false)? {
        Ok(s) => s,
        Err(msg) => return Ok(ModelSay::Miss(msg)),
    };
    if note.is_empty() {
        *note = scene.changed;
    }
    let shown = scene.snap.window.hwnd;
    let shot = scene.shot;
    let msgs = move_messages(
        goal,
        chapter_title,
        chapter_goal,
        done_lines,
        hints,
        note,
        shot.is_some(),
        &scene.snap,
    );
    let reply = chat(msgs, shot)?;
    Ok(match parse_move(&reply) {
        Some(m) => ModelSay::Move(m, Some(shown)),
        None => ModelSay::Miss("your reply was not one JSON move".into()),
    })
}

/// `Err(msg)` is a miss the chapter can survive. Other errors end the run.
fn glance(player: &mut Player<'_>, just_acted: bool) -> TutorResult<Result<hand::Scene, String>> {
    match hand::see(player, None, just_acted) {
        Ok(s) => Ok(Ok(s)),
        Err(TutorError::Ui(msg)) => Ok(Err(msg)),
        Err(e) => Err(e),
    }
}

/// Ask the user. `Ok(Some(Next))` skips the chapter, `Ok(Some(End))` stops, `Ok(None)` carries on.
#[allow(clippy::too_many_arguments)]
fn user_answer(
    script: &mut TutorialScript,
    player: &mut Player<'_>,
    rig: &Rig,
    chapter: &mut Chapter,
    hints: &mut Vec<String>,
    note: &mut String,
    misses: &mut u32,
    on_event: &mut dyn FnMut(Event),
    ask: &mut dyn FnMut(&str) -> TutorResult<Answer>,
    question: &str,
) -> TutorResult<Option<ChapterStop>> {
    on_event(Event::Ask {
        question: clip(question, 300),
    });
    let answer = ask(&clip(question, 300))?;
    rig.input.begin();
    hand::track(player, false);
    let _ = rig.launcher.focus(player.window.hwnd);
    rig.pace.sleep_ms(400);
    *misses = 0;
    Ok(match answer {
        Answer::Hint(h) => {
            let h = clip(h.trim(), 300);
            note.clear();
            if !h.is_empty() {
                *note = h.clone();
                if !is_yes(&h) {
                    hints.push(h);
                }
            }
            None
        }
        Answer::DidIt => {
            *note = "the user did the last step by hand".into();
            None
        }
        Answer::SkipChapter => Some(ChapterStop::Next),
        Answer::Finish => {
            if !chapter.steps.is_empty() {
                script.chapters.push(std::mem::take(chapter));
            }
            Some(ChapterStop::End)
        }
    })
}

/// Do the step and keep it when the screen changed, including a title change.
#[allow(clippy::too_many_arguments)]
fn act_step(
    script: &mut TutorialScript,
    player: &mut Player<'_>,
    rig: &Rig,
    ci: usize,
    budget: &mut u32,
    chapter: &mut Chapter,
    done_lines: &mut Vec<String>,
    note: &mut String,
    misses: &mut u32,
    on_event: &mut dyn FnMut(Event),
    snap: &UiSnapshot,
    step: &mut Step,
) -> TutorResult<Option<ChapterStop>> {
    let before_win = snap.window.clone();
    let before = fingerprint(snap);
    let stray = player
        .rig
        .ui
        .foreground()
        .is_some_and(|w| w.pid != player.guard.home_pid && w.hwnd != player.window.hwnd);
    if stray {
        let _ = rig.launcher.focus(player.window.hwnd);
        rig.pace.sleep_ms(150);
    }
    let allow = player.guard.allow_destructive;
    player.guard.allow_destructive = allow || step.destructive;
    let result = player.run_step(step);
    let result = match result {
        Err(TutorError::Safety(msg)) if msg.contains("foreground") => {
            let _ = rig.launcher.focus(player.window.hwnd);
            rig.pace.sleep_ms(200);
            player.run_step(step)
        }
        other => other,
    };
    player.guard.allow_destructive = allow;
    match result {
        Ok(marker) => {
            let after = match hand::settle(player) {
                Ok(s) => s,
                Err(TutorError::Ui(msg)) => {
                    *misses += 1;
                    *note = msg;
                    return Ok(None);
                }
                Err(e) => {
                    keep_partial(script, chapter);
                    return Err(e);
                }
            };
            if fingerprint(&after) != before {
                pin(step, marker.rect);
                step.id = format!("c{}s{}", ci + 1, chapter.steps.len() + 1);
                on_event(Event::Step {
                    chapter: ci,
                    step_id: step.id.clone(),
                });
                done_lines.push(step.narration.clone());
                chapter.steps.push(step.clone());
                *budget = budget.saturating_sub(1);
                *misses = 0;
                *note = window_note(&before_win, &after.window);
            } else {
                *misses += 1;
                *note = format!("\"{}\" changed nothing on screen", step.narration);
            }
            Ok(None)
        }
        Err(TutorError::Step(msg) | TutorError::Ui(msg) | TutorError::Safety(msg)) => {
            *misses += 1;
            *note = msg;
            hand::track(player, true);
            Ok(None)
        }
        Err(e) => {
            keep_partial(script, chapter);
            Err(e)
        }
    }
}

fn window_note(before: &WindowInfo, after: &WindowInfo) -> String {
    if before.hwnd != after.hwnd {
        format!("Now showing \"{}\".", after.title)
    } else if before.title != after.title {
        format!(
            "The window title changed from \"{}\" to \"{}\".",
            before.title, after.title
        )
    } else {
        String::new()
    }
}

fn keep_partial(script: &mut TutorialScript, chapter: &mut Chapter) {
    if !chapter.steps.is_empty() {
        script.chapters.push(std::mem::take(chapter));
    }
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
    let mut player = Player::new(rig, guard, window, Mode::Guide);
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

    for (ci, bc) in board.chapters.iter().take(12).enumerate() {
        if budget == 0 {
            break;
        }
        on_event(Event::Chapter {
            index: ci,
            title: bc.title.clone(),
        });
        player.begin_chapter(ci);
        if matches!(
            drive_chapter(
                script,
                &mut player,
                rig,
                &goal,
                &bc.title,
                &bc.goal,
                ci,
                &mut budget,
                &mut hints,
                opts.max_misses,
                &chat,
                on_event,
                ask,
            )?,
            ChapterStop::End
        ) {
            break;
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
