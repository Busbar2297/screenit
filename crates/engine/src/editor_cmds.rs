//! Editor tools: annotation objects (arrow, line, shape, text, callout, step, blur, highlighter,
//! pen, stamp, magnify), object move/delete/style, crop, cut out, fill, magic wand, eraser,
//! selection delete. Each command is exactly one history step.

use crate::args::Args;
use crate::{CmdResult, CommandError, CommandSpec, Session, need_doc};
use screenit_algo as algo;
use screenit_doc::{Color, ObjectKind, Style};
use screenit_geom::{PointF, Rect};
use screenit_vector as vector;
use serde_json::{Value, json};

const STYLE: &str = "style";

fn add(s: &mut Session, label: &str, kind: ObjectKind, style: Style) -> CmdResult {
    let id = s.edit(label, None, |d| {
        d.add(kind, style).map_err(CommandError::failed)
    })?;
    Ok(json!({"id": id}))
}

fn arrow(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["from", "to", STYLE])?;
    let st = a.style(Style::default())?;
    add(
        s,
        "Arrow",
        ObjectKind::Arrow {
            from: a.pointf("from")?,
            to: a.pointf("to")?,
        },
        st,
    )
}

fn line(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["from", "to", STYLE])?;
    let st = a.style(Style::default())?;
    add(
        s,
        "Line",
        ObjectKind::Line {
            from: a.pointf("from")?,
            to: a.pointf("to")?,
        },
        st,
    )
}

fn rect(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["rect", "radius", STYLE])?;
    let st = a.style(Style::default())?;
    add(
        s,
        "Rectangle",
        ObjectKind::Rect {
            rect: a.rectf("rect")?,
            radius: a.opt_f32("radius")?.unwrap_or(0.0),
        },
        st,
    )
}

fn ellipse(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["rect", STYLE])?;
    let st = a.style(Style::default())?;
    add(
        s,
        "Ellipse",
        ObjectKind::Ellipse {
            rect: a.rectf("rect")?,
        },
        st,
    )
}

/// Size a text box to its content: an unset width takes the natural width of the text, and the
/// height never drops below the wrapped text.
fn fit_text(kind: &mut ObjectKind) {
    if let ObjectKind::Text {
        text,
        size,
        width,
        height,
        ..
    } = kind
    {
        let lay = vector::layout_text(text, *size, *width);
        if *width <= 0.0 {
            *width = (lay.width + 2.0).ceil().max(*size);
        }
        let need = lay.height().ceil();
        if need.is_finite() && need > *height {
            *height = need;
        }
    }
}

fn text(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["pos", "text", "size", "width", "height", STYLE])?;
    let st = a.style(Style {
        width: 1.0,
        ..Style::default()
    })?;
    let mut kind = ObjectKind::Text {
        pos: a.pointf("pos")?,
        text: a.str("text")?.to_string(),
        size: a.opt_f32("size")?.unwrap_or(24.0),
        width: a.opt_f32("width")?.unwrap_or(0.0),
        height: a.opt_f32("height")?.unwrap_or(0.0),
    };
    kind.validate().map_err(CommandError::failed)?;
    fit_text(&mut kind);
    add(s, "Text", kind, st)
}

/// Change the text (and optionally the size) of a text box or callout.
fn set_text(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["id", "text", "size", "coalesce"])?;
    let id = a.u64("id")?;
    let new_text = a.opt_str("text")?.map(str::to_string);
    let new_size = a.opt_f32("size")?;
    let key = a.opt_str("coalesce")?.map(str::to_string);
    let mut kind = s
        .doc_ref()?
        .get(id)
        .map(|o| o.kind.clone())
        .ok_or_else(|| CommandError::Failed(format!("no object with id {id}")))?;
    match &mut kind {
        ObjectKind::Text { text, size, .. } | ObjectKind::Callout { text, size, .. } => {
            if let Some(t) = new_text {
                *text = t;
            }
            if let Some(sz) = new_size {
                *size = sz;
            }
        }
        _ => return Err(CommandError::Failed("object has no text".into())),
    }
    kind.validate().map_err(CommandError::failed)?;
    fit_text(&mut kind);
    s.edit("Edit text", key.as_deref(), |d| {
        d.replace_kind(id, kind).map_err(CommandError::failed)
    })?;
    Ok(Value::Null)
}

