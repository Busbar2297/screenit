//! Prompts for the language model and parsing of its replies. The model sees a compact text list
//! of UI elements (with their real screen rectangles) and a small slice of the script. A repair
//! may also attach a screenshot of the window in front.

use crate::error::{TutorError, TutorResult};
use crate::script::{Step, TutorialScript};
use crate::traits::{ChatMessage, UiSnapshot};

const SCHEMA: &str = r#"The whole reply is ONE JSON object. The first character is {. No markdown and no explanation.
{"title":str,"chapters":[{"title":str,"summary":str,"steps":[STEP,...]}]}
STEP = {"narration":str,"action":ACTION,"expect":[EXPECT,...],"destructive":bool?}
ACTION is one of:
 {"type":"click","target":SEL} {"type":"double_click","target":SEL} {"type":"right_click","target":SEL}
 {"type":"hover","target":SEL} {"type":"type","target":SEL?,"text":str} {"type":"key","keys":"ctrl+s"}
 {"type":"scroll","target":SEL?,"dy":int} {"type":"drag","from":SEL,"to":SEL} {"type":"wait","ms":int}
SEL = {"role":str,"name":str,"automation_id":str?,"within":str?,"nth":int?}  (use names and ids that appear in the UI list)
EXPECT is one of:
 {"type":"appears","target":SEL} {"type":"disappears","target":SEL} {"type":"focused","target":SEL}
 {"type":"value_is","target":SEL,"value":str} {"type":"value_contains","target":SEL,"value":str}
 {"type":"title_contains","text":str}
Rules: one small user action per step; narration is one short sentence a viewer reads; add an expect for anything
that visibly changes; mark delete, send, buy and close-without-saving steps "destructive":true; never type secrets."#;

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Messages asking for a whole script.
pub fn plan_messages(
    goal: &str,
    app: &str,
    audience: &str,
    max_steps: u32,
    snap: &UiSnapshot,
) -> Vec<ChatMessage> {
    let system = format!(
        "You write software tutorials as machine-runnable scripts. Each script is played on the \
         real app through UI Automation, so only use controls that exist.\n{SCHEMA}"
    );
    let user = format!(
        "Goal: {}\nApp: {}\nAudience: {}\nUse at most {} steps, grouped into short chapters.\n\
         Controls visible now:\n{}\n\
         Do not explain or plan aloud: your reply must be the JSON object itself, starting with {{.",
        clip(goal, 600),
        clip(app, 120),
        if audience.is_empty() {
            "beginners"
        } else {
            audience
        },
        max_steps.clamp(1, 200),
        snap.compact_within(60, 3000, false)
    );
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// What the repair prompt should trust: a picture, and a window that was not the one we started on.
pub struct RepairLook<'a> {
    pub picture: bool,
    pub switched_to: Option<&'a str>,
}

/// Messages asking for a fixed replacement of one failed step.
pub fn repair_messages(
    goal: &str,
    chapter: &str,
    done: &[Step],
    failed: &Step,
    error: &str,
    snap: &UiSnapshot,
    look: RepairLook<'_>,
) -> Vec<ChatMessage> {
    // Only the step part of the schema: a small model window has no room for the rest.
    let step_schema = SCHEMA
        .split_once("STEP = ")
        .map_or(SCHEMA, |(_, rest)| rest);
    let system = format!(
        "You repair one failed step of a UI tutorial script. Reply with ONE JSON STEP object only, first character {{.\nSTEP = {step_schema}\n\
         A SEL may also carry \"rect\":{{\"x\":int,\"y\":int,\"w\":int,\"h\":int}}: copy it from the control list."
    );
    let recent: Vec<&Step> = done.iter().rev().take(3).rev().collect();
    let look_line = if look.picture {
        "A screenshot of this window is attached. Trust the picture when a dialog, menu or other window appeared."
    } else {
        "No screenshot was available."
    };
    let switched = look
        .switched_to
        .map(|title| format!("The window in front changed to \"{title}\". Act on that window.\n"))
        .unwrap_or_default();
    let user = format!(
        "Tutorial goal: {}\nChapter: {}\nPrevious steps: {}\nFailed step: {}\nError: {}\n\
         {switched}{look_line}\nCopy role, name and the at x,y wxh rectangle from the list below. \
         Never invent a position.\nControls visible now:\n{}\nReturn a corrected STEP. \
         This step replaces the failed one in the script. Do not explain: reply with the JSON object only.",
        clip(goal, 300),
        clip(chapter, 100),
        serde_json::to_string(&recent).unwrap_or_default(),
        serde_json::to_string(failed).unwrap_or_default(),
        clip(error, 300),
        snap.compact_within(30, 1800, true)
    );
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// Pull a JSON object out of a reply. Thinking blocks are skipped, an opening brace that never
/// closes does not hide a later object, and a script (`chapters`) wins over a stray object.
pub fn extract_json(text: &str) -> Option<String> {
    let stripped = strip_think(text);
    pick(&stripped).or_else(|| if stripped == text { None } else { pick(text) })
}

/// The last JSON object in a reply that has a top-level `key` (`chapters` for a script, `action`
/// for a step). A stray `{"type":...}` quoted while the model thinks aloud is never taken for it.
fn extract_keyed(text: &str, key: &str) -> Option<String> {
    let stripped = strip_think(text);
    keyed(&stripped, key).or_else(|| {
        if stripped == text {
            None
        } else {
            keyed(text, key)
        }
    })
}

fn keyed(text: &str, key: &str) -> Option<String> {
    let mut found = None;
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        let slice = &rest[i..];
        let hit = balanced(slice).filter(|obj| {
            serde_json::from_str::<serde_json::Value>(obj).is_ok_and(|v| v.get(key).is_some())
        });
        if let Some(obj) = hit {
            found = Some(obj.to_string());
            rest = &slice[obj.len()..];
        } else {
            rest = &slice[1..];
        }
    }
    found
}

