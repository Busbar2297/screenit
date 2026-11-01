//! Effects, canvas, combine, batch convert. All are single undoable history steps.

use crate::args::Args;
use crate::image_cmds::transform;
use crate::{CmdResult, CommandError, CommandSpec, Session, always};
use screenit_algo::fx::{self, EdgeStyle, Side};
use screenit_codecs as codecs;
use screenit_doc::{CaptureDoc, Color};
use screenit_geom::Rect;
use screenit_raster::Raster;
use serde_json::{Value, json};
use std::path::Path;

fn side(v: &str) -> Result<Side, CommandError> {
    Ok(match v {
        "top" => Side::Top,
        "right" => Side::Right,
        "bottom" => Side::Bottom,
        "left" => Side::Left,
        o => return Err(CommandError::Params(format!("unknown side `{o}`"))),
    })
}

fn load(path: &str) -> Result<Raster, CommandError> {
    let bytes = std::fs::read(path).map_err(CommandError::failed)?;
    codecs::decode(&bytes).map_err(CommandError::failed)
}

fn edges(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["style", "depth", "sides"])?;
    let style = match a.opt_str("style")?.unwrap_or("torn") {
        "wave" => EdgeStyle::Wave,
        "saw" => EdgeStyle::Saw,
        "torn" => EdgeStyle::Torn,
        o => {
            return Err(CommandError::Params(format!(
                "unknown style `{o}` (wave|saw|torn)"
            )));
        }
    };
    let depth = a.opt_u32("depth")?.unwrap_or(8).min(200);
    let sides = if a.has("sides") {
        a.array("sides")?
            .iter()
            .map(|v| side(v.as_str().unwrap_or("")))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        vec![Side::Top, Side::Right, Side::Bottom, Side::Left]
    };
    transform(s, "Edges", |d| {
        d.set_base(fx::edges(d.base(), style, depth, &sides));
        Ok(())
    })
}

fn reflection(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["height", "gap", "opacity"])?;
    let (h, gap, op) = (
        a.opt_f32("height")?.unwrap_or(0.4),
        a.opt_u32("gap")?.unwrap_or(2).min(500),
        a.opt_f32("opacity")?.unwrap_or(0.5),
    );
    transform(s, "Reflection", |d| {
        let r = fx::reflection(d.base(), h, gap, op).map_err(CommandError::failed)?;
        d.set_base(r);
        Ok(())
    })
}

fn spotlight(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["rect", "dim"])?;
    let full = s.doc_ref()?.base().rect();
    let rect: Rect = a.rectf("rect")?.to_rect();
    let rect = rect
        .intersect(&full)
        .ok_or_else(|| CommandError::Failed("spotlight rectangle is outside the image".into()))?;
    let dim = a.opt_f32("dim")?.unwrap_or(0.6);
    transform(s, "Spotlight", |d| {
        let mut b = d.base().clone();
        fx::spotlight(&mut b, rect, dim);
        d.set_base(b);
        Ok(())
    })
}

fn watermark(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "corner", "margin", "opacity"])?;
    let mark = load(a.str("path")?)?;
    let (right, bottom) = match a.opt_str("corner")?.unwrap_or("bottom_right") {
        "top_left" => (false, false),
        "top_right" => (true, false),
        "bottom_left" => (false, true),
        "bottom_right" => (true, true),
        o => return Err(CommandError::Params(format!("unknown corner `{o}`"))),
    };
    let margin = a.opt_i32("margin")?.unwrap_or(10).clamp(0, 2000);
    let op = a.opt_f32("opacity")?.unwrap_or(0.5);
    transform(s, "Watermark", |d| {
        let mut b = d.base().clone();
        fx::watermark(&mut b, &mark, right, bottom, margin, op);
        d.set_base(b);
        Ok(())
    })
}

fn fade(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["side", "length"])?;
    let sd = side(a.str("side")?)?;
    let len = a.u32("length")?.min(10_000);
    transform(s, "Fade", |d| {
        let mut b = d.base().clone();
        fx::fade(&mut b, sd, len);
        d.set_base(b);
        Ok(())
    })
}

fn perspective(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["top", "bottom"])?;
    let (t, b) = (
        a.opt_f32("top")?.unwrap_or(0.3),
        a.opt_f32("bottom")?.unwrap_or(0.0),
    );
    transform(s, "Perspective", |d| {
        let r = fx::perspective(d.base(), t, b).map_err(CommandError::failed)?;
        d.set_base(r);
        Ok(())
    })
}

fn pagecurl(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["size"])?;
    let size = a.opt_u32("size")?.unwrap_or(60).min(5000);
    transform(s, "Page Curl", |d| {
        d.set_base(fx::page_curl(d.base(), size));
        Ok(())
    })
}

fn canvas(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["width", "height", "anchor_x", "anchor_y", "color"])?;
    let (w, h) = (a.u32("width")?, a.u32("height")?);
    if w == 0 || h == 0 || w > 16_384 || h > 16_384 {
        return Err(CommandError::Params(
            "canvas must be 1..=16384 per side".into(),
        ));
    }
    let (ax, ay) = (
        a.opt_f32("anchor_x")?.unwrap_or(0.5),
        a.opt_f32("anchor_y")?.unwrap_or(0.5),
    );
    let fill = a.color("color")?.unwrap_or(Color([0, 0, 0, 0]));
    transform(s, "Canvas Size", |d| {
        let r = fx::canvas(d.base(), w, h, ax, ay, fill.0).map_err(CommandError::failed)?;
        d.set_base(r);
        Ok(())
    })
}