/// Resize an object's box. `rect` is in the object's own (unrotated) frame; the point at
/// fraction `anchor` of the box stays put on screen.
fn resize(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["id", "rect", "anchor", "coalesce"])?;
    let id = a.u64("id")?;
    let mut rect = a.rectf("rect")?.normalized();
    let anchor = a.opt_pointf("anchor")?.map_or((0.0, 0.0), |q| (q.x, q.y));
    let key = a.opt_str("coalesce")?.map(str::to_string);
    if let Some(ObjectKind::Text { text, size, .. }) = s.doc_ref()?.get(id).map(|o| &o.kind) {
        // A text box never gets shorter than its wrapped text.
        let lay = vector::layout_text(text, *size, rect.w.max(1.0));
        rect.h = rect.h.max(lay.height().ceil());
    }
    s.edit("Resize", key.as_deref(), |d| {
        d.resize(id, rect, anchor).map_err(CommandError::failed)
    })?;
    Ok(Value::Null)
}

fn rotate(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["id", "delta", "coalesce"])?;
    let (id, delta) = (a.u64("id")?, a.f32("delta")?);
    let key = a.opt_str("coalesce")?.map(str::to_string);
    s.edit("Rotate", key.as_deref(), |d| {
        d.rotate(id, delta).map_err(CommandError::failed)
    })?;
    Ok(Value::Null)
}

/// Move one end of a line or arrow, or a callout's tail.
fn endpoint(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["id", "which", "point", "coalesce"])?;
    let id = a.u64("id")?;
    let which = a.str("which")?.to_string();
    let pt = a.pointf("point")?;
    let key = a.opt_str("coalesce")?.map(str::to_string);
    let mut kind = s
        .doc_ref()?
        .get(id)
        .map(|o| o.kind.clone())
        .ok_or_else(|| CommandError::Failed(format!("no object with id {id}")))?;
    match (&mut kind, which.as_str()) {
        (ObjectKind::Line { from, .. } | ObjectKind::Arrow { from, .. }, "from") => *from = pt,
        (ObjectKind::Line { to, .. } | ObjectKind::Arrow { to, .. }, "to") => *to = pt,
        (ObjectKind::Callout { tail, .. }, "tail") => *tail = pt,
        _ => {
            return Err(CommandError::Params(
                "`which` must be from/to for a line or arrow, or tail for a callout".into(),
            ));
        }
    }
    s.edit("Move end", key.as_deref(), |d| {
        d.replace_kind(id, kind).map_err(CommandError::failed)
    })?;
    Ok(Value::Null)
}

fn callout(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["rect", "text", "tail", "size", STYLE])?;
    let st = a.style(Style {
        width: 2.0,
        ..Style::default()
    })?;
    let r = a.rectf("rect")?;
    let tail = a
        .opt_pointf("tail")?
        .unwrap_or(PointF::new(r.x - 30.0, r.y + r.h + 30.0));
    add(
        s,
        "Callout",
        ObjectKind::Callout {
            rect: r,
            text: a.str("text")?.to_string(),
            tail,
            size: a.opt_f32("size")?.unwrap_or(18.0),
        },
        st,
    )
}

fn step(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["center", "number", "radius", STYLE])?;
    let st = a.style(Style::default())?;
    let radius = a.opt_f32("radius")?.unwrap_or(s.prefs.default_step_radius);
    let number = match a.opt_u32("number")? {
        Some(n) => n,
        None => s.doc_ref()?.next_step_number(),
    };
    add(
        s,
        "Step",
        ObjectKind::Step {
            center: a.pointf("center")?,
            number,
            radius,
        },
        st,
    )
}

fn blur(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["rect", "radius", "pixelate"])?;
    add(
        s,
        "Blur",
        ObjectKind::Blur {
            rect: a.rectf("rect")?,
            radius: a.opt_u32("radius")?.unwrap_or(8).min(200),
            pixelate: a.bool("pixelate", false)?,
        },
        Style::default(),
    )
}

fn highlight(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["rect", STYLE])?;
    let st = a.style(Style {
        stroke: Color::YELLOW,
        ..Style::default()
    })?;
    add(
        s,
        "Highlight",
        ObjectKind::Highlight {
            rect: a.rectf("rect")?,
        },
        st,
    )
}

fn pen(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["points", STYLE])?;
    let st = a.style(Style::default())?;
    let points = a
        .array("points")?
        .iter()
        .map(|v| {
            let w = serde_json::json!({ "p": v });
            Args::new(&w, &["p"])?.pointf("p")
        })
        .collect::<Result<Vec<_>, _>>()?;
    add(s, "Pen", ObjectKind::Pen { points }, st)
}

