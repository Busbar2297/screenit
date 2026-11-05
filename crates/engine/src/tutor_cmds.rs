//! AI tutorial commands: UI primitives (`ui.*`, `input.*`, `app.launch`) and the tutorial
//! pipeline (`tutor.plan` -> `tutor.rehearse` -> `tutor.perform` -> `tutor.produce`). Long steps
//! run as background jobs; `tutor.status` reports progress and `tutor.wait` blocks until done.

use crate::args::Args;
use crate::tutor_render::render_video;
use crate::{CmdResult, CommandError, CommandSpec, Session, always};
use screenit_geom::Rect;
use screenit_record::{Codec, OutputSize, RecordConfig, Recorder};
use screenit_tutor::explore::{Answer, ExploreOpts, empty_script, explore};
use screenit_tutor::perform::{ChapterRecorder, Performance, Segment, Stage, perform};
use screenit_tutor::plan::{parse_script, plan_messages};
use screenit_tutor::produce::{Plan, ProduceOpts};
use screenit_tutor::rehearse::{RehearseOpts, rehearse};
use screenit_tutor::safety::Guard;
use screenit_tutor::script::{AppTarget, TutorialScript};
use screenit_tutor::{
    ChatMessage, ChatOptions, MouseButton, Pace, Rig, TutorError, TutorResult, WindowInfo,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

#[derive(Default)]
struct Inner {
    phase: String,
    running: bool,
    script: Option<TutorialScript>,
    performance: Option<Performance>,
    events: Vec<Value>,
    error: Option<String>,
    result: Option<Value>,
    started: Option<std::time::Instant>,
    took_ms: u64,
    /// Exploring: the model's open question for the user, and the reply once given.
    question: Option<String>,
    answer: Option<Answer>,
    /// Phase of the job that ran last, so the panel can say what just finished.
    last_phase: String,
    /// Steps or frames finished, and how many the job expects.
    progress_done: u32,
    progress_total: u32,
}

/// State shared between commands and the background job thread.
pub struct TutorShared {
    inner: Mutex<Inner>,
    cv: Condvar,
    pub abort: Arc<AtomicBool>,
}

impl Default for TutorShared {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                phase: "idle".into(),
                ..Inner::default()
            }),
            cv: Condvar::new(),
            abort: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl TutorShared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn event(&self, v: Value) {
        let mut g = self.lock();
        if g.events.len() >= 500 {
            g.events.remove(0);
        }
        g.events.push(v);
    }

    fn set_progress(&self, done: u32, total: u32) {
        let mut g = self.lock();
        g.progress_done = done;
        g.progress_total = total;
    }
}

fn terr(e: TutorError) -> CommandError {
    CommandError::Failed(e.to_string())
}

fn rig(s: &Session) -> Result<Rig, CommandError> {
    let need = |what: &str| CommandError::Disabled(format!("no {what} available"));
    Ok(Rig {
        ui: s.services.ui.clone().ok_or_else(|| need("UI Automation"))?,
        input: s
            .services
            .input
            .clone()
            .ok_or_else(|| need("input driver"))?,
        launcher: s
            .services
            .launcher
            .clone()
            .ok_or_else(|| need("app launcher"))?,
        pace: s.services.pace.clone(),
    })
}

fn ui_only(s: &Session) -> Result<Arc<dyn screenit_tutor::UiTree>, CommandError> {
    s.services
        .ui
        .clone()
        .ok_or_else(|| CommandError::Disabled("no UI Automation available".into()))
}

/// The window named by `window` (id) or `title`, or the foreground window.
fn resolve_window(
    ui: &dyn screenit_tutor::UiTree,
    launcher: Option<&dyn screenit_tutor::AppLauncher>,
    a: &Args<'_>,
) -> Result<WindowInfo, CommandError> {
    if a.has("window") {
        let id = a.u64("window")?;
        return ui.snapshot(id).map(|s| s.window).map_err(terr);
    }
    if let Some(t) = a.opt_str("title")? {
        return launcher
            .and_then(|l| l.find_window(t))
            .ok_or_else(|| CommandError::Failed(format!("no window titled `{t}`")));
    }
    ui.foreground()
        .ok_or_else(|| CommandError::Failed("no foreground window".into()))
}

fn window_json(w: &WindowInfo) -> Value {
    json!({"window": w.hwnd, "pid": w.pid, "title": w.title, "rect": w.rect})
}

fn snapshot(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["window", "title", "max_lines"])?;
    let ui = ui_only(s)?;
    let launcher = s.services.launcher.clone();
    let w = resolve_window(&*ui, launcher.as_deref(), &a)?;
    let snap = ui.snapshot(w.hwnd).map_err(terr)?;
    let max = a.opt_u32("max_lines")?.unwrap_or(150).clamp(1, 1000) as usize;
    Ok(json!({
        "window": window_json(&snap.window),
        "elements": snap.elements.len(),
        "text": snap.compact(max),
    }))
}

/// Raw input is only delivered to the app the caller names, while it has the keyboard focus.
fn guarded_input(s: &Session, a: &Args<'_>) -> Result<(Rig, WindowInfo), CommandError> {
    let rig = rig(s)?;
    if !a.has("window") && a.opt_str("title")?.is_none() {
        return Err(CommandError::Params(
            "input commands need `window` (id) or `title` of the target app".into(),
        ));
    }
    let w = resolve_window(&*rig.ui, Some(&*rig.launcher), a)?;
    if rig.input.interrupted() {
        return Err(CommandError::Failed("stopped by the user".into()));
    }
    match rig.ui.foreground() {
        Some(f) if f.pid == w.pid => Ok((rig, w)),
        _ => Err(CommandError::Failed(format!(
            "`{}` does not have the keyboard focus; input is only sent to the target app",
            w.title.chars().take(60).collect::<String>()
        ))),
    }
}

fn input_move(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["window", "title", "x", "y"])?;
    let (rig, _) = guarded_input(s, &a)?;
    rig.input.move_to(a.i32("x")?, a.i32("y")?).map_err(terr)?;
    Ok(json!({"ok": true}))
}

fn input_click(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["window", "title", "x", "y", "button", "count"])?;
    let (rig, _) = guarded_input(s, &a)?;
    let b = match a.opt_str("button")? {
        None | Some("left") => MouseButton::Left,
        Some("right") => MouseButton::Right,
        Some("middle") => MouseButton::Middle,
        Some(o) => return Err(CommandError::Params(format!("unknown button `{o}`"))),
    };
    let n = a.opt_u32("count")?.unwrap_or(1);
    if !(1..=3).contains(&n) {
        return Err(CommandError::Params("count must be 1..=3".into()));
    }
    rig.input
        .click(a.i32("x")?, a.i32("y")?, b, n as u8)
        .map_err(terr)?;
    Ok(json!({"ok": true}))
}