fn strip_think(text: &str) -> String {
    const OPENS: &[&str] = &["<think>", "<|think|>", "<|channel>thought"];
    const CLOSES: &[&str] = &["</think>", "<|/think|>", "<channel|>"];
    let mut out = String::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let open = OPENS.iter().find(|m| rest.starts_with(*m)).copied();
        if let Some(marker) = open {
            let after = i + marker.len();
            let tail = &text[after..];
            if let Some((at, n)) = CLOSES
                .iter()
                .filter_map(|end| tail.find(end).map(|p| (p, end.len())))
                .min_by_key(|(p, _)| *p)
            {
                i = after + at + n;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap_or('\u{0}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// The first balanced `{...}` in `slice`, which must start at `{`.
fn balanced(slice: &str) -> Option<&str> {
    let bytes = slice.as_bytes();
    if bytes.first() != Some(&b'{') {
        return None;
    }
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return slice.get(..=i);
                }
            }
            _ => {}
        }
    }
    None
}

fn pick(text: &str) -> Option<String> {
    let mut step: Option<String> = None;
    let mut any: Option<String> = None;
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        let slice = &rest[i..];
        if let Some(obj) = balanced(slice)
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(obj)
        {
            if v.get("chapters").is_some() {
                return Some(obj.to_string());
            }
            if step.is_none() && v.get("action").is_some() {
                step = Some(obj.to_string());
            } else if any.is_none() && v.is_object() {
                any = Some(obj.to_string());
            }
        }
        rest = &slice[1..];
    }
    step.or(any)
}

pub fn parse_script(
    reply: &str,
    goal: &str,
    app: crate::script::AppTarget,
) -> TutorResult<TutorialScript> {
    let json = extract_keyed(reply, "chapters").ok_or_else(|| {
        TutorError::Llm(
            "the reply holds no script (no object with `chapters`). The model probably ran out of room while thinking: raise its context length in LM Studio"
                .into(),
        )
    })?;
    let mut s: TutorialScript = serde_json::from_str(&json)
        .map_err(|e| TutorError::Llm(format!("the script does not parse: {e}")))?;
    s.goal = clip(goal, 2000);
    s.app = app;
    s.validate()?;
    Ok(s)
}

