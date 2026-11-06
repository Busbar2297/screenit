//! Speech commands: transcribe a recording with a local speech server, edit and export the
//! subtitles, cut silences and filler words, reduce noise and narrate with a synthetic voice.
//! Renders reuse the auto-polish pipeline with every picture effect off, so the sound stays in
//! step. Long work runs as the shared background job (`tutor.status` / `tutor.wait`).

use crate::args::Args;
use crate::polish_cmds::{AudioEdits, Sound, out_path, render_job};
use crate::tutor_cmds::spawn_job;
use crate::{CmdResult, CommandError, CommandSpec, Session, always};
use screenit_geom::Rect;
use screenit_record::audio_fx::{filler_ranges, merge_ranges, silent_ranges};
use screenit_record::events::EventLog;
use screenit_record::polish::{PolishOpts, PolishPlan};
use screenit_record::speech::{Line, Transcript, srt, wav_16k_mono};
use screenit_tutor::TutorError;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const MAX_LINES: usize = 5000;
const MAX_LINE_CHARS: usize = 600;

/// How silence is found and trimmed.
#[derive(Clone, Debug)]
pub(crate) struct SilenceOpts {
    pub min_ms: u32,
    pub keep_ms: u32,
    pub threshold_db: f32,
}

impl Default for SilenceOpts {
    fn default() -> Self {
        Self {
            min_ms: 1000,
            keep_ms: 250,
            threshold_db: -45.0,
        }
    }
}

fn transcript_path(video: &Path) -> PathBuf {
    video.with_extension("transcript.json")
}

fn captions_path(video: &Path) -> PathBuf {
    video.with_extension("captions.json")
}

fn load_transcript(video: &Path) -> Option<Transcript> {
    serde_json::from_str(&std::fs::read_to_string(transcript_path(video)).ok()?).ok()
}

/// Edited subtitle lines, or the lines derived from the transcript.
pub(crate) fn load_captions(video: &Path) -> Result<Vec<Line>, CommandError> {
    if let Ok(text) = std::fs::read_to_string(captions_path(video)) {
        let v: Value = serde_json::from_str(&text).map_err(CommandError::failed)?;
        return serde_json::from_value(v.get("lines").cloned().unwrap_or(Value::Null))
            .map_err(CommandError::failed);
    }
    load_transcript(video)
        .map(|t| t.lines(42, 5000))
        .ok_or_else(|| {
            CommandError::Failed("no subtitles yet: run `video.transcribe` first".into())
        })
}

pub(crate) fn save_captions(video: &Path, lines: &[Line]) -> Result<(), CommandError> {
    let text = serde_json::to_string(&json!({ "lines": lines })).map_err(CommandError::failed)?;
    std::fs::write(captions_path(video), text).map_err(CommandError::failed)
}

pub(crate) fn video_path(a: &Args<'_>) -> Result<PathBuf, CommandError> {
    let p = PathBuf::from(a.str("path")?);
    if !p.is_file() {
        return Err(CommandError::Params(format!("no video at {}", p.display())));
    }
    Ok(p)
}

pub(crate) fn reader(
    s: &Session,
) -> Result<std::sync::Arc<dyn screenit_record::VideoReader>, CommandError> {
    s.services
        .video
        .clone()
        .ok_or_else(|| CommandError::Disabled("no video reader available".into()))
}

fn audio_of(s: &Session, path: &Path) -> Result<Vec<i16>, CommandError> {
    let r = reader(s)?;
    if !r.probe(path).map_err(CommandError::failed)?.has_audio {
        return Err(CommandError::Failed("this recording has no sound".into()));
    }
    r.audio_pcm(path).map_err(CommandError::failed)
}

/// Cut ranges and subtitle lines for a render, from the sound and the transcript.
pub(crate) fn audio_edits(
    s: &Session,
    path: &Path,
    silence: bool,
    fillers: bool,
    captions: bool,
    so: &SilenceOpts,
) -> Result<AudioEdits, CommandError> {
    let mut cuts = Vec::new();
    if silence {
        let pcm = audio_of(s, path)?;
        cuts.extend(silent_ranges(&pcm, so.min_ms, so.keep_ms, so.threshold_db));
    }
    if fillers {
        let t = load_transcript(path).ok_or_else(|| {
            CommandError::Failed("no transcript yet: run `video.transcribe` first".into())
        })?;
        cuts.extend(filler_ranges(&t, &[]));
    }
    Ok(AudioEdits {
        cuts: merge_ranges(cuts),
        captions: if captions {
            load_captions(path)?
        } else {
            Vec::new()
        },
    })
}

