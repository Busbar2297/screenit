//! Prompts for the language model and parsing of its replies. The model only ever sees a compact
//! text list of UI elements and a small slice of the script, never the whole history.

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
         Controls visible now:\n{}",
        clip(goal, 600),
        clip(app, 120),
        if audience.is_empty() {
            "beginners"
        } else {
            audience
        },
        max_steps.clamp(1, 200),
        snap.compact(150)
    );
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// Messages asking for a fixed replacement of one failed step.
pub fn repair_messages(
    goal: &str,
    chapter: &str,
    done: &[Step],
    failed: &Step,
    error: &str,
    snap: &UiSnapshot,
) -> Vec<ChatMessage> {
    let system = format!(
        "You repair one failed step of a UI tutorial script. Reply with ONE JSON STEP object only.\n{SCHEMA}"
    );
    let recent: Vec<&Step> = done.iter().rev().take(3).rev().collect();
    let user = format!(
        "Tutorial goal: {}\nChapter: {}\nPrevious steps: {}\nFailed step: {}\nError: {}\n\
         Controls visible now:\n{}\nReturn a corrected STEP.",
        clip(goal, 300),
        clip(chapter, 100),
        serde_json::to_string(&recent).unwrap_or_default(),
        serde_json::to_string(failed).unwrap_or_default(),
        clip(error, 400),
        snap.compact(120)
    );
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// Pull a JSON object out of a reply. Thinking blocks are skipped, an opening brace that never
/// closes does not hide a later object, and a script (`chapters`) wins over a stray object.
pub fn extract_json(text: &str) -> Option<String> {
    let stripped = strip_think(text);
    pick(&stripped).or_else(|| if stripped == text { None } else { pick(text) })
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
    let json = extract_json(reply)
        .ok_or_else(|| TutorError::Llm("the reply holds no JSON object".into()))?;
    let mut s: TutorialScript = serde_json::from_str(&json)
        .map_err(|e| TutorError::Llm(format!("the script does not parse: {e}")))?;
    s.goal = clip(goal, 2000);
    s.app = app;
    s.validate()?;
    Ok(s)
}

pub fn parse_step(reply: &str, keep_id: &str) -> TutorResult<Step> {
    let json = extract_json(reply)
        .ok_or_else(|| TutorError::Llm("the reply holds no JSON object".into()))?;
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
}
