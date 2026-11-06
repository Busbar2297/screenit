//! Auto-polish commands: read the event log of a recording, plan the polish, render it. The
//! render is a background job reported by `tutor.status` / `tutor.wait` / `tutor.abort` (the
//! same job slot the tutorial pipeline uses, so only one long job runs at a time).

use crate::args::Args;
use crate::polish_render::{PolishJob, render_polish};
use crate::tutor_cmds::{Capture, queue_capture, spawn_job};
use crate::{CmdResult, CommandError, CommandSpec, Session, always};
use screenit_geom::Rect;
use screenit_record::events::{EventKind, EventLog};
use screenit_record::polish::{Aspect, CursorStyle, PolishOpts, PolishPlan};
use screenit_tutor::TutorError;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const OPT_KEYS: [&str; 12] = [
    "zoom",
    "zoom_factor",
    "click_rings",
    "cursor",
    "typing_speedup",
    "speedup_factor",
    "trim_idle",
    "idle_min_ms",
    "shortcut_badges",
    "aspect",
    "padding_pct",
    "corner_pct",
];

fn keys_with(extra: &[&'static str]) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = extra.to_vec();
    v.extend(OPT_KEYS);
    v
}

fn opts_from(a: &Args<'_>) -> Result<PolishOpts, CommandError> {
    let d = PolishOpts::default();
    let pct = |k: &str, dv: u8| -> Result<u8, CommandError> {
        match a.opt_u32(k)? {
            None => Ok(dv),
            Some(v) => {
                u8::try_from(v).map_err(|_| CommandError::Params(format!("`{k}` is out of range")))
            }
        }
    };
    let aspect = match a.opt_str("aspect")? {
        None => d.aspect,
        Some(n) => Aspect::parse(n).ok_or_else(|| {
            CommandError::Params(format!(
                "unknown aspect `{n}` (native|square|vertical|wide)"
            ))
        })?,
    };
    let cursor = match a.opt_str("cursor")? {
        None => d.cursor,
        Some(n) => CursorStyle::parse(n).ok_or_else(|| {
            CommandError::Params(format!("unknown cursor `{n}` (native|disc|ring)"))
        })?,
    };
    let o = PolishOpts {
        zoom: a.bool("zoom", d.zoom)?,
        zoom_factor: a.opt_f32("zoom_factor")?.unwrap_or(d.zoom_factor),
        click_rings: a.bool("click_rings", d.click_rings)?,
        cursor,
        typing_speedup: a.bool("typing_speedup", d.typing_speedup)?,
        speedup_factor: a.opt_f32("speedup_factor")?.unwrap_or(d.speedup_factor),
        trim_idle: a.bool("trim_idle", d.trim_idle)?,
        idle_min_ms: a
            .opt_u32("idle_min_ms")?
            .unwrap_or(d.idle_min_ms)
            .clamp(500, 60_000),
        keep_ms: d.keep_ms,
        shortcut_badges: a.bool("shortcut_badges", d.shortcut_badges)?,
        aspect,
        padding_pct: pct("padding_pct", d.padding_pct)?,
        corner_pct: pct("corner_pct", d.corner_pct)?,
        extra_cuts: Vec::new(),
    };
    o.validate().map_err(CommandError::Params)?;
    Ok(o)
}

fn video_path(a: &Args<'_>) -> Result<PathBuf, CommandError> {
    let p = PathBuf::from(a.str("path")?);
    if !p.is_file() {
        return Err(CommandError::Params(format!("no video at {}", p.display())));
    }
    Ok(p)
}

fn load_log(path: &Path) -> Result<EventLog, CommandError> {
    EventLog::load(path).map_err(|_| {
        CommandError::Failed(
            "this recording has no event log; record with `event_log` on to polish it".into(),
        )
    })
}

/// Build the plan for a recording on disk.
fn plan_for(
    s: &Session,
    path: &Path,
    opts: &PolishOpts,
) -> Result<(PolishPlan, EventLog), CommandError> {
    let log = load_log(path)?;
    let reader = s
        .services
        .video
        .clone()
        .ok_or_else(|| CommandError::Disabled("no video reader available".into()))?;
    let info = reader.probe(path).map_err(CommandError::failed)?;
    let video = if log.region.is_empty() {
        Rect::new(0, 0, info.width, info.height)
    } else {
        Rect::new(0, 0, log.region.w, log.region.h)
    };
    Ok((PolishPlan::build(&log, info.duration_ms, video, opts), log))
}

fn events_get(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "limit"])?;
    let _ = s;
    let log = load_log(&video_path(&a)?)?;
    let n = |k: EventKind| log.events.iter().filter(|e| e.kind == k).count();
    let limit = a.opt_u32("limit")?.unwrap_or(0).min(2000) as usize;
    Ok(json!({
        "region": log.region, "duration_ms": log.duration_ms, "count": log.events.len(),
        "clicks": n(EventKind::Down), "keys": n(EventKind::Key), "typing": n(EventKind::Type),
        "events": log.events.iter().take(limit).collect::<Vec<_>>(),
    }))
}