/// A plan with every picture effect off: the recording as it is, minus `cuts`.
fn plain_plan(s: &Session, path: &Path, cuts: Vec<(u64, u64)>) -> Result<PolishPlan, CommandError> {
    let info = reader(s)?.probe(path).map_err(CommandError::failed)?;
    let opts = PolishOpts {
        zoom: false,
        click_rings: false,
        typing_speedup: false,
        trim_idle: false,
        shortcut_badges: false,
        extra_cuts: cuts,
        ..PolishOpts::default()
    };
    opts.validate().map_err(CommandError::Params)?;
    let log = EventLog {
        duration_ms: info.duration_ms,
        ..EventLog::default()
    };
    Ok(PolishPlan::build(
        &log,
        info.duration_ms,
        Rect::new(0, 0, info.width, info.height),
        &opts,
    ))
}

fn fps_of(a: &Args<'_>) -> Result<u32, CommandError> {
    Ok(a.opt_u32("fps")?.unwrap_or(30).clamp(5, 60))
}

fn transcribe(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "language"])?;
    let path = video_path(&a)?;
    let sp = &s.settings.speech;
    if sp.stt_url.trim().is_empty() {
        return Err(CommandError::Failed(
            "no speech-to-text server: set one in Settings > Speech".into(),
        ));
    }
    let stt = s
        .services
        .stt
        .clone()
        .ok_or_else(|| CommandError::Disabled("no speech-to-text available".into()))?;
    let cfg = sp.stt_config();
    let language = a
        .opt_str("language")?
        .map(str::to_string)
        .filter(|l| !l.trim().is_empty())
        .or_else(|| Some(sp.stt_language.clone()).filter(|l| !l.trim().is_empty()));
    let pcm = audio_of(s, &path)?;
    spawn_job(s, "transcribing", move |sh| {
        sh.set_progress(0, 1);
        let wav = wav_16k_mono(&pcm);
        drop(pcm);
        let t = stt
            .transcribe(&wav, language.as_deref(), &cfg)
            .map_err(|e| TutorError::Record(e.to_string()))?;
        let text = serde_json::to_string(&t).map_err(|e| TutorError::Record(e.to_string()))?;
        std::fs::write(transcript_path(&path), text)
            .map_err(|e| TutorError::Record(e.to_string()))?;
        let lines = t.lines(42, 5000);
        let text = serde_json::to_string(&json!({ "lines": lines }))
            .map_err(|e| TutorError::Record(e.to_string()))?;
        std::fs::write(captions_path(&path), text)
            .map_err(|e| TutorError::Record(e.to_string()))?;
        sh.set_progress(1, 1);
        Ok(json!({
            "language": t.language, "text": t.text, "words": t.words.len(),
            "lines": lines.len(), "transcript": transcript_path(&path),
            "captions": captions_path(&path),
        }))
    })
}

fn captions_get(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "srt"])?;
    let _ = s;
    let path = video_path(&a)?;
    let lines = load_captions(&path)?;
    let mut v = json!({ "lines": lines, "count": lines.len() });
    if a.bool("srt", false)? {
        v["srt"] = json!(srt(&lines));
    }
    Ok(v)
}

fn parse_lines(v: &Value) -> Result<Vec<Line>, CommandError> {
    let lines: Vec<Line> = serde_json::from_value(v.clone()).map_err(|e| {
        CommandError::Params(format!("`lines` must be [{{start_ms,end_ms,text}}]: {e}"))
    })?;
    if lines.len() > MAX_LINES {
        return Err(CommandError::Params(format!(
            "at most {MAX_LINES} subtitle lines"
        )));
    }
    for l in &lines {
        if l.end_ms <= l.start_ms {
            return Err(CommandError::Params(
                "every line needs end_ms greater than start_ms".into(),
            ));
        }
        if l.text.chars().count() > MAX_LINE_CHARS {
            return Err(CommandError::Params(format!(
                "a line is longer than {MAX_LINE_CHARS} characters"
            )));
        }
    }
    Ok(lines)
}