fn stamp(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["pos", "name", "size", STYLE])?;
    let st = a.style(Style::default())?;
    add(
        s,
        "Stamp",
        ObjectKind::Stamp {
            pos: a.pointf("pos")?,
            name: a.str("name")?.to_string(),
            size: a.opt_f32("size")?.unwrap_or(48.0),
        },
        st,
    )
}

fn magnify(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["src", "dst", "factor", STYLE])?;
    let st = a.style(Style {
        width: 2.0,
        ..Style::default()
    })?;
    add(
        s,
        "Magnify",
        ObjectKind::Magnify {
            src: a.rectf("src")?,
            dst: a.rectf("dst")?,
            factor: a.opt_f32("factor")?.unwrap_or(2.0),
        },
        st,
    )
}

fn list(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    Ok(json!({"objects": s.doc_ref()?.objects}))
}

fn mv(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["id", "dx", "dy", "coalesce"])?;
    let (id, dx, dy) = (a.u64("id")?, a.f32("dx")?, a.f32("dy")?);
    let key = a.opt_str("coalesce")?.map(str::to_string);
    s.edit("Move", key.as_deref(), |d| {
        d.translate(id, dx, dy).map_err(CommandError::failed)
    })?;
    Ok(Value::Null)
}

fn delete(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["id"])?;
    let id = a.u64("id")?;
    s.edit("Delete", None, |d| {
        d.remove(id).map(|_| ()).map_err(CommandError::failed)
    })?;
    Ok(Value::Null)
}

fn restyle(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["id", STYLE])?;
    let id = a.u64("id")?;
    let base = s
        .doc_ref()?
        .get(id)
        .map(|o| o.style.clone())
        .ok_or_else(|| CommandError::Failed(format!("no object with id {id}")))?;
    let st = a.style(base)?;
    s.edit("Style", None, |d| {
        d.set_style(id, st).map_err(CommandError::failed)
    })?;
    Ok(Value::Null)
}

fn hit(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["point"])?;
    Ok(json!({"id": s.doc_ref()?.hit_test(a.pointf("point")?)}))
}

fn clear_objects(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    s.edit("Clear annotations", None, |d| {
        d.objects.clear();
        Ok::<_, CommandError>(())
    })?;
    Ok(Value::Null)
}

fn irect(a: &Args<'_>, s: &Session) -> Result<Rect, CommandError> {
    let r = a.rectf("rect")?.to_rect();
    let base = s.doc_ref()?.base().rect();
    r.intersect(&base)
        .ok_or_else(|| CommandError::Failed("rectangle is outside the image".into()))
}

fn crop(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["rect"])?;
    let r = irect(&a, s)?;
    s.edit("Crop", None, |d| {
        let c = d
            .base()
            .crop(r)
            .ok_or_else(|| CommandError::Failed("nothing to crop".into()))?;
        d.set_base(c);
        for o in &mut d.objects {
            o.translate(-(r.x as f32), -(r.y as f32));
        }
        Ok(())
    })?;
    Ok(json!({"width": r.w, "height": r.h}))
}

