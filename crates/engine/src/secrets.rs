//! Hide secrets: find passwords, keys and tokens in an image and cover them.
//!
//! Three detectors feed one list of rectangles: password fields reported by the OS (exact),
//! pattern scanners over OCR text (offline), and an optional language-model pass over the same
//! OCR text. The scan is a plain struct of shared handles so it can run off the UI thread.

use crate::args::Args;
use crate::ml_cmds::span_rect;
use crate::{CmdResult, CommandError, CommandSpec, Session, need_doc};
use screenit_algo::redact::{self, Kind};
use screenit_capture::{LlmConfig, LlmOcr, Ocr, OcrLine};
use screenit_doc::{ObjectKind, Style};
use screenit_geom::{Rect, RectF};
use screenit_raster::Raster;
use serde_json::{Value, json};
use std::sync::Arc;

/// Lines and characters per line offered to the language model.
const LLM_LINES: usize = 400;
const LLM_LINE_CHARS: usize = 300;
/// Findings accepted from one language-model reply.
const LLM_MAX_FINDINGS: usize = 200;
/// Rectangles accepted by `editor.redact.apply`.
const MAX_RECTS: usize = 500;

/// A rectangle to cover and the detector that found it.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub rect: Rect,
    pub by: &'static str,
}

/// Everything needed to scan one image; cheap to clone and `Send`.
#[derive(Clone)]
pub struct SecretScan {
    pub(crate) ocr: Option<Arc<dyn Ocr>>,
    pub(crate) llm: Option<Arc<dyn LlmOcr>>,
    pub(crate) cfg: LlmConfig,
    pub(crate) lang: String,
    pub(crate) use_llm: bool,
    /// Password fields, already in image coordinates.
    pub(crate) fields: Vec<Rect>,
    pub(crate) pad: i32,
}

pub struct ScanResult {
    pub found: Vec<Found>,
    /// Something could not run (no OCR engine, model offline); the rest still ran.
    pub warning: Option<String>,
}

fn contains(outer: &Rect, inner: &Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x + inner.w as i32 <= outer.x + outer.w as i32
        && inner.y + inner.h as i32 <= outer.y + outer.h as i32
}

fn push(found: &mut Vec<Found>, rect: Rect, by: &'static str) {
    if rect.w == 0 || rect.h == 0 || found.iter().any(|f| contains(&f.rect, &rect)) {
        return;
    }
    found.retain(|f| !contains(&rect, &f.rect));
    found.push(Found { rect, by });
}

/// The line below `label` (or to its right) that holds the value of a label-only line.
fn value_line<'a>(lines: &'a [OcrLine], label: &OcrLine) -> Option<&'a OcrLine> {
    let l = &label.rect;
    let (cy, h) = (l.y + l.h as i32 / 2, l.h as i32);
    lines
        .iter()
        .filter(|o| !std::ptr::eq(*o, label) && !o.text.trim().is_empty())
        .filter(|o| {
            let r = &o.rect;
            let right =
                (r.y + r.h as i32 / 2 - cy).abs() < h / 2 + 1 && r.x >= l.x + l.w as i32 - 2;
            let below = r.y > l.y + h / 2
                && r.y <= l.y + l.h as i32 + h
                && r.x < l.x + l.w as i32 + h * 4
                && r.x + r.w as i32 > l.x;
            right || below
        })
        .min_by_key(|o| {
            let (dx, dy) = (o.rect.x - l.x, o.rect.y - l.y);
            dx.abs() + dy.abs()
        })
}

/// The `[{"line":N,"text":"..."}]` list in a model reply (code fences and chatter tolerated).
/// `N` counts from 1 as shown in the prompt.
pub(crate) fn parse_findings(reply: &str) -> Vec<(usize, String)> {
    let (Some(a), Some(b)) = (reply.find('['), reply.rfind(']')) else {
        return Vec::new();
    };
    if b <= a {
        return Vec::new();
    }
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&reply[a..=b]) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|v| {
            let line = v.get("line")?.as_u64()? as usize;
            let text = v.get("text").and_then(Value::as_str).unwrap_or("");
            (line >= 1).then(|| (line, text.chars().take(LLM_LINE_CHARS).collect()))
        })
        .take(LLM_MAX_FINDINGS)
        .collect()
}

fn prompt(lines: &[OcrLine]) -> String {
    let mut p = String::from(
        "Below are the text lines read from a screenshot, numbered from 1. Find every password, \
API key, access token, private key, secret or other credential. Reply with ONLY a JSON array of \
objects {\"line\": <number>, \"text\": \"<the exact secret as written in that line>\"}. Reply [] \
if there are none.\n\n",
    );
    for (i, l) in lines.iter().take(LLM_LINES).enumerate() {
        let t: String = l.text.chars().take(LLM_LINE_CHARS).collect();
        p.push_str(&format!("{}: {}\n", i + 1, t));
    }
    p
}

