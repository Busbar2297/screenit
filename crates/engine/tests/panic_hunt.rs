//! Never-crash fuzz: every registered command gets adversarial parameters; none may panic.
//! A recovered panic (`CommandError::Panic`) is a test failure. Run: `cargo xtask panic-hunt`.

use screenit_engine::CommandError;
use screenit_testkit::{FakeScreen, fake_session};
use serde_json::{Value, json};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[(self.next() % v.len() as u64) as usize]
    }
}

const NUMS: &[f64] = &[
    0.0,
    -1.0,
    1.0,
    2.0,
    7.0,
    49.0,
    50.0,
    1e9,
    -1e9,
    1e12,
    0.5,
    -0.5,
    4294967296.0,
    2147483648.0,
    255.0,
    256.0,
    65536.0,
];
const STRS: &[&str] = &[
    "",
    "x",
    "#ff0000",
    "#ff000080",
    "not a color",
    "🦀🦀",
    "none",
    "image",
    "h264",
    "date_asc",
    "h265",
    "title",
];
// Commands that write to caller-chosen paths are hunted separately with temp-dir paths.
const FS: &[&str] = &[
    "doc.save",
    "doc.export",
    "doc.open",
    "library.delete",
    "library.open_dir",
    "library.open",
    "prefs.set",
    "record.start",
    "video.frame",
    "video.probe",
    "batch.convert",
    "image.watermark",
    "image.combine",
    "video.trim",
    "video.cut",
    "video.combine",
    "video.gif",
    "video.from_images",
    "video.export",
    "library.share",
    "capture.timelapse",
];

fn scalar(r: &mut Rng) -> Value {
    match r.next() % 6 {
        0 => json!(*r.pick(NUMS)),
        1 => json!(*r.pick(NUMS) as i64),
        2 => json!(*r.pick(STRS)),
        3 => json!(r.next() % 2 == 0),
        4 => Value::Null,
        _ => json!(r.next() % 100),
    }
}

fn value(r: &mut Rng, depth: u32) -> Value {
    match r.next() % 5 {
        0 if depth < 3 => Value::Array((0..r.next() % 6).map(|_| value(r, depth + 1)).collect()),
        1 if depth < 3 => {
            let keys = [
                "x", "y", "w", "h", "id", "rect", "from", "to", "points", "style", "radius",
                "text", "width", "height", "color", "factor",
            ];
            Value::Object(
                (0..r.next() % 5)
                    .map(|_| ((*r.pick(&keys)).to_string(), value(r, depth + 1)))
                    .collect(),
            )
        }
        _ => scalar(r),
    }
}

/// Params built from the command's real keys but with hostile values (reaches deep code paths).
fn shaped(r: &mut Rng, params_doc: &str) -> Value {
    let mut m = serde_json::Map::new();
    for key in params_doc.split('"').skip(1).step_by(2) {
        if !key.is_empty()
            && key.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            && r.next() % 5 != 0
        {
            let v = match r.next() % 4 {
                0 => json!([*r.pick(NUMS), *r.pick(NUMS)]),
                1 => json!([*r.pick(NUMS), *r.pick(NUMS), *r.pick(NUMS), *r.pick(NUMS)]),
                _ => scalar(r),
            };
            m.insert(key.to_string(), v);
        }
    }
    Value::Object(m)
}

#[test]
#[ignore = "long fuzz; run via `cargo xtask panic-hunt`"]
fn every_command_survives_hostile_params() {
    let rounds: u64 = std::env::var("SCREENIT_HUNT_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(150);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut s, _) = fake_session(96, 64);
    let specs: Vec<(String, String)> = s
        .registry()
        .iter()
        .map(|c| (c.id.to_string(), c.params.to_string()))
        .collect();
    let mut calls = 0u64;
    for round in 0..rounds {
        if s.doc.is_none() || round % 25 == 0 {
            let _ = s.dispatch("capture.fullscreen", &json!({}));
        }
        for (id, doc) in &specs {
            if FS.contains(&id.as_str())
                || id == "doc.close"
                || id.starts_with("capture.scroll") && round % 7 != 0
            {
                continue;
            }
            let params = if rng.next() % 3 == 0 {
                value(&mut rng, 0)
            } else {
                shaped(&mut rng, doc)
            };
            calls += 1;
            let res = s.dispatch(id, &params);
            if let Err(CommandError::Panic(m)) = res {
                panic!("command `{id}` panicked with params {params}: {m}");
            }
            if s.doc.is_none() {
                let _ = s.dispatch("doc.new", &json!({"width": 40, "height": 30}));
            }
        }
        // Cap runaway growth from repeated resize/border/shadow commands.
        if let Some(d) = &s.doc {
            if d.base().width() > 1500 || d.base().height() > 1500 || d.objects.len() > 500 {
                let _ = s.dispatch("doc.new", &json!({"width": 96, "height": 64}));
            }
        }
    }
    assert!(calls > 1000);
}

#[test]
fn fs_commands_reject_hostile_paths_without_panicking() {
    let (mut s, _) = fake_session(32, 32);
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    let dir = std::env::temp_dir().join(format!("scit-hunt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for bad in [
        "",
        "\u{0}",
        "no/such/dir/x.png",
        "CON",
        "x.unknownext",
        &dir.to_string_lossy(),
    ] {
        for id in [
            "doc.save",
            "doc.export",
            "doc.open",
            "library.open_dir",
            "video.frame",
            "video.probe",
        ] {
            let r = s.dispatch(id, &json!({"path": bad, "dir": bad, "ms": 5}));
            assert!(
                !matches!(r, Err(CommandError::Panic(_))),
                "{id} panicked on {bad:?}"
            );
        }
        let r = s.dispatch(
            "record.start",
            &json!({"x":0,"y":0,"w":10,"h":10,"output":bad}),
        );
        assert!(!matches!(r, Err(CommandError::Panic(_))));
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn empty_screen_capture_is_an_error() {
    let mut s = fake_session(8, 8).0;
    s.services.screen = Some(FakeScreen::new(8, 8).shared());
    assert!(
        s.dispatch("capture.region", &json!({"x":100,"y":100,"w":5,"h":5}))
            .is_err()
    );
}