fn plan_value(plan: &PolishPlan) -> Value {
    json!({
        "total_ms": plan.total_ms, "video": plan.video,
        "segments": plan.segs, "shots": plan.shots, "clicks": plan.clicks,
        "badges": plan.badges, "options": plan.opts,
    })
}

fn polish_plan(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &keys_with(&["path"]))?;
    let path = video_path(&a)?;
    let opts = opts_from(&a)?;
    let (plan, _) = plan_for(s, &path, &opts)?;
    Ok(plan_value(&plan))
}

fn polish_opts(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let _ = s;
    Ok(json!({
        "defaults": PolishOpts::default(),
        "aspect": ["native", "square", "vertical", "wide"],
        "cursor": ["native", "disc", "ring"],
    }))
}

/// The `output` parameter: an `.mp4` path that is not the source recording.
pub(crate) fn out_path(a: &Args<'_>, src: &Path) -> Result<PathBuf, CommandError> {
    let out = PathBuf::from(a.str("output")?);
    if out
        .extension()
        .is_none_or(|e| !e.eq_ignore_ascii_case("mp4"))
    {
        return Err(CommandError::Params("output must be an .mp4 path".into()));
    }
    if out == src {
        return Err(CommandError::Params(
            "output must be a different file than the recording".into(),
        ));
    }
    Ok(out)
}

/// Sound edits applied while rendering.
#[derive(Default)]
pub(crate) struct Sound {
    pub denoise: bool,
    /// `(output start ms, text)` spoken with `tts`.
    pub narration: Vec<(u64, String)>,
    pub tts: Option<(
        std::sync::Arc<dyn screenit_record::speech::TextToSpeech>,
        screenit_record::speech::SpeechConfig,
    )>,
}

/// Render `plan` over the recording at `path` into `out` as a background job.
pub(crate) fn render_job(
    s: &mut Session,
    path: PathBuf,
    out: PathBuf,
    plan: PolishPlan,
    fps: u32,
    sound: Sound,
    tag: &'static str,
) -> CmdResult {
    if plan.total_ms == 0 {
        return Err(CommandError::Failed("the recording is empty".into()));
    }
    let reader = s
        .services
        .video
        .clone()
        .ok_or_else(|| CommandError::Disabled("no video reader available".into()))?;
    let writer = s
        .services
        .video_writer
        .clone()
        .ok_or_else(|| CommandError::Disabled("no video writer available".into()))?;
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(CommandError::failed)?;
    }
    let title = out
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "polished".into());
    spawn_job(s, "polishing", move |sh| {
        let mut clips: Vec<(u64, Vec<i16>)> = Vec::new();
        if let Some((tts, cfg)) = &sound.tts {
            for (at, text) in &sound.narration {
                if sh.abort.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(TutorError::Record("aborted".into()));
                }
                clips.push((
                    *at,
                    tts.speak(text, cfg)
                        .map_err(|e| TutorError::Record(e.to_string()))?,
                ));
            }
        }
        let res = render_polish(&PolishJob {
            reader: &*reader,
            writer: &*writer,
            video: &path,
            plan: &plan,
            fps,
            out: &out,
            abort: &sh.abort,
            denoise: sound.denoise,
            narration: &clips,
            on_frame: &|done, total| sh.set_progress(done, total),
        })
        .map_err(TutorError::Record)?;
        queue_capture(
            sh,
            Capture {
                path: out.clone(),
                title,
                app: String::new(),
                width: res.width,
                height: res.height,
                tag,
            },
        );
        Ok(json!({
            "video": out, "duration_ms": plan.total_ms, "frames": res.frames,
            "width": res.width, "height": res.height, "audio": res.has_audio,
        }))
    })
}