fn cut_out(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["vertical", "start", "length"])?;
    let (vertical, start, len) = (
        a.bool("vertical", false)?,
        a.u32("start")?,
        a.u32("length")?,
    );
    s.edit("Cut Out", None, |d| {
        let total = if vertical {
            d.base().width()
        } else {
            d.base().height()
        };
        if len == 0 || start >= total || len >= total {
            return Err(CommandError::Failed(
                "cut-out range must lie inside the image and leave something".into(),
            ));
        }
        let out = algo::cut_out(d.base(), vertical, start, len).map_err(CommandError::failed)?;
        let end = start + len.min(total - start);
        d.set_base(out);
        // Objects past the removed band move up/left; objects inside it are dropped.
        let (lo, hi, cut) = (start as f32, end as f32, len.min(total - start) as f32);
        d.objects.retain(|o| {
            let b = o.bounds();
            let (a0, a1) = if vertical {
                (b.x, b.x + b.w)
            } else {
                (b.y, b.y + b.h)
            };
            !(a0 >= lo && a1 <= hi)
        });
        for o in &mut d.objects {
            let b = o.bounds();
            let pos = if vertical { b.x } else { b.y };
            if pos >= hi {
                if vertical {
                    o.translate(-cut, 0.0)
                } else {
                    o.translate(0.0, -cut)
                }
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn parse_fill(a: &Args<'_>) -> Result<[u8; 4], CommandError> {
    Ok(a.color("color")?.unwrap_or(Color::RED).0)
}

fn fill(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["x", "y", "tolerance", "color"])?;
    let (x, y, tol, c) = (
        a.i32("x")?,
        a.i32("y")?,
        a.opt_u32("tolerance")?.unwrap_or(0).min(255) as u8,
        parse_fill(&a)?,
    );
    let n = s.edit("Fill", None, |d| {
        let mut r = d.base().clone();
        let n = algo::flood_fill(&mut r, x, y, tol, c);
        if n == 0 {
            return Err(CommandError::Failed("no pixel at that point".into()));
        }
        d.set_base(r);
        Ok(n)
    })?;
    Ok(json!({"pixels": n}))
}

fn magic_wand(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["x", "y", "tolerance"])?;
    let d = s.doc_ref()?;
    let mask = algo::flood_select(
        d.base(),
        a.i32("x")?,
        a.i32("y")?,
        a.opt_u32("tolerance")?.unwrap_or(16).min(255) as u8,
    );
    let w = d.base().width() as usize;
    let (mut x0, mut y0, mut x1, mut y1, mut n) = (usize::MAX, usize::MAX, 0usize, 0usize, 0usize);
    for (i, m) in mask.iter().enumerate() {
        if *m {
            let (x, y) = (i % w.max(1), i / w.max(1));
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            n += 1;
        }
    }
    if n == 0 {
        return Ok(json!({"pixels": 0, "bounds": null}));
    }
    Ok(json!({"pixels": n, "bounds": [x0, y0, x1 - x0 + 1, y1 - y0 + 1]}))
}

fn erase(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["rect", "color"])?;
    let r = irect(&a, s)?;
    let c = a.color("color")?.map_or([0, 0, 0, 0], |c| c.0);
    s.edit("Erase", None, |d| {
        let mut b = d.base().clone();
        b.fill_rect(r, c);
        d.set_base(b);
        Ok::<_, CommandError>(())
    })?;
    Ok(Value::Null)
}

/// Selection delete: fill the rectangle with the surrounding (edge-average) colour.
fn selection_delete(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["rect"])?;
    let r = irect(&a, s)?;
    s.edit("Delete Selection", None, |d| {
        let mut b = d.base().clone();
        let (mut sum, mut n) = ([0u64; 4], 0u64);
        let mut sample = |x: i32, y: i32| {
            if let Some(px) = d.base().get(x, y) {
                for i in 0..4 {
                    sum[i] += u64::from(px[i]);
                }
                n += 1;
            }
        };
        for x in r.x - 1..=r.x + r.w as i32 {
            sample(x, r.y - 1);
            sample(x, r.y + r.h as i32);
        }
        for y in r.y..r.y + r.h as i32 {
            sample(r.x - 1, y);
            sample(r.x + r.w as i32, y);
        }
        let fill = if n == 0 {
            [255, 255, 255, 255]
        } else {
            [
                (sum[0] / n) as u8,
                (sum[1] / n) as u8,
                (sum[2] / n) as u8,
                (sum[3] / n) as u8,
            ]
        };
        b.fill_rect(r, fill);
        d.set_base(b);
        Ok::<_, CommandError>(())
    })?;
    Ok(Value::Null)
}

