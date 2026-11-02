//! Capture commands: region/window/fullscreen/fixed/multi-area/freehand/last region, scrolling,
//! presets, import from file/clipboard. Every successful capture becomes the active document.

use crate::args::{Args, point_keys, rect_keys};
use crate::{CmdResult, CommandError, CommandSpec, Session, always, need_screen};
use screenit_capture::{self as cap, CaptureOutput, Effects, Mode, Preset, Selection};
use screenit_doc::CaptureDoc;
use screenit_geom::{Point, Rect, Size};
use screenit_raster::Raster;
use serde_json::{Value, json};

const COMMON: &[&str] = &["delay_ms", "cursor", "clipboard", "effects"];

fn keys(extra: &[&'static str]) -> Vec<&'static str> {
    let mut v = COMMON.to_vec();
    v.extend_from_slice(extra);
    v
}

pub(crate) struct Opts {
    pub(crate) delay_ms: u32,
    pub(crate) cursor: bool,
    pub(crate) clipboard: bool,
    pub(crate) effects: Effects,
}

fn parse_effects(v: &Value) -> Result<Effects, CommandError> {
    let a = Args::new(
        v,
        &[
            "border",
            "border_color",
            "corner_radius",
            "scale_percent",
            "shadow",
        ],
    )?;
    let color = a.color("border_color")?.map_or([0, 0, 0, 255], |c| c.0);
    Ok(Effects {
        border: a.opt_u32("border")?.map(|w| (w, color)),
        corner_radius: a.opt_u32("corner_radius")?,
        scale_percent: a.opt_u32("scale_percent")?,
        shadow: a.bool("shadow", false)?,
    })
}

fn opts(s: &Session, a: &Args<'_>, raw: &Value) -> Result<Opts, CommandError> {
    let effects = match raw.get("effects") {
        Some(v) if !v.is_null() => parse_effects(v)?,
        _ => Effects::default(),
    };
    Ok(Opts {
        delay_ms: a.opt_u32("delay_ms")?.unwrap_or(0).min(60_000),
        cursor: a.bool("cursor", s.prefs.include_cursor)?,
        clipboard: a.bool("clipboard", s.prefs.copy_to_clipboard)?,
        effects,
    })
}

pub(crate) fn finish(s: &mut Session, out: CaptureOutput, o: &Opts) -> CmdResult {
    let CaptureOutput {
        mut image,
        mut metadata,
        region,
    } = out;
    if o.cursor {
        if let (Some(src), Some(r)) = (s.services.screen.clone(), region) {
            if let Some(c) = src.cursor() {
                cap::composite_cursor(&mut image, Point::new(r.x, r.y), c);
            }
        }
    }
    let image = cap::apply_effects(&image, &o.effects).map_err(CommandError::failed)?;
    let secs = s.services.clock.unix_secs();
    metadata.captured_at = chrono::DateTime::from_timestamp(secs, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default();
    s.capture_counter += 1;
    let (w, h) = (image.width(), image.height());
    if o.clipboard {
        if let Some(cb) = &s.services.clipboard {
            cb.set_image(&image).map_err(CommandError::Failed)?;
        }
    }
    let mut doc = CaptureDoc::new(image);
    doc.name = format!("Capture {}", s.capture_counter);
    let (title, app) = (metadata.title.clone(), metadata.app.clone());
    doc.metadata = metadata;
    if let Some(r) = region {
        s.last_region = Some(r);
    }
    let mut saved = Value::Null;
    if let Some(dir) = s.prefs.auto_save_dir.clone() {
        std::fs::create_dir_all(&dir).map_err(CommandError::failed)?;
        let path = dir.join(format!(
            "capture-{secs}-{}.{}",
            s.capture_counter,
            screenit_format::EXT
        ));
        screenit_format::save(&doc, &path).map_err(CommandError::failed)?;
        s.library
            .add(screenit_library::Entry {
                id: 0,
                kind: screenit_library::Kind::Image,
                path: path.clone(),
                title: if title.is_empty() {
                    doc.name.clone()
                } else {
                    title
                },
                app,
                url: doc.metadata.url.clone(),
                created: secs,
                width: w,
                height: h,
                tags: vec![],
            })
            .map_err(CommandError::failed)?;
        saved = json!(path.to_string_lossy());
    }
    s.new_document(doc);
    Ok(json!({"width": w, "height": h, "region": region, "saved": saved}))
}

fn run_selection(s: &mut Session, sel: Selection, o: &Opts) -> CmdResult {
    if o.delay_ms > 0 {
        s.services.clock.sleep_ms(o.delay_ms);
    }
    let src = s.screen()?;
    let out = cap::capture(src.as_ref(), &sel, s.last_region).map_err(CommandError::failed)?;
    finish(s, out, o)
}

fn region(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &keys(&["x", "y", "w", "h"]))?;
    let o = opts(s, &a, p)?;
    run_selection(
        s,
        Selection::Region {
            rect: rect_keys(&a)?,
        },
        &o,
    )
}

fn fixed(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &keys(&["x", "y", "w", "h"]))?;
    let o = opts(s, &a, p)?;
    let r = rect_keys(&a)?;
    run_selection(
        s,
        Selection::Fixed {
            size: Size { w: r.w, h: r.h },
            at: Point::new(r.x, r.y),
        },
        &o,
    )
}

fn window(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &keys(&["id"]))?;
    let o = opts(s, &a, p)?;
    run_selection(s, Selection::Window { id: a.u64("id")? }, &o)
}