fn input_type(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["window", "title", "text", "delay_ms"])?;
    let (rig, w) = guarded_input(s, &a)?;
    let text = a.str("text")?;
    if text.is_empty() || text.chars().count() > 2000 {
        return Err(CommandError::Params(
            "text must be 1..=2000 characters".into(),
        ));
    }
    let snap = rig.ui.snapshot(w.hwnd).map_err(terr)?;
    Guard::new(w.pid, w.hwnd, Arc::new(AtomicBool::new(false)))
        .check_typing(&snap, None)
        .map_err(terr)?;
    rig.input
        .type_text(text, a.opt_u32("delay_ms")?.unwrap_or(20).min(500))
        .map_err(terr)?;
    Ok(json!({"ok": true}))
}

fn input_key(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["window", "title", "keys"])?;
    let (rig, _) = guarded_input(s, &a)?;
    let k = a.str("keys")?;
    if k.trim().is_empty() || k.len() > 60 {
        return Err(CommandError::Params("keys must be a short chord".into()));
    }
    screenit_tutor::keys::parse_chord(k).map_err(terr)?;
    rig.input.key(k).map_err(terr)?;
    Ok(json!({"ok": true}))
}

fn input_scroll(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["window", "title", "x", "y", "dy"])?;
    let (rig, _) = guarded_input(s, &a)?;
    let dy = a.i32("dy")?;
    if dy.unsigned_abs() > 100 {
        return Err(CommandError::Params("dy is capped at 100 notches".into()));
    }
    rig.input
        .scroll(a.i32("x")?, a.i32("y")?, dy)
        .map_err(terr)?;
    Ok(json!({"ok": true}))
}

fn app_launch(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["target", "args"])?;
    let l = s
        .services
        .launcher
        .clone()
        .ok_or_else(|| CommandError::Disabled("no app launcher available".into()))?;
    let t = a.str("target")?;
    if t.trim().is_empty() || t.len() > 500 {
        return Err(CommandError::Params(
            "target must be 1..=500 characters".into(),
        ));
    }
    let w = l
        .launch(t, a.opt_str("args")?.unwrap_or(""))
        .map_err(terr)?;
    Ok(window_json(&w))
}

// ------------------------------------------------------------------ jobs

fn spawn_job(
    s: &Session,
    phase: &'static str,
    f: impl FnOnce(&Arc<TutorShared>) -> TutorResult<Value> + Send + 'static,
) -> CmdResult {
    let sh = s.tutor.clone();
    {
        let mut g = sh.lock();
        if g.running {
            return Err(CommandError::Failed(format!(
                "a tutorial job is already running ({})",
                g.phase
            )));
        }
        g.running = true;
        g.phase = phase.into();
        g.error = None;
        g.result = None;
        g.events.clear();
        g.started = Some(std::time::Instant::now());
        g.question = None;
        g.answer = None;
        g.last_phase = phase.into();
        g.progress_done = 0;
        g.progress_total = 0;
    }
    sh.abort.store(false, Ordering::SeqCst);
    let shared = sh.clone();
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&shared)));
        let mut g = shared.lock();
        match r {
            Ok(Ok(v)) => g.result = Some(v),
            Ok(Err(e)) => g.error = Some(e.to_string()),
            Err(_) => g.error = Some("internal error (recovered)".into()),
        }
        g.running = false;
        g.question = None;
        g.took_ms = g
            .started
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
        g.phase = "idle".into();
        shared.cv.notify_all();
    });
    Ok(json!({"started": phase}))
}

fn chat_setup(s: &Session) -> Option<(Arc<dyn screenit_tutor::LlmChat>, ChatOptions)> {
    let chat = s.services.chat.clone()?;
    let o = &s.settings.ocr;
    Some((
        chat,
        ChatOptions {
            url: o.llm_url.clone(),
            model: o.llm_model.clone(),
            api_key: o.llm_api_key.clone(),
            timeout_secs: o.llm_timeout_secs.max(120),
            json: true,
            max_tokens: 8192,
        },
    ))
}

/// Bring the app forward and return its fresh window info.
fn prepare(rig: &Rig, w: &WindowInfo) -> TutorResult<WindowInfo> {
    rig.launcher.focus(w.hwnd)?;
    rig.pace.sleep_ms(400);
    Ok(rig.ui.snapshot(w.hwnd)?.window)
}

fn app_of(a: &Args<'_>) -> Result<AppTarget, CommandError> {
    Ok(AppTarget {
        launch: a.opt_str("launch")?.unwrap_or("").to_string(),
        args: a.opt_str("args")?.unwrap_or("").to_string(),
        window_title: a.opt_str("window_title")?.unwrap_or("").to_string(),
    })
}

fn model_failure(model: &str, reply: &str, err: TutorError) -> TutorError {
    let who = if model.is_empty() {
        "the model loaded in LM Studio".to_string()
    } else {
        model.to_string()
    };
    let detail = match err {
        TutorError::Llm(m) => m,
        other => other.to_string(),
    };
    let snippet: String = reply
        .chars()
        .filter(|c| !c.is_control())
        .take(140)
        .collect();
    TutorError::Llm(format!("{who}: {detail}. Reply: {snippet}"))
}

fn plan(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(
        p,
        &[
            "goal",
            "launch",
            "args",
            "window_title",
            "audience",
            "max_steps",
        ],
    )?;
    let goal = a.str("goal")?.trim().to_string();
    if goal.is_empty() || goal.chars().count() > 2000 {
        return Err(CommandError::Params(
            "goal must be 1..=2000 characters".into(),
        ));
    }
    let mut app = app_of(&a)?;
    let rig = rig(s)?;
    if app.launch.is_empty() && app.window_title.is_empty() {
        app.window_title = rig
            .ui
            .other_windows()
            .into_iter()
            .next()
            .map(|w| w.title)
            .ok_or_else(|| {
                CommandError::Params(
                    "no other window is open. Open the app, or set its window title.".into(),
                )
            })?;
    }
    let audience = a.opt_str("audience")?.unwrap_or("").to_string();
    let max_steps = a.opt_u32("max_steps")?.unwrap_or(30).clamp(1, 200);
    let (chat, copts) = chat_setup(s)
        .ok_or_else(|| CommandError::Disabled("no language model available".into()))?;
    spawn_job(s, "planning", move |sh| {
        let w = rig.window(&app)?;
        let w = prepare(&rig, &w)?;
        let snap = rig.ui.snapshot(w.hwnd)?;
        let mut app = app;
        if app.window_title.is_empty() {
            app.window_title = w.title.clone();
        }
        let mut msgs = plan_messages(&goal, &app.window_title, &audience, max_steps, &snap);
        let model = copts.model.trim().to_string();
        let mut last = None;
        let mut last_reply = String::new();
        for attempt in 0..2 {
            sh.event(json!({"event": "asking_model", "model": model, "attempt": attempt + 1}));
            let reply = screenit_tutor::wait_for_stop(&sh.abort, {
                let chat = Arc::clone(&chat);
                let msgs = msgs.clone();
                let copts = copts.clone();
                move || chat.chat(&msgs, &copts)
            })??;
            last_reply = reply.clone();
            match parse_script(&reply, &goal, app.clone()) {
                Ok(script) => {
                    let out = json!({"title": script.title, "chapters": script.chapters.len(), "steps": script.step_count()});
                    sh.lock().script = Some(script);
                    return Ok(out);
                }
                Err(e) => {
                    msgs.push(ChatMessage {
                        role: "assistant".into(),
                        content: reply.chars().take(1200).collect(),
                    });
                    msgs.push(ChatMessage::user(format!(
                        "That reply was rejected: {e}. Reply again with only the JSON object. The first character must be {{."
                    )));
                    last = Some(e);
                }
            }
        }
        Err(model_failure(
            &model,
            &last_reply,
            last.unwrap_or_else(|| TutorError::Llm("no usable script".into())),
        ))
    })
}

