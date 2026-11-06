//! Text layer on finished videos, produced by the local language model: chapters and a title,
//! a description, cleaned-up subtitles, captions from keyframes. Each is an optional background
//! job (`tutor.status` / `tutor.wait`) whose result is plain data kept beside the video
//! (`clip.chapters.json`, `clip.captions.json`) and editable by hand or with the `*.set` commands.

use crate::args::Args;
use crate::speech_cmds::{load_captions, reader, save_captions, video_path};
use crate::tutor_cmds::{chat_setup, spawn_job};
use crate::{CmdResult, CommandError, CommandSpec, Session, always};
use screenit_record::speech::Line;
use screenit_tutor::text::{self, Chapter, Item};
use screenit_tutor::{ChatOptions, LlmChat, Shot, TutorError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Replies are short; keeping the cap low also suits a 4096-token model window.
const REPLY_TOKENS: u32 = 1024;

/// Title, description and chapters of a video.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    pub title: String,
    pub description: String,
    pub chapters: Vec<Chapter>,
}

fn meta_path(video: &Path) -> PathBuf {
    video.with_extension("chapters.json")
}

fn load_meta(video: &Path) -> Meta {
    std::fs::read_to_string(meta_path(video))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_meta(video: &Path, m: &Meta) -> Result<(), String> {
    let text = serde_json::to_string_pretty(m).map_err(|e| e.to_string())?;
    std::fs::write(meta_path(video), text).map_err(|e| e.to_string())
}

/// `0:00 Intro` lines in the form video sites read as chapters.
pub fn youtube_chapters(chapters: &[Chapter]) -> String {
    chapters
        .iter()
        .map(|c| {
            let s = c.start_ms / 1000;
            if s >= 3600 {
                format!("{}:{:02}:{:02} {}", s / 3600, s / 60 % 60, s % 60, c.title)
            } else {
                format!("{}:{:02} {}", s / 60, s % 60, c.title)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn llm(s: &Session) -> Result<(Arc<dyn LlmChat>, ChatOptions), CommandError> {
    let (chat, mut o) = chat_setup(s)
        .ok_or_else(|| CommandError::Disabled("no language model available".into()))?;
    o.max_tokens = REPLY_TOKENS;
    Ok((chat, o))
}

fn items_of(video: &Path) -> Result<Vec<Item>, CommandError> {
    let lines = load_captions(video)?;
    if lines.is_empty() {
        return Err(CommandError::Failed(
            "the subtitles are empty: run `video.transcribe` first".into(),
        ));
    }
    Ok(lines.into_iter().map(|l| (l.start_ms, l.text)).collect())
}

fn duration_of(s: &Session, video: &Path) -> Result<u64, CommandError> {
    Ok(reader(s)?
        .probe(video)
        .map_err(CommandError::failed)?
        .duration_ms)
}

fn io(e: impl std::fmt::Display) -> TutorError {
    TutorError::Llm(e.to_string())
}

fn chapters_suggest(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "max_chapters"])?;
    let path = video_path(&a)?;
    let max = a.opt_u32("max_chapters")?.unwrap_or(8).clamp(1, 30);
    let items = items_of(&path)?;
    let duration = duration_of(s, &path)?;
    let (chat, opts) = llm(s)?;
    spawn_job(s, "chapters", move |sh| {
        sh.set_progress(0, 1);
        let reply = chat.chat(&text::chapters_messages(&items, duration, max), &opts)?;
        let (title, chapters) = text::parse_chapters(&reply, duration, max as usize)?;
        let mut m = load_meta(&path);
        if !title.is_empty() {
            m.title = title;
        }
        m.chapters = chapters;
        save_meta(&path, &m).map_err(io)?;
        sh.set_progress(1, 1);
        Ok(json!({
            "title": m.title, "chapters": m.chapters,
            "youtube": youtube_chapters(&m.chapters), "path": meta_path(&path),
        }))
    })
}

fn describe(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path"])?;
    let path = video_path(&a)?;
    let items = items_of(&path)?;
    let (chat, opts) = llm(s)?;
    spawn_job(s, "describing", move |sh| {
        sh.set_progress(0, 1);
        let mut m = load_meta(&path);
        let reply = chat.chat(&text::describe_messages(&items, &m.chapters), &opts)?;
        let (title, desc) = text::parse_description(&reply)?;
        if m.title.is_empty() && !title.is_empty() {
            m.title = title;
        }
        m.description = desc;
        save_meta(&path, &m).map_err(io)?;
        sh.set_progress(1, 1);
        Ok(json!({
            "title": m.title, "description": m.description, "path": meta_path(&path),
        }))
    })
}

fn chapters_get(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path"])?;
    let _ = s;
    let m = load_meta(&video_path(&a)?);
    Ok(json!({
        "title": m.title, "description": m.description, "chapters": m.chapters,
        "youtube": youtube_chapters(&m.chapters),
    }))
}

fn chapters_set(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "title", "description", "chapters"])?;
    let path = video_path(&a)?;
    let mut m = load_meta(&path);
    if let Some(t) = a.opt_str("title")? {
        m.title = t.chars().take(200).collect();
    }
    if let Some(d) = a.opt_str("description")? {
        m.description = d.chars().take(5000).collect();
    }
    if p.get("chapters").is_some() {
        let mut list: Vec<Chapter> =
            serde_json::from_value(p["chapters"].clone()).map_err(|e| {
                CommandError::Params(format!("`chapters` must be [{{start_ms,title}}]: {e}"))
            })?;
        if list.len() > 100 || list.iter().any(|c| c.title.chars().count() > 200) {
            return Err(CommandError::Params(
                "at most 100 chapters with titles up to 200 characters".into(),
            ));
        }
        let duration = duration_of(s, &path).unwrap_or(u64::MAX);
        if list.iter().any(|c| c.start_ms >= duration) {
            return Err(CommandError::Params(
                "a chapter starts after the end of the video".into(),
            ));
        }
        list.sort_by_key(|c| c.start_ms);
        m.chapters = list;
    }
    save_meta(&path, &m).map_err(CommandError::Failed)?;
    Ok(json!({"chapters": m.chapters.len()}))
}

fn polish_captions(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "mode", "segments", "replace"])?;
    let path = video_path(&a)?;
    let mode = a.opt_str("mode")?.unwrap_or("cleanup").to_string();
    if mode != "cleanup" && mode != "frames" {
        return Err(CommandError::Params(
            "mode must be cleanup or frames".into(),
        ));
    }
    let (chat, opts) = llm(s)?;
    if mode == "cleanup" {
        let lines = load_captions(&path)?;
        if lines.is_empty() {
            return Err(CommandError::Failed(
                "there are no subtitles to clean".into(),
            ));
        }
        return spawn_job(s, "polishing text", move |sh| {
            let total = lines.len();
            let mut out: Vec<Line> = Vec::with_capacity(total);
            let (mut cleaned, mut kept) = (0usize, 0usize);
            for batch in lines.chunks(12) {
                if sh.abort.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(TutorError::Llm("aborted".into()));
                }
                let texts: Vec<String> = batch.iter().map(|l| l.text.clone()).collect();
                let fixed = chat
                    .chat(&text::cleanup_messages(&texts), &opts)
                    .and_then(|r| text::parse_cleanup(&r, texts.len()));
                match fixed {
                    Ok(f) => {
                        cleaned += batch.len();
                        out.extend(batch.iter().zip(f).map(|(l, t)| Line {
                            text: t,
                            ..l.clone()
                        }));
                    }
                    Err(_) => {
                        kept += batch.len();
                        out.extend(batch.iter().cloned());
                    }
                }
                sh.set_progress(
                    u32::try_from(out.len()).unwrap_or(u32::MAX),
                    u32::try_from(total).unwrap_or(u32::MAX),
                );
            }
            if cleaned == 0 {
                return Err(TutorError::Llm(
                    "the model gave no usable answer; the subtitles are unchanged".into(),
                ));
            }
            save_captions(&path, &out).map_err(|e| TutorError::Llm(e.to_string()))?;
            Ok(json!({"lines": total, "cleaned": cleaned, "unchanged": kept}))
        });
    }
    // frames: caption evenly spaced keyframes, for a recording with no speech
    if !a.bool("replace", false)? && load_captions(&path).is_ok_and(|l| !l.is_empty()) {
        return Err(CommandError::Failed(
            "subtitles already exist: pass `replace` to write new ones from the picture".into(),
        ));
    }
    let n = a.opt_u32("segments")?.unwrap_or(8).clamp(1, 20) as u64;
    let rd = reader(s)?;
    let duration = rd.probe(&path).map_err(CommandError::failed)?.duration_ms;
    if duration < 1000 {
        return Err(CommandError::Failed("the recording is too short".into()));
    }
    spawn_job(s, "captioning", move |sh| {
        let mut lines: Vec<Line> = Vec::new();
        let mut last = String::new();
        for i in 0..n {
            if sh.abort.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(TutorError::Llm("aborted".into()));
            }
            sh.set_progress(i as u32, n as u32);
            let (start, end) = (duration * i / n, duration * (i + 1) / n);
            let frame = rd.frame_at(&path, start + (end - start) / 2).map_err(io)?;
            let shot = Shot {
                width: frame.width(),
                height: frame.height(),
                rgba: frame.data().to_vec(),
            };
            let caption = chat
                .chat_image(
                    &text::frame_caption_messages(i as usize, n as usize, &last),
                    &shot,
                    &opts,
                )
                .and_then(|r| text::parse_frame_caption(&r));
            if let Ok(c) = caption {
                last.clone_from(&c);
                lines.push(Line {
                    start_ms: start,
                    end_ms: end,
                    text: c,
                });
            }
        }
        if lines.is_empty() {
            return Err(TutorError::Llm(
                "the model gave no caption for any keyframe".into(),
            ));
        }
        save_captions(&path, &lines).map_err(|e| TutorError::Llm(e.to_string()))?;
        Ok(json!({"lines": lines.len(), "segments": n}))
    })
}

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, params, run| CommandSpec {
        id,
        label,
        menu: "Video",
        shortcut: "",
        params,
        enabled: always,
        run,
    };
    vec![
        c(
            "video.chapters.suggest",
            "Suggest Chapters",
            r#"{"path":str,"max_chapters"?:1..30} -> background job; needs subtitles; writes clip.chapters.json"#,
            chapters_suggest,
        ),
        c(
            "video.describe",
            "Write Description",
            r#"{"path":str} -> background job; needs subtitles; writes clip.chapters.json"#,
            describe,
        ),
        c(
            "video.chapters.get",
            "Get Chapters",
            r#"{"path":str}"#,
            chapters_get,
        ),
        c(
            "video.chapters.set",
            "Set Chapters",
            r#"{"path":str,"title"?:str,"description"?:str,"chapters"?:[{"start_ms":int,"title":str}]}"#,
            chapters_set,
        ),
        c(
            "video.captions.polish",
            "Polish Subtitles",
            r#"{"path":str,"mode"?:cleanup|frames,"segments"?:1..20,"replace"?:bool} -> background job; cleanup fixes the subtitle text, frames captions keyframes of a silent recording"#,
            polish_captions,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenit_record::{RecordError, VideoInfo, VideoReader};
    use screenit_tutor::fakes::FakeLlm;

    struct Reader;
    impl VideoReader for Reader {
        fn probe(&self, _: &Path) -> Result<VideoInfo, RecordError> {
            Ok(VideoInfo {
                duration_ms: 60_000,
                width: 64,
                height: 36,
                has_audio: true,
            })
        }
        fn frame_at(&self, _: &Path, _: u64) -> Result<screenit_raster::Raster, RecordError> {
            screenit_raster::Raster::new(64, 36, [10, 20, 30, 255])
                .map_err(|e| RecordError::Platform(e.to_string()))
        }
    }

    fn session(llm: Arc<FakeLlm>) -> Session {
        Session::new(crate::Services {
            video: Some(Arc::new(Reader)),
            chat: Some(llm),
            ..crate::Services::default()
        })
    }

    fn clip(tag: &str, with_captions: bool) -> PathBuf {
        let d = std::env::temp_dir().join(format!("scit-text-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let v = d.join("clip.mp4");
        std::fs::write(&v, b"x").unwrap();
        if with_captions {
            let lines: Vec<Line> = (0..14)
                .map(|i| Line {
                    start_ms: i * 4000,
                    end_ms: i * 4000 + 3000,
                    text: format!("line {i}"),
                })
                .collect();
            save_captions(&v, &lines).unwrap();
        }
        v
    }

    fn wait(s: &mut Session) -> Value {
        let st = s
            .dispatch("tutor.wait", &json!({"timeout_ms": 30000}))
            .unwrap();
        assert_eq!(st["running"], false, "{st}");
        st
    }

    #[test]
    fn chapters_and_description_are_written_beside_the_video() {
        let v = clip("meta", true);
        let llm = FakeLlm::new(vec![
            Ok(r#"{"title":"Setup","chapters":[{"start_ms":0,"title":"Intro"},{"start_ms":30000,"title":"Install"},{"start_ms":3700000,"title":"Past the end"}]}"#.into()),
            Ok(r#"{"title":"Ignored","description":"You will install it."}"#.into()),
        ]);
        let mut s = session(llm.clone());
        s.dispatch("video.chapters.suggest", &json!({"path": v}))
            .unwrap();
        let st = wait(&mut s);
        assert!(st["error"].is_null(), "{st}");
        assert!(
            st["result"]["youtube"]
                .as_str()
                .unwrap()
                .starts_with("0:00 Intro\n0:30 Install")
        );
        s.dispatch("video.describe", &json!({"path": v})).unwrap();
        assert!(wait(&mut s)["error"].is_null());
        let g = s
            .dispatch("video.chapters.get", &json!({"path": v}))
            .unwrap();
        assert_eq!(g["title"], "Setup", "the first title is kept");
        assert_eq!(g["description"], "You will install it.");
        assert_eq!(g["chapters"].as_array().map(Vec::len), Some(3));
        // the prompts were small and the model saw the transcript
        let calls = llm.calls();
        assert!(calls[0][1].content.contains("line 3"));
        assert!(calls[1][1].content.contains("Install"));
        // hand edits
        s.dispatch(
            "video.chapters.set",
            &json!({"path": v, "title": "Mine", "chapters": [{"start_ms": 5000, "title": "B"}, {"start_ms": 0, "title": "A"}]}),
        )
        .unwrap();
        let g = s
            .dispatch("video.chapters.get", &json!({"path": v}))
            .unwrap();
        assert_eq!(g["title"], "Mine");
        assert_eq!(g["chapters"][0]["title"], "A");
        assert!(
            s.dispatch(
                "video.chapters.set",
                &json!({"path": v, "chapters": [{"start_ms": 99_000_000u64, "title": "X"}]})
            )
            .is_err()
        );
        assert!(
            s.dispatch("video.chapters.set", &json!({"path": v, "chapters": "no"}))
                .is_err()
        );
        let _ = std::fs::remove_dir_all(v.parent().unwrap());
    }

    #[test]
    fn subtitle_cleanup_keeps_lines_when_the_model_misbehaves() {
        let v = clip("clean", true);
        // 14 lines = two batches: the first answers well, the second returns the wrong count
        let good: Vec<String> = (0..12).map(|i| format!("Line {i}.")).collect();
        let llm = FakeLlm::new(vec![
            Ok(json!({"lines": good}).to_string()),
            Ok(r#"{"lines":["only one"]}"#.into()),
        ]);
        let mut s = session(llm);
        s.dispatch("video.captions.polish", &json!({"path": v}))
            .unwrap();
        let st = wait(&mut s);
        assert!(st["error"].is_null(), "{st}");
        assert_eq!(st["result"]["cleaned"], 12);
        assert_eq!(st["result"]["unchanged"], 2);
        let g = s
            .dispatch("video.captions.get", &json!({"path": v}))
            .unwrap();
        assert_eq!(g["lines"][0]["text"], "Line 0.");
        assert_eq!(g["lines"][0]["start_ms"], 0, "times are untouched");
        assert_eq!(g["lines"][13]["text"], "line 13");
        // a model that never answers well changes nothing and says so
        let mut s = session(FakeLlm::new(vec![]));
        s.dispatch("video.captions.polish", &json!({"path": v}))
            .unwrap();
        let st = wait(&mut s);
        assert!(st["error"].as_str().unwrap().contains("unchanged"), "{st}");
        let _ = std::fs::remove_dir_all(v.parent().unwrap());
    }

    #[test]
    fn captions_come_from_keyframes_for_a_silent_recording() {
        let v = clip("frames", false);
        let llm = FakeLlm::new(vec![
            Ok(r#"{"caption":"Open the menu."}"#.into()),
            Ok("nonsense".into()),
            Ok(r#"{"caption":"Press Save."}"#.into()),
        ]);
        let mut s = session(llm.clone());
        s.dispatch(
            "video.captions.polish",
            &json!({"path": v, "mode": "frames", "segments": 3}),
        )
        .unwrap();
        let st = wait(&mut s);
        assert!(st["error"].is_null(), "{st}");
        assert_eq!(st["result"]["lines"], 2);
        assert_eq!(llm.image_calls(), 3);
        let g = s
            .dispatch("video.captions.get", &json!({"path": v}))
            .unwrap();
        assert_eq!(g["lines"][0]["end_ms"], 20_000);
        assert_eq!(g["lines"][1]["text"], "Press Save.");
        // existing subtitles are not overwritten by accident
        assert!(
            s.dispatch(
                "video.captions.polish",
                &json!({"path": v, "mode": "frames"})
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(v.parent().unwrap());
    }

    #[test]
    fn missing_input_and_bad_params_are_errors() {
        let v = clip("bad", false);
        let mut s = session(FakeLlm::new(vec![]));
        for (id, p) in [
            ("video.chapters.suggest", json!({"path": v})),
            ("video.describe", json!({"path": v})),
            ("video.captions.polish", json!({"path": v})),
            ("video.captions.polish", json!({"path": v, "mode": "magic"})),
            ("video.chapters.suggest", json!({"path": "nope.mp4"})),
            ("video.describe", json!({"path": v, "bogus": 1})),
        ] {
            assert!(s.dispatch(id, &p).is_err(), "{id}");
        }
        let mut none = Session::new(crate::Services::default());
        let v2 = clip("bad2", true);
        assert!(
            none.dispatch("video.describe", &json!({"path": v2}))
                .is_err()
        );
        assert_eq!(youtube_chapters(&[]), "");
        assert!(
            youtube_chapters(&[Chapter {
                start_ms: 3_725_000,
                title: "X".into()
            }])
            .starts_with("1:02:05")
        );
        let _ = std::fs::remove_dir_all(v.parent().unwrap());
        let _ = std::fs::remove_dir_all(v2.parent().unwrap());
    }
}