/// Cuts and subtitles to apply to a polish, from the audio and the transcript sidecar.
pub(crate) struct AudioEdits {
    pub cuts: Vec<(u64, u64)>,
    pub captions: Vec<screenit_record::speech::Line>,
}

fn polish(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(
        p,
        &keys_with(&[
            "path", "output", "fps", "silence", "fillers", "captions", "denoise",
        ]),
    )?;
    let path = video_path(&a)?;
    let mut opts = opts_from(&a)?;
    let out = out_path(&a, &path)?;
    let fps = a.opt_u32("fps")?.unwrap_or(30).clamp(5, 60);
    let edits = crate::speech_cmds::audio_edits(
        s,
        &path,
        a.bool("silence", false)?,
        a.bool("fillers", false)?,
        a.bool("captions", false)?,
        &crate::speech_cmds::SilenceOpts::default(),
    )?;
    opts.extra_cuts = edits.cuts;
    let (mut plan, _) = plan_for(s, &path, &opts)?;
    plan.set_captions(&edits.captions);
    let sound = Sound {
        denoise: a.bool("denoise", false)?,
        ..Sound::default()
    };
    render_job(s, path, out, plan, fps, sound, "polished")
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
    macro_rules! opts {
        () => {
            r#""zoom"?:bool,"zoom_factor"?:1..8,"click_rings"?:bool,"cursor"?:native|disc|ring,"typing_speedup"?:bool,"speedup_factor"?:1..16,"trim_idle"?:bool,"idle_min_ms"?:500..60000,"shortcut_badges"?:bool,"aspect"?:native|square|vertical|wide,"padding_pct"?:0..25,"corner_pct"?:0..25"#
        };
    }
    vec![
        c(
            "video.events.get",
            "Recording Events",
            r#"{"path":str,"limit"?:int (events returned, default 0)}"#,
            events_get,
        ),
        c("video.polish.opts", "Polish Options", "{}", polish_opts),
        c(
            "video.polish.plan",
            "Plan Auto-polish",
            concat!(r#"{"path":str,"#, opts!(), "}"),
            polish_plan,
        ),
        c(
            "video.polish",
            "Auto-polish Video",
            concat!(
                r#"{"path":str,"output":str.mp4,"fps"?:5..60,"silence"?:bool,"fillers"?:bool,"captions"?:bool,"denoise"?:bool,"#,
                opts!(),
                "}"
            ),
            polish,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Reader;
    impl screenit_record::VideoReader for Reader {
        fn probe(
            &self,
            _: &Path,
        ) -> Result<screenit_record::VideoInfo, screenit_record::RecordError> {
            Ok(screenit_record::VideoInfo {
                duration_ms: 6000,
                width: 320,
                height: 180,
                has_audio: false,
            })
        }
        fn frame_at(
            &self,
            _: &Path,
            _: u64,
        ) -> Result<screenit_raster::Raster, screenit_record::RecordError> {
            screenit_raster::Raster::new(320, 180, [90, 120, 160, 255])
                .map_err(|e| screenit_record::RecordError::Platform(e.to_string()))
        }
    }

    fn session() -> Session {
        Session::new(crate::Services {
            video: Some(std::sync::Arc::new(Reader)),
            video_writer: Some(std::sync::Arc::new(screenit_testkit::FakeVideoWriter)),
            ..crate::Services::default()
        })
    }

    fn clip(tag: &str, with_log: bool) -> PathBuf {
        let d = std::env::temp_dir().join(format!("scit-polish-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let v = d.join("clip.mp4");
        std::fs::write(&v, b"x").unwrap();
        if with_log {
            let ev = |t, kind, x, y| screenit_record::events::InputEvent {
                t_ms: t,
                kind,
                x,
                y,
                button: 0,
                chord: String::new(),
                rect: Some(Rect::new(150, 80, 40, 20)),
            };
            EventLog {
                region: Rect::new(0, 0, 320, 180),
                duration_ms: 6000,
                events: vec![
                    ev(0, EventKind::Move, 5, 5),
                    ev(1000, EventKind::Down, 170, 90),
                    ev(5500, EventKind::Move, 9, 9),
                ],
            }
            .save(&v)
            .unwrap();
        }
        v
    }

    #[test]
    fn events_plan_and_render_through_the_commands() {
        let v = clip("ok", true);
        let mut s = session();
        let ev = s
            .dispatch("video.events.get", &json!({"path": v, "limit": 1}))
            .unwrap();
        assert_eq!(ev["clicks"], 1);
        assert_eq!(ev["events"].as_array().map(Vec::len), Some(1));
        let plan = s
            .dispatch("video.polish.plan", &json!({"path": v, "aspect": "square"}))
            .unwrap();
        assert!(
            plan["total_ms"].as_u64().unwrap() < 6000,
            "idle is cut: {plan}"
        );
        assert_eq!(plan["shots"].as_array().map(Vec::len), Some(1));
        let out = v.with_file_name("polished.mp4");
        s.dispatch(
            "video.polish",
            &json!({"path": v, "output": out, "fps": 10}),
        )
        .unwrap();
        let st = s
            .dispatch("tutor.wait", &json!({"timeout_ms": 20000}))
            .unwrap();
        assert_eq!(st["running"], false, "{st}");
        assert!(st["error"].is_null(), "{st}");
        assert!(st["result"]["frames"].as_u64().unwrap() > 0, "{st}");
        assert!(s.dispatch("video.polish.opts", &json!({})).is_ok());
        let _ = std::fs::remove_dir_all(v.parent().unwrap());
    }

    #[test]
    fn missing_log_and_bad_params_are_errors() {
        let v = clip("nolog", false);
        let mut s = session();
        assert!(s.dispatch("video.events.get", &json!({"path": v})).is_err());
        assert!(
            s.dispatch("video.polish.plan", &json!({"path": v}))
                .is_err()
        );
        let v2 = clip("bad", true);
        let out = v2.with_file_name("o.mp4");
        assert!(
            s.dispatch("video.polish", &json!({"path": v2, "output": v2}))
                .is_err()
        );
        assert!(
            s.dispatch(
                "video.polish",
                &json!({"path": v2, "output": out.with_extension("avi")})
            )
            .is_err()
        );
        assert!(s.dispatch("video.polish", &json!({"path": v2})).is_err());
        assert!(
            s.dispatch("video.polish.plan", &json!({"path": v2, "bogus": 1}))
                .is_err()
        );
        assert!(
            s.dispatch("video.polish.plan", &json!({"path": "nope.mp4"}))
                .is_err()
        );
        assert!(
            Session::new(crate::Services::default())
                .dispatch("video.polish.plan", &json!({"path": v2}))
                .is_err()
        );
        let _ = std::fs::remove_dir_all(v.parent().unwrap());
        let _ = std::fs::remove_dir_all(v2.parent().unwrap());
    }

    #[test]
    fn options_are_validated() {
        let bad = |v: Value| {
            let a = Args::new(&v, &keys_with(&[])).unwrap();
            opts_from(&a).is_err()
        };
        assert!(bad(json!({"zoom_factor": 0.2})));
        assert!(bad(json!({"aspect": "tall"})));
        assert!(bad(json!({"cursor": "laser"})));
        assert!(bad(json!({"padding_pct": 90})));
        assert!(bad(json!({"speedup_factor": 99})));
        let v = json!({"aspect": "vertical", "cursor": "ring"});
        let a = Args::new(&v, &keys_with(&[])).unwrap();
        let o = opts_from(&a).unwrap();
        assert_eq!(o.aspect, Aspect::Vertical);
        assert_eq!(o.cursor, CursorStyle::Ring);
    }
}