fn captions_set(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "lines"])?;
    let _ = s;
    let path = video_path(&a)?;
    let mut lines = parse_lines(p.get("lines").unwrap_or(&Value::Null))?;
    for l in &mut lines {
        l.text = l.text.replace('\r', "");
    }
    lines.sort_by_key(|l| l.start_ms);
    save_captions(&path, &lines)?;
    Ok(json!({ "count": lines.len() }))
}

fn captions_export(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "output"])?;
    let _ = s;
    let path = video_path(&a)?;
    let out = match a.opt_str("output")? {
        Some(o) => PathBuf::from(o),
        None => path.with_extension("srt"),
    };
    if out
        .extension()
        .is_none_or(|e| !e.eq_ignore_ascii_case("srt"))
    {
        return Err(CommandError::Params("output must be an .srt path".into()));
    }
    let lines = load_captions(&path)?;
    // UTF-8 with a byte order mark: the widest-supported form for Arabic text.
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(srt(&lines).as_bytes());
    std::fs::write(&out, bytes).map_err(CommandError::failed)?;
    Ok(json!({ "path": out, "count": lines.len() }))
}

fn captions_burn(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "output", "fps"])?;
    let path = video_path(&a)?;
    let out = out_path(&a, &path)?;
    let lines = load_captions(&path)?;
    let mut plan = plain_plan(s, &path, Vec::new())?;
    plan.set_captions(&lines);
    render_job(
        s,
        path,
        out,
        plan,
        fps_of(&a)?,
        Sound::default(),
        "polished",
    )
}

fn silence_cut(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(
        p,
        &[
            "path",
            "output",
            "fps",
            "min_ms",
            "keep_ms",
            "threshold_db",
            "fillers",
            "captions",
            "denoise",
        ],
    )?;
    let path = video_path(&a)?;
    let out = out_path(&a, &path)?;
    let d = SilenceOpts::default();
    let so = SilenceOpts {
        min_ms: a.opt_u32("min_ms")?.unwrap_or(d.min_ms).clamp(200, 60_000),
        keep_ms: a.opt_u32("keep_ms")?.unwrap_or(d.keep_ms).min(2000),
        threshold_db: a.opt_f32("threshold_db")?.unwrap_or(d.threshold_db),
    };
    if !so.threshold_db.is_finite() || !(-80.0..=-10.0).contains(&so.threshold_db) {
        return Err(CommandError::Params(
            "threshold_db must be -80..=-10".into(),
        ));
    }
    let e = audio_edits(
        s,
        &path,
        true,
        a.bool("fillers", false)?,
        a.bool("captions", false)?,
        &so,
    )?;
    if e.cuts.is_empty() {
        return Err(CommandError::Failed(
            "nothing to cut: no silence longer than min_ms was found".into(),
        ));
    }
    let cut_ms: u64 = e.cuts.iter().map(|(x, y)| y - x).sum();
    let mut plan = plain_plan(s, &path, e.cuts)?;
    plan.set_captions(&e.captions);
    let sound = Sound {
        denoise: a.bool("denoise", false)?,
        ..Sound::default()
    };
    let mut v = render_job(s, path, out, plan, fps_of(&a)?, sound, "polished")?;
    v["cut_ms"] = json!(cut_ms);
    Ok(v)
}

fn denoise(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "output", "fps"])?;
    let path = video_path(&a)?;
    let out = out_path(&a, &path)?;
    if !reader(s)?
        .probe(&path)
        .map_err(CommandError::failed)?
        .has_audio
    {
        return Err(CommandError::Failed("this recording has no sound".into()));
    }
    let plan = plain_plan(s, &path, Vec::new())?;
    let sound = Sound {
        denoise: true,
        ..Sound::default()
    };
    render_job(s, path, out, plan, fps_of(&a)?, sound, "polished")
}