pub fn parse_step(reply: &str, keep_id: &str) -> TutorResult<Step> {
    let json = extract_keyed(reply, "action").ok_or_else(|| {
        TutorError::Llm("the reply holds no step (no object with `action`)".into())
    })?;
    let mut s: Step = serde_json::from_str(&json)
        .map_err(|e| TutorError::Llm(format!("the step does not parse: {e}")))?;
    s.id = keep_id.to_string();
    s.verified_rect = None;
    s.timeout_ms = s.timeout_ms.clamp(200, 60_000);
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::AppTarget;

    #[test]
    fn extracts_json_from_prose_and_fences() {
        let t = "Sure!\n```json\n{\"a\":{\"b\":\"}\"},\"c\":1}\n```\nbye";
        assert_eq!(
            extract_json(t).as_deref(),
            Some("{\"a\":{\"b\":\"}\"},\"c\":1}")
        );
        assert_eq!(extract_json("no json"), None);
        assert_eq!(extract_json("{\"unclosed\":1"), None);
        let thought =
            "<think>draft {title and never close</think>\n{\"title\":\"T\",\"chapters\":[]}";
        assert!(extract_json(thought).unwrap().contains("chapters"));
        let later = "note {this then {\"title\":\"T\",\"chapters\":[]}";
        assert!(extract_json(later).unwrap().contains("chapters"));
    }

    #[test]
    fn parses_a_script_and_a_step() {
        let reply = r#"Here: {"title":"T","chapters":[{"title":"C","steps":[
          {"narration":"Click Save","action":{"type":"click","target":{"role":"Button","name":"Save"}}}]}]}"#;
        let s = parse_script(reply, "goal", AppTarget::default()).unwrap();
        assert_eq!(s.goal, "goal");
        assert_eq!(s.chapters[0].steps[0].id, "c1s1");
        let step = parse_step(
            r#"{"action":{"type":"key","keys":"enter"},"timeout_ms":5}"#,
            "c1s1",
        )
        .unwrap();
        assert_eq!(step.id, "c1s1");
        assert_eq!(step.timeout_ms, 200);
    }

    #[test]
    fn a_stray_object_in_thinking_is_not_the_script() {
        let thinking = r#"*   Goal: open a file. I could use {"type":"click","target":{"role":"Button","name":"Open"}} here."#;
        let e = parse_script(thinking, "g", AppTarget::default()).unwrap_err();
        assert!(e.to_string().contains("no script"), "{e}");
        assert!(parse_step(thinking, "x").is_err());
        let both = format!(
            "{thinking}\n{{\"title\":\"T\",\"chapters\":[{{\"title\":\"C\",\"steps\":[{{\"action\":{{\"type\":\"key\",\"keys\":\"enter\"}}}}]}}]}}"
        );
        assert!(parse_script(&both, "g", AppTarget::default()).is_ok());
    }

    #[test]
    fn a_misspelled_field_from_the_model_is_ignored() {
        let reply = r#"{"title":"T","chapters":[{"title":"C","steps":[
          {"narration":"Open File","action":{"type":"click","target":{"role":"Button","name":"File","automation_s_id":"x"}},"extra":1}]}]}"#;
        let s = parse_script(reply, "g", AppTarget::default()).unwrap();
        assert_eq!(s.chapters[0].steps.len(), 1);
    }

    #[test]
    fn garbage_replies_are_errors() {
        assert!(parse_script("hello", "g", AppTarget::default()).is_err());
        assert!(parse_script("{\"title\":1}", "g", AppTarget::default()).is_err());
        assert!(parse_step("{}", "x").is_err());
    }

    #[test]
    fn prompts_carry_the_ui_list_and_cap_input() {
        let snap = UiSnapshot::default();
        let m = plan_messages(&"g".repeat(5000), "App", "", 999, &snap);
        assert_eq!(m.len(), 2);
        assert!(m[1].content.contains("at most 200 steps"));
        assert!(m[1].content.len() < 2000);
    }

    #[test]
    fn prompts_fit_a_small_context_window() {
        use crate::traits::UiElement;
        use screenit_geom::Rect;
        let elements: Vec<UiElement> = (0..400)
            .map(|i| UiElement {
                role: "Button".into(),
                name: format!("Button number {i} with a rather long label"),
                rect: Rect::new(i, i, 50, 20),
                enabled: true,
                depth: 1,
                ..UiElement::default()
            })
            .collect();
        let snap = UiSnapshot {
            elements,
            ..UiSnapshot::default()
        }
        .finish();
        let plan = plan_messages("teach me", "App", "", 30, &snap);
        let total: usize = plan.iter().map(|m| m.content.len()).sum();
        assert!(total < 7000, "{total}");
        assert!(!plan[1].content.contains("50x20"), "no rects when planning");
        let failed = parse_step(r#"{"action":{"type":"key","keys":"enter"}}"#, "c1s1").unwrap();
        let fix = repair_messages(
            "g",
            "c",
            &[],
            &failed,
            "e",
            &snap,
            RepairLook {
                picture: true,
                switched_to: None,
            },
        );
        let total: usize = fix.iter().map(|m| m.content.len()).sum();
        assert!(total < 7000, "{total}");
    }

    #[test]
    fn repair_prompt_names_real_rects_and_the_screenshot() {
        use crate::traits::{UiElement, WindowInfo};
        use screenit_geom::Rect;
        let mut snap = UiSnapshot {
            window: WindowInfo {
                title: "Open".into(),
                ..WindowInfo::default()
            },
            elements: vec![UiElement {
                role: "Button".into(),
                name: "Open".into(),
                rect: Rect::new(10, 20, 30, 40),
                enabled: true,
                depth: 1,
                ..UiElement::default()
            }],
        };
        snap = snap.finish();
        assert!(snap.compact(10).contains("at 10,20 30x40"));
        let failed = parse_step(
            r#"{"action":{"type":"click","target":{"role":"ListItem","name":"Open"}}}"#,
            "c2s1",
        )
        .unwrap();
        let msgs = repair_messages(
            "open a file",
            "Opening",
            &[],
            &failed,
            "not found",
            &snap,
            RepairLook {
                picture: true,
                switched_to: Some("Open"),
            },
        );
        let prompt = &msgs[1].content;
        assert!(prompt.contains("screenshot"), "{prompt}");
        assert!(prompt.contains("changed to \"Open\""), "{prompt}");
        assert!(prompt.contains("at 10,20 30x40"), "{prompt}");
        assert!(prompt.contains("Never invent a position"), "{prompt}");
    }
}
