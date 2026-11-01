//! Recording commands. The state machine (`record::Session`) lives here; the actual encoding is
//! delegated to the injected `Recorder`.

use crate::args::Args;
use crate::{ActiveRecording, CmdResult, CommandError, CommandSpec, Session, always};
use screenit_geom::Rect;
use screenit_record::{
    Codec, Corner, PipLayout, PipShape, RecState, RecordConfig, Session as RecSession,
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
            "mic",
            "system_audio",
            "cursor_highlight",
            "click_ripples",
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
    let cfg = RecordConfig {
        region: Rect::new(a.i32("x")?, a.i32("y")?, a.u32("w")?, a.u32("h")?),
        window: None,
        fps: a.opt_u32("fps")?.unwrap_or(30),
        codec,
        mic: a.opt_str("mic")?.map(str::to_string),
        system_audio: a.bool("system_audio", false)?,
        cursor_highlight: a.bool("cursor_highlight", false)?,
        click_ripples: a.bool("click_ripples", false)?,
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
            r#"{"x","y","w","h","output":str.mp4,"fps"?:int,"codec"?:h264|h265,"mic"?:str,"system_audio"?:bool,"cursor_highlight"?:bool,"click_ripples"?:bool,"countdown_ms"?:int,"webcam"?:bool,"pip_shape"?:rectangle|rounded|circle,"pip_size"?:5..100,"pip_corner"?:top_left|top_right|bottom_left|bottom_right,"pip_margin"?:int}"#,
            start,
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
    ]
}