type QuickStyle = (&'static str, [u8; 4], Option<[u8; 4]>, f32, bool);
const QUICK_STYLES: &[QuickStyle] = &[
    ("Red", [230, 30, 30, 255], None, 3.0, false),
    ("Blue", [0, 120, 215, 255], None, 3.0, false),
    ("Green", [16, 160, 70, 255], None, 3.0, false),
    (
        "Yellow marker",
        [255, 235, 0, 255],
        Some([255, 235, 0, 90]),
        2.0,
        false,
    ),
    ("Black", [0, 0, 0, 255], None, 2.0, false),
    (
        "Shadowed white",
        [255, 255, 255, 255],
        Some([0, 120, 215, 255]),
        2.0,
        true,
    ),
];

const THEMES: &[(&str, [[u8; 4]; 4])] = &[
    (
        "Classic",
        [
            [230, 30, 30, 255],
            [0, 120, 215, 255],
            [16, 160, 70, 255],
            [255, 160, 0, 255],
        ],
    ),
    (
        "Mono",
        [
            [0, 0, 0, 255],
            [70, 70, 70, 255],
            [140, 140, 140, 255],
            [200, 200, 200, 255],
        ],
    ),
    (
        "Ocean",
        [
            [0, 90, 160, 255],
            [0, 150, 190, 255],
            [30, 60, 120, 255],
            [90, 200, 220, 255],
        ],
    ),
];

fn quickstyle_list(_: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let l: Vec<Value> = QUICK_STYLES
        .iter()
        .map(
            |(n, s, f, w, sh)| json!({"name": n, "stroke": s, "fill": f, "width": w, "shadow": sh}),
        )
        .collect();
    Ok(json!({"styles": l}))
}

fn quickstyle_apply(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["id", "name"])?;
    let name = a.str("name")?;
    let (_, stroke, fill, width, shadow) = QUICK_STYLES
        .iter()
        .find(|q| q.0.eq_ignore_ascii_case(name))
        .ok_or_else(|| CommandError::Params(format!("unknown quick style `{name}`")))?;
    let st = Style {
        stroke: Color(*stroke),
        fill: fill.map(Color),
        width: *width,
        shadow: *shadow,
    };
    let id = a.u64("id")?;
    s.edit("Quick Style", None, |d| {
        d.set_style(id, st).map_err(CommandError::failed)
    })?;
    Ok(Value::Null)
}

fn theme_list(_: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    Ok(
        json!({"themes": THEMES.iter().map(|(n, c)| json!({"name": n, "colors": c})).collect::<Vec<_>>()}),
    )
}