fn char_span(line: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let at = line.find(needle)?;
    let a = line[..at].chars().count();
    Some((a, a + needle.chars().count()))
}

impl SecretScan {
    pub fn run(&self, image: &Raster) -> Result<ScanResult, CommandError> {
        let mut found: Vec<Found> = Vec::new();
        let mut warning = None;
        let bounds = Rect::new(0, 0, image.width(), image.height());
        for f in &self.fields {
            if let Some(r) = f.intersect(&bounds) {
                push(&mut found, grow(r, 2, &bounds), "field");
            }
        }
        let lines = match &self.ocr {
            Some(o) => match o.recognize_lang(image, &self.lang) {
                Ok(l) => l,
                Err(e) => {
                    warning = Some(format!("text recognition failed: {e}"));
                    Vec::new()
                }
            },
            None => {
                warning = Some("no text recognition engine available".into());
                Vec::new()
            }
        };
        for l in &lines {
            let n = l.text.chars().count();
            for (kind, a, b) in redact::find(&l.text, &Kind::SECRETS) {
                // A value after a label may contain spaces: cover the rest of the line.
                let b = if kind == Kind::LabeledSecret { n } else { b };
                let by = match kind {
                    Kind::Secret => "secret",
                    Kind::LabeledSecret => "label",
                    _ => "masked",
                };
                push(
                    &mut found,
                    grow(span_rect(l.rect, n, a, b.min(n), self.pad), 0, &bounds),
                    by,
                );
            }
            if redact::label_only(&l.text) {
                if let Some(v) = value_line(&lines, l) {
                    push(&mut found, grow(v.rect, self.pad, &bounds), "label");
                }
            }
        }
        if self.use_llm && !lines.is_empty() {
            match &self.llm {
                Some(llm) => match llm.complete_text(&prompt(&lines), &self.cfg) {
                    Ok(reply) => {
                        for (no, text) in parse_findings(&reply) {
                            let Some(l) = lines.get(no - 1) else { continue };
                            let n = l.text.chars().count();
                            let rect = match char_span(&l.text, &text) {
                                Some((a, b)) => span_rect(l.rect, n, a, b.min(n), self.pad),
                                None => grow(l.rect, self.pad, &bounds),
                            };
                            push(&mut found, grow(rect, 0, &bounds), "model");
                        }
                    }
                    Err(e) => warning = Some(format!("language model: {e}")),
                },
                None => warning = Some("no language model available".into()),
            }
        }
        Ok(ScanResult { found, warning })
    }
}

/// Grow by `by` pixels and clip to the image.
fn grow(r: Rect, by: i32, bounds: &Rect) -> Rect {
    let g = Rect::new(
        r.x - by,
        r.y - by,
        (r.w as i32 + 2 * by).max(1) as u32,
        (r.h as i32 + 2 * by).max(1) as u32,
    );
    g.intersect(bounds).unwrap_or(Rect::new(0, 0, 0, 0))
}

impl Session {
    /// A scan using the current settings. `fields` are password fields in image coordinates;
    /// `use_llm` overrides the privacy setting.
    pub fn secret_scan(&self, fields: Vec<Rect>, use_llm: Option<bool>) -> SecretScan {
        let o = &self.settings.ocr;
        SecretScan {
            ocr: self.services.ocr.clone(),
            llm: self.services.llm.clone(),
            cfg: LlmConfig {
                url: o.llm_url.clone(),
                model: o.llm_model.clone(),
                api_key: o.llm_api_key.clone(),
                timeout_secs: o.llm_timeout_secs,
            },
            lang: o.language.clone(),
            use_llm: use_llm.unwrap_or(self.settings.privacy.use_llm),
            fields,
            pad: 4,
        }
    }

    /// Cover the password fields and secrets in a fresh capture, in the pixels themselves.
    /// Never fails a capture: problems come back as a warning. `origin` is where the image sits
    /// on the virtual screen.
    pub(crate) fn burn_secrets(
        &self,
        image: &mut Raster,
        origin: Option<Rect>,
    ) -> (usize, Option<String>) {
        let fields = match (self.services.screen.as_ref(), origin) {
            (Some(src), Some(r)) => src
                .secret_fields(r)
                .into_iter()
                .map(|f| Rect::new(f.x - r.x, f.y - r.y, f.w, f.h))
                .collect(),
            _ => Vec::new(),
        };
        let scan = self.secret_scan(fields, Some(false));
        match scan.run(image) {
            Ok(res) => {
                let pixelate = self.settings.privacy.mode != "blur";
                for f in &res.found {
                    cover(image, f.rect, pixelate);
                }
                (res.found.len(), res.warning)
            }
            Err(e) => (0, Some(e.to_string())),
        }
    }
}

