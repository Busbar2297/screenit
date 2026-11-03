//! "ML-adjacent" features implemented classically: Smart Redact (OCR + pattern scanners), OCR text
//! replace, background removal, time-lapse capture.

use crate::args::{Args, rect_keys};
use crate::{CmdResult, CommandError, CommandSpec, Session, always, need_doc};
use screenit_algo::redact::{self, Kind};
use screenit_doc::{Color, ObjectKind, Style};
use screenit_geom::{Rect, RectF};
use screenit_record::RecordError;
use serde_json::{Value, json};
use std::path::PathBuf;

fn ocr_of(s: &Session) -> Result<std::sync::Arc<dyn screenit_capture::Ocr>, CommandError> {
    s.services
        .ocr
        .clone()
        .ok_or_else(|| CommandError::Disabled("no OCR engine available".into()))
}

/// Estimate the pixel rectangle of chars `[a, b)` within an OCR line (proportional glyph widths).
pub(crate) fn span_rect(line: Rect, n_chars: usize, a: usize, b: usize, pad: i32) -> Rect {
    let n = n_chars.max(1) as f32;
    let x0 = line.x + (line.w as f32 * a as f32 / n) as i32 - pad;
    let x1 = line.x + (line.w as f32 * b as f32 / n).ceil() as i32 + pad;
    Rect::new(
        x0,
        line.y - pad,
        (x1 - x0).max(1) as u32,
        line.h + 2 * pad as u32,
    )
}

fn smart_redact(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["kinds", "terms", "mode", "pad"])?;
    let kinds: Vec<Kind> = if a.has("kinds") {
        a.array("kinds")?
            .iter()
            .map(|v| {
                v.as_str().and_then(Kind::parse).ok_or_else(|| {
                    CommandError::Params("kinds: email|phone|card|ssn|ip|url".into())
                })
            })
            .collect::<Result<_, _>>()?
    } else {
        Kind::DEFAULT.to_vec()
    };
    let terms: Vec<String> = if a.has("terms") {
        a.array("terms")?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_lowercase))
            .filter(|t| !t.is_empty())
            .collect()
    } else {
        vec![]
    };
    let pixelate = match a.opt_str("mode")?.unwrap_or("pixelate") {
        "pixelate" => true,
        "blur" => false,
        o => {
            return Err(CommandError::Params(format!(
                "unknown mode `{o}` (pixelate|blur)"
            )));
        }
    };
    let pad = a.opt_i32("pad")?.unwrap_or(2).clamp(0, 50);
    let ocr = ocr_of(s)?;
    let flat = screenit_compose::flatten(s.doc_ref()?);
    let lines = ocr.recognize(&flat).map_err(CommandError::failed)?;
    let mut rects: Vec<Rect> = Vec::new();
    for l in &lines {
        let n = l.text.chars().count();
        let mut spans: Vec<(usize, usize)> = redact::find(&l.text, &kinds)
            .into_iter()
            .map(|(_, x, y)| (x, y))
            .collect();
        let lower = l.text.to_lowercase();
        for t in &terms {
            let mut from = 0;
            while let Some(pos) = lower[from..].find(t.as_str()) {
                let bs = from + pos;
                let (ca, cb) = (
                    lower[..bs].chars().count(),
                    lower[..bs + t.len()].chars().count(),
                );
                spans.push((ca, cb));
                from = bs + t.len();
            }
        }
        for (x, y) in spans {
            rects.push(span_rect(l.rect, n, x, y.min(n), pad));
        }
    }
    let count = rects.len();
    s.edit("Smart Redact", None, |d| {
        for r in &rects {
            d.add(
                ObjectKind::Blur {
                    rect: RectF::new(r.x as f32, r.y as f32, r.w as f32, r.h as f32),
                    radius: 12,
                    pixelate,
                },
                Style::default(),
            )
            .map_err(CommandError::failed)?;
        }
        Ok(())
    })?;
    Ok(json!({"redacted": count}))
}

