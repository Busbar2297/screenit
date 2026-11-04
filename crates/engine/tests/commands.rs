use screenit_engine::control::handle_request;
use screenit_testkit::fake_session;
use serde_json::json;

#[test]
fn capture_region_then_annotate_undo_export() {
    let (mut s, clip) = fake_session(200, 120);
    let r = s
        .dispatch(
            "capture.region",
            &json!({"x":10,"y":10,"w":100,"h":60,"clipboard":true}),
        )
        .unwrap();
    assert_eq!(r["width"], 100);
    assert!(
        clip.image.lock().unwrap().is_some(),
        "clipboard receives the capture"
    );
    let id = s
        .dispatch("editor.arrow.add", &json!({"from":[5,5],"to":[90,50]}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    s.dispatch("editor.step.add", &json!({"center":[20,20]}))
        .unwrap();
    assert_eq!(
        s.dispatch("editor.object.list", &json!({})).unwrap()["objects"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    s.dispatch("editor.object.move", &json!({"id":id,"dx":3,"dy":3}))
        .unwrap();
    s.dispatch("edit.undo", &json!({})).unwrap();
    s.dispatch("edit.undo", &json!({})).unwrap();
    assert_eq!(
        s.dispatch("editor.object.list", &json!({})).unwrap()["objects"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    s.dispatch("edit.redo", &json!({})).unwrap();

    let dir = std::env::temp_dir().join(format!("scit-engine-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("out.png");
    let scit = dir.join("out.scit");
    s.dispatch("doc.export", &json!({"path": png})).unwrap();
    s.dispatch("doc.save", &json!({"path": scit})).unwrap();
    s.dispatch("doc.close", &json!({})).unwrap();
    let o = s.dispatch("doc.open", &json!({"path": scit})).unwrap();
    assert_eq!(o["objects"], 2);
    // Projects saved by earlier versions as .sxs still open with their annotations.
    let legacy = dir.join("old.sxs");
    std::fs::copy(&scit, &legacy).unwrap();
    s.dispatch("doc.close", &json!({})).unwrap();
    let o = s.dispatch("doc.open", &json!({"path": legacy})).unwrap();
    assert_eq!(o["objects"], 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn crop_moves_annotations_and_resizes_base() {
    let (mut s, _) = fake_session(100, 100);
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    s.dispatch("editor.rect.add", &json!({"rect":[30,30,10,10]}))
        .unwrap();
    s.dispatch("editor.crop", &json!({"rect":[20,20,50,50]}))
        .unwrap();
    let info = s.dispatch("doc.info", &json!({})).unwrap();
    assert_eq!(
        (info["width"].as_u64(), info["height"].as_u64()),
        (Some(50), Some(50))
    );
    assert_eq!(
        info["objects"][0]["kind"]["rect"][0]
            .as_f64()
            .or(info["objects"][0]["kind"]["rect"]["x"].as_f64()),
        Some(10.0)
    );
}

#[test]
fn bad_params_are_errors_not_panics() {
    let (mut s, _) = fake_session(50, 50);
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    assert!(
        s.dispatch("editor.arrow.add", &json!({"from":[0,0]}))
            .is_err()
    );
    assert!(
        s.dispatch(
            "editor.arrow.add",
            &json!({"from":[0,0],"to":[1,1],"bogus":1})
        )
        .is_err()
    );
    assert!(
        s.dispatch("editor.crop", &json!({"rect":[500,500,5,5]}))
            .is_err()
    );
    assert_eq!(
        s.dispatch("doc.info", &json!({})).unwrap()["objects"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn control_protocol_round_trip() {
    let (mut s, _) = fake_session(40, 40);
    let r = handle_request(
        &mut s,
        &json!({"id":7,"method":"engine.execute","params":{"command":"doc.new","params":{"width":8,"height":8}}}),
    );
    assert_eq!(r["ok"], true);
    assert_eq!(r["id"], 7);
    let r = handle_request(
        &mut s,
        &json!({"id":8,"method":"engine.execute","params":{"command":"nope"}}),
    );
    assert_eq!(r["error"]["code"], "unknown_command");
    let r = handle_request(&mut s, &json!({"id":9,"method":"engine.commands"}));
    assert!(r["result"].as_array().unwrap().len() > 40);
}

#[test]
fn recording_lifecycle_adds_library_entry() {
    let (mut s, _) = fake_session(64, 64);
    let out = std::env::temp_dir().join("scit-rec-test.mp4");
    s.dispatch(
        "record.start",
        &json!({"x":0,"y":0,"w":33,"h":20,"output":out}),
    )
    .unwrap();
    assert_eq!(
        s.dispatch("record.status", &json!({})).unwrap()["state"],
        "recording"
    );
    s.dispatch("record.pause", &json!({})).unwrap();
    s.dispatch("record.resume", &json!({})).unwrap();
    let r = s.dispatch("record.stop", &json!({})).unwrap();
    assert_eq!(
        r["result"]["width"], 32,
        "odd width is made encoder-friendly"
    );
    assert_eq!(
        s.dispatch("library.recent", &json!({})).unwrap()["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn recording_output_size_param_and_setting() {
    let (mut s, _) = fake_session(64, 64);
    let out = std::env::temp_dir().join("scit-rec-size-test.mp4");
    let region = json!({"x":0,"y":0,"w":1920,"h":1080,"output":out});
    let start = |s: &mut screenit_engine::Session, extra: serde_json::Value| {
        let mut p = region.clone();
        if let (Some(p), Some(e)) = (p.as_object_mut(), extra.as_object()) {
            p.extend(e.clone());
        }
        s.dispatch("record.start", &p)
    };
    // Bad names are refused and nothing starts.
    assert!(start(&mut s, json!({"size":"8k"})).is_err());
    assert_eq!(
        s.dispatch("record.status", &json!({})).unwrap()["state"],
        "idle"
    );
    // An explicit preset wins.
    start(&mut s, json!({"size":"2160p"})).unwrap();
    let r = s.dispatch("record.stop", &json!({})).unwrap();
    assert_eq!(
        (
            r["result"]["width"].as_u64(),
            r["result"]["height"].as_u64()
        ),
        (Some(3840), Some(2160))
    );
    // Without one, the Recording setting decides.
    s.dispatch(
        "settings.set",
        &json!({"path":"recording.output","value":"1440p"}),
    )
    .unwrap();
    start(&mut s, json!({})).unwrap();
    let r = s.dispatch("record.stop", &json!({})).unwrap();
    assert_eq!(
        (
            r["result"]["width"].as_u64(),
            r["result"]["height"].as_u64()
        ),
        (Some(2560), Some(1440))
    );
    // Native keeps the captured size.
    start(&mut s, json!({"size":"native"})).unwrap();
    let r = s.dispatch("record.stop", &json!({})).unwrap();
    assert_eq!(r["result"]["width"], 1920);
}

#[test]
fn image_ops_flatten_and_change_size() {
    let (mut s, _) = fake_session(60, 40);
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    s.dispatch("editor.rect.add", &json!({"rect":[5,5,10,10]}))
        .unwrap();
    s.dispatch("image.rotate", &json!({})).unwrap();
    let i = s.dispatch("doc.info", &json!({})).unwrap();
    assert_eq!(
        (i["width"].as_u64(), i["height"].as_u64()),
        (Some(40), Some(60))
    );
    assert_eq!(i["objects"].as_array().unwrap().len(), 0);
    s.dispatch("image.resize", &json!({"percent":50})).unwrap();
    s.dispatch("edit.undo", &json!({})).unwrap();
    assert_eq!(s.dispatch("doc.info", &json!({})).unwrap()["width"], 40);
}

#[test]
fn quick_styles_and_themes_are_single_undoable_steps() {
    let (mut s, _) = fake_session(60, 40);
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    let a = s
        .dispatch("editor.rect.add", &json!({"rect":[1,1,10,10]}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    s.dispatch("editor.arrow.add", &json!({"from":[0,0],"to":[5,5]}))
        .unwrap();
    s.dispatch("editor.quickstyle.apply", &json!({"id": a, "name": "blue"}))
        .unwrap();
    assert!(
        s.dispatch("editor.quickstyle.apply", &json!({"id": a, "name": "nope"}))
            .is_err()
    );
    let r = s
        .dispatch("editor.theme.apply", &json!({"name": "Mono"}))
        .unwrap();
    assert_eq!(r["recoloured"], 2);
    s.dispatch("edit.undo", &json!({})).unwrap();
    let objs = s.dispatch("editor.object.list", &json!({})).unwrap();
    assert_eq!(
        objs["objects"][0]["style"]["stroke"],
        json!([0, 120, 215, 255])
    );
}

#[test]
fn automatic_scrolling_capture_stitches_the_whole_content() {
    use screenit_geom::Rect;
    use screenit_testkit::{FakeScreen, pattern};
    use std::sync::Arc;
    let content = pattern(80, 400);
    let view = Rect::new(0, 0, 80, 100);
    let screen = FakeScreen::new(200, 150).with_scroller(content.clone(), view, 30);
    let (mut s, _) = fake_session(10, 10);
    s.services.screen = Some(Arc::new(screen));
    let r = s
        .dispatch(
            "capture.scroll.auto",
            &json!({"x":0,"y":0,"w":80,"h":100,"max_pages":40,"notches":2}),
        )
        .unwrap();
    assert!(r["pages"].as_u64().unwrap() > 4, "{r}");
    assert_eq!(r["width"], 80);
    assert_eq!(
        r["height"], 400,
        "stitched height equals the content height"
    );
    let got = screenit_compose::flatten(s.doc.as_ref().unwrap());
    assert!(got.mean_abs_diff(&content).unwrap() < 0.5);
}

#[test]
fn scroll_step_walks_to_the_end_and_finish_stitches() {
    use screenit_geom::Rect;
    use screenit_testkit::{FakeScreen, pattern};
    use std::sync::Arc;
    let content = pattern(80, 400);
    let screen =
        FakeScreen::new(200, 150).with_scroller(content.clone(), Rect::new(0, 0, 80, 100), 30);
    let (mut s, _) = fake_session(10, 10);
    s.services.screen = Some(Arc::new(screen));
    assert!(
        s.dispatch("capture.scroll.step", &json!({})).is_err(),
        "needs begin first"
    );
    s.dispatch("capture.scroll.begin", &json!({"x":0,"y":0,"w":80,"h":100}))
        .unwrap();
    assert!(
        s.dispatch("capture.scroll.step", &json!({"notches": 0.5}))
            .is_err()
    );
    let mut steps = 0;
    loop {
        let r = s
            .dispatch("capture.scroll.step", &json!({"notches":2}))
            .unwrap();
        steps += 1;
        if r["end"] == true {
            break;
        }
        assert!(steps < 100, "never reached the end");
    }
    let r = s
        .dispatch("capture.scroll.finish", &json!({"clipboard": false}))
        .unwrap();
    assert_eq!(r["height"], 400);
}

#[test]
fn object_menu_and_grab_text() {
    use screenit_geom::Rect;
    use screenit_testkit::{FakeOcr, FakeScreen};
    use std::sync::Arc;
    let (mut s, _) = fake_session(10, 10);
    let screen = FakeScreen::new(200, 150).with_window(7, "Menu File", Rect::new(20, 20, 60, 40));
    s.services.screen = Some(Arc::new(screen));
    s.dispatch("capture.menu", &json!({})).unwrap();
    assert_eq!(s.dispatch("doc.info", &json!({})).unwrap()["width"], 60);
    s.dispatch("capture.object", &json!({"point":[30,30]}))
        .unwrap();
    assert_eq!(s.dispatch("doc.info", &json!({})).unwrap()["height"], 40);
    assert!(
        s.dispatch("capture.grab_text", &json!({})).is_err(),
        "no OCR engine configured"
    );
    s.services.ocr = Some(Arc::new(FakeOcr(vec!["hello".into(), "world".into()])));
    let t = s
        .dispatch("capture.grab_text", &json!({"clipboard": false}))
        .unwrap();
    assert_eq!(t["text"], "hello\nworld");
}

#[test]
fn webcam_pip_and_swap_commands_track_layout() {
    let (mut s, _) = fake_session(200, 120);
    let out = std::env::temp_dir().join("scit-pip-test.mp4");
    s.dispatch(
        "record.start",
        &json!({"x":0,"y":0,"w":200,"h":120,"output":out.to_str().unwrap(),
                "webcam":true,"pip_shape":"circle","pip_corner":"top_left"}),
    )
    .unwrap();
    let st = s.dispatch("record.status", &json!({})).unwrap();
    assert_eq!(st["pip"]["shape"], "circle");
    assert_eq!(st["pip"]["corner"], "top_left");
    s.dispatch(
        "record.pip",
        &json!({"pip_size": 40, "pip_corner": "bottom_left"}),
    )
    .unwrap();
    assert!(s.dispatch("record.pip", &json!({"pip_size": 1})).is_err());
    let sw = s.dispatch("record.swap", &json!({})).unwrap();
    assert_eq!(sw["swapped"], true);
    let st = s.dispatch("record.status", &json!({})).unwrap();
    assert_eq!(st["pip"]["size_percent"], 40);
    assert_eq!(st["pip"]["swapped"], true);
    s.dispatch("record.stop", &json!({})).unwrap();
    assert!(
        s.dispatch("record.swap", &json!({})).is_err(),
        "nothing recording"
    );
}

#[test]
fn recording_devices_audio_and_camera_release() {
    let (mut s, _) = fake_session(200, 120);
    let d = s.dispatch("record.devices", &json!({})).unwrap();
    assert_eq!(d["microphones"][0], "Fake Microphone");
    assert_eq!(d["cameras"][0], "Fake Camera");
    assert!(s.dispatch("record.devices", &json!({"x": 1})).is_err());

    let cam = std::sync::Arc::new(screenit_testkit::FakeCamera::new(
        screenit_raster::Raster::new(8, 8, [0, 0, 0, 255]).unwrap(),
    ));
    s.services.camera = Some(cam.clone());
    let out = std::env::temp_dir().join("scit-av-test.mp4");
    let out = out.to_str().unwrap();
    s.dispatch(
        "record.start",
        &json!({"x":0,"y":0,"w":200,"h":120,"output":out,"mic":"default","system_audio":true,"webcam":true}),
    )
    .unwrap();
    let r = s.dispatch("record.stop", &json!({})).unwrap();
    assert_eq!(r["result"]["has_audio"], true);
    assert_eq!(
        cam.stops.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "webcam released when the recording stops"
    );
    // Without a webcam the camera is left alone; without audio there is no audio track.
    s.dispatch(
        "record.start",
        &json!({"x":0,"y":0,"w":200,"h":120,"output":out}),
    )
    .unwrap();
    let r = s.dispatch("record.stop", &json!({})).unwrap();
    assert_eq!(r["result"]["has_audio"], false);
    assert_eq!(cam.stops.load(std::sync::atomic::Ordering::SeqCst), 1);

    s.services.devices = None;
    let d = s.dispatch("record.devices", &json!({})).unwrap();
    assert_eq!(d["microphones"].as_array().map(Vec::len), Some(0));
}

#[test]
fn video_export_copies_mp4_and_rejects_bad_paths() {
    let (mut s, _) = fake_session(10, 10);
    let dir = std::env::temp_dir().join("scit-export-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("rec.mp4");
    std::fs::write(&src, b"fake mp4 bytes").unwrap();
    let out = dir.join("sub").join("Exported.MP4");
    let r = s
        .dispatch("video.export", &json!({"path": src, "output": out}))
        .unwrap();
    assert_eq!(r["bytes"], 14);
    assert_eq!(std::fs::read(&out).unwrap(), b"fake mp4 bytes");
    // Exporting onto itself leaves the file intact.
    s.dispatch("video.export", &json!({"path": src, "output": src}))
        .unwrap();
    assert_eq!(std::fs::read(&src).unwrap(), b"fake mp4 bytes");
    let bad = [
        json!({"path": src, "output": dir.join("x.png")}),
        json!({"path": dir.join("missing.mp4"), "output": out}),
        json!({"path": src}),
        json!({"path": 3, "output": out}),
        json!({"path": src, "output": out, "extra": 1}),
    ];
    for p in bad {
        assert!(s.dispatch("video.export", &p).is_err(), "{p}");
    }
    assert!(!dir.join("x.png").exists());
}

#[test]
fn effects_canvas_trim_are_undoable_steps() {
    let (mut s, _) = fake_session(120, 80);
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    s.dispatch(
        "image.edges",
        &json!({"style":"saw","depth":6,"sides":["bottom"]}),
    )
    .unwrap();
    s.dispatch("image.reflection", &json!({"height":0.5}))
        .unwrap();
    let info = s.dispatch("doc.info", &json!({})).unwrap();
    assert_eq!(info["height"], 80 + 2 + 40);
    s.dispatch(
        "image.canvas",
        &json!({"width":200,"height":200,"color":"#ff0000"}),
    )
    .unwrap();
    assert_eq!(
        s.dispatch("doc.pixel", &json!({"x":0,"y":0})).unwrap()["rgba"],
        json!([255, 0, 0, 255])
    );
    s.dispatch("image.trim", &json!({})).unwrap();
    let t = s.dispatch("doc.info", &json!({})).unwrap();
    assert!(t["width"].as_u64().unwrap() < 200);
    s.dispatch("edit.undo", &json!({})).unwrap();
    assert_eq!(s.dispatch("doc.info", &json!({})).unwrap()["width"], 200);
    assert!(
        s.dispatch("image.edges", &json!({"style":"zigzag"}))
            .is_err()
    );
    assert!(
        s.dispatch("image.fade", &json!({"side":"up","length":3}))
            .is_err()
    );
}

#[test]
fn export_writes_jpg_gif_bmp_tiff_webp() {
    let dir = std::env::temp_dir().join("scit-export-formats");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut s, _) = fake_session(32, 24);
    s.dispatch(
        "doc.new",
        &json!({"width": 32, "height": 24, "color": "#336699"}),
    )
    .unwrap();
    let checks: &[(&str, &[u8])] = &[
        ("jpg", &[0xFF, 0xD8]),
        ("gif", b"GIF"),
        ("bmp", b"BM"),
        ("tiff", b"II"),
        ("webp", b"RIFF"),
        ("pdf", b"%PDF"),
    ];
    for (ext, magic) in checks {
        let path = dir.join(format!("out.{ext}"));
        s.dispatch(
            "doc.export",
            &json!({"path": path.to_str().unwrap(), "quality": 80}),
        )
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            bytes.starts_with(magic),
            "{ext} did not start with the expected header"
        );
    }
    let bad = dir.join("nope.xyz");
    assert!(
        s.dispatch("doc.export", &json!({"path": bad.to_str().unwrap()}))
            .is_err()
    );
}

#[test]
fn combine_batch_convert_and_pdf_export() {
    let dir = std::env::temp_dir().join("scit-m6");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut s, _) = fake_session(60, 40);
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    let a = dir.join("a.png");
    s.dispatch("doc.export", &json!({"path": a.to_str().unwrap()}))
        .unwrap();
    s.dispatch(
        "image.combine",
        &json!({"paths":[a.to_str().unwrap(), a.to_str().unwrap()],"vertical":false,"spacing":4}),
    )
    .unwrap();
    let info = s.dispatch("doc.info", &json!({})).unwrap();
    assert_eq!(
        (info["width"].as_u64(), info["height"].as_u64()),
        (Some(124), Some(40))
    );
    s.dispatch(
        "image.watermark",
        &json!({"path": a.to_str().unwrap(), "opacity": 0.5}),
    )
    .unwrap();
    s.dispatch("image.spotlight", &json!({"rect":[10,10,30,20]}))
        .unwrap();
    s.dispatch("image.perspective", &json!({})).unwrap();
    s.dispatch("image.pagecurl", &json!({"size":20})).unwrap();
    let pdf = dir.join("out.pdf");
    s.dispatch("doc.export", &json!({"path": pdf.to_str().unwrap()}))
        .unwrap();
    assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF-1.4"));
    let r = s
        .dispatch(
            "batch.convert",
            &json!({"inputs":[a.to_str().unwrap(), "missing.png"],
        "out_dir": dir.join("out").to_str().unwrap(), "format":"jpg","percent":50}),
        )
        .unwrap();
    assert_eq!(r["converted"].as_array().unwrap().len(), 1);
    assert_eq!(r["failed"].as_array().unwrap().len(), 1);
    assert!(dir.join("out").join("a.jpg").exists());
}

#[test]
fn mcp_lists_every_command_and_calls_tools() {
    use screenit_engine::mcp::handle_mcp;
    let (mut s, _) = fake_session(50, 40);
    let init = handle_mcp(
        &mut s,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
    )
    .unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "screenit");
    assert!(
        handle_mcp(
            &mut s,
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .is_none()
    );
    let list = handle_mcp(
        &mut s,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .unwrap();
    let tools = list["result"]["tools"].as_array().unwrap();
    assert!(tools.len() > 100, "{} tools", tools.len());
    assert!(tools.iter().any(|t| t["name"] == "capture_fullscreen"));
    let call = handle_mcp(
        &mut s,
        &json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"capture_fullscreen","arguments":{}}}),
    )
    .unwrap();
    assert_eq!(call["result"]["isError"], false);
    let bad = handle_mcp(
        &mut s,
        &json!({"jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"image_resize","arguments":{"bogus":1}}}),
    )
    .unwrap();
    assert_eq!(bad["result"]["isError"], true);
    let unk = handle_mcp(
        &mut s,
        &json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope"}}),
    )
    .unwrap();
    assert!(unk.get("error").is_some());
    let m = handle_mcp(&mut s, &json!({"jsonrpc":"2.0","id":6,"method":"zzz"})).unwrap();
    assert_eq!(m["error"]["code"], -32601);
}

#[test]
fn library_share_copies_file_and_clipboard() {
    let dir = std::env::temp_dir().join("scit-share");
    let _ = std::fs::remove_dir_all(&dir);
    let (mut s, clip) = fake_session(40, 30);
    s.dispatch(
        "library.open_dir",
        &json!({"dir": dir.join("lib").to_str().unwrap()}),
    )
    .unwrap();
    s.dispatch(
        "prefs.set",
        &json!({"auto_save_dir": dir.join("caps").to_str().unwrap()}),
    )
    .unwrap();
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    let id = s.dispatch("library.list", &json!({})).unwrap()["entries"][0]["id"]
        .as_u64()
        .unwrap();
    s.dispatch("library.share", &json!({"id": id, "mode":"clipboard"}))
        .unwrap();
    assert!(clip.image.lock().unwrap().is_some());
    let r = s
        .dispatch(
            "library.share",
            &json!({"id": id, "dest_dir": dir.join("out").to_str().unwrap()}),
        )
        .unwrap();
    assert!(std::path::Path::new(r["path"].as_str().unwrap()).exists());
    assert!(s.dispatch("library.share", &json!({"id": 99999})).is_err());
    assert!(
        s.dispatch("library.share", &json!({"id": id, "mode": "pigeon"}))
            .is_err()
    );
}

#[test]
fn smart_redact_blurs_only_sensitive_spans() {
    use std::sync::Arc;
    let (mut s, _) = fake_session(400, 60);
    s.services.ocr = Some(Arc::new(screenit_testkit::FakeOcr(vec![
        "hello world".into(),
        "mail bob@example.com please".into(),
        "card 4111 1111 1111 1111".into(),
    ])));
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    let r = s.dispatch("editor.smart_redact", &json!({})).unwrap();
    assert_eq!(r["redacted"], 2);
    let info = s.dispatch("doc.info", &json!({})).unwrap();
    let objs = info["objects"].as_array().unwrap();
    assert_eq!(objs.len(), 2);
    // Email starts at char 5 of 27: its box must start well inside the line, not at x=0.
    let x = objs[0]["kind"]["rect"]["x"].as_f64().unwrap();
    assert!(x > 40.0 && x < 120.0, "span-level rect expected, got x={x}");
    s.dispatch("edit.undo", &json!({})).unwrap();
    assert_eq!(
        s.dispatch("doc.info", &json!({})).unwrap()["objects"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let t = s
        .dispatch(
            "editor.smart_redact",
            &json!({"kinds": [], "terms": ["WORLD"]}),
        )
        .unwrap();
    assert_eq!(t["redacted"], 1);
    assert!(
        s.dispatch("editor.smart_redact", &json!({"kinds": ["zip"]}))
            .is_err()
    );
}

struct FakeModel(&'static str);

impl screenit_capture::LlmOcr for FakeModel {
    fn transcribe(
        &self,
        _: &screenit_raster::Raster,
        _: &screenit_capture::LlmConfig,
    ) -> Result<String, screenit_capture::CaptureError> {
        Ok(String::new())
    }
    fn complete_text(
        &self,
        prompt: &str,
        _: &screenit_capture::LlmConfig,
    ) -> Result<String, screenit_capture::CaptureError> {
        assert!(prompt.contains("1: "), "lines are numbered from 1");
        Ok(self.0.to_string())
    }
    fn models(
        &self,
        _: &screenit_capture::LlmConfig,
    ) -> Result<Vec<String>, screenit_capture::CaptureError> {
        Ok(vec![])
    }
}

fn objects(s: &mut screenit_engine::Session) -> Vec<serde_json::Value> {
    s.dispatch("doc.info", &json!({})).unwrap()["objects"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn hide_secrets_covers_secrets_not_prose_and_is_one_undo_step() {
    use std::sync::Arc;
    let (mut s, _) = fake_session(400, 80);
    assert!(
        s.dispatch("editor.hide_secrets", &json!({})).is_err(),
        "needs a document"
    );
    s.services.ocr = Some(Arc::new(screenit_testkit::FakeOcr(vec![
        "hello world, nothing to see".into(),
        "Password: hunter2".into(),
        "token ghp_16C7e42F292c6912E7710c838347Ae178B4a".into(),
        "API key:".into(),
        "sk-proj-abcdefghijklmnopqrstuv1234".into(),
    ])));
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    let r = s.dispatch("editor.hide_secrets", &json!({})).unwrap();
    assert_eq!(r["hidden"], 3, "{r}");
    let objs = objects(&mut s);
    assert_eq!(objs.len(), 3);
    let ys: Vec<f64> = objs
        .iter()
        .map(|o| o["kind"]["rect"]["y"].as_f64().unwrap())
        .collect();
    assert!(
        ys.iter().all(|y| *y >= 6.0),
        "line 0 (prose) is untouched: {ys:?}"
    );
    s.dispatch("edit.undo", &json!({})).unwrap();
    assert_eq!(objects(&mut s).len(), 0, "one history step");
    assert!(
        s.dispatch("editor.hide_secrets", &json!({"mode": "smudge"}))
            .is_err()
    );
    assert!(
        s.dispatch("editor.hide_secrets", &json!({"nope": 1}))
            .is_err(),
        "unknown params are rejected"
    );
}

#[test]
fn hide_secrets_label_on_its_own_line_covers_the_value_line() {
    use std::sync::Arc;
    let (mut s, _) = fake_session(300, 60);
    s.services.ocr = Some(Arc::new(screenit_testkit::FakeOcr(vec![
        "Password".into(),
        "correct horse battery".into(),
        "unrelated footer text".into(),
    ])));
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    let r = s.dispatch("editor.hide_secrets", &json!({})).unwrap();
    assert_eq!(r["hidden"], 1, "{r}");
    let y = objects(&mut s)[0]["kind"]["rect"]["y"].as_f64().unwrap();
    assert!(
        (6.0..=11.0).contains(&y),
        "value line 1 is covered, got y={y}"
    );
}

#[test]
fn hide_secrets_model_pass_is_optional_and_failures_only_warn() {
    use std::sync::Arc;
    let (mut s, _) = fake_session(300, 60);
    s.services.ocr = Some(Arc::new(screenit_testkit::FakeOcr(vec![
        "nothing here".into(),
        "my pin is 8841 ok".into(),
    ])));
    s.services.llm = Some(Arc::new(FakeModel(
        "```json\n[{\"line\": 2, \"text\": \"8841\"}, {\"line\": 99, \"text\": \"x\"}]\n```",
    )));
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    let off = s.dispatch("editor.hide_secrets", &json!({})).unwrap();
    assert_eq!(off["hidden"], 0, "the model is off by default");
    let on = s
        .dispatch("editor.hide_secrets", &json!({"llm": true}))
        .unwrap();
    assert_eq!(on["hidden"], 1, "{on}");
    let x = objects(&mut s)[0]["kind"]["rect"]["x"].as_f64().unwrap();
    assert!(x > 80.0, "only the number is covered, got x={x}");
    // A model that errors must not fail the command.
    s.services.llm = Some(Arc::new(screenit_testkit::FakeLlmDown));
    let bad = s
        .dispatch("editor.hide_secrets", &json!({"llm": true}))
        .unwrap();
    assert!(bad["warning"].as_str().is_some());
}

#[test]
fn redact_apply_validates_rects_and_is_one_step() {
    let (mut s, _) = fake_session(100, 40);
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    let r = s
        .dispatch(
            "editor.redact.apply",
            &json!({"rects": [[1,1,20,10],[30,5,10,10]]}),
        )
        .unwrap();
    assert_eq!(r["applied"], 2);
    assert_eq!(objects(&mut s).len(), 2);
    s.dispatch("edit.undo", &json!({})).unwrap();
    assert_eq!(objects(&mut s).len(), 0);
    for bad in [
        json!({}),
        json!({"rects": [[1,2,3]]}),
        json!({"rects": [[1,2,0,5]]}),
        json!({"rects": [[1,2,-3,5]]}),
        json!({"rects": [["a",2,3,5]]}),
        json!({"rects": [[1,2,3,5]], "mode": "x"}),
    ] {
        assert!(s.dispatch("editor.redact.apply", &bad).is_err(), "{bad}");
    }
}

#[test]
fn auto_hide_burns_secrets_into_the_clipboard_image_and_password_fields_too() {
    use screenit_geom::Rect;
    use screenit_testkit::{FakeOcr, FakeScreen};
    use std::sync::Arc;
    let lines = vec![
        "hello".to_string(),
        "Password: hunter2 and more".to_string(),
    ];
    let (mut s, clip) = fake_session(300, 60);
    s.services.ocr = Some(Arc::new(FakeOcr(lines.clone())));
    s.dispatch("capture.fullscreen", &json!({"clipboard": true}))
        .unwrap();
    let raw = clip.image.lock().unwrap().clone().unwrap();

    s.settings.privacy.auto_hide_secrets = true;
    let r = s
        .dispatch("capture.fullscreen", &json!({"clipboard": true}))
        .unwrap();
    assert_eq!(r["secrets_hidden"], 1, "{r}");
    let hidden = clip.image.lock().unwrap().clone().unwrap();
    let differs = |img: &screenit_raster::Raster, y: std::ops::Range<i32>| {
        y.flat_map(|y| (0..300).map(move |x| (x, y)))
            .any(|(x, y)| img.get(x, y) != raw.get(x, y))
    };
    assert!(differs(&hidden, 12..18), "the secret line was pixelated");
    assert!(!differs(&hidden, 0..4), "the first line is untouched");

    // An OS-reported password field is covered even when OCR finds nothing.
    let screen = FakeScreen::new(300, 60).with_secret_field(Rect::new(200, 30, 60, 14));
    s.services.screen = Some(Arc::new(screen));
    s.services.ocr = Some(Arc::new(FakeOcr(vec![])));
    let r = s
        .dispatch("capture.fullscreen", &json!({"clipboard": true}))
        .unwrap();
    assert_eq!(r["secrets_hidden"], 1, "{r}");
    // Off by default: nothing is reported or changed.
    s.settings.privacy.auto_hide_secrets = false;
    let r = s.dispatch("capture.fullscreen", &json!({})).unwrap();
    assert!(r.get("secrets_hidden").is_none());
}

#[test]
fn privacy_settings_round_trip_and_reject_bad_mode() {
    let (mut s, _) = fake_session(10, 10);
    s.dispatch(
        "settings.set",
        &json!({"path": "privacy.auto_hide_secrets", "value": true}),
    )
    .unwrap();
    assert!(s.settings.privacy.auto_hide_secrets);
    assert!(
        s.dispatch(
            "settings.set",
            &json!({"path": "privacy.mode", "value": "smudge"}),
        )
        .is_err()
    );
}

#[test]
fn ocr_replace_and_background_removal() {
    use std::sync::Arc;
    let (mut s, _) = fake_session(100, 40);
    s.services.ocr = Some(Arc::new(screenit_testkit::FakeOcr(vec!["old".into()])));
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    s.dispatch("editor.ocr.replace", &json!({"line": 0, "text": "new"}))
        .unwrap();
    let info = s.dispatch("doc.info", &json!({})).unwrap();
    assert_eq!(info["objects"][0]["kind"]["text"], "new");
    assert!(
        s.dispatch("editor.ocr.replace", &json!({"line": 9, "text": "x"}))
            .is_err()
    );
    let r = s
        .dispatch("image.remove_background", &json!({"tolerance": 255}))
        .unwrap();
    assert_eq!(r["cleared"], 100 * 40);
    assert_eq!(
        s.dispatch("doc.pixel", &json!({"x": 0, "y": 0})).unwrap()["rgba"][3],
        0
    );
}

#[test]
fn timelapse_captures_n_frames_at_interval() {
    let (mut s, _) = fake_session(64, 48);
    let out = std::env::temp_dir().join("scit-tl").join("t.mp4");
    let r = s
        .dispatch(
            "capture.timelapse",
            &json!({"x":0,"y":0,"w":63,"h":47,"interval_ms":50,"count":6,"output":out.to_str().unwrap(),"fps":10}),
        )
        .unwrap();
    assert_eq!(r["result"]["frames"], 6);
    assert_eq!(
        (
            r["result"]["width"].as_u64(),
            r["result"]["height"].as_u64()
        ),
        (Some(62), Some(46))
    );
    assert!(s.dispatch("capture.timelapse", &json!({"x":0,"y":0,"w":10,"h":10,"interval_ms":50,"count":1,"output":out.to_str().unwrap()})).is_err());
    assert!(
        s.dispatch(
            "capture.timelapse",
            &json!({"x":0,"y":0,"w":10,"h":10,"interval_ms":50,"count":3,"output":"x.avi"})
        )
        .is_err()
    );
}

#[test]
fn ocr_engine_setting_selects_windows_llm_or_fallback() {
    use screenit_testkit::{FakeLlm, FakeOcr};
    use std::sync::Arc;
    let (mut s, _) = fake_session(40, 30);
    s.dispatch("capture.region", &json!({"x":0,"y":0,"w":20,"h":20}))
        .unwrap();
    s.services.ocr = Some(Arc::new(FakeOcr(vec!["win".into()])));
    s.services.llm = Some(Arc::new(FakeLlm(Some("llm text\nsecond".into()))));
    let grab = |s: &mut screenit_engine::Session| {
        s.dispatch("capture.grab_text", &json!({"clipboard": false}))
    };
    assert_eq!(grab(&mut s).unwrap()["engine"], "windows");
    s.dispatch("settings.set", &json!({"path":"ocr.engine","value":"llm"}))
        .unwrap();
    let r = grab(&mut s).unwrap();
    assert_eq!(
        (r["engine"].as_str(), r["text"].as_str()),
        (Some("llm"), Some("llm text\nsecond"))
    );
    // Fallback: Windows finds nothing, so the model is asked.
    s.dispatch(
        "settings.set",
        &json!({"path":"ocr.engine","value":"windows_then_llm"}),
    )
    .unwrap();
    assert_eq!(grab(&mut s).unwrap()["engine"], "windows");
    s.services.ocr = Some(Arc::new(FakeOcr(vec![])));
    assert_eq!(grab(&mut s).unwrap()["engine"], "llm");
    // A dead server is an error, not a panic.
    s.services.llm = Some(Arc::new(FakeLlm(None)));
    assert!(grab(&mut s).is_err());
    assert!(s.dispatch("ocr.info", &json!({"llm": true})).is_err());
    assert!(
        s.dispatch("capture.grab_text", &json!({"lang": "../x"}))
            .is_err(),
        "bad language tags are rejected"
    );
    s.services.llm = Some(Arc::new(FakeLlm(Some("x".into()))));
    let info = s.dispatch("ocr.info", &json!({"llm": true})).unwrap();
    assert_eq!(info["llm_models"][0], "fake-model");
}
