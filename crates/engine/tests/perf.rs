//! Performance budgets (see perf/budgets.toml). Generous bounds so CI noise doesn't flake;
//! they exist to catch order-of-magnitude regressions.

use screenit_testkit::fake_session;
use serde_json::json;
use std::time::Instant;

fn timed(label: &str, budget_ms: u128, f: impl FnOnce()) {
    let t = Instant::now();
    f();
    let ms = t.elapsed().as_millis();
    assert!(
        ms <= budget_ms,
        "{label}: {ms}ms exceeds budget {budget_ms}ms"
    );
}

#[test]
fn capture_and_flatten_budgets() {
    let (mut s, _) = fake_session(1920, 1080);
    timed("capture fullscreen 1080p", 500, || {
        s.dispatch("capture.fullscreen", &json!({})).unwrap();
    });
    for i in 0..50 {
        s.dispatch("editor.rect.add", &json!({"rect":[i * 10, i * 5, 100, 60]}))
            .unwrap();
    }
    timed("export 1080p png with 50 objects", 4000, || {
        let p = std::env::temp_dir().join("scit-perf.png");
        s.dispatch("doc.export", &json!({"path": p})).unwrap();
        std::fs::remove_file(p).ok();
    });
    timed("100 undo/redo steps", 3000, || {
        for _ in 0..50 {
            s.dispatch("edit.undo", &json!({})).unwrap();
        }
        for _ in 0..50 {
            s.dispatch("edit.redo", &json!({})).unwrap();
        }
    });
}