fn script_get(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let g = s.tutor.lock();
    let sc = g.script.as_ref().ok_or_else(|| {
        CommandError::Failed("no tutorial script yet; run tutor.plan or tutor.script.set".into())
    })?;
    serde_json::to_value(sc).map_err(CommandError::failed)
}

fn script_set(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["script", "keep_checked"])?;
    if s.tutor.lock().running {
        return Err(CommandError::Failed("a tutorial job is running".into()));
    }
    let v = p
        .get("script")
        .filter(|_| a.has("script"))
        .ok_or_else(|| CommandError::Params("missing parameter `script`".into()))?;
    let mut sc: TutorialScript =
        serde_json::from_value(v.clone()).map_err(|e| CommandError::Params(e.to_string()))?;
    sc.validate()
        .map_err(|e| CommandError::Params(e.to_string()))?;
    if !a.bool("keep_checked", false)? {
        for c in &mut sc.chapters {
            c.checked = false;
        }
    }
    let out = json!({"chapters": sc.chapters.len(), "steps": sc.step_count()});
    s.tutor.lock().script = Some(sc);
    Ok(out)
}

fn save(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path"])?;
    let path = PathBuf::from(a.str("path")?);
    let g = s.tutor.lock();
    let sc = g
        .script
        .as_ref()
        .ok_or_else(|| CommandError::Failed("no tutorial script to save".into()))?;
    let text = serde_json::to_string_pretty(sc).map_err(CommandError::failed)?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(CommandError::failed)?;
    }
    std::fs::write(&path, text).map_err(CommandError::failed)?;
    Ok(json!({"path": path}))
}

fn load(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path"])?;
    let path = a.str("path")?;
    let meta = std::fs::metadata(path).map_err(CommandError::failed)?;
    if meta.len() > 4_000_000 {
        return Err(CommandError::Params("script file is too large".into()));
    }
    let text = std::fs::read_to_string(path).map_err(CommandError::failed)?;
    let v: Value = serde_json::from_str(&text).map_err(|e| CommandError::Params(e.to_string()))?;
    script_set(s, &json!({"script": v}))
}

fn current_script(s: &Session) -> Result<TutorialScript, CommandError> {
    s.tutor
        .lock()
        .script
        .clone()
        .ok_or_else(|| CommandError::Failed("no tutorial script yet; run tutor.plan".into()))
}

fn rehearse_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(
        p,
        &[
            "from_chapter",
            "max_repairs",
            "allow_destructive",
            "use_llm",
        ],
    )?;
    let from = a.opt_u32("from_chapter")?.unwrap_or(0) as usize;
    let repairs = a.opt_u32("max_repairs")?.unwrap_or(2).min(5);
    let allow = a.bool("allow_destructive", false)?;
    let use_llm = a.bool("use_llm", true)?;
    let mut script = current_script(s)?;
    if from >= script.chapters.len() {
        return Err(CommandError::Params(
            "from_chapter is past the last chapter".into(),
        ));
    }
    let rig = rig(s)?;
    let chat = if use_llm { chat_setup(s) } else { None };
    spawn_job(s, "rehearsing", move |sh| {
        let w = rig.window(&script.app)?;
        let w = prepare(&rig, &w)?;
        let mut guard = Guard::new(w.pid, w.hwnd, sh.abort.clone());
        guard.allow_destructive = allow;
        let opts = RehearseOpts {
            from_chapter: from,
            max_repairs: repairs,
            chat: chat.as_ref().map(|(c, o)| (Arc::clone(c), o.clone())),
        };
        let r = rehearse(&mut script, &rig, guard, w, &opts, &mut |e| {
            sh.event(serde_json::to_value(&e).unwrap_or(Value::Null));
        });
        sh.lock().script = Some(script);
        let r = r?;
        Ok(serde_json::to_value(r).unwrap_or(Value::Null))
    })
}

/// Records each chapter with its own recorder from the engine's factory.
struct EngineRecorder {
    spare: Vec<Box<dyn Recorder>>,
    active: Option<(Box<dyn Recorder>, u64)>,
    dir: PathBuf,
    name: String,
    fps: u32,
    pace: Arc<dyn Pace>,
}

impl ChapterRecorder for EngineRecorder {
    fn start(&mut self, chapter: usize, region: Rect) -> TutorResult<()> {
        let mut rec = self
            .spare
            .pop()
            .ok_or_else(|| TutorError::Record("no recorder left".into()))?;
        let cfg = RecordConfig {
            region,
            window: None,
            fps: self.fps,
            codec: Codec::H264,
            size: OutputSize::Native,
            mic: None,
            system_audio: false,
            cursor_highlight: true,
            click_ripples: false,
            output: self
                .dir
                .join(format!("{}_ch{:02}.mp4", self.name, chapter + 1)),
        };
        cfg.validate()
            .map_err(|e| TutorError::Record(e.to_string()))?;
        rec.start(&cfg)
            .map_err(|e| TutorError::Record(e.to_string()))?;
        self.active = Some((rec, self.pace.now_ms()));
        Ok(())
    }

    fn stop(&mut self) -> TutorResult<Segment> {
        let (mut rec, t0) = self
            .active
            .take()
            .ok_or_else(|| TutorError::Record("not recording".into()))?;
        let r = rec.stop().map_err(|e| TutorError::Record(e.to_string()))?;
        let elapsed = self.pace.now_ms().saturating_sub(t0);
        Ok(Segment {
            path: r.path,
            duration_ms: if r.duration_ms == 0 {
                elapsed
            } else {
                r.duration_ms
            },
        })
    }
}