fn ocr_replace(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["line", "text"])?;
    let (idx, text) = (a.u32("line")? as usize, a.str("text")?.to_string());
    let ocr = ocr_of(s)?;
    let flat = screenit_compose::flatten(s.doc_ref()?);
    let lines = ocr.recognize(&flat).map_err(CommandError::failed)?;
    let l = lines.get(idx).ok_or_else(|| {
        CommandError::Failed(format!("no recognised line {idx} ({} found)", lines.len()))
    })?;
    let r = l
        .rect
        .intersect(&flat.rect())
        .ok_or_else(|| CommandError::Failed("line is outside the image".into()))?;
    // Background = most frequent colour on the line's border ring.
    let mut counts: std::collections::HashMap<[u8; 4], u32> = std::collections::HashMap::new();
    for x in r.x..r.x + r.w as i32 {
        for y in [r.y, r.y + r.h as i32 - 1] {
            if let Some(px) = flat.get(x, y) {
                *counts.entry(px).or_default() += 1;
            }
        }
    }
    let bg = counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map_or([255, 255, 255, 255], |(c, _)| c);
    let lum = 0.299 * f32::from(bg[0]) + 0.587 * f32::from(bg[1]) + 0.114 * f32::from(bg[2]);
    let fg = if lum > 128.0 {
        Color::BLACK
    } else {
        Color::WHITE
    };
    let size = (r.h as f32 * 0.8).max(6.0);
    s.edit("Replace Text", None, |d| {
        let mut b = d.base().clone();
        b.fill_rect(r, bg);
        d.set_base(b);
        d.add(
            ObjectKind::Text {
                pos: screenit_geom::PointF::new(r.x as f32, r.y as f32),
                text,
                size,
                width: 0.0,
                height: 0.0,
            },
            Style {
                stroke: fg,
                ..Style::default()
            },
        )
        .map_err(CommandError::failed)?;
        Ok(())
    })?;
    Ok(json!({"replaced": l.text}))
}

fn remove_background(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["tolerance"])?;
    let tol = a.opt_u32("tolerance")?.unwrap_or(24).min(255) as u8;
    let mut cleared = 0usize;
    s.edit("Remove Background", None, |d| {
        let mut b = d.base().clone();
        cleared = screenit_algo::fx::remove_background(&mut b, tol);
        d.set_base(b);
        Ok(())
    })?;
    Ok(json!({"cleared": cleared}))
}

fn timelapse(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(
        p,
        &["x", "y", "w", "h", "interval_ms", "count", "output", "fps"],
    )?;
    let region = rect_keys(&a)?;
    let interval = a.u32("interval_ms")?.clamp(20, 60_000);
    let count = a.u32("count")?;
    if !(2..=5000).contains(&count) {
        return Err(CommandError::Params("count must be 2..=5000".into()));
    }
    let fps = a.opt_u32("fps")?.unwrap_or(10).clamp(1, 60);
    let out = PathBuf::from(a.str("output")?);
    if out
        .extension()
        .is_none_or(|e| !e.eq_ignore_ascii_case("mp4"))
    {
        return Err(CommandError::Params("output must be an .mp4 path".into()));
    }
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(CommandError::failed)?;
    }
    let screen = s.screen()?;
    let clock = s.services.clock.clone();
    let writer = s
        .services
        .video_writer
        .clone()
        .ok_or_else(|| CommandError::Disabled("no video writer available".into()))?;
    let (w, h) = (region.w & !1, region.h & !1);
    if w == 0 || h == 0 {
        return Err(CommandError::Params("region too small".into()));
    }
    let grab = Rect::new(region.x, region.y, w, h);
    let mut i = 0u32;
    let mut frames = std::iter::from_fn(|| {
        if i >= count {
            return None;
        }
        if i > 0 {
            clock.sleep_ms(interval);
        }
        i += 1;
        Some(
            screen
                .grab(grab)
                .map_err(|e| RecordError::Platform(e.to_string())),
        )
    });
    let res = writer
        .write(&out, w, h, fps, &mut frames)
        .map_err(CommandError::failed)?;
    Ok(json!({"result": res}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "editor.smart_redact",
            label: "Smart Redact",
            menu: "Edit",
            shortcut: "",
            params: r#"{"kinds"?:[email|phone|card|ssn|ip|url],"terms"?:[str],"mode"?:pixelate|blur,"pad"?:int}"#,
            enabled: need_doc,
            run: smart_redact,
        },
        CommandSpec {
            id: "editor.ocr.replace",
            label: "Replace Recognised Text",
            menu: "Edit",
            shortcut: "",
            params: r#"{"line":int,"text":str}"#,
            enabled: need_doc,
            run: ocr_replace,
        },
        CommandSpec {
            id: "image.remove_background",
            label: "Remove Background",
            menu: "Image",
            shortcut: "",
            params: r#"{"tolerance"?:0..255}"#,
            enabled: need_doc,
            run: remove_background,
        },
        CommandSpec {
            id: "capture.timelapse",
            label: "Time-lapse Capture",
            menu: "Capture",
            shortcut: "",
            params: r#"{"x","y","w","h","interval_ms":int,"count":2..5000,"output":str.mp4,"fps"?:int}"#,
            enabled: always,
            run: timelapse,
        },
    ]
}
