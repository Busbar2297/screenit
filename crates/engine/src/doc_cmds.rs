//! Document commands: new/open/save/export/info/close and undo/redo.

use crate::args::Args;
use crate::{CmdResult, CommandError, CommandSpec, Session, always, need_doc};
use screenit_codecs::{self as codecs, Format};
use screenit_doc::CaptureDoc;
use screenit_raster::Raster;
use serde_json::{Value, json};
use std::path::Path;

fn new(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["width", "height", "color"])?;
    let (w, h) = (a.u32("width")?, a.u32("height")?);
    if w == 0 || h == 0 {
        return Err(CommandError::Params(
            "width and height must be at least 1".into(),
        ));
    }
    let c = a.color("color")?.map_or([255, 255, 255, 255], |c| c.0);
    let r = Raster::new(w, h, c).map_err(CommandError::failed)?;
    let mut d = CaptureDoc::new(r);
    d.name = "Untitled".into();
    s.new_document(d);
    Ok(json!({"width": w, "height": h}))
}

fn open(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path"])?;
    let path = Path::new(a.str("path")?);
    let mut doc = if screenit_format::is_project(path) {
        screenit_format::load(path).map_err(CommandError::failed)?
    } else {
        let img = codecs::decode(&std::fs::read(path).map_err(CommandError::failed)?)
            .map_err(CommandError::failed)?;
        CaptureDoc::new(img)
    };
    if doc.name.is_empty() {
        doc.name = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
    let (w, h, n) = (doc.base().width(), doc.base().height(), doc.objects.len());
    s.new_document(doc);
    Ok(json!({"width": w, "height": h, "objects": n}))
}

fn save(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path"])?;
    let path = Path::new(a.str("path")?);
    screenit_format::save(s.doc_ref()?, path).map_err(CommandError::failed)?;
    Ok(json!({"path": path.to_string_lossy()}))
}

fn export(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "quality"])?;
    let path = Path::new(a.str("path")?);
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    let quality = a.opt_u32("quality")?.unwrap_or(90).clamp(1, 100) as u8;
    let flat = screenit_compose::flatten(s.doc_ref()?);
    let bytes = if ext.eq_ignore_ascii_case("pdf") {
        codecs::encode_pdf(std::slice::from_ref(&flat), quality).map_err(CommandError::failed)?
    } else {
        let fmt = Format::from_extension(&ext).map_err(CommandError::failed)?;
        codecs::encode(&flat, fmt, quality).map_err(CommandError::failed)?
    };
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, &bytes).map_err(CommandError::failed)?;
    std::fs::rename(&tmp, path).map_err(CommandError::failed)?;
    Ok(
        json!({"path": path.to_string_lossy(), "bytes": bytes.len(), "width": flat.width(), "height": flat.height()}),
    )
}

fn info(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let d = s.doc_ref()?;
    Ok(json!({
        "name": d.name, "width": d.base().width(), "height": d.base().height(),
        "objects": d.objects, "metadata": d.metadata,
        "canUndo": s.history.can_undo(), "canRedo": s.history.can_redo(), "history": s.history.labels(),
    }))
}

fn pixel(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["x", "y"])?;
    let flat = screenit_compose::flatten(s.doc_ref()?);
    let px = flat
        .get(a.i32("x")?, a.i32("y")?)
        .ok_or_else(|| CommandError::Failed("pixel outside the image".into()))?;
    Ok(json!({"rgba": px}))
}

fn close(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    s.doc = None;
    s.history = Default::default();
    Ok(Value::Null)
}

fn undo(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let doc = s
        .doc
        .as_mut()
        .ok_or_else(|| CommandError::Disabled("no document open".into()))?;
    let label = s.history.undo(doc).map_err(CommandError::failed)?;
    Ok(json!({"undone": label}))
}

fn redo(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let doc = s
        .doc
        .as_mut()
        .ok_or_else(|| CommandError::Disabled("no document open".into()))?;
    let label = s.history.redo(doc).map_err(CommandError::failed)?;
    Ok(json!({"redone": label}))
}

fn copy(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let cb = s
        .services
        .clipboard
        .clone()
        .ok_or_else(|| CommandError::Disabled("no clipboard available".into()))?;
    cb.set_image(&screenit_compose::flatten(s.doc_ref()?))
        .map_err(CommandError::Failed)?;
    Ok(Value::Null)
}

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, menu, shortcut, params, enabled, run| CommandSpec {
        id,
        label,
        menu,
        shortcut,
        params,
        enabled,
        run,
    };
    vec![
        c(
            "doc.new",
            "New Image",
            "File",
            "Ctrl+N",
            r##"{"width":int,"height":int,"color"?:"#rrggbb"}"##,
            always,
            new,
        ),
        c(
            "doc.open",
            "Open",
            "File",
            "Ctrl+O",
            r#"{"path":str (.scit, legacy .sxs, or image)}"#,
            always,
            open,
        ),
        c(
            "doc.save",
            "Save",
            "File",
            "Ctrl+S",
            r#"{"path":str (.scit)}"#,
            need_doc,
            save,
        ),
        c(
            "doc.export",
            "Export As",
            "File",
            "Ctrl+Shift+S",
            r#"{"path":str (png|jpg|gif|bmp|tiff|webp),"quality"?:1..100}"#,
            need_doc,
            export,
        ),
        c(
            "doc.info",
            "Document Info",
            "File",
            "",
            "{}",
            need_doc,
            info,
        ),
        c(
            "doc.pixel",
            "Read Pixel",
            "File",
            "",
            r#"{"x":int,"y":int}"#,
            need_doc,
            pixel,
        ),
        c(
            "doc.close",
            "Close",
            "File",
            "Ctrl+W",
            "{}",
            need_doc,
            close,
        ),
        c("edit.undo", "Undo", "Edit", "Ctrl+Z", "{}", need_doc, undo),
        c("edit.redo", "Redo", "Edit", "Ctrl+Y", "{}", need_doc, redo),
        c(
            "edit.copy",
            "Copy All",
            "Edit",
            "Ctrl+Shift+C",
            "{}",
            need_doc,
            copy,
        ),
    ]
}
