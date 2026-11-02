//! Advanced capture: automatic scrolling capture, menu and object capture, Grab Text (OCR).

use crate::args::{Args, rect_keys};
use crate::capture_cmds::{Opts, finish};
use crate::{CmdResult, CommandError, CommandSpec, Session, always, need_screen};
use screenit_capture::{self as cap, CaptureOutput, Effects};
use screenit_doc::Metadata;
use screenit_geom::{Point, Rect};
use serde_json::{Value, json};

const MAX_PAGES: u32 = 200;

fn plain_opts(a: &Args<'_>, s: &Session) -> Result<Opts, CommandError> {
    Ok(Opts {
        delay_ms: 0,
        cursor: false,
        clipboard: a.bool("clipboard", s.prefs.copy_to_clipboard)?,
        effects: Effects::default(),
    })
}

/// Scroll the content under the region repeatedly until the picture stops changing, then stitch.
fn scroll_auto(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(
        p,
        &[
            "x",
            "y",
            "w",
            "h",
            "max_pages",
            "notches",
            "settle_ms",
            "horizontal",
            "clipboard",
        ],
    )?;
    let r = rect_keys(&a)?;
    let max_pages = a.opt_u32("max_pages")?.unwrap_or(30).clamp(1, MAX_PAGES);
    let notches = a.opt_i32("notches")?.unwrap_or(3).clamp(1, 20);
    let settle = a.opt_u32("settle_ms")?.unwrap_or(150).min(5_000);
    let src = s.screen()?;
    let centre = Point::new(r.x + r.w as i32 / 2, r.y + r.h as i32 / 2);
    let mut frames = vec![src.grab(r).map_err(CommandError::failed)?];
    for _ in 1..max_pages {
        src.scroll(centre, notches).map_err(CommandError::failed)?;
        s.services.clock.sleep_ms(settle);
        // Wait until the picture is stable (smooth scrolling / delayed repaint) before using it.
        let mut next = src.grab(r).map_err(CommandError::failed)?;
        for _ in 0..8 {
            s.services.clock.sleep_ms((settle / 2).max(20));
            let again = src.grab(r).map_err(CommandError::failed)?;
            let stable = next.mean_abs_diff(&again).is_some_and(|d| d < 0.001);
            next = again;
            if stable {
                break;
            }
        }
        let same = frames
            .last()
            .and_then(|f| f.mean_abs_diff(&next))
            .is_some_and(|d| d < 0.01);
        if same {
            break; // reached the end of the content
        }
        frames.push(next);
    }
    let pages = frames.len();
    let img = if a.bool("horizontal", false)? {
        cap::stitch_horizontal(&frames)
    } else {
        cap::stitch_vertical(&frames)
    }
    .map_err(CommandError::failed)?;
    let metadata = Metadata {
        source: "scrolling-auto".into(),
        ..Metadata::default()
    };
    let o = plain_opts(&a, s)?;
    let mut out = finish(
        s,
        CaptureOutput {
            image: img,
            metadata,
            region: Some(r),
        },
        &o,
    )?;
    out["pages"] = json!(pages);
    Ok(out)
}

fn grab_rect(s: &mut Session, rect: Rect, source: &str, a: &Args<'_>) -> CmdResult {
    let src = s.screen()?;
    let image = src.grab(rect).map_err(CommandError::failed)?;
    let metadata = Metadata {
        source: source.into(),
        ..Metadata::default()
    };
    let o = plain_opts(a, s)?;
    finish(
        s,
        CaptureOutput {
            image,
            metadata,
            region: Some(rect),
        },
        &o,
    )
}

fn object(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["point", "clipboard"])?;
    let pf = a.pointf("point")?;
    let pt = Point::new(pf.x as i32, pf.y as i32);
    let obj = s
        .screen()?
        .object_at(pt)
        .ok_or_else(|| CommandError::Failed("no UI element at that point".into()))?;
    grab_rect(s, obj.rect, "object", &a)
}

fn menu(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["clipboard"])?;
    let menus = s.screen()?.menus();
    // Union of all open popup menus (a menu and its open submenus are captured together).
    let rect = menus
        .iter()
        .map(|m| m.rect)
        .reduce(|acc, r| acc.union(&r))
        .ok_or_else(|| CommandError::Failed("no open menu found".into()))?;
    grab_rect(s, rect, "menu", &a)
}

/// Result of one OCR run.
pub struct OcrOutput {
    pub lines: Vec<screenit_capture::OcrLine>,
    pub engine: &'static str,
}