fn safe_name(s: &str) -> String {
    let n: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    if n.is_empty() { "tutorial".into() } else { n }
}

fn perform_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(
        p,
        &["output_dir", "name", "chapters", "fps", "allow_destructive"],
    )?;
    let dir = PathBuf::from(a.str("output_dir")?);
    let script = current_script(s)?;
    let name = safe_name(a.opt_str("name")?.unwrap_or(&script.title));
    let fps = a.opt_u32("fps")?.unwrap_or(30).clamp(5, 60);
    let allow = a.bool("allow_destructive", false)?;
    let only: Option<Vec<usize>> = if a.has("chapters") {
        let v = a
            .array("chapters")?
            .iter()
            .map(|v| v.as_u64().map(|n| n as usize))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| CommandError::Params("chapters must be numbers".into()))?;
        if v.iter().any(|&c| c >= script.chapters.len()) {
            return Err(CommandError::Params("chapter index out of range".into()));
        }
        Some(v)
    } else {
        None
    };
    if let Some(bad) = script
        .chapters
        .iter()
        .enumerate()
        .find(|(i, c)| !c.checked && only.as_ref().is_none_or(|o| o.contains(i)))
    {
        return Err(CommandError::Failed(format!(
            "chapter {} was not rehearsed successfully; run tutor.rehearse first",
            bad.0 + 1
        )));
    }
    let rig = rig(s)?;
    let factory = s
        .services
        .recorder
        .as_ref()
        .ok_or_else(|| CommandError::Disabled("no recorder available".into()))?;
    let n = only.as_ref().map_or(script.chapters.len(), Vec::len);
    let spare: Vec<Box<dyn Recorder>> = (0..n).map(|_| factory()).collect();
    std::fs::create_dir_all(&dir).map_err(CommandError::failed)?;
    spawn_job(s, "performing", move |sh| {
        let w = rig.window(&script.app)?;
        let w = prepare(&rig, &w)?;
        let mut guard = Guard::new(w.pid, w.hwnd, sh.abort.clone());
        guard.allow_destructive = allow;
        let mut rec = EngineRecorder {
            spare,
            active: None,
            dir: dir.clone(),
            name: name.clone(),
            fps,
            pace: rig.pace.clone(),
        };
        let mut out = sh.lock().performance.clone().unwrap_or_default();
        let r = perform(
            &script,
            Stage {
                rig: &rig,
                guard,
                window: w,
            },
            &mut rec,
            only.as_deref(),
            &mut out,
            &mut |c| sh.event(json!({"event": "chapter", "index": c})),
        );
        sh.lock().performance = Some(out.clone());
        let sidecar = dir.join(format!("{name}.tutor.json"));
        let text =
            serde_json::to_string_pretty(&out).map_err(|e| TutorError::Record(e.to_string()))?;
        std::fs::write(&sidecar, text).map_err(|e| TutorError::Record(e.to_string()))?;
        r?;
        Ok(json!({"takes": out.takes.len(), "performance": sidecar}))
    })
}

fn produce_opts(a: &Args<'_>) -> Result<ProduceOpts, CommandError> {
    let d = ProduceOpts::default();
    let zf = a.opt_f32("zoom_factor")?.unwrap_or(d.zoom_factor);
    if !(1.0..=4.0).contains(&zf) {
        return Err(CommandError::Params("zoom_factor must be 1..=4".into()));
    }
    Ok(ProduceOpts {
        zoom: a.bool("zoom", d.zoom)?,
        captions: a.bool("captions", d.captions)?,
        callouts: a.bool("callouts", d.callouts)?,
        trim_idle: a.bool("trim_idle", d.trim_idle)?,
        chapter_cards: a.bool("chapter_cards", d.chapter_cards)?,
        zoom_factor: zf,
        ..d
    })
}

fn produce(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(
        p,
        &[
            "output",
            "performance",
            "fps",
            "zoom",
            "captions",
            "callouts",
            "trim_idle",
            "chapter_cards",
            "zoom_factor",
        ],
    )?;
    let out = PathBuf::from(a.str("output")?);
    if out
        .extension()
        .is_none_or(|e| !e.eq_ignore_ascii_case("mp4"))
    {
        return Err(CommandError::Params("output must be an .mp4 path".into()));
    }
    let opts = produce_opts(&a)?;
    let fps = a.opt_u32("fps")?.unwrap_or(30).clamp(5, 60);
    let perf = match a.opt_str("performance")? {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(CommandError::failed)?;
            serde_json::from_str::<Performance>(&text)
                .map_err(|e| CommandError::Params(e.to_string()))?
        }
        None => s.tutor.lock().performance.clone().ok_or_else(|| {
            CommandError::Failed("nothing performed yet; run tutor.perform".into())
        })?,
    };
    if perf.takes.is_empty() || perf.region.is_empty() {
        return Err(CommandError::Failed(
            "the performance holds no recordings".into(),
        ));
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
    spawn_job(s, "producing", move |sh| {
        let plan = Plan::build(&perf, &opts);
        let res = render_video(
            &*reader,
            &*writer,
            &perf,
            &plan,
            fps,
            &out,
            &sh.abort,
            &|done, total| sh.set_progress(done, total),
        )
        .map_err(TutorError::Record)?;
        let srt = out.with_extension("srt");
        if opts.captions {
            std::fs::write(&srt, plan.srt()).map_err(|e| TutorError::Record(e.to_string()))?;
        }
        Ok(json!({
            "video": out, "srt": opts.captions.then_some(srt),
            "duration_ms": plan.total_ms, "frames": res.frames,
            "chapters": plan.chapter_list(),
        }))
    })
}

fn status(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let g = s.tutor.lock();
    let chapters: Vec<Value> = g
        .script
        .as_ref()
        .map(|sc| {
            sc.chapters
                .iter()
                .map(|c| json!({"title": c.title, "steps": c.steps.len(), "checked": c.checked}))
                .collect()
        })
        .unwrap_or_default();
    let tail = g.events.len().saturating_sub(50);
    Ok(json!({
        "phase": g.phase, "running": g.running, "error": g.error, "result": g.result,
        "last_phase": g.last_phase,
        "question": g.question,
        "elapsed_ms": if g.running {
            g.started.map(|t| t.elapsed().as_millis() as u64).unwrap_or(0)
        } else {
            g.took_ms
        },
        "title": g.script.as_ref().map(|s| s.title.clone()),
        "chapters": chapters,
        "performed": g.performance.as_ref().map(|p| p.takes.len()),
        "progress_done": g.progress_done,
        "progress_total": g.progress_total,
        "events": &g.events[tail..],
    }))
}

