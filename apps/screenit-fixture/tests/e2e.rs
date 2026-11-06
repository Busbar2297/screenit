//! End-to-end capture against the real desktop: launch the fixture window (deterministic
//! pattern) and capture it through the real Win32 platform layer and the engine commands.
//! Needs an interactive desktop session: `cargo xtask ci --e2e`.
#![cfg(all(windows, feature = "e2e"))]

use screenit_engine::Session;
use screenit_testkit::pattern;
use serde_json::json;
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdin, Command, Stdio};

/// The desktop and the mouse cursor are shared: e2e tests that use them run one at a time.
static DESKTOP: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Fixture {
    _lock: std::sync::MutexGuard<'static, ()>,
    child: Child,
    _stdin: ChildStdin,
    id: u64,
    rect: (i32, i32, u32, u32),
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn launch(x: i32, y: i32, w: u32, h: u32) -> Fixture {
    launch_with(x, y, w, h, &[])
}

fn launch_with(x: i32, y: i32, w: u32, h: u32, extra: &[&str]) -> Fixture {
    let lock = DESKTOP.lock().unwrap_or_else(|e| e.into_inner());
    let mut child = Command::new(env!("CARGO_BIN_EXE_screenit-fixture"))
        .args([x.to_string(), y.to_string(), w.to_string(), h.to_string()])
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn fixture");
    let stdin = child.stdin.take().expect("stdin");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("READY line");
    let f: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(f.first(), Some(&"READY"), "fixture said {line:?}");
    std::thread::sleep(std::time::Duration::from_millis(400)); // let DWM present the first frame
    Fixture {
        _lock: lock,
        child,
        _stdin: stdin,
        id: f[1].parse().expect("hwnd"),
        rect: (x, y, w, h),
    }
}

fn session() -> Session {
    Session::new(screenit_platform::services())
}

#[test]
#[ignore = "needs an interactive desktop; run via cargo xtask ci --e2e"]
fn region_capture_matches_the_fixture_pattern_exactly() {
    let fx = launch(120, 120, 320, 200);
    let mut s = session();
    let (x, y, w, h) = fx.rect;
    let want = pattern(w, h);
    // DWM may take a few frames to present the window; poll up to 5 s.
    let mut diff = f64::MAX;
    for _ in 0..50 {
        s.dispatch("capture.region", &json!({"x": x, "y": y, "w": w, "h": h}))
            .expect("capture");
        diff = base(&s).mean_abs_diff(&want).expect("same size");
        if diff < 0.5 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        diff < 0.5,
        "captured pixels differ from the fixture pattern (mean abs diff {diff})"
    );
}

#[test]
#[ignore = "needs an interactive desktop; run via cargo xtask ci --e2e"]
fn window_capture_finds_the_fixture_window() {
    let fx = launch(160, 140, 300, 180);
    let mut s = session();
    let wins = s.dispatch("capture.windows", &json!({})).expect("windows");
    let found = wins["windows"]
        .as_array()
        .map(|a| a.iter().any(|w| w["id"].as_u64() == Some(fx.id)))
        .unwrap_or(false);
    assert!(found, "fixture window {} not listed: {wins}", fx.id);
    s.dispatch("capture.window", &json!({"id": fx.id}))
        .expect("window capture");
    let info = s.dispatch("doc.info", &json!({})).expect("info");
    assert_eq!(info["width"], 300);
    assert_eq!(info["height"], 180);
}

#[test]
#[ignore = "needs an interactive desktop; run via cargo xtask ci --e2e"]
fn monitors_and_clipboard_work() {
    let svc = screenit_platform::services();
    let screen = svc.screen.expect("screen");
    let mons = screen.monitors().expect("monitors");
    assert!(mons.iter().any(|m| m.primary), "a primary monitor exists");
    let clip = svc.clipboard.expect("clipboard");
    let img = pattern(40, 30);
    clip.set_image(&img).expect("set");
    let back = clip.get_image().expect("get");
    assert_eq!((back.width(), back.height()), (40, 30));
    assert!(back.mean_abs_diff(&img).expect("same size") < 0.01);
}

fn base(s: &Session) -> screenit_raster::Raster {
    s.doc.as_ref().expect("document").base().clone()
}

#[test]
#[ignore = "needs an interactive desktop; run via cargo xtask ci --e2e"]
fn automatic_scrolling_capture_of_a_real_window() {
    let fx = launch_with(200, 150, 300, 200, &["--scroll=4"]);
    let mut s = session();
    let (x, y, w, h) = fx.rect;
    let r = s
        .dispatch("capture.scroll.auto", &json!({"x": x, "y": y, "w": w, "h": h, "max_pages": 20, "notches": 3, "settle_ms": 200}))
        .expect("scroll capture");
    assert_eq!(r["width"], 300, "{r}");
    assert_eq!(
        r["height"], 800,
        "stitched height must equal the full scrollable content: {r}"
    );
    let got = base(&s);
    let want = pattern(300, 800);
    assert!(got.mean_abs_diff(&want).expect("same size") < 0.5);
}

#[test]
#[ignore = "needs an interactive desktop; run via cargo xtask ci --e2e"]
fn object_capture_returns_the_element_under_the_point() {
    let fx = launch(220, 180, 260, 160);
    let mut s = session();
    let r = s
        .dispatch(
            "capture.object",
            &json!({"point": [fx.rect.0 + 50, fx.rect.1 + 50]}),
        )
        .expect("object capture");
    assert_eq!(
        (r["width"].as_u64(), r["height"].as_u64()),
        (Some(260), Some(160))
    );
}

#[test]
#[ignore = "needs an interactive desktop; run via cargo xtask ci --e2e"]
fn grab_text_reads_rendered_text() {
    let mut img = screenit_raster::Raster::new(900, 140, [255, 255, 255, 255]).expect("raster");
    screenit_vector::draw_text(
        &mut img,
        "Hello World 2026",
        screenit_geom::PointF::new(20.0, 30.0),
        72.0,
        screenit_doc::Color::BLACK,
    );
    let mut s = session();
    s.new_document(screenit_doc::CaptureDoc::new(img));
    let r = s
        .dispatch("capture.grab_text", &json!({"clipboard": false}))
        .expect("ocr");
    let text = r["text"].as_str().unwrap_or_default().to_lowercase();
    assert!(
        text.contains("hello") && text.contains("world"),
        "ocr text: {text:?}"
    );
}

#[test]
#[ignore = "needs an interactive desktop; run via cargo xtask ci --e2e"]
fn records_an_mp4_that_decodes_back_to_the_fixture() {
    let fx = launch(140, 140, 320, 200);
    let mut s = session();
    let (x, y, w, h) = fx.rect;
    std::thread::sleep(std::time::Duration::from_millis(800));
    let out = std::env::temp_dir().join("screenit-e2e-rec.mp4");
    let _ = std::fs::remove_file(&out);
    s.dispatch(
        "record.start",
        &json!({"x": x, "y": y, "w": w, "h": h, "fps": 15, "output": out.to_str().unwrap()}),
    )
    .expect("start");
    std::thread::sleep(std::time::Duration::from_millis(1000));
    s.dispatch("record.pause", &json!({})).expect("pause");
    std::thread::sleep(std::time::Duration::from_millis(500));
    s.dispatch("record.resume", &json!({})).expect("resume");
    std::thread::sleep(std::time::Duration::from_millis(1000));
    let r = s.dispatch("record.stop", &json!({})).expect("stop");
    let frames = r["result"]["frames"].as_u64().unwrap();
    assert!(frames >= 10, "too few frames: {frames}");
    let info = s
        .dispatch("video.probe", &json!({"path": out.to_str().unwrap()}))
        .expect("probe");
    assert_eq!(info["width"].as_u64(), Some(u64::from(w)));
    let dur = info["duration_ms"].as_u64().unwrap();
    assert!(
        (1200..3500).contains(&dur),
        "duration {dur} (pause must be excluded)"
    );
    s.dispatch(
        "video.frame",
        &json!({"path": out.to_str().unwrap(), "ms": 500}),
    )
    .expect("frame");
    // H.264 is lossy: allow a modest mean error against the exact pattern.
    let diff = base(&s).mean_abs_diff(&pattern(w, h)).expect("same size");
    assert!(
        diff < 12.0,
        "decoded frame too different from fixture: {diff}"
    );
    // The recording keeps an event log, and auto-polish renders it through the real encoder.
    let ev = s
        .dispatch("video.events.get", &json!({"path": out.to_str().unwrap()}))
        .expect("event log written beside the recording");
    assert!(ev["count"].as_u64().unwrap() >= 1, "{ev}");
    let polished = std::env::temp_dir().join("screenit-e2e-rec-polished.mp4");
    let _ = std::fs::remove_file(&polished);
    s.dispatch(
        "video.polish",
        &json!({"path": out.to_str().unwrap(), "output": polished.to_str().unwrap(),
                "fps": 15, "aspect": "square", "padding_pct": 6, "corner_pct": 10}),
    )
    .expect("polish");
    let st = s
        .dispatch("tutor.wait", &json!({"timeout_ms": 50000}))
        .expect("wait");
    assert!(st["error"].is_null(), "{st}");
    let pinfo = s
        .dispatch("video.probe", &json!({"path": polished.to_str().unwrap()}))
        .expect("probe polished");
    assert_eq!(pinfo["width"], pinfo["height"], "square output: {pinfo}");
}

#[test]
#[ignore = "needs an interactive desktop; run via cargo xtask ci --e2e"]
fn records_a_system_audio_track_that_decodes_upright() {
    let fx = launch(160, 160, 320, 200);
    let mut s = session();
    let (x, y, w, h) = fx.rect;
    std::thread::sleep(std::time::Duration::from_millis(800));
    let out = std::env::temp_dir().join("screenit-e2e-audio.mp4");
    let _ = std::fs::remove_file(&out);
    s.dispatch(
        "record.start",
        &json!({"x": x, "y": y, "w": w, "h": h, "fps": 15, "system_audio": true,
                "output": out.to_str().unwrap()}),
    )
    .expect("start (needs an audio output device)");
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let r = s.dispatch("record.stop", &json!({})).expect("stop");
    assert_eq!(r["result"]["has_audio"], true);
    let info = s
        .dispatch("video.probe", &json!({"path": out.to_str().unwrap()}))
        .expect("probe");
    assert_eq!(info["has_audio"], true, "MP4 must carry an AAC track");
    s.dispatch(
        "video.frame",
        &json!({"path": out.to_str().unwrap(), "ms": 700}),
    )
    .expect("frame");
    let diff = base(&s).mean_abs_diff(&pattern(w, h)).expect("same size");
    assert!(diff < 12.0, "decoded frame differs from fixture: {diff}");
}

#[test]
#[ignore = "needs an interactive desktop; run via cargo xtask ci --e2e"]
fn odd_width_recording_decodes_without_shear() {
    // 330 px is not a multiple of 16: the H.264 decoder pads rows, which sheared frames when the
    // reader assumed the pre-decode stride.
    let fx = launch(170, 170, 330, 202);
    let mut s = session();
    let (x, y, w, h) = fx.rect;
    assert_ne!(w % 16, 0, "fixture width must not be a multiple of 16");
    std::thread::sleep(std::time::Duration::from_millis(800));
    let out = std::env::temp_dir().join("screenit-e2e-odd.mp4");
    let _ = std::fs::remove_file(&out);
    let out_s = out.to_str().unwrap();
    s.dispatch(
        "record.start",
        &json!({"x": x, "y": y, "w": w, "h": h, "fps": 15, "output": out_s}),
    )
    .expect("start");
    std::thread::sleep(std::time::Duration::from_millis(1200));
    s.dispatch("record.stop", &json!({})).expect("stop");
    s.dispatch("video.frame", &json!({"path": out_s, "ms": 500}))
        .expect("frame");
    let frame = base(&s);
    let (ew, eh) = (w & !1, h & !1);
    assert_eq!((frame.width(), frame.height()), (ew, eh));
    let diff = frame.mean_abs_diff(&pattern(ew, eh)).expect("same size");
    assert!(diff < 12.0, "decoded frame differs from fixture: {diff}");
}

#[test]
#[ignore = "needs an interactive desktop; run via cargo xtask ci --e2e"]
fn video_edit_pipeline_trim_cut_combine_gif_images() {
    let fx = launch(150, 150, 320, 200);
    let mut s = session();
    let (x, y, w, h) = fx.rect;
    std::thread::sleep(std::time::Duration::from_millis(800));
    let dir = std::env::temp_dir().join("screenit-e2e-vid");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.mp4");
    let p = |n: &str| dir.join(n).to_str().unwrap().to_string();
    s.dispatch(
        "record.start",
        &json!({"x": x, "y": y, "w": w, "h": h, "fps": 15, "output": src.to_str().unwrap()}),
    )
    .expect("start");
    std::thread::sleep(std::time::Duration::from_millis(2000));
    s.dispatch("record.stop", &json!({})).expect("stop");
    let dur = |s: &mut Session, f: &str| {
        s.dispatch("video.probe", &json!({"path": f}))
            .expect("probe")["duration_ms"]
            .as_u64()
            .unwrap()
    };
    let src_s = src.to_str().unwrap();
    s.dispatch("video.trim", &json!({"path": src_s, "start_ms": 200, "end_ms": 1200, "output": p("trim.mp4"), "fps": 15}))
        .expect("trim");
    let d = dur(&mut s, &p("trim.mp4"));
    assert!((800..1300).contains(&d), "trim duration {d}");
    s.dispatch(
        "video.cut",
        &json!({"path": src_s, "from_ms": 500, "to_ms": 1000, "output": p("cut.mp4"), "fps": 15}),
    )
    .expect("cut");
    let full = dur(&mut s, src_s);
    let d = dur(&mut s, &p("cut.mp4"));
    assert!(d + 300 < full && d + 800 > full, "cut {d} vs {full}");
    s.dispatch(
        "video.combine",
        &json!({"paths": [p("trim.mp4"), p("cut.mp4")], "output": p("both.mp4"), "fps": 15}),
    )
    .expect("combine");
    assert!(dur(&mut s, &p("both.mp4")) > 1500);
    let g = s
        .dispatch(
            "video.gif",
            &json!({"path": src_s, "output": p("a.gif"), "fps": 5, "end_ms": 1000}),
        )
        .expect("gif");
    assert!(g["frames"].as_u64().unwrap() >= 4);
    let bytes = std::fs::read(p("a.gif")).unwrap();
    assert!(bytes.starts_with(b"GIF89a"));
    s.dispatch("video.frame", &json!({"path": p("both.mp4"), "ms": 300}))
        .expect("frame");
    s.dispatch("doc.export", &json!({"path": p("f1.png")}))
        .expect("png");
    s.dispatch(
        "video.from_images",
        &json!({"paths": [p("f1.png"), p("f1.png")], "output": p("slides.mp4"), "ms_each": 1000, "fps": 10}),
    )
    .expect("from images");
    let d = dur(&mut s, &p("slides.mp4"));
    assert!((1500..2600).contains(&d), "slides duration {d}");
}

#[test]
#[ignore = "needs an interactive desktop; run via cargo xtask ci --e2e"]
fn timelapse_records_real_screen_frames_to_mp4() {
    let fx = launch(160, 160, 320, 200);
    let mut s = session();
    let (x, y, w, h) = fx.rect;
    std::thread::sleep(std::time::Duration::from_millis(800));
    let out = std::env::temp_dir().join("screenit-e2e-timelapse.mp4");
    let _ = std::fs::remove_file(&out);
    let r = s
        .dispatch(
            "capture.timelapse",
            &json!({"x": x, "y": y, "w": w, "h": h, "interval_ms": 100, "count": 8, "fps": 4, "output": out.to_str().unwrap()}),
        )
        .expect("timelapse");
    assert_eq!(r["result"]["frames"], 8);
    let info = s
        .dispatch("video.probe", &json!({"path": out.to_str().unwrap()}))
        .expect("probe");
    let d = info["duration_ms"].as_u64().unwrap();
    assert!((1500..2600).contains(&d), "8 frames at 4 fps ~2s, got {d}");
    s.dispatch(
        "video.frame",
        &json!({"path": out.to_str().unwrap(), "ms": 500}),
    )
    .expect("frame");
    let diff = base(&s).mean_abs_diff(&pattern(w, h)).expect("same size");
    assert!(diff < 12.0, "timelapse frame differs from fixture: {diff}");
}