impl OcrOutput {
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn llm_lines(text: &str) -> Vec<screenit_capture::OcrLine> {
    text.lines()
        .map(|l| screenit_capture::OcrLine {
            text: l.to_string(),
            rect: screenit_geom::Rect::default(),
        })
        .collect()
}

/// Everything needed to run OCR off the UI thread (engines are shared handles).
#[derive(Clone)]
pub struct OcrJob {
    ocr: Option<std::sync::Arc<dyn screenit_capture::Ocr>>,
    llm: Option<std::sync::Arc<dyn screenit_capture::LlmOcr>>,
    settings: crate::settings::OcrSettings,
    lang: String,
}

impl OcrJob {
    /// Recognise `image` with the engine chosen in settings (`windows`, `llm`, `windows_then_llm`).
    pub fn run(&self, image: &screenit_raster::Raster) -> Result<OcrOutput, CommandError> {
        let o = &self.settings;
        let cfg = screenit_capture::LlmConfig {
            url: o.llm_url.clone(),
            model: o.llm_model.clone(),
            timeout_secs: o.llm_timeout_secs,
        };
        let windows = || -> Result<Vec<screenit_capture::OcrLine>, CommandError> {
            let ocr = self
                .ocr
                .clone()
                .ok_or_else(|| CommandError::Disabled("no OCR engine available".into()))?;
            ocr.recognize_lang(image, &self.lang)
                .map_err(CommandError::failed)
        };
        let llm = || -> Result<Vec<screenit_capture::OcrLine>, CommandError> {
            let l = self
                .llm
                .clone()
                .ok_or_else(|| CommandError::Disabled("no language-model OCR available".into()))?;
            l.transcribe(image, &cfg)
                .map(|t| llm_lines(&t))
                .map_err(CommandError::failed)
        };
        match o.engine.as_str() {
            "llm" => Ok(OcrOutput {
                lines: llm()?,
                engine: "llm",
            }),
            "windows_then_llm" => match windows() {
                Ok(lines) if lines.iter().any(|l| !l.text.trim().is_empty()) => Ok(OcrOutput {
                    lines,
                    engine: "windows",
                }),
                _ => Ok(OcrOutput {
                    lines: llm()?,
                    engine: "llm",
                }),
            },
            _ => Ok(OcrOutput {
                lines: windows()?,
                engine: "windows",
            }),
        }
    }
}

impl Session {
    /// An OCR job using the current settings; `lang` overrides the configured language.
    pub fn ocr_job(&self, lang: Option<&str>) -> OcrJob {
        OcrJob {
            ocr: self.services.ocr.clone(),
            llm: self.services.llm.clone(),
            settings: self.settings.ocr.clone(),
            lang: lang.unwrap_or(&self.settings.ocr.language).to_string(),
        }
    }
}
/// Grab Text: recognise text in a screen region, or in the open document when no region is given.
fn grab_text(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["x", "y", "w", "h", "clipboard", "lang"])?;
    let lang = match a.opt_str("lang")? {
        Some(l) if l.len() <= 16 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => {
            l.to_string()
        }
        Some(_) => return Err(CommandError::Params("`lang` is not a language tag".into())),
        None => s.settings.ocr.language.clone(),
    };
    let job = s.ocr_job(Some(&lang));
    let image = if a.has("x") {
        s.screen()?
            .grab(rect_keys(&a)?)
            .map_err(CommandError::failed)?
    } else {
        screenit_compose::flatten(s.doc_ref()?)
    };
    let out = job.run(&image)?;
    let text = out.text();
    let lines = out.lines;
    if a.bool("clipboard", true)? {
        if let Some(cb) = &s.services.clipboard {
            cb.set_text(&text).map_err(CommandError::Failed)?;
        }
    }
    Ok(json!({"text": text, "lines": lines, "engine": out.engine}))
}

/// Installed Windows OCR languages and the language-model server's models (a connection test).
fn ocr_info(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["llm"])?;
    let windows = s
        .services
        .ocr
        .as_ref()
        .map(|o| o.languages())
        .unwrap_or_default();
    let mut v = json!({"windows_languages": windows});
    if a.bool("llm", false)? {
        let o = &s.settings.ocr;
        let cfg = screenit_capture::LlmConfig {
            url: o.llm_url.clone(),
            model: o.llm_model.clone(),
            timeout_secs: o.llm_timeout_secs.min(10),
        };
        let l = s
            .services
            .llm
            .clone()
            .ok_or_else(|| CommandError::Disabled("no language-model OCR available".into()))?;
        v["llm_models"] = json!(l.models(&cfg).map_err(CommandError::failed)?);
    }
    Ok(v)
}
pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "capture.scroll.auto",
            label: "Automatic Scrolling Capture",
            menu: "Capture",
            shortcut: "",
            params: r#"{"x","y","w","h","max_pages"?:int,"notches"?:int,"settle_ms"?:int,"horizontal"?:bool,"clipboard"?:bool}"#,
            enabled: need_screen,
            run: scroll_auto,
        },
        CommandSpec {
            id: "capture.object",
            label: "Object Capture",
            menu: "Capture",
            shortcut: "",
            params: r#"{"point":[x,y],"clipboard"?:bool}"#,
            enabled: need_screen,
            run: object,
        },
        CommandSpec {
            id: "capture.menu",
            label: "Menu Capture",
            menu: "Capture",
            shortcut: "",
            params: r#"{"clipboard"?:bool}"#,
            enabled: need_screen,
            run: menu,
        },
        CommandSpec {
            id: "capture.grab_text",
            label: "Grab Text",
            menu: "Capture",
            shortcut: "",
            params: r#"{"x"?,"y"?,"w"?,"h"?,"clipboard"?:bool,"lang"?:str}"#,
            enabled: always,
            run: grab_text,
        },
        CommandSpec {
            id: "ocr.info",
            label: "Text Recognition Info",
            menu: "Capture",
            shortcut: "",
            params: r#"{"llm"?:bool}"#,
            enabled: always,
            run: ocr_info,
        },
    ]
}
