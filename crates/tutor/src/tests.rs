use crate::fakes::{Effect, FakeChapterRecorder, FakeLlm, FakePace, FakeWorld, Trigger, element};
use crate::perform::{Performance, Stage, perform};
use crate::player::{Mode, Player};
use crate::rehearse::{RehearseOpts, rehearse};
use crate::safety::Guard;
use crate::script::{AppTarget, TutorialScript};
use crate::{ChatOptions, LlmChat, Pace, Rig, TutorError};
use screenit_geom::Rect;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

fn app() -> (Arc<FakeWorld>, Rig, Arc<FakePace>) {
    let w = FakeWorld::new("Notes");
    w.add(element("Button", "New", Rect::new(110, 110, 60, 24)));
    w.add(element("Button", "Save", Rect::new(180, 110, 60, 24)));
    w.add(element("Edit", "Password", Rect::new(110, 400, 200, 24)));
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
    w.rule(
        Trigger::Key("ctrl+d".into()),
        vec![Effect::Hide("Editor".into())],
    );
    let pace = FakePace::new();
    let rig = Rig {
        ui: w.clone(),
        input: w.clone(),
        launcher: w.clone(),
        pace: pace.clone(),
    };
    (w, rig, pace)
}

const SCRIPT: &str = r#"{"title":"Notes basics","goal":"save a note","chapters":[
 {"title":"Write","steps":[
  {"narration":"Open a new note.","action":{"type":"click","target":{"role":"Button","name":"New"}},
   "expect":[{"type":"appears","target":{"role":"Edit","name":"Editor"}}]},
  {"narration":"Type your text.","action":{"type":"type","target":{"role":"Edit","name":"Editor"},"text":"hello"},
   "expect":[{"type":"value_contains","target":{"name":"Editor"},"value":"hello"}]}]},
 {"title":"Save","steps":[
  {"narration":"Press Save.","action":{"type":"click","target":{"role":"Button","name":"Save"}},
   "expect":[{"type":"appears","target":{"name":"Saved"}}]}]}]}"#;

fn script() -> TutorialScript {
    let mut s: TutorialScript = serde_json::from_str(SCRIPT).unwrap();
    s.app = AppTarget {
        window_title: "notes".into(),
        ..AppTarget::default()
    };
    s.validate().unwrap();
    s
}

fn guard() -> Guard {
    Guard::new(FakeWorld::PID, 7, Arc::new(AtomicBool::new(false)))
}

fn win(rig: &Rig) -> crate::WindowInfo {
    rig.window(&script().app).unwrap()
}

fn opts(chat: Option<&dyn LlmChat>) -> RehearseOpts<'_> {
    RehearseOpts {
        from_chapter: 0,
        max_repairs: 2,
        chat: chat.map(|c| (c, ChatOptions::default())),
    }
}

#[test]
fn rehearsal_checks_every_step_and_pins_rects() {
    let (w, rig, _) = app();
    let mut s = script();
    let mut events = Vec::new();
    let r = rehearse(&mut s, &rig, guard(), win(&rig), &opts(None), &mut |e| {
        events.push(e)
    })
    .unwrap();
    assert_eq!((r.steps_ok, r.repairs, r.chapters_checked), (3, 0, 2));
    assert!(s.chapters.iter().all(|c| c.checked));
    assert_eq!(
        s.chapters[0].steps[0].verified_rect,
        Some(Rect::new(110, 110, 60, 24))
    );
    assert_eq!(w.value_of("Editor").as_deref(), Some("hello"));
    assert!(w.has("Saved"));
    assert_eq!(events.len(), 2 + 3 + 2);
}