fn trim(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["tolerance"])?;
    let tol = a.opt_u32("tolerance")?.unwrap_or(0).min(255) as u8;
    transform(s, "Trim", |d| {
        let r = fx::trim_rect(d.base(), tol)
            .ok_or_else(|| CommandError::Failed("nothing to trim: image is one colour".into()))?;
        let c = d
            .base()
            .crop(r)
            .ok_or_else(|| CommandError::Failed("trim failed".into()))?;
        d.set_base(c);
        Ok(())
    })
}

fn combine(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["paths", "vertical", "spacing", "color"])?;
    let paths = a.array("paths")?;
    if paths.len() < 2 || paths.len() > 64 {
        return Err(CommandError::Params("`paths` needs 2..=64 images".into()));
    }
    let images = paths
        .iter()
        .map(|v| {
            load(
                v.as_str()
                    .ok_or_else(|| CommandError::Params("paths must be strings".into()))?,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let fill = a.color("color")?.unwrap_or(Color([255, 255, 255, 255]));
    let r = fx::combine(
        &images,
        a.bool("vertical", true)?,
        a.opt_u32("spacing")?.unwrap_or(0).min(1000),
        fill.0,
    )
    .map_err(CommandError::failed)?;
    let (w, h) = (r.width(), r.height());
    let mut d = CaptureDoc::new(r);
    d.name = "Combined".into();
    s.new_document(d);
    Ok(json!({"width": w, "height": h}))
}

fn batch(_s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["inputs", "out_dir", "format", "quality", "percent"])?;
    let fmt = codecs::Format::from_extension(a.str("format")?).map_err(CommandError::failed)?;
    let quality = a.opt_u32("quality")?.unwrap_or(90).clamp(1, 100) as u8;
    let percent = a.opt_f32("percent")?;
    let out_dir = Path::new(a.str("out_dir")?);
    std::fs::create_dir_all(out_dir).map_err(CommandError::failed)?;
    let (mut done, mut failed) = (Vec::new(), Vec::new());
    for v in a.array("inputs")? {
        let Some(inp) = v.as_str() else {
            failed.push(json!({"input": v, "error": "not a string"}));
            continue;
        };
        let res = (|| -> Result<String, CommandError> {
            let mut img = load(inp)?;
            if let Some(pc) = percent {
                let k = (pc / 100.0).clamp(0.01, 10.0);
                let (w, h) = (
                    ((img.width() as f32 * k) as u32).max(1),
                    ((img.height() as f32 * k) as u32).max(1),
                );
                img = screenit_algo::resize(&img, w, h).map_err(CommandError::failed)?;
            }
            let stem = Path::new(inp)
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "image".into());
            let out = out_dir.join(format!("{stem}.{}", fmt.extension()));
            let bytes = codecs::encode(&img, fmt, quality).map_err(CommandError::failed)?;
            std::fs::write(&out, bytes).map_err(CommandError::failed)?;
            Ok(out.to_string_lossy().into_owned())
        })();
        match res {
            Ok(o) => done.push(o),
            Err(e) => failed.push(json!({"input": inp, "error": e.to_string()})),
        }
    }
    Ok(json!({"converted": done, "failed": failed}))
}

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, params, run| CommandSpec {
        id,
        label,
        menu: "Image",
        shortcut: "",
        params,
        enabled: always,
        run,
    };
    vec![
        c(
            "image.edges",
            "Edges",
            r#"{"style"?:wave|saw|torn,"depth"?:int,"sides"?:[top|right|bottom|left]}"#,
            edges,
        ),
        c(
            "image.reflection",
            "Reflection",
            r#"{"height"?:0.05..1,"gap"?:int,"opacity"?:0..1}"#,
            reflection,
        ),
        c(
            "image.spotlight",
            "Spotlight",
            r#"{"rect":[x,y,w,h],"dim"?:0..1}"#,
            spotlight,
        ),
        c(
            "image.watermark",
            "Watermark",
            r#"{"path":str,"corner"?:str,"margin"?:int,"opacity"?:0..1}"#,
            watermark,
        ),
        c("image.fade", "Fade", r#"{"side":str,"length":int}"#, fade),
        c(
            "image.perspective",
            "Perspective",
            r#"{"top"?:0..0.9,"bottom"?:0..0.9}"#,
            perspective,
        ),
        c("image.pagecurl", "Page Curl", r#"{"size"?:int}"#, pagecurl),
        c(
            "image.canvas",
            "Canvas Size",
            r##"{"width":int,"height":int,"anchor_x"?:0..1,"anchor_y"?:0..1,"color"?:"#hex"}"##,
            canvas,
        ),
        c("image.trim", "Trim", r#"{"tolerance"?:0..255}"#, trim),
        c(
            "image.combine",
            "Combine Images",
            r##"{"paths":[str],"vertical"?:bool,"spacing"?:int,"color"?:"#hex"}"##,
            combine,
        ),
        c(
            "batch.convert",
            "Batch Convert",
            r#"{"inputs":[str],"out_dir":str,"format":str,"quality"?:int,"percent"?:num}"#,
            batch,
        ),
    ]
}