fn fullscreen(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &keys(&["monitor"]))?;
    let o = opts(s, &a, p)?;
    run_selection(
        s,
        Selection::Fullscreen {
            monitor: a.opt_u32("monitor")?,
        },
        &o,
    )
}

fn last_region(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &keys(&[]))?;
    let o = opts(s, &a, p)?;
    run_selection(s, Selection::LastRegion, &o)
}

fn multi_area(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &keys(&["rects"]))?;
    let o = opts(s, &a, p)?;
    let rects = a
        .array("rects")?
        .iter()
        .map(|v| {
            let r = Args::new(v, &["x", "y", "w", "h"])?;
            rect_keys(&r)
        })
        .collect::<Result<Vec<Rect>, _>>()?;
    run_selection(s, Selection::MultiArea { rects }, &o)
}

fn freehand(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &keys(&["points"]))?;
    let o = opts(s, &a, p)?;
    let points = a
        .array("points")?
        .iter()
        .map(point_keys)
        .collect::<Result<Vec<_>, _>>()?;
    run_selection(s, Selection::Freehand { points }, &o)
}

fn windows(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let w = s.screen()?.windows().map_err(CommandError::failed)?;
    Ok(json!({"windows": w}))
}

fn monitors(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let m = s.screen()?.monitors().map_err(CommandError::failed)?;
    Ok(json!({"monitors": m}))
}

fn window_at(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["x", "y"])?;
    let pt = Point::new(a.i32("x")?, a.i32("y")?);
    let w = s.screen()?.windows().map_err(CommandError::failed)?;
    Ok(json!({"window": cap::window_at(&w, pt)}))
}

// ---- scrolling capture (manual): begin, add frames while the user scrolls, finish stitches ----

fn scroll_begin(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["x", "y", "w", "h"])?;
    let r = rect_keys(&a)?;
    let src = s.screen()?;
    let first = src.grab(r).map_err(CommandError::failed)?;
    s.scroll_frames = Some((r, vec![first]));
    Ok(json!({"frames": 1}))
}

fn scroll_add(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let src = s.screen()?;
    let (r, frames) = s
        .scroll_frames
        .as_mut()
        .ok_or_else(|| CommandError::Failed("no scrolling capture in progress".into()))?;
    if frames.len() >= 2000 {
        return Err(CommandError::Failed("too many frames".into()));
    }
    frames.push(src.grab(*r).map_err(CommandError::failed)?);
    Ok(json!({"frames": frames.len()}))
}

fn scroll_finish(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["horizontal", "clipboard"])?;
    let (r, frames) = s
        .scroll_frames
        .take()
        .ok_or_else(|| CommandError::Failed("no scrolling capture in progress".into()))?;
    let img = if a.bool("horizontal", false)? {
        cap::stitch_horizontal(&frames)
    } else {
        cap::stitch_vertical(&frames)
    }
    .map_err(CommandError::failed)?;
    let src = s.screen()?;
    let mut metadata = screenit_doc::Metadata::default();
    if let Some(fg) = src.foreground() {
        metadata.title = fg.title;
        metadata.app = fg.process;
    }
    metadata.source = "scrolling".into();
    let o = Opts {
        delay_ms: 0,
        cursor: false,
        clipboard: a.bool("clipboard", false)?,
        effects: Effects::default(),
    };
    finish(
        s,
        CaptureOutput {
            image: img,
            metadata,
            region: Some(r),
        },
        &o,
    )
}

