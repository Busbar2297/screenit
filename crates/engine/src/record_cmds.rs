//! Recording commands. The state machine (`record::Session`) lives here; the actual encoding is
//! delegated to the injected `Recorder`.

use crate::args::Args;
use crate::{ActiveRecording, CmdResult, CommandError, CommandSpec, Session, always};
use screenit_geom::Rect;
use screenit_record::{
    Codec, Corner, OutputSize, PipLayout, PipShape, RecState, RecordConfig, Session as RecSession,
};
use serde_json::{Value, json};

fn start(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(
        p,
        &[
            "x",
            "y",
            "w",
            "h",
            "fps",
            "codec",
            "size",
            "mic",
            "system_audio",
            "cursor_highlight",
            "click_ripples",
            "event_log",
            "output",
            "countdown_ms",
            "webcam",
            "pip_shape",
            "pip_size",
            "pip_corner",
            "pip_margin",
        ],
    )?;
    if s.recording.is_some() {
        return Err(CommandError::Failed(
            "a recording is already in progress".into(),
        ));
    }
    let factory = s
        .services
        .recorder
        .as_ref()
        .ok_or_else(|| CommandError::Disabled("no recorder available".into()))?;
    let codec = match a.opt_str("codec")? {
        None | Some("h264") => Codec::H264,
        Some("h265") => Codec::H265,
        Some(o) => {
            return Err(CommandError::Params(format!(
                "unknown codec `{o}` (h264|h265)"
            )));
        }
    };
    let size_name = a
        .opt_str("size")?
        .unwrap_or(s.settings.recording.output.as_str());
    let size = OutputSize::parse(size_name).ok_or_else(|| {
        CommandError::Params(format!(
            "unknown size `{size_name}` (native|1080p|1440p|2160p)"
        ))
    })?;
    let cfg = RecordConfig {
        region: Rect::new(a.i32("x")?, a.i32("y")?, a.u32("w")?, a.u32("h")?),
        window: None,
        fps: a.opt_u32("fps")?.unwrap_or(30),
        codec,
        size,
        mic: a.opt_str("mic")?.map(str::to_string),
        system_audio: a.bool("system_audio", false)?,
        cursor_highlight: a.bool("cursor_highlight", false)?,
        click_ripples: a.bool("click_ripples", false)?,
        event_log: a.bool("event_log", true)?,
        output: a.str("output")?.into(),
    };
    cfg.validate().map_err(CommandError::failed)?;
    let mut recorder = factory();
    let mut session = RecSession::new();
    let now = s.services.clock.now_ms();
    let countdown = a.opt_u32("countdown_ms")?.unwrap_or(0).min(10_000);
    session
        .start(now, countdown)
        .map_err(CommandError::failed)?;
    if countdown > 0 {
        s.services.clock.sleep_ms(countdown);
        session
            .tick_countdown(s.services.clock.now_ms(), countdown)
            .map_err(CommandError::failed)?;
    }
    let mut pip = None;
    if a.bool("webcam", false)? {
        let cam = s
            .services
            .camera
            .clone()
            .ok_or_else(|| CommandError::Disabled("no webcam available".into()))?;
        let layout = parse_pip(&a, PipLayout::default())?;
        recorder
            .attach_webcam(cam, layout)
            .map_err(CommandError::failed)?;
        pip = Some(layout);
    }
    recorder.start(&cfg).map_err(CommandError::failed)?;
    s.recording = Some(ActiveRecording {
        recorder,
        session,
        config: cfg,
        pip,
    });
    Ok(json!({"state": "recording"}))
}