/// Recolour every annotation by cycling through the theme palette (one history step).
fn theme_apply(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["name"])?;
    let name = a.str("name")?;
    let (_, colors) = THEMES
        .iter()
        .find(|t| t.0.eq_ignore_ascii_case(name))
        .ok_or_else(|| CommandError::Params(format!("unknown theme `{name}`")))?;
    let ids: Vec<u64> = s.doc_ref()?.objects.iter().map(|o| o.id).collect();
    s.edit("Apply Theme", None, |d| {
        for (i, id) in ids.iter().enumerate() {
            let mut st = d.get(*id).map(|o| o.style.clone()).unwrap_or_default();
            st.stroke = Color(colors[i % colors.len()]);
            d.set_style(*id, st).map_err(CommandError::failed)?;
        }
        Ok::<_, CommandError>(())
    })?;
    Ok(json!({"recoloured": ids.len()}))
}
pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, shortcut, params, run| CommandSpec {
        id,
        label,
        menu,
        shortcut,
        params,
        enabled: need_doc,
        run,
    };
    vec![
        c(
            "editor.arrow.add",
            "Arrow",
            "Tools",
            "Ctrl+A",
            r#"{"from":[x,y],"to":[x,y],"style"?:{}}"#,
            arrow,
        ),
        c(
            "editor.line.add",
            "Line",
            "Tools",
            "",
            r#"{"from":[x,y],"to":[x,y],"style"?:{}}"#,
            line,
        ),
        c(
            "editor.rect.add",
            "Rectangle",
            "Tools",
            "Ctrl+U",
            r#"{"rect":[x,y,w,h],"radius"?:num,"style"?:{}}"#,
            rect,
        ),
        c(
            "editor.ellipse.add",
            "Ellipse",
            "Tools",
            "",
            r#"{"rect":[x,y,w,h],"style"?:{}}"#,
            ellipse,
        ),
        c(
            "editor.text.add",
            "Text",
            "Tools",
            "Ctrl+T",
            r#"{"pos":[x,y],"text":str,"size"?:num,"width"?:num,"height"?:num,"style"?:{}}"#,
            text,
        ),
        c(
            "editor.object.text",
            "Edit Text",
            "Tools",
            "",
            r#"{"id":int,"text"?:str,"size"?:num,"coalesce"?:str}"#,
            set_text,
        ),
        c(
            "editor.object.resize",
            "Resize Object",
            "Tools",
            "",
            r#"{"id":int,"rect":[x,y,w,h],"anchor"?:[fx,fy],"coalesce"?:str}"#,
            resize,
        ),
        c(
            "editor.object.rotate",
            "Rotate Object",
            "Tools",
            "",
            r#"{"id":int,"delta":degrees,"coalesce"?:str}"#,
            rotate,
        ),
        c(
            "editor.object.endpoint",
            "Move Line End",
            "Tools",
            "",
            r#"{"id":int,"which":"from"|"to"|"tail","point":[x,y],"coalesce"?:str}"#,
            endpoint,
        ),
        c(
            "editor.callout.add",
            "Callout",
            "Tools",
            "Ctrl+D",
            r#"{"rect":[x,y,w,h],"text":str,"tail"?:[x,y],"size"?:num,"style"?:{}}"#,
            callout,
        ),
        c(
            "editor.step.add",
            "Step",
            "Tools",
            "Ctrl+S",
            r#"{"center":[x,y],"number"?:int,"radius"?:num,"style"?:{}}"#,
            step,
        ),
        c(
            "editor.blur.add",
            "Blur",
            "Tools",
            "Ctrl+B",
            r#"{"rect":[x,y,w,h],"radius"?:int,"pixelate"?:bool}"#,
            blur,
        ),
        c(
            "editor.highlight.add",
            "Highlighter",
            "Tools",
            "Ctrl+H",
            r#"{"rect":[x,y,w,h],"style"?:{}}"#,
            highlight,
        ),
        c(
            "editor.pen.add",
            "Pen",
            "Tools",
            "Ctrl+P",
            r#"{"points":[[x,y],...],"style"?:{}}"#,
            pen,
        ),
        c(
            "editor.stamp.add",
            "Stamp",
            "Tools",
            "Ctrl+I",
            r#"{"pos":[x,y],"name":str,"size"?:num,"style"?:{}}"#,
            stamp,
        ),
        c(
            "editor.magnify.add",
            "Magnify",
            "Tools",
            "Ctrl+M",
            r#"{"src":[x,y,w,h],"dst":[x,y,w,h],"factor"?:1..32,"style"?:{}}"#,
            magnify,
        ),
        c(
            "editor.object.list",
            "List Objects",
            "Tools",
            "",
            "{}",
            list,
        ),
        c(
            "editor.object.move",
            "Move Object",
            "Tools",
            "Ctrl+V",
            r#"{"id":int,"dx":num,"dy":num,"coalesce"?:str}"#,
            mv,
        ),
        c(
            "editor.object.delete",
            "Delete Object",
            "Tools",
            "Delete",
            r#"{"id":int}"#,
            delete,
        ),
        c(
            "editor.object.style",
            "Change Object Style",
            "Tools",
            "",
            r#"{"id":int,"style":{}}"#,
            restyle,
        ),
        c(
            "editor.object.hit_test",
            "Object Under Point",
            "Tools",
            "",
            r#"{"point":[x,y]}"#,
            hit,
        ),
        c(
            "editor.object.clear",
            "Clear Annotations",
            "Tools",
            "",
            "{}",
            clear_objects,
        ),
        c(
            "editor.crop",
            "Crop",
            "Tools",
            "Ctrl+C",
            r#"{"rect":[x,y,w,h]}"#,
            crop,
        ),
        c(
            "editor.cutout",
            "Cut Out",
            "Tools",
            "Ctrl+X",
            r#"{"vertical"?:bool,"start":int,"length":int}"#,
            cut_out,
        ),
        c(
            "editor.fill",
            "Fill",
            "Tools",
            "Ctrl+F",
            r##"{"x":int,"y":int,"tolerance"?:0..255,"color"?:"#hex"}"##,
            fill,
        ),
        c(
            "editor.magicwand",
            "Magic Wand",
            "Tools",
            "Ctrl+W",
            r#"{"x":int,"y":int,"tolerance"?:0..255}"#,
            magic_wand,
        ),
        c(
            "editor.eraser",
            "Eraser",
            "Tools",
            "Ctrl+E",
            r##"{"rect":[x,y,w,h],"color"?:"#hex"}"##,
            erase,
        ),
        c(
            "editor.selection.delete",
            "Delete Selection",
            "Tools",
            "",
            r#"{"rect":[x,y,w,h]}"#,
            selection_delete,
        ),
        c(
            "editor.quickstyle.list",
            "Quick Styles",
            "Tools",
            "",
            "{}",
            quickstyle_list,
        ),
        c(
            "editor.quickstyle.apply",
            "Apply Quick Style",
            "Tools",
            "",
            r#"{"id":int,"name":str}"#,
            quickstyle_apply,
        ),
        c("editor.theme.list", "Themes", "Tools", "", "{}", theme_list),
        c(
            "editor.theme.apply",
            "Apply Theme",
            "Tools",
            "",
            r#"{"name":str}"#,
            theme_apply,
        ),
    ]
}