fn wait(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["timeout_ms"])?;
    let ms = u64::from(a.opt_u32("timeout_ms")?.unwrap_or(30_000).min(55_000));
    let sh = s.tutor.clone();
    let mut g = sh.lock();
    let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while g.running {
        let now = std::time::Instant::now();
        if now >= end {
            break;
        }
        g = sh
            .cv
            .wait_timeout(g, end - now)
            .map(|(g, _)| g)
            .unwrap_or_else(|e| e.into_inner().0);
    }
    drop(g);
    status(s, &json!({}))
}

/// Block the explore job until the user answers in the panel (or presses Stop).
fn wait_answer(sh: &TutorShared, question: &str) -> TutorResult<Answer> {
    let mut g = sh.lock();
    g.question = Some(question.to_string());
    g.answer = None;
    loop {
        if sh.abort.load(Ordering::SeqCst) {
            g.question = None;
            return Err(TutorError::Aborted("stopped by the user".into()));
        }
        if let Some(a) = g.answer.take() {
            g.question = None;
            return Ok(a);
        }
        g = sh
            .cv
            .wait_timeout(g, std::time::Duration::from_millis(200))
            .map(|(g, _)| g)
            .unwrap_or_else(|e| e.into_inner().0);
    }
}

fn explore_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(
        p,
        &[
            "goal",
            "launch",
            "args",
            "window_title",
            "max_steps",
            "allow_destructive",
            "save",
        ],
    )?;
    let goal = a.str("goal")?.trim().to_string();
    if goal.is_empty() || goal.chars().count() > 2000 {
        return Err(CommandError::Params(
            "goal must be 1..=2000 characters".into(),
        ));
    }
    let mut app = app_of(&a)?;
    let rig = rig(s)?;
    if app.launch.is_empty() && app.window_title.is_empty() {
        app.window_title = rig
            .ui
            .other_windows()
            .into_iter()
            .next()
            .map(|w| w.title)
            .ok_or_else(|| {
                CommandError::Params(
                    "no other window is open. Open the app, or set its window title.".into(),
                )
            })?;
    }
    let max_steps = a.opt_u32("max_steps")?.unwrap_or(30).clamp(1, 200);
    let allow = a.bool("allow_destructive", false)?;
    let save = a.opt_str("save")?.map(PathBuf::from).filter(|p| {
        !p.as_os_str().is_empty()
            && p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
    });
    let (chat, copts) = chat_setup(s)
        .ok_or_else(|| CommandError::Disabled("no language model available".into()))?;
    spawn_job(s, "exploring", move |sh| {
        let w = rig.window(&app)?;
        let w = prepare(&rig, &w)?;
        if app.window_title.is_empty() {
            app.window_title = w.title.clone();
        }
        let mut guard = Guard::new(w.pid, w.hwnd, sh.abort.clone());
        guard.allow_destructive = allow;
        let opts = ExploreOpts {
            max_steps,
            max_misses: 3,
            chat,
            copts,
        };
        sh.set_progress(0, max_steps);
        let mut script = empty_script(&goal, app);
        let r = explore(
            &mut script,
            &rig,
            guard,
            w,
            &opts,
            &mut |e| {
                let v = serde_json::to_value(&e).unwrap_or(Value::Null);
                if v["event"] == "step" {
                    let done = sh.lock().progress_done.saturating_add(1).min(max_steps);
                    sh.set_progress(done, max_steps);
                }
                sh.event(v);
            },
            &mut |q| wait_answer(sh, q),
        );
        let mut out = json!({"title": script.title, "chapters": script.chapters.len(), "steps": script.step_count()});
        let keep = !script.chapters.is_empty() && (r.is_ok() || script.validate().is_ok());
        if keep {
            if let Some(path) = &save {
                match write_script(path, &script) {
                    Ok(()) => out["script"] = json!(path),
                    Err(e) if r.is_ok() => return Err(e),
                    Err(_) => {}
                }
            }
            sh.lock().script = Some(script);
        }
        r?;
        Ok(out)
    })
}

fn write_script(path: &std::path::Path, script: &TutorialScript) -> TutorResult<()> {
    let text = serde_json::to_string_pretty(script)
        .map_err(|e| TutorError::Script(format!("could not write the plan: {e}")))?;
    if let Some(d) = path.parent()
        && !d.as_os_str().is_empty()
    {
        std::fs::create_dir_all(d).map_err(|e| TutorError::Script(e.to_string()))?;
    }
    std::fs::write(path, text).map_err(|e| TutorError::Script(e.to_string()))
}

fn answer_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["text", "action"])?;
    let text = a.opt_str("text")?.unwrap_or("").trim().to_string();
    let ans = match a.opt_str("action")?.unwrap_or("hint") {
        "hint" => Answer::Hint(text.chars().take(500).collect()),
        "did_it" => Answer::DidIt,
        "skip" => Answer::SkipChapter,
        "finish" => Answer::Finish,
        other => {
            return Err(CommandError::Params(format!(
                "action must be hint, did_it, skip or finish, not `{other}`"
            )));
        }
    };
    let mut g = s.tutor.lock();
    if g.question.is_none() {
        return Err(CommandError::Failed(
            "nothing is waiting for an answer".into(),
        ));
    }
    g.answer = Some(ans);
    s.tutor.cv.notify_all();
    Ok(json!({"answered": true}))
}