fn narrate(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(
        p,
        &["path", "output", "fps", "text", "lines", "from_captions"],
    )?;
    let path = video_path(&a)?;
    let out = out_path(&a, &path)?;
    let mut said: Vec<(u64, String)> = Vec::new();
    if let Some(t) = a.opt_str("text")? {
        said.push((0, t.to_string()));
    }
    if p.get("lines").is_some() {
        for l in a.array("lines")? {
            let at = l
                .get("at_ms")
                .and_then(Value::as_u64)
                .ok_or_else(|| CommandError::Params("each line needs `at_ms` and `text`".into()))?;
            let text = l
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| CommandError::Params("each line needs `at_ms` and `text`".into()))?;
            said.push((at, text.to_string()));
        }
    }
    if a.bool("from_captions", false)? {
        said.extend(
            load_captions(&path)?
                .into_iter()
                .map(|l| (l.start_ms, l.text)),
        );
    }
    said.retain(|(_, t)| !t.trim().is_empty());
    if said.is_empty() {
        return Err(CommandError::Params(
            "nothing to say: give `text`, `lines` or `from_captions`".into(),
        ));
    }
    if said.len() > 500 || said.iter().any(|(_, t)| t.chars().count() > 4000) {
        return Err(CommandError::Params(
            "at most 500 lines of up to 4000 characters".into(),
        ));
    }
    let sp = &s.settings.speech;
    let tts = if sp.tts_url.trim().is_empty() {
        s.services.tts_fallback.clone()
    } else {
        s.services.tts.clone()
    }
    .ok_or_else(|| CommandError::Disabled("no text-to-speech available".into()))?;
    let cfg = sp.tts_config();
    let plan = plain_plan(s, &path, Vec::new())?;
    let sound = Sound {
        narration: said,
        tts: Some((tts, cfg)),
        ..Sound::default()
    };
    render_job(s, path, out, plan, fps_of(&a)?, sound, "polished")
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
            "video.transcribe",
            "Transcribe Recording",
            r#"{"path":str,"language"?:str (e.g. en, ar; default from Settings)} -> background job (tutor.wait); writes clip.transcript.json and clip.captions.json"#,
            transcribe,
        ),
        c(
            "video.captions.get",
            "Get Subtitles",
            r#"{"path":str,"srt"?:bool}"#,
            captions_get,
        ),
        c(
            "video.captions.set",
            "Set Subtitles",
            r#"{"path":str,"lines":[{"start_ms":int,"end_ms":int,"text":str}]}"#,
            captions_set,
        ),
        c(
            "video.captions.export",
            "Export Subtitles (SRT)",
            r#"{"path":str,"output"?:str.srt}"#,
            captions_export,
        ),
        c(
            "video.captions.burn",
            "Burn In Subtitles",
            r#"{"path":str,"output":str.mp4,"fps"?:5..60} -> background job"#,
            captions_burn,
        ),
        c(
            "video.silence.cut",
            "Cut Silences",
            r#"{"path":str,"output":str.mp4,"min_ms"?:200..60000,"keep_ms"?:0..2000,"threshold_db"?:-80..-10,"fillers"?:bool (needs a transcript),"captions"?:bool,"denoise"?:bool,"fps"?:5..60} -> background job"#,
            silence_cut,
        ),
        c(
            "video.audio.denoise",
            "Reduce Noise",
            r#"{"path":str,"output":str.mp4,"fps"?:5..60} -> background job"#,
            denoise,
        ),
        c(
            "video.narrate",
            "Add Narration",
            r#"{"path":str,"output":str.mp4,"text"?:str,"lines"?:[{"at_ms":int,"text":str}],"from_captions"?:bool,"fps"?:5..60} -> background job"#,
            narrate,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenit_record::speech::Word;
    use screenit_record::{RecordError, RecordingResult, VideoInfo, VideoReader, VideoWriter};
    use std::sync::{Arc, Mutex};

    struct Reader;
    impl VideoReader for Reader {
        fn probe(&self, _: &Path) -> Result<VideoInfo, RecordError> {
            Ok(VideoInfo {
                duration_ms: 6000,
                width: 160,
                height: 90,
                has_audio: true,
            })
        }
        fn frame_at(&self, _: &Path, _: u64) -> Result<screenit_raster::Raster, RecordError> {
            screenit_raster::Raster::new(160, 90, [80, 90, 100, 255])
                .map_err(|e| RecordError::Platform(e.to_string()))
        }
        /// 1 s of tone, 3 s of silence, 2 s of tone.
        fn audio_pcm(&self, _: &Path) -> Result<Vec<i16>, RecordError> {
            let tone = |ms: usize| -> Vec<i16> {
                (0..ms * 96)
                    .map(|i| if (i / 2 / 12) % 2 == 0 { 8000 } else { -8000 })
                    .collect()
            };
            let mut v = tone(1000);
            v.extend(vec![0; 3000 * 96]);
            v.extend(tone(2000));
            Ok(v)
        }
    }

    struct Sink(Arc<Mutex<Vec<i16>>>);
    impl VideoWriter for Sink {
        fn write(
            &self,
            out: &Path,
            w: u32,
            h: u32,
            fps: u32,
            frames: &mut dyn Iterator<Item = Result<screenit_raster::Raster, RecordError>>,
        ) -> Result<RecordingResult, RecordError> {
            self.write_with_audio(out, w, h, fps, frames, &[])
        }
        fn write_with_audio(
            &self,
            out: &Path,
            w: u32,
            h: u32,
            _fps: u32,
            frames: &mut dyn Iterator<Item = Result<screenit_raster::Raster, RecordError>>,
            audio: &[i16],
        ) -> Result<RecordingResult, RecordError> {
            let mut n = 0;
            for f in frames {
                f?;
                n += 1;
            }
            *self
                .0
                .lock()
                .map_err(|e| RecordError::Platform(e.to_string()))? = audio.to_vec();
            Ok(RecordingResult {
                path: out.to_path_buf(),
                duration_ms: 0,
                frames: n,
                width: w,
                height: h,
                has_audio: !audio.is_empty(),
            })
        }
    }

    fn session() -> (Session, Arc<Mutex<Vec<i16>>>) {
        let sink = Arc::new(Mutex::new(Vec::new()));
        let mut s = Session::new(crate::Services {
            video: Some(Arc::new(Reader)),
            video_writer: Some(Arc::new(Sink(sink.clone()))),
            stt: Some(Arc::new(screenit_testkit::FakeStt(Some(Transcript {
                language: "en".into(),
                text: "Hello um world.".into(),
                words: vec![
                    Word {
                        start_ms: 100,
                        end_ms: 400,
                        text: "Hello".into(),
                    },
                    Word {
                        start_ms: 450,
                        end_ms: 600,
                        text: "um".into(),
                    },
                    Word {
                        start_ms: 650,
                        end_ms: 900,
                        text: "world.".into(),
                    },
                ],
            })))),
            tts: Some(Arc::new(screenit_testkit::FakeTts {
                frames_per_char: 480,
            })),
            tts_fallback: Some(Arc::new(screenit_testkit::FakeTts {
                frames_per_char: 240,
            })),
            ..crate::Services::default()
        });
        s.settings.speech.stt_url = "http://localhost:1/v1".into();
        (s, sink)
    }

    fn clip(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("scit-speech-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let v = d.join("clip.mp4");
        std::fs::write(&v, b"x").unwrap();
        v
    }

    fn wait(s: &mut Session) -> Value {
        let st = s
            .dispatch("tutor.wait", &json!({"timeout_ms": 30000}))
            .unwrap();
        assert_eq!(st["running"], false, "{st}");
        assert!(st["error"].is_null(), "{st}");
        st["result"].clone()
    }

    #[test]
    fn transcribe_then_edit_export_and_burn_subtitles() {
        let v = clip("cap");
        let (mut s, _) = session();
        s.dispatch("video.transcribe", &json!({"path": v, "language": "en"}))
            .unwrap();
        let r = wait(&mut s);
        assert_eq!(r["words"], 3, "{r}");
        let g = s
            .dispatch("video.captions.get", &json!({"path": v, "srt": true}))
            .unwrap();
        assert_eq!(g["count"], 1, "{g}");
        assert!(g["srt"].as_str().unwrap().contains("Hello um world."));
        s.dispatch(
            "video.captions.set",
            &json!({"path": v, "lines": [
                {"start_ms": 2000, "end_ms": 3000, "text": "second"},
                {"start_ms": 100, "end_ms": 900, "text": "first"}
            ]}),
        )
        .unwrap();
        let g = s
            .dispatch("video.captions.get", &json!({"path": v}))
            .unwrap();
        assert_eq!(g["lines"][0]["text"], "first");
        let srt_path = v.with_file_name("c.srt");
        s.dispatch(
            "video.captions.export",
            &json!({"path": v, "output": srt_path}),
        )
        .unwrap();
        let bytes = std::fs::read(&srt_path).unwrap();
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
        assert!(String::from_utf8_lossy(&bytes).contains("second"));
        s.dispatch(
            "video.captions.burn",
            &json!({"path": v, "output": v.with_file_name("b.mp4"), "fps": 5}),
        )
        .unwrap();
        wait(&mut s);
        let _ = std::fs::remove_dir_all(v.parent().unwrap());
    }

    #[test]
    fn silence_and_fillers_are_cut_from_picture_and_sound() {
        let v = clip("cut");
        let (mut s, sink) = session();
        let out = v.with_file_name("cut.mp4");
        s.dispatch(
            "video.silence.cut",
            &json!({"path": v, "output": out, "fps": 5}),
        )
        .unwrap();
        let r = wait(&mut s);
        let cut = r["cut_ms"].as_u64();
        let _ = cut;
        let audio = sink.lock().unwrap().len() as u64;
        // 3 s of silence minus the 250 ms kept each side
        assert!(audio / 96 < 6000 - 2000, "{} ms left", audio / 96);
        assert!(audio / 96 > 2500);
        // fillers need a transcript first
        assert!(
            s.dispatch(
                "video.silence.cut",
                &json!({"path": v, "output": out, "fillers": true})
            )
            .is_err()
        );
        s.dispatch("video.transcribe", &json!({"path": v})).unwrap();
        wait(&mut s);
        s.dispatch(
            "video.silence.cut",
            &json!({"path": v, "output": out, "fillers": true, "captions": true, "denoise": true, "fps": 5}),
        )
        .unwrap();
        wait(&mut s);
        let with_filler = sink.lock().unwrap().len() as u64;
        assert!(with_filler < audio, "{with_filler} vs {audio}");
        let _ = std::fs::remove_dir_all(v.parent().unwrap());
    }

    #[test]
    fn denoise_and_narration_render_with_sound() {
        let v = clip("nar");
        let (mut s, sink) = session();
        let out = v.with_file_name("n.mp4");
        s.dispatch(
            "video.audio.denoise",
            &json!({"path": v, "output": out, "fps": 5}),
        )
        .unwrap();
        wait(&mut s);
        assert_eq!(sink.lock().unwrap().len(), 6000 * 96);
        // the narration is mixed over the silent middle
        s.dispatch(
            "video.narrate",
            &json!({"path": v, "output": out, "fps": 5,
                    "lines": [{"at_ms": 2000, "text": "hello there"}]}),
        )
        .unwrap();
        wait(&mut s);
        {
            let a = sink.lock().unwrap();
            assert_eq!(a.len(), 6000 * 96);
            assert!(a[2000 * 96..2000 * 96 + 960].iter().any(|v| *v != 0));
            assert!(a[3900 * 96..3900 * 96 + 960].iter().all(|v| *v == 0));
        }
        // the OS voice is used when no TTS server is configured
        assert!(s.settings.speech.tts_url.is_empty());
        let _ = std::fs::remove_dir_all(v.parent().unwrap());
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        let v = clip("bad");
        let (mut s, _) = session();
        let out = v.with_file_name("o.mp4");
        let no_stt = &mut Session::new(crate::Services::default());
        assert!(
            no_stt
                .dispatch("video.transcribe", &json!({"path": v}))
                .is_err()
        );
        s.settings.speech.stt_url.clear();
        assert!(s.dispatch("video.transcribe", &json!({"path": v})).is_err());
        for (id, p) in [
            ("video.captions.get", json!({"path": v})),
            ("video.captions.set", json!({"path": v, "lines": "x"})),
            (
                "video.captions.set",
                json!({"path": v, "lines": [{"start_ms": 5, "end_ms": 5, "text": "a"}]}),
            ),
            (
                "video.captions.export",
                json!({"path": v, "output": "a.txt"}),
            ),
            ("video.captions.burn", json!({"path": v, "output": out})),
            ("video.narrate", json!({"path": v, "output": out})),
            ("video.narrate", json!({"path": v, "output": v})),
            (
                "video.narrate",
                json!({"path": v, "output": out, "lines": [{"text": "x"}]}),
            ),
            (
                "video.silence.cut",
                json!({"path": v, "output": out, "threshold_db": 5}),
            ),
            (
                "video.silence.cut",
                json!({"path": v, "output": out, "min_ms": 1, "bogus": 1}),
            ),
            (
                "video.audio.denoise",
                json!({"path": "nope.mp4", "output": out}),
            ),
        ] {
            assert!(s.dispatch(id, &p).is_err(), "{id} {p}");
        }
        let _ = std::fs::remove_dir_all(v.parent().unwrap());
    }
}
