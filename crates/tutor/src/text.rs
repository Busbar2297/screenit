//! Prompts and reply parsing for the text layer on finished videos: chapters, title and
//! description from a transcript, transcript clean-up, captions from a keyframe, and which zoom
//! shots matter. Every prompt is small (a few thousand characters) so a 4096-token model can
//! answer, and every parser treats the reply as untrusted: bad output is an error or is ignored,
//! never a panic.

use crate::error::{TutorError, TutorResult};
use crate::plan::extract_keyed;
use crate::traits::ChatMessage;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A transcript line: start time and text.
pub type Item = (u64, String);

/// Longest transcript excerpt put into one prompt.
pub const MAX_EXCERPT: usize = 5000;
const MAX_TITLE: usize = 90;
const MAX_DESCRIPTION: usize = 2000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chapter {
    pub start_ms: u64,
    pub title: String,
}

fn clip(s: &str, n: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(n)
        .collect()
}

/// `[12s] text` lines, thinned evenly when the whole transcript would not fit `budget` characters.
pub fn excerpt(items: &[Item], budget: usize) -> String {
    let line = |(t, s): &Item| format!("[{}s] {}", t / 1000, clip(s, 240).trim());
    let all: Vec<String> = items
        .iter()
        .filter(|(_, s)| !s.trim().is_empty())
        .map(line)
        .collect();
    let total: usize = all.iter().map(|l| l.len() + 1).sum();
    if total <= budget || all.is_empty() {
        return all.join("\n");
    }
    // keep every k-th line so the excerpt still spans the whole video
    let k = total.div_ceil(budget.max(1));
    all.iter()
        .step_by(k.max(2))
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Messages asking for a title and chapters of a recording `duration_ms` long.
pub fn chapters_messages(items: &[Item], duration_ms: u64, max_chapters: u32) -> Vec<ChatMessage> {
    let system = "You name the chapters of a tutorial video from its transcript. Reply with ONE JSON \
        object only, first character {. No markdown, no explanation.\n\
        {\"title\":str,\"chapters\":[{\"start_ms\":int,\"title\":str}]}\n\
        Chapter titles are 2 to 6 words. The first chapter starts at 0. start_ms values increase.";
    let user = format!(
        "Video length: {} seconds. Use at most {} chapters.\nTranscript (seconds from the start):\n{}\n\
         Reply with the JSON object itself, starting with {{.",
        duration_ms / 1000,
        max_chapters.clamp(1, 30),
        excerpt(items, MAX_EXCERPT)
    );
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// Title and chapters from a reply. Chapters are sorted, clamped inside the video, de-duplicated
/// and limited to `max`; the first one is moved to 0.
pub fn parse_chapters(
    reply: &str,
    duration_ms: u64,
    max: usize,
) -> TutorResult<(String, Vec<Chapter>)> {
    let json = extract_keyed(reply, "chapters")
        .ok_or_else(|| TutorError::Llm("the reply holds no chapters".into()))?;
    let v: Value = serde_json::from_str(&json)
        .map_err(|e| TutorError::Llm(format!("the chapters do not parse: {e}")))?;
    let title = clip(
        v.get("title").and_then(Value::as_str).unwrap_or(""),
        MAX_TITLE,
    )
    .trim()
    .to_string();
    let mut list: Vec<Chapter> = v
        .get("chapters")
        .and_then(Value::as_array)
        .ok_or_else(|| TutorError::Llm("the reply holds no chapters".into()))?
        .iter()
        .filter_map(|c| {
            let t = clip(c.get("title")?.as_str()?, MAX_TITLE)
                .trim()
                .to_string();
            let ms = c.get("start_ms")?.as_f64()?;
            if t.is_empty() || !ms.is_finite() || ms < 0.0 {
                return None;
            }
            Some(Chapter {
                start_ms: (ms as u64).min(duration_ms.saturating_sub(1)),
                title: t,
            })
        })
        .collect();
    list.sort_by_key(|c| c.start_ms);
    list.dedup_by_key(|c| c.start_ms);
    list.truncate(max.clamp(1, 30));
    if list.is_empty() {
        return Err(TutorError::Llm("the reply holds no usable chapters".into()));
    }
    list[0].start_ms = 0;
    Ok((title, list))
}

/// Messages asking for a video title and description.
pub fn describe_messages(items: &[Item], chapters: &[Chapter]) -> Vec<ChatMessage> {
    let system = "You write the title and description of a tutorial video from its transcript. \
        Reply with ONE JSON object only, first character {.\n\
        {\"title\":str,\"description\":str}\n\
        The title is under 70 characters. The description is 2 to 4 plain sentences that say what \
        a viewer will learn. No hashtags, no marketing words.";
    let chapter_list: String = chapters
        .iter()
        .take(30)
        .map(|c| format!("- {}s {}\n", c.start_ms / 1000, clip(&c.title, 60)))
        .collect();
    let user = format!(
        "Chapters:\n{chapter_list}Transcript:\n{}\nReply with the JSON object itself, starting with {{.",
        excerpt(items, MAX_EXCERPT - chapter_list.len().min(2000))
    );
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

pub fn parse_description(reply: &str) -> TutorResult<(String, String)> {
    let json = extract_keyed(reply, "description")
        .ok_or_else(|| TutorError::Llm("the reply holds no description".into()))?;
    let v: Value = serde_json::from_str(&json)
        .map_err(|e| TutorError::Llm(format!("the description does not parse: {e}")))?;
    let title = clip(
        v.get("title").and_then(Value::as_str).unwrap_or(""),
        MAX_TITLE,
    )
    .trim()
    .to_string();
    let desc = clip(
        v.get("description").and_then(Value::as_str).unwrap_or(""),
        MAX_DESCRIPTION,
    )
    .trim()
    .to_string();
    if desc.is_empty() {
        return Err(TutorError::Llm("the description is empty".into()));
    }
    Ok((title, desc))
}

/// Messages asking to clean up a batch of subtitle lines (same count back).
pub fn cleanup_messages(lines: &[String]) -> Vec<ChatMessage> {
    let system = "You fix machine-made subtitles. Reply with ONE JSON object only, first character {.\n\
        {\"lines\":[str,...]}\n\
        Return exactly as many lines as you were given, in the same order. Fix punctuation, \
        capitalisation and clearly mis-heard words. Keep the language, the meaning and the length. \
        Never merge, split, add or drop lines.";
    let numbered: String = lines
        .iter()
        .take(40)
        .enumerate()
        .map(|(i, l)| format!("{}. {}\n", i + 1, clip(l, 300).replace('\n', " ")))
        .collect();
    let user = format!(
        "{} lines:\n{numbered}Reply with the JSON object itself, starting with {{.",
        lines.len().min(40)
    );
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// The cleaned lines, or an error when the count does not match (the caller keeps the originals).
pub fn parse_cleanup(reply: &str, expected: usize) -> TutorResult<Vec<String>> {
    let json = extract_keyed(reply, "lines")
        .ok_or_else(|| TutorError::Llm("the reply holds no lines".into()))?;
    let v: Value = serde_json::from_str(&json)
        .map_err(|e| TutorError::Llm(format!("the lines do not parse: {e}")))?;
    let out: Vec<String> = v
        .get("lines")
        .and_then(Value::as_array)
        .ok_or_else(|| TutorError::Llm("the reply holds no lines".into()))?
        .iter()
        .map(|l| {
            clip(l.as_str().unwrap_or(""), 600)
                .replace(['\r', '\n'], " ")
                .trim()
                .to_string()
        })
        .collect();
    if out.len() != expected || out.iter().any(String::is_empty) {
        return Err(TutorError::Llm(format!(
            "the reply has {} usable lines, {expected} were expected",
            out.len()
        )));
    }
    Ok(out)
}

/// Messages asking for one instructional caption for a screenshot (attach the picture).
pub fn frame_caption_messages(index: usize, total: usize, previous: &str) -> Vec<ChatMessage> {
    let system = "You caption a screen recording for viewers. Reply with ONE JSON object only, first \
        character {.\n{\"caption\":str}\n\
        The caption is one short sentence (under 90 characters) saying what the viewer should notice \
        or do in this screenshot. Do not read out passwords, keys or personal data.";
    let user = format!(
        "Screenshot {} of {}. {}Reply with the JSON object itself, starting with {{.",
        index + 1,
        total.max(1),
        if previous.trim().is_empty() {
            String::new()
        } else {
            format!(
                "The previous caption was: {}. Do not repeat it. ",
                clip(previous, 120)
            )
        }
    );
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

pub fn parse_frame_caption(reply: &str) -> TutorResult<String> {
    let json = extract_keyed(reply, "caption")
        .ok_or_else(|| TutorError::Llm("the reply holds no caption".into()))?;
    let v: Value = serde_json::from_str(&json)
        .map_err(|e| TutorError::Llm(format!("the caption does not parse: {e}")))?;
    let c = clip(v.get("caption").and_then(Value::as_str).unwrap_or(""), 160)
        .replace('\n', " ")
        .trim()
        .to_string();
    if c.is_empty() {
        Err(TutorError::Llm("the caption is empty".into()))
    } else {
        Ok(c)
    }
}

/// A zoom shot described for the importance filter: when, and where on screen (percent).
#[derive(Clone, Debug)]
pub struct ShotInfo {
    pub at_ms: u64,
    pub x_pct: u32,
    pub y_pct: u32,
    pub w_pct: u32,
    pub h_pct: u32,
}

/// Messages asking which zoom shots deserve a zoom.
pub fn shots_messages(shots: &[ShotInfo], speech: &str) -> Vec<ChatMessage> {
    let system = "You decide which zoom-ins to keep in an edited screen recording. Reply with ONE \
        JSON object only, first character {.\n{\"keep\":[int,...]}\n\
        Keep the zooms that point at something the narration talks about or an important control. \
        Drop zooms for accidental or repeated clicks. Always keep at least one when any are listed.";
    let list: String = shots
        .iter()
        .take(60)
        .enumerate()
        .map(|(i, s)| {
            format!(
                "{i}: at {}s, target {}%,{}% size {}x{}%\n",
                s.at_ms / 1000,
                s.x_pct,
                s.y_pct,
                s.w_pct,
                s.h_pct
            )
        })
        .collect();
    let user = format!(
        "Zoom candidates:\n{list}Narration excerpt:\n{}\nReply with the JSON object itself, starting with {{.",
        clip(speech, 2500)
    );
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// Indices to keep, restricted to `0..n`. Never empty when `n > 0`.
pub fn parse_keep(reply: &str, n: usize) -> TutorResult<Vec<usize>> {
    let json = extract_keyed(reply, "keep")
        .ok_or_else(|| TutorError::Llm("the reply holds no `keep` list".into()))?;
    let v: Value = serde_json::from_str(&json)
        .map_err(|e| TutorError::Llm(format!("the list does not parse: {e}")))?;
    let mut keep: Vec<usize> = v
        .get("keep")
        .and_then(Value::as_array)
        .ok_or_else(|| TutorError::Llm("the reply holds no `keep` list".into()))?
        .iter()
        .filter_map(Value::as_u64)
        .filter_map(|i| usize::try_from(i).ok())
        .filter(|i| *i < n)
        .collect();
    keep.sort_unstable();
    keep.dedup();
    if keep.is_empty() && n > 0 {
        keep.push(0);
    }
    Ok(keep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: usize) -> Vec<Item> {
        (0..n)
            .map(|i| {
                (
                    i as u64 * 4000,
                    format!("Line number {i} about opening the settings dialog."),
                )
            })
            .collect()
    }

    #[test]
    fn prompts_fit_a_small_context_window() {
        let long = items(5000);
        let m = chapters_messages(&long, 20_000_000, 99);
        let total: usize = m.iter().map(|x| x.content.len()).sum();
        assert!(total < 7000, "{total}");
        assert!(m[1].content.contains("at most 30 chapters"));
        // thinning still spans the whole video
        assert!(m[1].content.contains("[0s]"));
        let d = describe_messages(
            &long,
            &(0..99)
                .map(|i| Chapter {
                    start_ms: i * 1000,
                    title: "x".repeat(500),
                })
                .collect::<Vec<_>>(),
        );
        let total: usize = d.iter().map(|x| x.content.len()).sum();
        assert!(total < 8000, "{total}");
        let c = cleanup_messages(&vec!["a".repeat(5000); 100]);
        assert!(c[1].content.len() < 14_000);
        assert!(excerpt(&[], 100).is_empty());
        assert_eq!(excerpt(&items(2), 10_000).lines().count(), 2);
    }

    #[test]
    fn chapters_are_sorted_clamped_and_capped() {
        let reply = r#"Sure {"title":"Setup guide","chapters":[
            {"start_ms":9000,"title":"Second"},{"start_ms":500,"title":"First"},
            {"start_ms":9000,"title":"Dup"},{"start_ms":-5,"title":"Bad"},
            {"start_ms":99999999,"title":"Late"},{"start_ms":1,"title":""}]}"#;
        let (title, ch) = parse_chapters(reply, 60_000, 10).unwrap();
        assert_eq!(title, "Setup guide");
        assert_eq!(ch[0].start_ms, 0, "first chapter starts at zero");
        assert_eq!(ch[0].title, "First");
        assert!(ch.windows(2).all(|w| w[0].start_ms < w[1].start_ms));
        assert!(ch.iter().all(|c| c.start_ms < 60_000));
        assert_eq!(parse_chapters(reply, 60_000, 1).unwrap().1.len(), 1);
        assert!(parse_chapters("nothing", 1000, 5).is_err());
        assert!(parse_chapters(r#"{"chapters":[]}"#, 1000, 5).is_err());
        assert!(parse_chapters(r#"{"chapters":"x"}"#, 1000, 5).is_err());
    }

    #[test]
    fn description_cleanup_and_caption_parsers() {
        let (t, d) =
            parse_description(r#"{"title":"T","description":"Learn it.\n\nFast."}"#).unwrap();
        assert_eq!((t.as_str(), d.as_str()), ("T", "Learn it.\n\nFast."));
        assert!(parse_description(r#"{"description":" "}"#).is_err());
        assert!(parse_description("hi").is_err());
        let ok = parse_cleanup(r#"{"lines":["One.","Two."]}"#, 2).unwrap();
        assert_eq!(ok, vec!["One.", "Two."]);
        assert!(parse_cleanup(r#"{"lines":["One."]}"#, 2).is_err());
        assert!(parse_cleanup(r#"{"lines":["One.",""]}"#, 2).is_err());
        assert!(parse_cleanup(r#"{"lines":[1,2]}"#, 2).is_err());
        assert_eq!(
            parse_frame_caption(r#"{"caption":" Click Save. "}"#).unwrap(),
            "Click Save."
        );
        assert!(parse_frame_caption(r#"{"caption":""}"#).is_err());
        let m = frame_caption_messages(0, 3, "Open the menu");
        assert!(
            m[1].content.contains("Screenshot 1 of 3") && m[1].content.contains("Open the menu")
        );
    }

    #[test]
    fn keep_list_is_bounded_and_never_empty() {
        assert_eq!(
            parse_keep(r#"{"keep":[2,0,0,9,-1,"x"]}"#, 3).unwrap(),
            vec![0, 2]
        );
        assert_eq!(parse_keep(r#"{"keep":[]}"#, 3).unwrap(), vec![0]);
        assert!(parse_keep(r#"{"keep":[]}"#, 0).unwrap().is_empty());
        assert!(parse_keep("no", 3).is_err());
        let shots = vec![
            ShotInfo {
                at_ms: 1000,
                x_pct: 10,
                y_pct: 20,
                w_pct: 5,
                h_pct: 3
            };
            100
        ];
        let m = shots_messages(&shots, &"x".repeat(9000));
        assert!(m[1].content.len() < 6000);
    }
}