#[test]
fn failing_step_without_a_model_reports_where() {
    let (_, rig, _) = app();
    let mut s = script();
    s.chapters[1].steps[0].timeout_ms = 300;
    s.chapters[1].steps[0].action =
        serde_json::from_str(r#"{"type":"click","target":{"role":"Button","name":"Store"}}"#)
            .unwrap();
    let e = rehearse(&mut s, &rig, guard(), win(&rig), &opts(None), &mut |_| {}).unwrap_err();
    let TutorError::Step(m) = e else {
        panic!("{e:?}")
    };
    assert!(m.contains("chapter 2 step c2s1"), "{m}");
    assert!(s.chapters[0].checked);
    assert!(!s.chapters[1].checked);
}

#[test]
fn the_model_repairs_a_failed_step_and_gets_a_small_prompt() {
    let (w, rig, _) = app();
    let mut s = script();
    s.chapters[1].steps[0].timeout_ms = 300;
    s.chapters[1].steps[0].action =
        serde_json::from_str(r#"{"type":"click","target":{"role":"Button","name":"Store"}}"#)
            .unwrap();
    let fix = r#"```json
{"narration":"Press Save.","action":{"type":"click","target":{"role":"Button","name":"Save"}},
 "expect":[{"type":"appears","target":{"name":"Saved"}}]}
```"#;
    let llm = FakeLlm::new(vec![Ok(fix.into())]);
    let r = rehearse(
        &mut s,
        &rig,
        guard(),
        win(&rig),
        &opts(Some(&*llm)),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(r.repairs, 1);
    assert!(w.has("Saved"));
    let calls = llm.calls();
    assert_eq!(calls.len(), 1);
    let prompt = &calls[0][1].content;
    assert!(prompt.contains("Store") && prompt.contains("#"), "{prompt}");
    assert!(prompt.len() < 6000);
}

#[test]
fn repairs_are_bounded() {
    let (_, rig, _) = app();
    let mut s = script();
    s.chapters[0].steps[0].timeout_ms = 300;
    s.chapters[0].steps[0].action =
        serde_json::from_str(r#"{"type":"click","target":{"role":"Button","name":"Nope"}}"#)
            .unwrap();
    let bad = r#"{"action":{"type":"click","target":{"role":"Button","name":"Still nope"}},"timeout_ms":300}"#;
    let llm = FakeLlm::new(vec![Ok(bad.into()), Ok(bad.into()), Ok(bad.into())]);
    let e = rehearse(
        &mut s,
        &rig,
        guard(),
        win(&rig),
        &opts(Some(&*llm)),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(e.to_string().contains("gave up after 2 repairs"), "{e}");
    assert_eq!(llm.calls().len(), 2);
    let silent = FakeLlm::new(vec![]);
    let e = rehearse(
        &mut s,
        &rig,
        guard(),
        win(&rig),
        &opts(Some(&*silent)),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(e, TutorError::Llm(_)), "{e:?}");
}

#[test]
fn input_only_reaches_the_target_app() {
    let (w, rig, _) = app();
    w.set_foreground_pid(1);
    let mut s = script();
    let e = rehearse(&mut s, &rig, guard(), win(&rig), &opts(None), &mut |_| {}).unwrap_err();
    assert!(matches!(e, TutorError::Safety(_)), "{e:?}");
    assert!(w.log().is_empty(), "{:?}", w.log());
    w.set_foreground_pid(FakeWorld::PID);
    w.set_alive(false);
    assert!(matches!(
        rehearse(
            &mut s,
            &rig,
            guard(),
            crate::WindowInfo {
                hwnd: 7,
                ..Default::default()
            },
            &opts(None),
            &mut |_| {}
        ),
        Err(TutorError::Safety(_))
    ));
}

#[test]
fn user_interrupt_and_abort_flag_stop_the_run() {
    let (w, rig, _) = app();
    let mut s = script();
    w.set_interrupted(true);
    let e = rehearse(&mut s, &rig, guard(), win(&rig), &opts(None), &mut |_| {}).unwrap_err();
    assert!(matches!(e, TutorError::Aborted(_)), "{e:?}");
    assert!(w.log().is_empty());
    w.set_interrupted(false);
    let g = guard();
    g.abort.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        rehearse(&mut s, &rig, g, win(&rig), &opts(None), &mut |_| {}),
        Err(TutorError::Aborted(_))
    ));
}

#[test]
fn password_fields_are_never_typed_into() {
    let (w, rig, _) = app();
    let mut e = element("Edit", "Secret", Rect::new(110, 450, 200, 24));
    e.password = true;
    w.add(e);
    let mut s: TutorialScript = serde_json::from_str(
        r#"{"title":"t","chapters":[{"title":"c","steps":[
        {"action":{"type":"type","target":{"name":"Secret"},"text":"hunter2"}}]}]}"#,
    )
    .unwrap();
    s.validate().unwrap();
    let e = rehearse(&mut s, &rig, guard(), win(&rig), &opts(None), &mut |_| {}).unwrap_err();
    assert!(matches!(e, TutorError::Safety(_)), "{e:?}");
    assert!(!w.log().iter().any(|l| l.starts_with("type")));
    // snapshot text for the model never shows password values
    let snap = rig.ui.snapshot(7).unwrap();
    assert!(!snap.compact(50).contains("hunter2"));
}

#[test]
fn destructive_steps_need_confirmation() {
    let (w, rig, _) = app();
    let mut s: TutorialScript = serde_json::from_str(
        r#"{"title":"t","chapters":[{"title":"c","steps":[
        {"destructive":true,"action":{"type":"click","target":{"name":"Save"}}}]}]}"#,
    )
    .unwrap();
    s.validate().unwrap();
    assert!(matches!(
        rehearse(&mut s, &rig, guard(), win(&rig), &opts(None), &mut |_| {}),
        Err(TutorError::Safety(_))
    ));
    assert!(w.log().is_empty());
    let mut g = guard();
    g.allow_destructive = true;
    assert!(rehearse(&mut s, &rig, g, win(&rig), &opts(None), &mut |_| {}).is_ok());
}

#[test]
fn budget_limits_actions() {
    let (_, rig, _) = app();
    let mut s = script();
    let mut g = guard();
    g.budget = 2;
    let e = rehearse(&mut s, &rig, g, win(&rig), &opts(None), &mut |_| {}).unwrap_err();
    assert!(matches!(e, TutorError::Safety(_)), "{e:?}");
}

#[test]
fn performance_records_each_chapter_with_markers() {
    let (w, rig, pace) = app();
    let mut s = script();
    let err = {
        let mut rec = FakeChapterRecorder::new(pace.clone());
        let mut out = Performance::default();
        perform(
            &s,
            Stage {
                rig: &rig,
                guard: guard(),
                window: win(&rig),
            },
            &mut rec,
            None,
            &mut out,
            &mut |_| {},
        )
        .unwrap_err()
    };
    assert!(err.to_string().contains("rehearsed"), "{err}");
    rehearse(&mut s, &rig, guard(), win(&rig), &opts(None), &mut |_| {}).unwrap();
    // reset the fake app by looking only at relative effects: Editor/Saved already exist, steps still pass
    let mut rec = FakeChapterRecorder::new(pace.clone());
    let mut out = Performance::default();
    let before = pace.now_ms();
    let mut seen = Vec::new();
    perform(
        &s,
        Stage {
            rig: &rig,
            guard: guard(),
            window: win(&rig),
        },
        &mut rec,
        None,
        &mut out,
        &mut |c| seen.push(c),
    )
    .unwrap();
    assert_eq!(seen, vec![0, 1]);
    assert_eq!(
        rec.log,
        vec!["start 0 800x600", "stop 0", "start 1 800x600", "stop 1"]
    );
    assert_eq!(out.takes.len(), 2);
    assert_eq!(out.region, Rect::new(100, 100, 800, 600));
    let t0 = &out.takes[0];
    assert_eq!(t0.markers.len(), 2);
    let m = &t0.markers[0];
    assert!(m.start_ms >= 700, "lead-in: {}", m.start_ms);
    assert!(m.start_ms <= m.act_ms && m.act_ms <= m.end_ms);
    assert!(m.end_ms - m.start_ms >= crate::player::narration_ms("Open a new note."));
    assert_eq!(t0.markers[1].text, "hello");
    assert!(t0.duration_ms >= t0.markers[1].end_ms + 900);
    assert!(pace.now_ms() > before + 4000);
    assert!(
        w.log().iter().filter(|l| l.starts_with("move")).count() > 20,
        "human-paced cursor path"
    );
    // only chapter 2
    let mut again = out.clone();
    let mut rec = FakeChapterRecorder::new(pace.clone());
    perform(
        &s,
        Stage {
            rig: &rig,
            guard: guard(),
            window: win(&rig),
        },
        &mut rec,
        Some(&[1]),
        &mut again,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(rec.log.len(), 2);
    assert_eq!(again.takes.len(), 2);
}

#[test]
fn recorder_is_stopped_when_a_step_fails() {
    let (w, rig, pace) = app();
    let mut s = script();
    rehearse(&mut s, &rig, guard(), win(&rig), &opts(None), &mut |_| {}).unwrap();
    w.set_foreground_pid(9);
    let mut rec = FakeChapterRecorder::new(pace);
    let mut out = Performance::default();
    let e = perform(
        &s,
        Stage {
            rig: &rig,
            guard: guard(),
            window: win(&rig),
        },
        &mut rec,
        None,
        &mut out,
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(e, TutorError::Safety(_)));
    assert_eq!(rec.log, vec!["start 0 800x600", "stop 0"]);
    assert!(out.takes.is_empty());
}

#[test]
fn a_bare_app_name_uses_the_open_window() {
    let (w, rig, _) = app();
    let info = rig
        .window(&AppTarget {
            launch: "notes".into(),
            ..AppTarget::default()
        })
        .unwrap();
    assert_eq!(info.title, "Notes");
    assert!(w.launched().is_empty());
}

#[test]
fn wait_and_key_steps_work() {
    let (w, rig, _) = app();
    let mut s: TutorialScript = serde_json::from_str(
        r#"{"title":"t","chapters":[{"title":"c","steps":[
        {"action":{"type":"click","target":{"name":"New"}}},
        {"action":{"type":"wait","ms":500}},
        {"action":{"type":"key","keys":"ctrl+d"},"expect":[{"type":"disappears","target":{"name":"Editor"}}]}]}]}"#,
    )
    .unwrap();
    s.validate().unwrap();
    let mut p = Player::new(&rig, guard(), win(&rig), Mode::Rehearse);
    for st in &s.chapters[0].steps {
        p.run_step(st).unwrap();
    }
    assert!(!w.has("Editor"));
}
