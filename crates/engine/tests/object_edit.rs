//! Select / resize / rotate / edit-text commands on annotation objects.
use screenit_testkit::fake_session;
use serde_json::{Value, json};

fn session() -> screenit_engine::Session {
    let (mut s, _) = fake_session(300, 200);
    s.dispatch("capture.fullscreen", &json!({})).unwrap();
    s
}

fn obj(s: &mut screenit_engine::Session, id: u64) -> Value {
    let l = s.dispatch("editor.object.list", &json!({})).unwrap();
    l["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == id)
        .cloned()
        .unwrap()
}

#[test]
fn text_box_wraps_and_edits_in_one_step_each() {
    let mut s = session();
    let id = s
        .dispatch(
            "editor.text.add",
            &json!({"pos":[10,10],"text":"alpha beta gamma delta","size":20,"width":90,"height":10}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let o = obj(&mut s, id);
    assert_eq!(o["kind"]["width"], 90.0);
    assert!(o["kind"]["height"].as_f64().unwrap() > 40.0, "{o}");
    s.dispatch(
        "editor.object.text",
        &json!({"id":id,"text":"hi","coalesce":"t"}),
    )
    .unwrap();
    s.dispatch(
        "editor.object.text",
        &json!({"id":id,"text":"hi there","coalesce":"t"}),
    )
    .unwrap();
    assert_eq!(obj(&mut s, id)["kind"]["text"], "hi there");
    s.dispatch("edit.undo", &json!({})).unwrap();
    assert_eq!(
        obj(&mut s, id)["kind"]["text"],
        "alpha beta gamma delta",
        "typing coalesces into one undo step"
    );
}

#[test]
fn text_without_width_gets_a_natural_box() {
    let mut s = session();
    let id = s
        .dispatch("editor.text.add", &json!({"pos":[5,5],"text":"Hello"}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let o = obj(&mut s, id);
    assert!(o["kind"]["width"].as_f64().unwrap() > 20.0);
    assert!(o["kind"]["height"].as_f64().unwrap() > 20.0);
}

#[test]
fn resize_keeps_the_anchor_and_undoes() {
    let mut s = session();
    let id = s
        .dispatch("editor.rect.add", &json!({"rect":[50,50,40,20]}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    // Drag the top-left handle: bottom-right (anchor 1,1) stays.
    s.dispatch(
        "editor.object.resize",
        &json!({"id":id,"rect":[30,40,60,30],"anchor":[1,1]}),
    )
    .unwrap();
    let r = obj(&mut s, id)["kind"]["rect"].clone();
    assert_eq!(r, json!({"x":30.0,"y":40.0,"w":60.0,"h":30.0}), "{r}");
    s.dispatch("edit.undo", &json!({})).unwrap();
    assert_eq!(
        obj(&mut s, id)["kind"]["rect"],
        json!({"x":50.0,"y":50.0,"w":40.0,"h":20.0})
    );
}

#[test]
fn rotated_resize_keeps_the_anchor_on_screen() {
    let mut s = session();
    let id = s
        .dispatch("editor.rect.add", &json!({"rect":[100,80,40,20]}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    s.dispatch("editor.object.rotate", &json!({"id":id,"delta":90}))
        .unwrap();
    assert_eq!(obj(&mut s, id)["rotation"], 90.0);
    // Anchor = the box's top-left corner; after the resize it must be where it was.
    let corner = |o: &Value| {
        let r = &o["kind"]["rect"];
        let (x, y, w, h) = (
            r["x"].as_f64().unwrap(),
            r["y"].as_f64().unwrap(),
            r["w"].as_f64().unwrap(),
            r["h"].as_f64().unwrap(),
        );
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let (dx, dy) = (x - cx, y - cy);
        let a = o["rotation"].as_f64().unwrap().to_radians();
        (
            cx + dx * a.cos() - dy * a.sin(),
            cy + dx * a.sin() + dy * a.cos(),
        )
    };
    let before = corner(&obj(&mut s, id));
    s.dispatch(
        "editor.object.resize",
        &json!({"id":id,"rect":[100,80,80,50],"anchor":[0,0]}),
    )
    .unwrap();
    let after = corner(&obj(&mut s, id));
    assert!((before.0 - after.0).abs() < 0.01 && (before.1 - after.1).abs() < 0.01);
}

#[test]
fn rotate_lines_and_reject_pixel_kinds() {
    let mut s = session();
    let line = s
        .dispatch("editor.line.add", &json!({"from":[10,50],"to":[110,50]}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    s.dispatch("editor.object.rotate", &json!({"id":line,"delta":90}))
        .unwrap();
    let k = obj(&mut s, line)["kind"].clone();
    // A horizontal line turned 90 degrees about its centre is vertical.
    assert!(
        (k["from"]["x"].as_f64().unwrap() - 60.0).abs() < 0.01,
        "{k}"
    );
    assert!((k["to"]["x"].as_f64().unwrap() - 60.0).abs() < 0.01, "{k}");
    let blur = s
        .dispatch("editor.blur.add", &json!({"rect":[0,0,20,20]}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    assert!(
        s.dispatch("editor.object.rotate", &json!({"id":blur,"delta":10}))
            .is_err()
    );
}

#[test]
fn endpoints_move_and_bad_params_err() {
    let mut s = session();
    let a = s
        .dispatch("editor.arrow.add", &json!({"from":[10,10],"to":[50,50]}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    s.dispatch(
        "editor.object.endpoint",
        &json!({"id":a,"which":"to","point":[90,20]}),
    )
    .unwrap();
    assert_eq!(obj(&mut s, a)["kind"]["to"], json!({"x":90.0,"y":20.0}));
    for bad in [
        json!({"id":a,"which":"tail","point":[1,1]}),
        json!({"id":a,"which":"to","point":["x",1]}),
        json!({"id":9999,"which":"to","point":[1,1]}),
    ] {
        assert!(s.dispatch("editor.object.endpoint", &bad).is_err());
    }
    for (cmd, bad) in [
        ("editor.object.resize", json!({"id":a,"rect":[0,0,"nan",1]})),
        ("editor.object.resize", json!({"id":9999,"rect":[0,0,5,5]})),
        ("editor.object.rotate", json!({"id":a,"delta":"x"})),
        ("editor.object.rotate", json!({"id":9999,"delta":5})),
        ("editor.object.text", json!({"id":a,"text":"x"})),
        ("editor.object.text", json!({"id":9999,"text":"x"})),
    ] {
        assert!(s.dispatch(cmd, &bad).is_err(), "{cmd} {bad}");
    }
}

#[test]
fn hit_test_follows_rotation_and_thin_lines() {
    let mut s = session();
    let id = s
        .dispatch("editor.rect.add", &json!({"rect":[100,100,100,20]}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    s.dispatch("editor.object.rotate", &json!({"id":id,"delta":90}))
        .unwrap();
    // Centre (150,110): the rotated box is now 20 wide and 100 tall.
    let hit = |s: &mut screenit_engine::Session, x: f64, y: f64| {
        s.dispatch("editor.object.hit_test", &json!({"point":[x, y]}))
            .unwrap()["id"]
            .clone()
    };
    assert_eq!(hit(&mut s, 150.0, 70.0), json!(id));
    assert_eq!(hit(&mut s, 110.0, 110.0), Value::Null);
    let line = s
        .dispatch("editor.line.add", &json!({"from":[0,0],"to":[100,100]}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    assert_eq!(hit(&mut s, 50.0, 50.0), json!(line));
    assert_eq!(hit(&mut s, 90.0, 10.0), Value::Null);
}
