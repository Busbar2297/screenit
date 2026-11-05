//! End-to-end AI tutorial run on the real desktop: a genuine Win32 form is rehearsed through UI
//! Automation and SendInput, performed under the Media Foundation recorder, and produced into one
//! video with chapter cards, zoom and captions. Needs an interactive desktop and moves the mouse:
//! `cargo xtask ci --e2e`.
#![cfg(all(windows, feature = "e2e"))]

use screenit_engine::Session;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

const SCRIPT: &str = r#"{"title":"Form basics","app":{"window_title":"Form Fixture"},"chapters":[
 {"title":"Add an item","steps":[
  {"narration":"Start with an empty list.","action":{"type":"click","target":{"role":"Button","name":"Clear"}},
   "expect":[{"type":"appears","target":{"name":"Cleared"}}]},
  {"narration":"Click the item box and type a name.","action":{"type":"type","target":{"automation_id":"101"},"text":"apples"},
   "expect":[{"type":"value_contains","target":{"automation_id":"101"},"value":"apples"}]},
  {"narration":"Press Add.","action":{"type":"click","target":{"role":"Button","name":"Add"}},
   "expect":[{"type":"appears","target":{"name":"Added: apples"}}]}]},
 {"title":"Use the Uppercase option","steps":[
  {"narration":"Turn on Uppercase.","action":{"type":"click","target":{"name":"Uppercase"}}},
  {"narration":"Type another item.","action":{"type":"type","target":{"automation_id":"101"},"text":"pears"},
   "expect":[{"type":"value_contains","target":{"automation_id":"101"},"value":"pears"}]},
  {"narration":"Add it: it is stored in capitals.","action":{"type":"click","target":{"role":"Button","name":"Add"}},
   "expect":[{"type":"appears","target":{"name":"Added: PEARS"}}]},
  {"narration":"Turn Uppercase off again.","action":{"type":"click","target":{"name":"Uppercase"}}}]}]}"#;

fn wait_done(s: &mut Session) -> Value {
    let t0 = std::time::Instant::now();
    for _ in 0..40 {
        let st = s
            .dispatch("tutor.wait", &json!({"timeout_ms": 10000}))
            .expect("wait");
        if st["running"] == false {
            eprintln!("job finished in {:?}", t0.elapsed());
            return st;
        }
    }
    panic!("tutorial job did not finish");
}

fn launch_form() -> (std::process::Child, std::process::ChildStdin) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_screenit-fixture"))
        .args(["--form", "150", "120", "520", "380"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn fixture");
    let stdin = child.stdin.take().expect("stdin");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("READY");
    assert!(line.starts_with("READY"), "{line}");
    std::thread::sleep(std::time::Duration::from_millis(800));
    (child, stdin)
}

#[test]
#[ignore = "needs an interactive desktop; run via cargo xtask ci --e2e"]
fn ui_snapshot_is_quick() {
    let (mut child, _stdin) = launch_form();
    let mut s = Session::new(screenit_platform::services());
    for _ in 0..3 {
        let t0 = std::time::Instant::now();
        let r = s
            .dispatch("ui.snapshot", &json!({"title": "Form Fixture"}))
            .expect("snapshot");
        let took = t0.elapsed();
        eprintln!("snapshot of {} elements took {took:?}", r["elements"]);
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }
    let _ = child.kill();
}

#[test]
#[ignore = "needs an interactive desktop; run via cargo xtask ci --e2e"]
fn form_tutorial_rehearse_perform_produce() {
    let (mut child, _stdin) = launch_form();
    let mut s = Session::new(screenit_platform::services());
    let snap = s
        .dispatch("ui.snapshot", &json!({"title": "Form Fixture"}))
        .expect("snapshot");
    let text = snap["text"].as_str().unwrap_or("");
    assert!(text.contains("Add") && text.contains("Uppercase"), "{text}");

    let dir = std::env::temp_dir().join(format!("scit-tutor-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let script: Value = serde_json::from_str(SCRIPT).expect("script json");
    s.dispatch("tutor.script.set", &json!({"script": script}))
        .expect("set");

    s.dispatch("tutor.rehearse", &json!({"use_llm": false}))
        .expect("rehearse");
    let st = wait_done(&mut s);
    assert_eq!(st["error"], Value::Null, "rehearsal failed: {st}");

    s.dispatch("tutor.perform", &json!({"output_dir": dir, "name": "form"}))
        .expect("perform");
    let st = wait_done(&mut s);
    assert_eq!(st["error"], Value::Null, "performance failed: {st}");
    for n in ["form_ch01.mp4", "form_ch02.mp4", "form.tutor.json"] {
        let len = std::fs::metadata(dir.join(n)).map(|m| m.len()).unwrap_or(0);
        assert!(len > 500, "{n} is {len} bytes");
    }

    let out = dir.join("final.mp4");
    s.dispatch("tutor.produce", &json!({"output": out, "fps": 15}))
        .expect("produce");
    let st = wait_done(&mut s);
    assert_eq!(st["error"], Value::Null, "production failed: {st}");
    let len = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    assert!(len > 5_000, "final video is {len} bytes");
    let srt = std::fs::read_to_string(dir.join("final.srt")).expect("srt");
    assert!(srt.contains("Press Add."), "{srt}");

    let _ = child.kill();
    let _ = std::fs::remove_dir_all(dir);
}