fn scroll_cancel(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    Ok(json!({"cancelled": s.scroll_frames.take().is_some()}))
}

// ---- presets ----

fn preset_list(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    Ok(json!({"presets": s.presets, "conflicts": cap::hotkey_conflicts(&s.presets)}))
}

fn preset_save(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["preset"])?;
    let v = p
        .get("preset")
        .ok_or_else(|| CommandError::Params("missing parameter `preset`".into()))?;
    let _ = a;
    let preset: Preset =
        serde_json::from_value(v.clone()).map_err(|e| CommandError::Params(e.to_string()))?;
    if preset.name.trim().is_empty() || preset.name.len() > 100 {
        return Err(CommandError::Params(
            "preset name must be 1..=100 characters".into(),
        ));
    }
    if let Some(h) = &preset.hotkey {
        cap::parse_hotkey(h).map_err(|e| CommandError::Params(e.to_string()))?;
    }
    s.presets
        .retain(|x| !x.name.eq_ignore_ascii_case(&preset.name));
    s.presets.push(preset);
    s.persist_settings()?;
    Ok(json!({"count": s.presets.len(), "conflicts": cap::hotkey_conflicts(&s.presets)}))
}

fn preset_delete(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["name"])?;
    let name = a.str("name")?;
    let before = s.presets.len();
    s.presets.retain(|x| !x.name.eq_ignore_ascii_case(name));
    if s.presets.len() == before {
        return Err(CommandError::Failed(format!("no preset named `{name}`")));
    }
    s.persist_settings()?;
    Ok(json!({"count": s.presets.len()}))
}

fn preset_run(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["name", "x", "y", "w", "h"])?;
    let name = a.str("name")?;
    let preset = s
        .presets
        .iter()
        .find(|x| x.name.eq_ignore_ascii_case(name))
        .cloned()
        .ok_or_else(|| CommandError::Failed(format!("no preset named `{name}`")))?;
    if preset.mode == Mode::Video {
        return Err(CommandError::Failed(
            "video presets are started with record.start".into(),
        ));
    }
    let sel = if a.has("w") {
        Selection::Region {
            rect: rect_keys(&a)?,
        }
    } else {
        s.selection_of(&preset)
    };
    if let Selection::Region { rect } = &sel {
        if rect.is_empty() {
            return Err(CommandError::Params(
                "this preset needs a region (x, y, w, h)".into(),
            ));
        }
    }
    let o = Opts {
        delay_ms: preset.delay_ms,
        cursor: preset.include_cursor,
        clipboard: preset.copy_to_clipboard,
        effects: preset.effects.clone(),
    };
    run_selection(s, sel, &o)
}

fn hotkey_check(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["hotkey"])?;
    let _ = s;
    let h = cap::parse_hotkey(a.str("hotkey")?).map_err(|e| CommandError::Params(e.to_string()))?;
    Ok(json!({"ctrl": h.ctrl, "shift": h.shift, "alt": h.alt, "win": h.win, "vk": h.vk}))
}

// ---- import ----

fn import_file(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path"])?;
    let path = std::path::Path::new(a.str("path")?);
    let bytes = std::fs::read(path).map_err(CommandError::failed)?;
    let img = screenit_codecs::decode(&bytes).map_err(CommandError::failed)?;
    let (w, h) = (img.width(), img.height());
    let mut doc = CaptureDoc::new(img);
    doc.name = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    doc.metadata.source = "import".into();
    s.new_document(doc);
    Ok(json!({"width": w, "height": h}))
}