fn parse_pip(a: &Args, mut l: PipLayout) -> Result<PipLayout, CommandError> {
    if let Some(v) = a.opt_str("pip_shape")? {
        l.shape = match v {
            "rectangle" => PipShape::Rectangle,
            "rounded" => PipShape::RoundedRectangle,
            "circle" => PipShape::Circle,
            o => return Err(CommandError::Params(format!("unknown pip_shape `{o}`"))),
        };
    }
    if let Some(v) = a.opt_str("pip_corner")? {
        l.corner = match v {
            "top_left" => Corner::TopLeft,
            "top_right" => Corner::TopRight,
            "bottom_left" => Corner::BottomLeft,
            "bottom_right" => Corner::BottomRight,
            o => return Err(CommandError::Params(format!("unknown pip_corner `{o}`"))),
        };
    }
    if let Some(v) = a.opt_u32("pip_size")? {
        if !(5..=100).contains(&v) {
            return Err(CommandError::Params("pip_size must be 5..=100".into()));
        }
        l.size_percent = v;
    }
    if let Some(v) = a.opt_u32("pip_margin")? {
        l.margin = v;
    }
    Ok(l)
}

fn pip(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["pip_shape", "pip_size", "pip_corner", "pip_margin"])?;
    let r = active(s)?;
    let cur = r
        .pip
        .ok_or_else(|| CommandError::Failed("no webcam in this recording".into()))?;
    let l = parse_pip(&a, cur)?;
    r.recorder.set_pip_layout(l).map_err(CommandError::failed)?;
    r.pip = Some(l);
    Ok(json!(l))
}

fn swap(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let r = active(s)?;
    let mut l = r
        .pip
        .ok_or_else(|| CommandError::Failed("no webcam in this recording".into()))?;
    l.swapped = !l.swapped;
    r.recorder.set_pip_layout(l).map_err(CommandError::failed)?;
    r.pip = Some(l);
    Ok(json!({"swapped": l.swapped}))
}

fn active(s: &mut Session) -> Result<&mut ActiveRecording, CommandError> {
    s.recording
        .as_mut()
        .ok_or_else(|| CommandError::Failed("no recording in progress".into()))
}

fn move_region(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["x", "y"])?;
    let (x, y) = (a.i32("x")?, a.i32("y")?);
    if x.abs() > 100_000 || y.abs() > 100_000 {
        return Err(CommandError::Params(
            "x and y must be within ±100000".into(),
        ));
    }
    let r = active(s)?;
    r.config.region.x = x;
    r.config.region.y = y;
    r.recorder.move_region(x, y).map_err(CommandError::failed)?;
    Ok(json!({"x": x, "y": y}))
}

fn pause(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let now = s.services.clock.now_ms();
    let r = active(s)?;
    r.session.pause(now).map_err(CommandError::failed)?;
    r.recorder.pause().map_err(CommandError::failed)?;
    Ok(Value::Null)
}

fn resume(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let now = s.services.clock.now_ms();
    let r = active(s)?;
    r.session.resume(now).map_err(CommandError::failed)?;
    r.recorder.resume().map_err(CommandError::failed)?;
    Ok(Value::Null)
}

fn stop(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let now = s.services.clock.now_ms();
    let mut r = s
        .recording
        .take()
        .ok_or_else(|| CommandError::Failed("no recording in progress".into()))?;
    r.session.stop(now).map_err(CommandError::failed)?;
    let res = r.recorder.stop();
    if r.pip.is_some() {
        if let Some(cam) = &s.services.camera {
            cam.stop();
        }
    }
    let res = res.map_err(CommandError::failed)?;
    let secs = s.services.clock.unix_secs();
    let title = res
        .path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    s.library
        .add(screenit_library::Entry {
            id: 0,
            kind: screenit_library::Kind::Video,
            path: res.path.clone(),
            title,
            app: String::new(),
            url: None,
            created: secs,
            width: res.width,
            height: res.height,
            tags: vec![],
            body: String::new(),
        })
        .map_err(CommandError::failed)?;
    let out = json!({"result": res});
    s.last_recording = Some(res);
    Ok(out)
}

fn status(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let now = s.services.clock.now_ms();
    Ok(match &s.recording {
        None => json!({"state": "idle"}),
        Some(r) => {
            let st = match r.session.state() {
                RecState::Idle => "idle",
                RecState::Countdown { .. } => "countdown",
                RecState::Recording => "recording",
                RecState::Paused => "paused",
                RecState::Stopped => "stopped",
            };
            json!({"state": st, "elapsed_ms": r.session.elapsed_ms(now), "output": r.config.output, "pip": r.pip})
        }
    })
}