fn abort(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    s.tutor.abort.store(true, Ordering::SeqCst);
    if let Some(input) = &s.services.input {
        input.user_stop();
    }
    Ok(json!({"abort": true, "running": s.tutor.lock().running}))
}

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, params, run| CommandSpec {
        id,
        label,
        menu,
        shortcut: "",
        params,
        enabled: always,
        run,
    };
    vec![
        c(
            "ui.snapshot",
            "UI Snapshot",
            "Tutorial",
            r#"{"window"?:int,"title"?:str,"max_lines"?:int}"#,
            snapshot,
        ),
        c(
            "input.move",
            "Move Mouse",
            "Tutorial",
            r#"{"window"|"title","x":int,"y":int}"#,
            input_move,
        ),
        c(
            "input.click",
            "Click",
            "Tutorial",
            r#"{"window"|"title","x":int,"y":int,"button"?:left|right|middle,"count"?:1..3}"#,
            input_click,
        ),
        c(
            "input.type",
            "Type Text",
            "Tutorial",
            r#"{"window"|"title","text":str,"delay_ms"?:int}"#,
            input_type,
        ),
        c(
            "input.key",
            "Press Keys",
            "Tutorial",
            r#"{"window"|"title","keys":"ctrl+s"}"#,
            input_key,
        ),
        c(
            "input.scroll",
            "Scroll",
            "Tutorial",
            r#"{"window"|"title","x":int,"y":int,"dy":int}"#,
            input_scroll,
        ),
        c(
            "app.launch",
            "Launch App",
            "Tutorial",
            r#"{"target":str,"args"?:str}"#,
            app_launch,
        ),
        c(
            "tutor.plan",
            "Plan Tutorial",
            "Tutorial",
            r#"{"goal":str,"launch"?:str,"args"?:str,"window_title"?:str,"audience"?:str,"max_steps"?:int}"#,
            plan,
        ),
        c(
            "tutor.explore",
            "Explore Tutorial",
            "Tutorial",
            r#"{"goal":str,"launch"?:str,"args"?:str,"window_title"?:str,"max_steps"?:int,"allow_destructive"?:bool,"save"?:str.json}"#,
            explore_cmd,
        ),
        c(
            "tutor.answer",
            "Answer Tutorial Question",
            "Tutorial",
            r#"{"text"?:str,"action"?:hint|did_it|skip|finish}"#,
            answer_cmd,
        ),
        c(
            "tutor.script.get",
            "Get Tutorial Script",
            "Tutorial",
            "{}",
            script_get,
        ),
        c(
            "tutor.script.set",
            "Set Tutorial Script",
            "Tutorial",
            r#"{"script":object,"keep_checked"?:bool}"#,
            script_set,
        ),
        c(
            "tutor.save",
            "Save Tutorial Script",
            "Tutorial",
            r#"{"path":str}"#,
            save,
        ),
        c(
            "tutor.load",
            "Load Tutorial Script",
            "Tutorial",
            r#"{"path":str}"#,
            load,
        ),
        c(
            "tutor.rehearse",
            "Rehearse Tutorial",
            "Tutorial",
            r#"{"from_chapter"?:int,"max_repairs"?:0..5,"allow_destructive"?:bool,"use_llm"?:bool}"#,
            rehearse_cmd,
        ),
        c(
            "tutor.perform",
            "Perform and Record Tutorial",
            "Tutorial",
            r#"{"output_dir":str,"name"?:str,"chapters"?:[int],"fps"?:int,"allow_destructive"?:bool}"#,
            perform_cmd,
        ),
        c(
            "tutor.produce",
            "Produce Tutorial Video",
            "Tutorial",
            r#"{"output":str.mp4,"performance"?:str,"fps"?:int,"zoom"?:bool,"captions"?:bool,"callouts"?:bool,"trim_idle"?:bool,"chapter_cards"?:bool,"zoom_factor"?:1..4}"#,
            produce,
        ),
        c("tutor.status", "Tutorial Status", "Tutorial", "{}", status),
        c(
            "tutor.wait",
            "Wait for Tutorial Job",
            "Tutorial",
            r#"{"timeout_ms"?:int}"#,
            wait,
        ),
        c("tutor.abort", "Abort Tutorial Run", "Tutorial", "{}", abort),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use screenit_raster::Raster;
    use screenit_record::{RecordError, VideoInfo, VideoReader};
    use screenit_tutor::fakes::{Effect, FakeLlm, FakePace, FakeWorld, Trigger, element};

    struct Reader;

    impl VideoReader for Reader {
        fn probe(&self, _: &std::path::Path) -> Result<VideoInfo, RecordError> {
            Ok(VideoInfo {
                duration_ms: 10_000,
                width: 800,
                height: 600,
                has_audio: false,
            })
        }
        fn frame_at(&self, _: &std::path::Path, _: u64) -> Result<Raster, RecordError> {
            Raster::new(800, 600, [90, 120, 160, 255])
                .map_err(|e| RecordError::Platform(e.to_string()))
        }
    }

    const SCRIPT: &str = r#"{"title":"Notes basics","chapters":[
     {"title":"Write","steps":[
      {"narration":"Open a new note.","action":{"type":"click","target":{"role":"Button","name":"New"}},
       "expect":[{"type":"appears","target":{"name":"Editor"}}]},
      {"narration":"Type your text.","action":{"type":"type","target":{"name":"Editor"},"text":"hi"},
       "expect":[{"type":"value_contains","target":{"name":"Editor"},"value":"hi"}]}]},
     {"title":"Save","steps":[
      {"narration":"Press Save.","action":{"type":"click","target":{"name":"Save"}},
       "expect":[{"type":"appears","target":{"name":"Saved"}}]}]}]}"#;

    fn world() -> Arc<FakeWorld> {
        let w = FakeWorld::new("Notes");
        w.add(element("Button", "New", Rect::new(110, 110, 60, 24)));
        w.add(element("Button", "Save", Rect::new(180, 110, 60, 24)));
        let mut pw = element("Edit", "Secret", Rect::new(110, 450, 200, 24));
        pw.password = true;
        w.add(pw);
        w.rule(
            Trigger::Click("New".into()),
            vec![Effect::Show(element(
                "Edit",
                "Editor",
                Rect::new(110, 200, 400, 200),
            ))],
        );
        w.rule(
            Trigger::Click("Save".into()),
            vec![Effect::Show(element(
                "Text",
                "Saved",
                Rect::new(110, 500, 80, 20),
            ))],
        );
        w
    }

    fn session(w: &Arc<FakeWorld>, llm: Option<Arc<FakeLlm>>) -> Session {
        let svc = Services {
            ui: Some(w.clone()),
            input: Some(w.clone()),
            launcher: Some(w.clone()),
            chat: llm.map(|l| l as Arc<dyn screenit_tutor::LlmChat>),
            pace: FakePace::new(),
            recorder: Some(Box::new(|| {
                Box::new(screenit_testkit::FakeRecorder::default())
            })),
            video: Some(Arc::new(Reader)),
            video_writer: Some(Arc::new(screenit_testkit::FakeVideoWriter)),
            ..Services::default()
        };
        Session::new(svc)
    }

    fn done(s: &mut Session) -> Value {
        let st = s
            .dispatch("tutor.wait", &json!({"timeout_ms": 20000}))
            .unwrap();
        assert_eq!(st["running"], false, "{st}");
        st
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("scit-tutor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn snapshot_lists_controls() {
        let w = world();
        let mut s = session(&w, None);
        let r = s
            .dispatch("ui.snapshot", &json!({"title": "notes"}))
            .unwrap();
        let text = r["text"].as_str().unwrap();
        assert!(
            text.contains("\"New\"") && text.contains("Button"),
            "{text}"
        );
        assert!(
            s.dispatch("ui.snapshot", &json!({"title": "nope"}))
                .is_err()
        );
        assert!(s.dispatch("ui.snapshot", &json!({"bogus": 1})).is_err());
        assert!(
            Session::new(Services::default())
                .dispatch("ui.snapshot", &json!({}))
                .is_err()
        );
    }

    #[test]
    fn input_needs_a_named_focused_target() {
        let w = world();
        let mut s = session(&w, None);
        assert!(
            s.dispatch("input.click", &json!({"x": 1, "y": 1})).is_err(),
            "no window"
        );
        let ok = s.dispatch(
            "input.click",
            &json!({"title": "notes", "x": 120, "y": 120}),
        );
        assert!(ok.is_ok(), "{ok:?}");
        assert!(w.has("Editor"), "click reached the app");
        w.set_foreground_pid(5);
        let e = s
            .dispatch("input.key", &json!({"title": "notes", "keys": "ctrl+s"}))
            .unwrap_err();
        assert!(e.to_string().contains("keyboard focus"), "{e}");
        w.set_foreground_pid(FakeWorld::PID);
        for bad in [
            (
                "input.click",
                json!({"title": "notes", "x": 1, "y": 1, "button": "side"}),
            ),
            (
                "input.click",
                json!({"title": "notes", "x": 1, "y": 1, "count": 9}),
            ),
            ("input.type", json!({"title": "notes", "text": ""})),
            ("input.key", json!({"title": "notes", "keys": " "})),
            (
                "input.scroll",
                json!({"title": "notes", "x": 1, "y": 1, "dy": 5000}),
            ),
            ("input.move", json!({"title": "notes", "x": 1})),
        ] {
            assert!(s.dispatch(bad.0, &bad.1).is_err(), "{}", bad.0);
        }
        w.set_interrupted(true);
        assert!(
            s.dispatch("input.move", &json!({"title": "notes", "x": 1, "y": 1}))
                .is_err()
        );
    }

    #[test]
    fn typing_into_a_password_field_is_refused() {
        let w = world();
        let mut s = session(&w, None);
        s.dispatch(
            "input.click",
            &json!({"title": "notes", "x": 150, "y": 460}),
        )
        .unwrap();
        let e = s
            .dispatch("input.type", &json!({"title": "notes", "text": "hunter2"}))
            .unwrap_err();
        assert!(e.to_string().contains("password"), "{e}");
        assert!(!w.log().iter().any(|l| l.starts_with("type")));
    }

    #[test]
    fn app_launch_validates_and_returns_the_window() {
        let w = world();
        let mut s = session(&w, None);
        let r = s
            .dispatch("app.launch", &json!({"target": "notes.exe"}))
            .unwrap();
        assert_eq!(r["pid"], FakeWorld::PID);
        assert_eq!(w.launched(), vec!["notes.exe"]);
        assert!(s.dispatch("app.launch", &json!({"target": ""})).is_err());
        assert!(s.dispatch("app.launch", &json!({})).is_err());
    }

    #[test]
    fn script_get_set_save_load() {
        let w = world();
        let mut s = session(&w, None);
        assert!(s.dispatch("tutor.script.get", &json!({})).is_err());
        let v: Value = serde_json::from_str(SCRIPT).unwrap();
        let r = s
            .dispatch("tutor.script.set", &json!({"script": v}))
            .unwrap();
        assert_eq!(r["steps"], 3);
        assert_eq!(
            s.dispatch("tutor.script.get", &json!({})).unwrap()["title"],
            "Notes basics"
        );
        let dir = tmp("save");
        let file = dir.join("s.json");
        s.dispatch("tutor.save", &json!({"path": file})).unwrap();
        let mut s2 = session(&w, None);
        s2.dispatch("tutor.load", &json!({"path": file})).unwrap();
        assert_eq!(
            s2.dispatch("tutor.status", &json!({})).unwrap()["chapters"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(
            s2.dispatch("tutor.load", &json!({"path": dir.join("none.json")}))
                .is_err()
        );
        assert!(
            s.dispatch(
                "tutor.script.set",
                &json!({"script": {"title": "x", "chapters": []}})
            )
            .is_err()
        );
        let mut edited = s.dispatch("tutor.script.get", &json!({})).unwrap();
        edited["chapters"][0]["checked"] = json!(true);
        s.dispatch(
            "tutor.script.set",
            &json!({"script": edited.clone(), "keep_checked": true}),
        )
        .unwrap();
        assert_eq!(
            s.dispatch("tutor.status", &json!({})).unwrap()["chapters"][0]["checked"],
            true
        );
        s.dispatch("tutor.script.set", &json!({"script": edited}))
            .unwrap();
        assert_eq!(
            s.dispatch("tutor.status", &json!({})).unwrap()["chapters"][0]["checked"],
            false
        );
        assert!(s.dispatch("tutor.script.set", &json!({})).is_err());
        assert!(
            s.dispatch("tutor.script.set", &json!({"script": 5}))
                .is_err()
        );
        assert!(s2.dispatch("tutor.save", &json!({})).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plan_asks_the_model_and_retries_once() {
        let w = world();
        let llm = FakeLlm::new(vec![Ok("sorry, no json".into()), Ok(SCRIPT.into())]);
        let mut s = session(&w, Some(llm.clone()));
        s.dispatch(
            "tutor.plan",
            &json!({"goal": "write a note", "window_title": "notes"}),
        )
        .unwrap();
        let st = done(&mut s);
        assert_eq!(st["error"], Value::Null, "{st}");
        assert_eq!(st["result"]["steps"], 3);
        assert_eq!(llm.calls().len(), 2);
        assert!(llm.calls()[1].last().unwrap().content.contains("rejected"));
        assert_eq!(
            s.dispatch("tutor.script.get", &json!({})).unwrap()["goal"],
            "write a note"
        );
    }

    #[test]
    fn explore_waits_for_the_users_answer() {
        let w = world();
        let llm = FakeLlm::new(vec![
            Ok(r#"{"title":"Notes","chapters":[{"title":"Write","goal":"editor open"}]}"#.into()),
            Ok(r#"{"ask":"Which button starts a note?"}"#.into()),
            Ok(r#"{"narration":"Open a new note.","do":"click","id":1}"#.into()),
            Ok(r#"{"done":true}"#.into()),
        ]);
        let mut s = session(&w, Some(llm));
        assert!(
            s.dispatch("tutor.answer", &json!({"text": "x"})).is_err(),
            "nothing to answer yet"
        );
        let path = std::env::temp_dir().join(format!("scit-plan-{}.json", std::process::id()));
        s.dispatch(
            "tutor.explore",
            &json!({"goal": "write a note", "window_title": "notes", "save": path}),
        )
        .unwrap();
        let mut asked = Value::Null;
        for _ in 0..200 {
            let st = s.dispatch("tutor.status", &json!({})).unwrap();
            if !st["question"].is_null() {
                asked = st["question"].clone();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(asked, "Which button starts a note?");
        assert!(
            s.dispatch("tutor.answer", &json!({"action": "nope"}))
                .is_err()
        );
        s.dispatch("tutor.answer", &json!({"text": "the New button"}))
            .unwrap();
        let st = done(&mut s);
        assert_eq!(st["error"], Value::Null, "{st}");
        assert_eq!(st["result"]["steps"], 1);
        assert_eq!(st["last_phase"], "exploring");
        let sc = s.dispatch("tutor.script.get", &json!({})).unwrap();
        assert_eq!(sc["chapters"][0]["checked"], true);
        assert!(
            st["result"]["script"]
                .as_str()
                .is_some_and(|p| p.ends_with(".json")),
            "{}",
            st["result"]
        );
        let saved = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(saved.contains("Write"), "{saved}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn plan_reports_bad_input_and_model_failure() {
        let w = world();
        let mut s = session(&w, None);
        let e = s
            .dispatch("tutor.plan", &json!({"goal": "g", "window_title": "notes"}))
            .unwrap_err();
        assert!(matches!(e, CommandError::Disabled(_)));
        let mut s = session(&w, Some(FakeLlm::new(vec![])));
        assert!(
            s.dispatch("tutor.plan", &json!({"goal": "", "window_title": "n"}))
                .is_err()
        );
        s.dispatch("tutor.plan", &json!({"goal": "g"})).unwrap();
        let open = done(&mut s);
        assert!(
            open["error"]
                .as_str()
                .unwrap_or("")
                .contains("language model"),
            "{open}"
        );
        w.set_alive(false);
        assert!(s.dispatch("tutor.plan", &json!({"goal": "g"})).is_err());
        w.set_alive(true);
        s.dispatch("tutor.plan", &json!({"goal": "g", "window_title": "notes"}))
            .unwrap();
        let st = done(&mut s);
        assert!(
            st["error"].as_str().unwrap().contains("language model"),
            "{st}"
        );
    }

    #[test]
    fn rehearse_perform_produce_end_to_end() {
        let w = world();
        let mut s = session(&w, None);
        let mut v: Value = serde_json::from_str(SCRIPT).unwrap();
        v["app"] = json!({"window_title": "notes"});
        s.dispatch("tutor.script.set", &json!({"script": v}))
            .unwrap();
        let dir = tmp("e2e");
        // performing before rehearsal is refused
        let e = s
            .dispatch("tutor.perform", &json!({"output_dir": dir}))
            .unwrap_err();
        assert!(e.to_string().contains("rehearsed"), "{e}");
        assert!(
            s.dispatch("tutor.produce", &json!({"output": dir.join("a.mp4")}))
                .is_err()
        );

        s.dispatch("tutor.rehearse", &json!({"use_llm": false}))
            .unwrap();
        let st = done(&mut s);
        assert_eq!(st["error"], Value::Null, "{st}");
        assert_eq!(st["result"]["chapters_checked"], 2);
        assert!(
            st["chapters"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["checked"] == true)
        );

        s.dispatch(
            "tutor.perform",
            &json!({"output_dir": dir, "name": "Notes demo!"}),
        )
        .unwrap();
        let st = done(&mut s);
        assert_eq!(st["error"], Value::Null, "{st}");
        assert_eq!(st["performed"], 2);
        assert!(dir.join("Notes_demo_.tutor.json").is_file());

        let out = dir.join("final.mp4");
        s.dispatch("tutor.produce", &json!({"output": out, "fps": 10}))
            .unwrap();
        let st = done(&mut s);
        assert_eq!(st["error"], Value::Null, "{st}");
        let r = &st["result"];
        assert!(r["frames"].as_u64().unwrap() > 20, "{r}");
        assert!(r["chapters"].as_str().unwrap().contains("Write"));
        let srt = std::fs::read_to_string(dir.join("final.srt")).unwrap();
        assert!(srt.contains("Open a new note."), "{srt}");

        // produce again from the saved sidecar in a fresh session
        let mut s2 = session(&w, None);
        s2.dispatch(
            "tutor.produce",
            &json!({"output": dir.join("b.mp4"), "performance": dir.join("Notes_demo_.tutor.json"), "captions": false, "zoom_factor": 2.0}),
        )
        .unwrap();
        assert_eq!(done(&mut s2)["error"], Value::Null);
        assert!(!dir.join("b.srt").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rehearse_failure_is_reported_and_progress_kept() {
        let w = world();
        let mut s = session(&w, None);
        let mut v: Value = serde_json::from_str(SCRIPT).unwrap();
        v["app"] = json!({"window_title": "notes"});
        v["chapters"][1]["steps"][0]["action"]["target"]["name"] = json!("Store");
        v["chapters"][1]["steps"][0]["timeout_ms"] = json!(300);
        s.dispatch("tutor.script.set", &json!({"script": v}))
            .unwrap();
        s.dispatch("tutor.rehearse", &json!({"use_llm": false}))
            .unwrap();
        let st = done(&mut s);
        assert!(
            st["error"]
                .as_str()
                .unwrap()
                .contains("chapter 2 step c2s1"),
            "{st}"
        );
        assert_eq!(st["chapters"][0]["checked"], true);
        assert_eq!(st["chapters"][1]["checked"], false);
        assert!(!st["events"].as_array().unwrap().is_empty());
        for bad in [
            json!({"from_chapter": 9}),
            json!({"max_repairs": "x"}),
            json!({"nope": 1}),
        ] {
            assert!(s.dispatch("tutor.rehearse", &bad).is_err());
        }
    }

    #[test]
    fn perform_and_produce_validate_params() {
        let w = world();
        let mut s = session(&w, None);
        let mut v: Value = serde_json::from_str(SCRIPT).unwrap();
        v["app"] = json!({"window_title": "notes"});
        s.dispatch("tutor.script.set", &json!({"script": v}))
            .unwrap();
        assert!(s.dispatch("tutor.perform", &json!({})).is_err());
        assert!(
            s.dispatch(
                "tutor.perform",
                &json!({"output_dir": "x", "chapters": [9]})
            )
            .is_err()
        );
        assert!(
            s.dispatch(
                "tutor.perform",
                &json!({"output_dir": "x", "chapters": ["a"]})
            )
            .is_err()
        );
        assert!(
            s.dispatch("tutor.produce", &json!({"output": "a.txt"}))
                .is_err()
        );
        assert!(
            s.dispatch(
                "tutor.produce",
                &json!({"output": "a.mp4", "zoom_factor": 9})
            )
            .is_err()
        );
        assert!(
            s.dispatch("tutor.wait", &json!({"timeout_ms": -1}))
                .is_err()
        );
        assert_eq!(
            s.dispatch("tutor.abort", &json!({})).unwrap()["abort"],
            true
        );
    }
}