fn import_clipboard(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let cb = s
        .services
        .clipboard
        .clone()
        .ok_or_else(|| CommandError::Disabled("no clipboard available".into()))?;
    let img: Raster = cb.get_image().map_err(CommandError::Failed)?;
    let (w, h) = (img.width(), img.height());
    let mut doc = CaptureDoc::new(img);
    doc.name = "Clipboard".into();
    doc.metadata.source = "clipboard".into();
    s.new_document(doc);
    Ok(json!({"width": w, "height": h}))
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
    let common = r#"delay_ms?:int, cursor?:bool, clipboard?:bool, effects?:{border,border_color,corner_radius,scale_percent,shadow}"#;
    let _ = common;
    vec![
        c(
            "capture.region",
            "Capture Region",
            "Capture",
            "PrintScreen",
            r#"{"x":int,"y":int,"w":int,"h":int,"delay_ms"?:int,"cursor"?:bool,"clipboard"?:bool,"effects"?:{}}"#,
            need_screen,
            region,
        ),
        c(
            "capture.window",
            "Capture Window",
            "Capture",
            "",
            r#"{"id":int,"delay_ms"?:int,"cursor"?:bool,"clipboard"?:bool,"effects"?:{}}"#,
            need_screen,
            window,
        ),
        c(
            "capture.fullscreen",
            "Capture Fullscreen",
            "Capture",
            "Ctrl+Shift+F",
            r#"{"monitor"?:int,"delay_ms"?:int,"cursor"?:bool,"clipboard"?:bool,"effects"?:{}}"#,
            need_screen,
            fullscreen,
        ),
        c(
            "capture.fixed",
            "Capture Fixed Size",
            "Capture",
            "",
            r#"{"x":int,"y":int,"w":int,"h":int,...}"#,
            need_screen,
            fixed,
        ),
        c(
            "capture.last_region",
            "Repeat Last Region",
            "Capture",
            "",
            r#"{"delay_ms"?:int,"cursor"?:bool,"clipboard"?:bool,"effects"?:{}}"#,
            need_screen,
            last_region,
        ),
        c(
            "capture.multi_area",
            "Capture Multiple Areas",
            "Capture",
            "",
            r#"{"rects":[{"x","y","w","h"}],...}"#,
            need_screen,
            multi_area,
        ),
        c(
            "capture.freehand",
            "Capture Freehand",
            "Capture",
            "",
            r#"{"points":[[x,y],...],...}"#,
            need_screen,
            freehand,
        ),
        c(
            "capture.windows",
            "List Windows",
            "Capture",
            "",
            "{}",
            need_screen,
            windows,
        ),
        c(
            "capture.monitors",
            "List Monitors",
            "Capture",
            "",
            "{}",
            need_screen,
            monitors,
        ),
        c(
            "capture.window_at",
            "Window Under Point",
            "Capture",
            "",
            r#"{"x":int,"y":int}"#,
            need_screen,
            window_at,
        ),
        c(
            "capture.scroll.begin",
            "Begin Scrolling Capture",
            "Capture",
            "",
            r#"{"x":int,"y":int,"w":int,"h":int}"#,
            need_screen,
            scroll_begin,
        ),
        c(
            "capture.scroll.add_frame",
            "Add Scrolling Frame",
            "Capture",
            "",
            "{}",
            need_screen,
            scroll_add,
        ),
        c(
            "capture.scroll.finish",
            "Finish Scrolling Capture",
            "Capture",
            "",
            r#"{"horizontal"?:bool,"clipboard"?:bool}"#,
            need_screen,
            scroll_finish,
        ),
        c(
            "capture.scroll.cancel",
            "Cancel Scrolling Capture",
            "Capture",
            "",
            "{}",
            always,
            scroll_cancel,
        ),
        c(
            "capture.preset.list",
            "List Capture Presets",
            "Capture",
            "",
            "{}",
            always,
            preset_list,
        ),
        c(
            "capture.preset.save",
            "Save Capture Preset",
            "Capture",
            "",
            r#"{"preset":{name,mode,selection,delay_ms,include_cursor,copy_to_clipboard,open_in_editor,hotkey,effects}}"#,
            always,
            preset_save,
        ),
        c(
            "capture.preset.delete",
            "Delete Capture Preset",
            "Capture",
            "",
            r#"{"name":str}"#,
            always,
            preset_delete,
        ),
        c(
            "capture.preset.run",
            "Run Capture Preset",
            "Capture",
            "",
            r#"{"name":str,"x"?:int,"y"?:int,"w"?:int,"h"?:int}"#,
            need_screen,
            preset_run,
        ),
        c(
            "capture.hotkey.parse",
            "Parse Hotkey",
            "Capture",
            "",
            r#"{"hotkey":str}"#,
            always,
            hotkey_check,
        ),
        c(
            "capture.import.file",
            "Import Image From File",
            "File",
            "Ctrl+O",
            r#"{"path":str}"#,
            always,
            import_file,
        ),
        c(
            "capture.import.clipboard",
            "Import Image From Clipboard",
            "File",
            "",
            "{}",
            always,
            import_clipboard,
        ),
    ]
}