fn devices(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let (mics, cams) = match &s.services.devices {
        Some(d) => (d.microphones(), d.cameras()),
        None => (vec![], vec![]),
    };
    Ok(json!({"microphones": mics, "cameras": cams}))
}

fn probe(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path"])?;
    let v = s
        .services
        .video
        .clone()
        .ok_or_else(|| CommandError::Disabled("no video reader available".into()))?;
    let info = v
        .probe(std::path::Path::new(a.str("path")?))
        .map_err(CommandError::failed)?;
    Ok(json!(info))
}

fn audio_play(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "ms"])?;
    let player = s
        .services
        .player
        .clone()
        .ok_or_else(|| CommandError::Disabled("no audio player available".into()))?;
    player
        .play(
            std::path::Path::new(a.str("path")?),
            u64::from(a.u32("ms")?),
        )
        .map_err(CommandError::failed)?;
    Ok(json!({}))
}

fn audio_pause(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    if let Some(player) = &s.services.player {
        player.pause();
    }
    Ok(json!({}))
}

fn audio_stop(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    if let Some(player) = &s.services.player {
        player.stop();
    }
    Ok(json!({}))
}

fn frame(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "ms"])?;
    let v = s
        .services
        .video
        .clone()
        .ok_or_else(|| CommandError::Disabled("no video reader available".into()))?;
    let img = v
        .frame_at(
            std::path::Path::new(a.str("path")?),
            u64::from(a.u32("ms")?),
        )
        .map_err(CommandError::failed)?;
    let (w, h) = (img.width(), img.height());
    let mut d = screenit_doc::CaptureDoc::new(img);
    d.name = "Video frame".into();
    s.new_document(d);
    Ok(json!({"width": w, "height": h}))
}

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, params, run| CommandSpec {
        id,
        label,
        menu: "Record",
        shortcut: "",
        params,
        enabled: always,
        run,
    };
    vec![
        c(
            "record.start",
            "Start Recording",
            r#"{"x","y","w","h","output":str.mp4,"fps"?:int,"codec"?:h264|h265,"size"?:native|1080p|1440p|2160p,"mic"?:str,"system_audio"?:bool,"cursor_highlight"?:bool,"click_ripples"?:bool,"event_log"?:bool(default true; writes <output>.events.json),"countdown_ms"?:int,"webcam"?:bool,"pip_shape"?:rectangle|rounded|circle,"pip_size"?:5..100,"pip_corner"?:top_left|top_right|bottom_left|bottom_right,"pip_margin"?:int}"#,
            start,
        ),
        c(
            "record.move",
            "Move Recording Area",
            r#"{"x":int,"y":int}"#,
            move_region,
        ),
        c("record.pause", "Pause", "{}", pause),
        c("record.resume", "Resume", "{}", resume),
        c("record.stop", "Stop", "{}", stop),
        c(
            "record.pip",
            "Webcam Layout",
            r#"{"pip_shape"?:str,"pip_size"?:int,"pip_corner"?:str,"pip_margin"?:int}"#,
            pip,
        ),
        c("record.swap", "Swap Screen and Webcam", "{}", swap),
        c("record.status", "Recording Status", "{}", status),
        c("record.devices", "Recording Devices", "{}", devices),
        c("video.probe", "Video Info", r#"{"path":str}"#, probe),
        c(
            "video.frame",
            "Grab Video Frame",
            r#"{"path":str,"ms":int}"#,
            frame,
        ),
        c(
            "video.audio.play",
            "Play Video Audio",
            r#"{"path":str,"ms":int}"#,
            audio_play,
        ),
        c("video.audio.pause", "Pause Video Audio", "{}", audio_pause),
        c("video.audio.stop", "Stop Video Audio", "{}", audio_stop),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenit_record::{RecordError, TrackPlayer};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    struct Mem {
        plays: Mutex<Vec<(String, u64)>>,
        pauses: AtomicUsize,
        stops: AtomicUsize,
    }

    impl TrackPlayer for Mem {
        fn play(&self, path: &std::path::Path, ms: u64) -> Result<(), RecordError> {
            if path.as_os_str().is_empty() {
                return Err(RecordError::BadConfig("empty path".into()));
            }
            if let Ok(mut g) = self.plays.lock() {
                g.push((path.display().to_string(), ms));
            }
            Ok(())
        }
        fn pause(&self) {
            self.pauses.fetch_add(1, Ordering::SeqCst);
        }
        fn stop(&self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn session(player: Option<Arc<dyn TrackPlayer>>) -> Session {
        Session::new(crate::Services {
            player,
            ..crate::Services::default()
        })
    }

    struct Shift {
        at: Arc<Mutex<(i32, i32)>>,
    }

    impl screenit_record::Recorder for Shift {
        fn start(&mut self, cfg: &screenit_record::RecordConfig) -> Result<(), RecordError> {
            if let Ok(mut g) = self.at.lock() {
                *g = (cfg.region.x, cfg.region.y);
            }
            Ok(())
        }
        fn pause(&mut self) -> Result<(), RecordError> {
            Ok(())
        }
        fn resume(&mut self) -> Result<(), RecordError> {
            Ok(())
        }
        fn stop(&mut self) -> Result<screenit_record::RecordingResult, RecordError> {
            Ok(screenit_record::RecordingResult {
                path: "clip.mp4".into(),
                duration_ms: 0,
                frames: 0,
                width: 2,
                height: 2,
                has_audio: false,
            })
        }
        fn move_region(&mut self, x: i32, y: i32) -> Result<(), RecordError> {
            if let Ok(mut g) = self.at.lock() {
                *g = (x, y);
            }
            Ok(())
        }
    }

    #[test]
    fn move_shifts_the_area_and_rejects_bad_params() {
        let at = Arc::new(Mutex::new((0, 0)));
        let shared = at.clone();
        let mut s = session(None);
        s.services.recorder = Some(Box::new(move || Box::new(Shift { at: shared.clone() })));
        assert!(s.dispatch("record.move", &json!({"x": 1, "y": 2})).is_err());
        s.dispatch(
            "record.start",
            &json!({"x": 10, "y": 20, "w": 100, "h": 80, "output": "clip.mp4"}),
        )
        .unwrap();
        let r = s
            .dispatch("record.move", &json!({"x": 40, "y": 50}))
            .unwrap();
        assert_eq!(r["x"], 40);
        assert_eq!(*at.lock().unwrap(), (40, 50));
        assert!(s.dispatch("record.move", &json!({"x": 1})).is_err());
        assert!(
            s.dispatch("record.move", &json!({"x": 200_000, "y": 0}))
                .is_err()
        );
        s.dispatch("record.stop", &json!({})).unwrap();
    }

    #[test]
    fn audio_commands_need_a_player_and_reject_bad_params() {
        let mut bare = session(None);
        assert!(matches!(
            bare.dispatch("video.audio.play", &json!({"path": "a.mp4", "ms": 0})),
            Err(CommandError::Disabled(_))
        ));
        assert!(bare.dispatch("video.audio.pause", &json!({})).is_ok());
        let mem = Arc::new(Mem {
            plays: Mutex::new(Vec::new()),
            pauses: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
        });
        let mut s = session(Some(mem.clone()));
        assert!(s.dispatch("video.audio.play", &json!({})).is_err());
        assert!(
            s.dispatch("video.audio.play", &json!({"path": "", "ms": 0}))
                .is_err()
        );
        assert!(
            s.dispatch("video.audio.play", &json!({"path": "clip.mp4", "ms": 1200}))
                .is_ok()
        );
        assert!(s.dispatch("video.audio.pause", &json!({})).is_ok());
        assert!(s.dispatch("video.audio.stop", &json!({})).is_ok());
        let plays = mem.plays.lock().ok();
        let got = plays.as_ref().and_then(|g| g.first());
        assert_eq!(
            got.map(|(p, ms)| (p.as_str(), *ms)),
            Some(("clip.mp4", 1200))
        );
        assert_eq!(mem.pauses.load(Ordering::SeqCst), 1);
        assert_eq!(mem.stops.load(Ordering::SeqCst), 1);
    }
}