/// Destroy the pixels of `rect` (same look as a Blur object).
fn cover(image: &mut Raster, rect: Rect, pixelate: bool) {
    if pixelate {
        screenit_algo::pixelate(image, rect, 12);
    } else {
        screenit_algo::box_blur(image, rect, 12);
        screenit_algo::box_blur(image, rect, 12);
    }
}

fn mode_arg(a: &Args<'_>, s: &Session) -> Result<bool, CommandError> {
    let default = s.settings.privacy.mode.clone();
    match a.opt_str("mode")?.unwrap_or(&default) {
        "pixelate" => Ok(true),
        "blur" => Ok(false),
        o => Err(CommandError::Params(format!(
            "unknown mode `{o}` (pixelate|blur)"
        ))),
    }
}

fn add_blurs(
    s: &mut Session,
    label: &str,
    rects: &[Rect],
    pixelate: bool,
) -> Result<(), CommandError> {
    s.edit(label, None, |d| {
        for r in rects {
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
    })
}

fn hide_secrets(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["mode", "llm", "pad"])?;
    let pixelate = mode_arg(&a, s)?;
    let llm = match a.has("llm") {
        true => Some(a.bool("llm", false)?),
        false => None,
    };
    let pad = a.opt_i32("pad")?.unwrap_or(4).clamp(0, 50);
    let mut scan = s.secret_scan(Vec::new(), llm);
    scan.pad = pad;
    let flat = screenit_compose::flatten(s.doc_ref()?);
    let res = scan.run(&flat)?;
    let rects: Vec<Rect> = res.found.iter().map(|f| f.rect).collect();
    if !rects.is_empty() {
        add_blurs(s, "Hide secrets", &rects, pixelate)?;
    }
    let by: Vec<&str> = res.found.iter().map(|f| f.by).collect();
    Ok(json!({"hidden": rects.len(), "by": by, "warning": res.warning}))
}

fn redact_apply(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["rects", "mode"])?;
    let pixelate = mode_arg(&a, s)?;
    let items = a.array("rects")?;
    if items.len() > MAX_RECTS {
        return Err(CommandError::Params(format!(
            "at most {MAX_RECTS} rectangles"
        )));
    }
    let mut rects = Vec::new();
    for v in items {
        let n: Option<Vec<i64>> = v
            .as_array()
            .filter(|q| q.len() == 4)
            .map(|q| q.iter().filter_map(Value::as_i64).collect());
        match n.as_deref() {
            Some([x, y, w, h])
                if *w > 0
                    && *h > 0
                    && x.abs() < 1_000_000
                    && y.abs() < 1_000_000
                    && *w < 1_000_000
                    && *h < 1_000_000 =>
            {
                rects.push(Rect::new(*x as i32, *y as i32, *w as u32, *h as u32));
            }
            _ => {
                return Err(CommandError::Params(
                    "rects must be [x,y,w,h] with positive width and height".into(),
                ));
            }
        }
    }
    s.doc_ref()?;
    if !rects.is_empty() {
        add_blurs(s, "Hide secrets", &rects, pixelate)?;
    }
    Ok(json!({"applied": rects.len()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "editor.hide_secrets",
            label: "Hide Secrets",
            menu: "Edit",
            shortcut: "",
            params: r#"{"mode"?:pixelate|blur,"llm"?:bool,"pad"?:int}"#,
            enabled: need_doc,
            run: hide_secrets,
        },
        CommandSpec {
            id: "editor.redact.apply",
            label: "Cover Areas",
            menu: "Edit",
            shortcut: "",
            params: r#"{"rects":[[x,y,w,h]...],"mode"?:pixelate|blur}"#,
            enabled: need_doc,
            run: redact_apply,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_parsing_tolerates_fences_and_junk() {
        let r = "Sure!\n```json\n[{\"line\": 2, \"text\": \"hunter2\"}, {\"line\": 0, \"text\": \"x\"}, 5]\n```";
        assert_eq!(parse_findings(r), vec![(2, "hunter2".to_string())]);
        assert!(parse_findings("none").is_empty());
        assert!(parse_findings("] [").is_empty());
        assert!(parse_findings("[{\"line\":\"a\"}]").is_empty());
        let many: Vec<String> = (1..=500)
            .map(|i| format!("{{\"line\":{i},\"text\":\"t\"}}"))
            .collect();
        assert_eq!(
            parse_findings(&format!("[{}]", many.join(","))).len(),
            LLM_MAX_FINDINGS
        );
    }

    #[test]
    fn char_spans_are_in_characters() {
        assert_eq!(char_span("كلمة abc12", "abc12"), Some((5, 10)));
        assert_eq!(char_span("abc", ""), None);
        assert_eq!(char_span("abc", "z"), None);
    }
}
