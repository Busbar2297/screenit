//! Image operations on the base raster: filters, resize, rotate/flip, border, shadow.
//! Geometry-changing operations flatten annotations first (documented in docs/gaps.md).

use crate::args::Args;
use crate::{CmdResult, CommandError, CommandSpec, Session, need_doc};
use screenit_algo as algo;
use screenit_doc::{CaptureDoc, Color};
use screenit_geom::Rect;
use screenit_raster::Raster;
use serde_json::{Value, json};

fn flatten_into(d: &mut CaptureDoc) {
    if !d.objects.is_empty() {
        let f = screenit_compose::flatten(d);
        d.set_base(f);
        d.objects.clear();
    }
}

fn region(a: &Args<'_>, s: &Session) -> Result<Rect, CommandError> {
    let full = s.doc_ref()?.base().rect();
    if !a.has("rect") {
        return Ok(full);
    }
    a.rectf("rect")?
        .to_rect()
        .intersect(&full)
        .ok_or_else(|| CommandError::Failed("rectangle is outside the image".into()))
}

fn filter(
    s: &mut Session,
    p: &Value,
    label: &str,
    extra: &[&str],
    f: impl FnOnce(&Args<'_>, &mut Raster, Rect) -> Result<(), CommandError>,
) -> CmdResult {
    let mut keys = vec!["rect"];
    keys.extend_from_slice(extra);
    let a = Args::new(p, &keys)?;
    let r = region(&a, s)?;
    s.edit(label, None, |d| {
        let mut b = d.base().clone();
        f(&a, &mut b, r)?;
        d.set_base(b);
        Ok(())
    })?;
    Ok(Value::Null)
}

fn grayscale(s: &mut Session, p: &Value) -> CmdResult {
    filter(s, p, "Grayscale", &[], |_, b, r| Ok(algo::grayscale(b, r)))
}
fn invert(s: &mut Session, p: &Value) -> CmdResult {
    filter(s, p, "Invert", &[], |_, b, r| Ok(algo::invert(b, r)))
}
fn sepia(s: &mut Session, p: &Value) -> CmdResult {
    filter(s, p, "Sepia", &[], |_, b, r| Ok(algo::sepia(b, r)))
}
fn opacity(s: &mut Session, p: &Value) -> CmdResult {
    filter(s, p, "Opacity", &["opacity"], |a, b, r| {
        Ok(algo::set_opacity(b, r, a.f32("opacity")?.clamp(0.0, 1.0)))
    })
}
fn adjust(s: &mut Session, p: &Value) -> CmdResult {
    filter(s, p, "Adjust", &["brightness", "contrast"], |a, b, r| {
        Ok(algo::adjust(
            b,
            r,
            a.opt_f32("brightness")?.unwrap_or(0.0).clamp(-1.0, 1.0),
            a.opt_f32("contrast")?.unwrap_or(0.0).clamp(-1.0, 1.0),
        ))
    })
}
fn sharpen(s: &mut Session, p: &Value) -> CmdResult {
    filter(s, p, "Sharpen", &["amount"], |a, b, r| {
        Ok(algo::sharpen(
            b,
            r,
            a.opt_f32("amount")?.unwrap_or(1.0).clamp(0.0, 10.0),
        ))
    })
}
fn blur(s: &mut Session, p: &Value) -> CmdResult {
    filter(s, p, "Blur", &["radius"], |a, b, r| {
        Ok(algo::box_blur(
            b,
            r,
            a.opt_u32("radius")?.unwrap_or(6).min(200),
        ))
    })
}
fn pixelate(s: &mut Session, p: &Value) -> CmdResult {
    filter(s, p, "Pixelate", &["block"], |a, b, r| {
        Ok(algo::pixelate(
            b,
            r,
            a.opt_u32("block")?.unwrap_or(8).clamp(1, 512),
        ))
    })
}
fn replace_color(s: &mut Session, p: &Value) -> CmdResult {
    filter(
        s,
        p,
        "Replace Color",
        &["from", "to", "tolerance"],
        |a, b, r| {
            let from = a
                .color("from")?
                .ok_or_else(|| CommandError::Params("missing parameter `from`".into()))?;
            let to = a.color("to")?.unwrap_or(Color::TRANSPARENT);
            Ok(algo::replace_color(
                b,
                r,
                from.0,
                to.0,
                a.opt_u32("tolerance")?.unwrap_or(0).min(255) as u8,
            ))
        },
    )
}

pub(crate) fn transform(
    s: &mut Session,
    label: &str,
    f: impl FnOnce(&mut CaptureDoc) -> Result<(), CommandError>,
) -> CmdResult {
    s.edit(label, None, |d| {
        flatten_into(d);
        f(d)
    })?;
    let b = s.doc_ref()?.base();
    Ok(json!({"width": b.width(), "height": b.height()}))
}

fn resize(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["width", "height", "percent"])?;
    let (cw, ch) = {
        let b = s.doc_ref()?.base();
        (b.width(), b.height())
    };
    let (w, h) = if let Some(pc) = a.opt_f32("percent")? {
        if !(1.0..=1000.0).contains(&pc) {
            return Err(CommandError::Params("percent must be 1..=1000".into()));
        }
        (
            ((cw as f32 * pc / 100.0).round() as u32).max(1),
            ((ch as f32 * pc / 100.0).round() as u32).max(1),
        )
    } else {
        (a.u32("width")?, a.u32("height")?)
    };
    transform(s, "Resize", |d| {
        let r = algo::resize(d.base(), w, h).map_err(CommandError::failed)?;
        d.set_base(r);
        Ok(())
    })
}

fn rotate(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["quarter_turns"])?;
    let n = a.opt_u32("quarter_turns")?.unwrap_or(1) % 4;
    transform(s, "Rotate", |d| {
        let mut r = d.base().clone();
        for _ in 0..n {
            r = algo::rotate90(&r).map_err(CommandError::failed)?;
        }
        d.set_base(r);
        Ok(())
    })
}

fn flip_h(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    transform(s, "Flip Horizontal", |d| {
        let mut r = d.base().clone();
        algo::flip_horizontal(&mut r);
        d.set_base(r);
        Ok(())
    })
}

fn flip_v(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    transform(s, "Flip Vertical", |d| {
        let mut r = d.base().clone();
        algo::flip_vertical(&mut r);
        d.set_base(r);
        Ok(())
    })
}

fn border(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["width", "color"])?;
    let (w, c) = (
        a.u32("width")?.min(1000),
        a.color("color")?.unwrap_or(Color::BLACK),
    );
    transform(s, "Border", |d| {
        let r = algo::border(d.base(), w, c.0).map_err(CommandError::failed)?;
        d.set_base(r);
        Ok(())
    })
}

fn shadow(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["offset", "blur", "color"])?;
    let (o, bl) = (
        a.opt_i32("offset")?.unwrap_or(6).clamp(-500, 500),
        a.opt_u32("blur")?.unwrap_or(6).min(200),
    );
    let c = a.color("color")?.unwrap_or(Color([0, 0, 0, 160]));
    transform(s, "Drop Shadow", |d| {
        let r = algo::drop_shadow(d.base(), o, bl, c.0).map_err(CommandError::failed)?;
        d.set_base(r);
        Ok(())
    })
}

fn flatten(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    transform(s, "Flatten", |_| Ok(()))
}

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, params, run| CommandSpec {
        id,
        label,
        menu: "Image",
        shortcut: "",
        params,
        enabled: need_doc,
        run,
    };
    let r = r#"{"rect"?:[x,y,w,h]}"#;
    vec![
        c("image.grayscale", "Grayscale", r, grayscale),
        c("image.invert", "Invert", r, invert),
        c("image.sepia", "Sepia", r, sepia),
        c(
            "image.opacity",
            "Opacity",
            r#"{"rect"?:[x,y,w,h],"opacity":0..1}"#,
            opacity,
        ),
        c(
            "image.adjust",
            "Brightness/Contrast",
            r#"{"rect"?:[x,y,w,h],"brightness"?:-1..1,"contrast"?:-1..1}"#,
            adjust,
        ),
        c(
            "image.sharpen",
            "Sharpen",
            r#"{"rect"?:[x,y,w,h],"amount"?:num}"#,
            sharpen,
        ),
        c(
            "image.blur",
            "Blur",
            r#"{"rect"?:[x,y,w,h],"radius"?:int}"#,
            blur,
        ),
        c(
            "image.pixelate",
            "Pixelate",
            r#"{"rect"?:[x,y,w,h],"block"?:int}"#,
            pixelate,
        ),
        c(
            "image.replace_color",
            "Replace Color",
            r##"{"rect"?:[x,y,w,h],"from":"#hex","to"?:"#hex","tolerance"?:0..255}"##,
            replace_color,
        ),
        c(
            "image.resize",
            "Resize Image",
            r#"{"width":int,"height":int}|{"percent":num}"#,
            resize,
        ),
        c(
            "image.rotate",
            "Rotate",
            r#"{"quarter_turns"?:int}"#,
            rotate,
        ),
        c("image.flip_horizontal", "Flip Horizontal", "{}", flip_h),
        c("image.flip_vertical", "Flip Vertical", "{}", flip_v),
        c(
            "image.border",
            "Border",
            r##"{"width":int,"color"?:"#hex"}"##,
            border,
        ),
        c(
            "image.shadow",
            "Drop Shadow",
            r##"{"offset"?:int,"blur"?:int,"color"?:"#hex"}"##,
            shadow,
        ),
        c("image.flatten", "Flatten Annotations", "{}", flatten),
    ]
}
