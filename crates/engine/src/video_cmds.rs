//! Video editing commands: trim, cut, combine, GIF, video from images. They decode through the
//! injected `VideoReader` and encode through the injected `VideoWriter`.

use crate::args::Args;
use crate::{CmdResult, CommandError, CommandSpec, Session, always};
use screenit_codecs as codecs;
use screenit_raster::Raster;
use screenit_record::{EditList, RecordError, VideoReader, VideoWriter, gif_frame_times};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type Codecs = (Arc<dyn VideoReader>, Arc<dyn VideoWriter>);

fn services(s: &Session) -> Result<Codecs, CommandError> {
    let r = s
        .services
        .video
        .clone()
        .ok_or_else(|| CommandError::Disabled("no video reader available".into()))?;
    let w = s
        .services
        .video_writer
        .clone()
        .ok_or_else(|| CommandError::Disabled("no video writer available".into()))?;
    Ok((r, w))
}

fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

fn mp4_out(a: &Args<'_>) -> Result<PathBuf, CommandError> {
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
    Ok(out)
}

/// Render `edit` over `clips` into `out`.
fn render(s: &Session, clips: &[PathBuf], edit: &EditList, fps: u32, out: &Path) -> CmdResult {
    let (reader, writer) = services(s)?;
    let info = reader.probe(&clips[0]).map_err(CommandError::failed)?;
    let (w, h) = (even(info.width), even(info.height));
    let total = edit.total_ms();
    let n = (total * u64::from(fps) / 1000).clamp(1, 200_000);
    let mut i = 0u64;
    let mut frames = std::iter::from_fn(|| {
        if i >= n {
            return None;
        }
        let t = i * 1000 / u64::from(fps);
        i += 1;
        let Some((clip, ms)) = edit.map_time(t.min(total.saturating_sub(1))) else {
            return Some(Err(RecordError::BadEdit(
                "time outside the edit list".into(),
            )));
        };
        let r = reader.frame_at(&clips[clip], ms).and_then(|f| {
            if f.width() == w && f.height() == h {
                Ok(f)
            } else {
                screenit_algo::resize(&f, w, h).map_err(|e| RecordError::Platform(e.to_string()))
            }
        });
        Some(r)
    });
    let res = writer
        .write(out, w, h, fps, &mut frames)
        .map_err(CommandError::failed)?;
    Ok(json!({"result": res}))
}

fn probe_ms(s: &Session, path: &str) -> Result<u64, CommandError> {
    let (r, _) = services(s)?;
    Ok(r.probe(Path::new(path))
        .map_err(CommandError::failed)?
        .duration_ms)
}

fn fps_of(a: &Args<'_>) -> Result<u32, CommandError> {
    Ok(a.opt_u32("fps")?.unwrap_or(30).clamp(1, 60))
}

fn trim(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "start_ms", "end_ms", "output", "fps"])?;
    let path = a.str("path")?;
    let mut e = EditList::from_clip(0, probe_ms(s, path)?);
    e.trim(u64::from(a.u32("start_ms")?), u64::from(a.u32("end_ms")?))
        .map_err(CommandError::failed)?;
    render(s, &[path.into()], &e, fps_of(&a)?, &mp4_out(&a)?)
}

fn cut(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "from_ms", "to_ms", "output", "fps"])?;
    let path = a.str("path")?;
    let mut e = EditList::from_clip(0, probe_ms(s, path)?);
    e.cut(u64::from(a.u32("from_ms")?), u64::from(a.u32("to_ms")?))
        .map_err(CommandError::failed)?;
    render(s, &[path.into()], &e, fps_of(&a)?, &mp4_out(&a)?)
}

fn combine(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["paths", "output", "fps"])?;
    let paths: Vec<PathBuf> = a
        .array("paths")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(PathBuf::from)
                .ok_or_else(|| CommandError::Params("paths must be strings".into()))
        })
        .collect::<Result<_, _>>()?;
    if paths.len() < 2 || paths.len() > 32 {
        return Err(CommandError::Params("`paths` needs 2..=32 clips".into()));
    }
    let mut e = EditList::default();
    for (i, pa) in paths.iter().enumerate() {
        e.combine(&EditList::from_clip(i, probe_ms(s, &pa.to_string_lossy())?));
    }
    render(s, &paths, &e, fps_of(&a)?, &mp4_out(&a)?)
}

fn gif(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(
        p,
        &["path", "output", "fps", "start_ms", "end_ms", "max_width"],
    )?;
    let (reader, _) = services(s)?;
    let path = PathBuf::from(a.str("path")?);
    let out = PathBuf::from(a.str("output")?);
    if out
        .extension()
        .is_none_or(|e| !e.eq_ignore_ascii_case("gif"))
    {
        return Err(CommandError::Params("output must be a .gif path".into()));
    }
    let info = reader.probe(&path).map_err(CommandError::failed)?;
    let start = u64::from(a.opt_u32("start_ms")?.unwrap_or(0));
    let end =
        u64::from(a.opt_u32("end_ms")?.unwrap_or(info.duration_ms as u32)).min(info.duration_ms);
    if start >= end {
        return Err(CommandError::Params(
            "start_ms must be before end_ms".into(),
        ));
    }
    let fps = a.opt_u32("fps")?.unwrap_or(10).clamp(1, 30);
    let max_w = a.opt_u32("max_width")?.unwrap_or(640).clamp(16, 4096);
    let mut frames: Vec<Raster> = Vec::new();
    for t in gif_frame_times(end - start, fps) {
        let mut f = reader
            .frame_at(&path, start + t)
            .map_err(CommandError::failed)?;
        if f.width() > max_w {
            let h = (u64::from(f.height()) * u64::from(max_w) / u64::from(f.width())).max(1) as u32;
            f = screenit_algo::resize(&f, max_w, h).map_err(CommandError::failed)?;
        }
        frames.push(f);
    }
    let bytes = codecs::encode_gif_animation(&frames, 1000 / fps).map_err(CommandError::failed)?;
    std::fs::write(&out, &bytes).map_err(CommandError::failed)?;
    Ok(json!({"path": out, "frames": frames.len(), "bytes": bytes.len()}))
}

fn from_images(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["paths", "output", "ms_each", "fps"])?;
    let (_, writer) = services(s)?;
    let imgs: Vec<Raster> = a
        .array("paths")?
        .iter()
        .map(|v| {
            let path = v
                .as_str()
                .ok_or_else(|| CommandError::Params("paths must be strings".into()))?;
            let bytes = std::fs::read(path).map_err(CommandError::failed)?;
            codecs::decode(&bytes).map_err(CommandError::failed)
        })
        .collect::<Result<_, _>>()?;
    if imgs.is_empty() || imgs.len() > 500 {
        return Err(CommandError::Params("`paths` needs 1..=500 images".into()));
    }
    let out = mp4_out(&a)?;
    let fps = fps_of(&a)?;
    let ms_each = u64::from(a.opt_u32("ms_each")?.unwrap_or(2000).clamp(100, 60_000));
    let (w, h) = (even(imgs[0].width()), even(imgs[0].height()));
    let per = (ms_each * u64::from(fps) / 1000).max(1);
    let mut k = 0u64;
    let total = per * imgs.len() as u64;
    let mut frames = std::iter::from_fn(|| {
        if k >= total {
            return None;
        }
        let im = &imgs[(k / per) as usize];
        k += 1;
        Some(if im.width() == w && im.height() == h {
            Ok(im.clone())
        } else {
            screenit_algo::resize(im, w, h).map_err(|e| RecordError::Platform(e.to_string()))
        })
    });
    let res = writer
        .write(&out, w, h, fps, &mut frames)
        .map_err(CommandError::failed)?;
    Ok(json!({"result": res}))
}

/// Export (copy) a recorded MP4, audio track included, to a user-chosen path.
fn export(_s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "output"])?;
    let src = PathBuf::from(a.str("path")?);
    if !src.is_file() {
        return Err(CommandError::Params(format!(
            "no video at {}",
            src.display()
        )));
    }
    let out = mp4_out(&a)?;
    let same = match (src.canonicalize(), out.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    };
    let bytes = if same {
        std::fs::metadata(&src).map_err(CommandError::failed)?.len()
    } else {
        std::fs::copy(&src, &out).map_err(CommandError::failed)?
    };
    Ok(json!({"path": out, "bytes": bytes}))
}

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, params, run| CommandSpec {
        id,
        label,
        menu: "Video",
        shortcut: "",
        params,
        enabled: always,
        run,
    };
    vec![
        c(
            "video.trim",
            "Trim Video",
            r#"{"path":str,"start_ms":int,"end_ms":int,"output":str.mp4,"fps"?:int}"#,
            trim,
        ),
        c(
            "video.cut",
            "Cut Out Section",
            r#"{"path":str,"from_ms":int,"to_ms":int,"output":str.mp4,"fps"?:int}"#,
            cut,
        ),
        c(
            "video.combine",
            "Combine Clips",
            r#"{"paths":[str],"output":str.mp4,"fps"?:int}"#,
            combine,
        ),
        c(
            "video.gif",
            "Create GIF",
            r#"{"path":str,"output":str.gif,"fps"?:int,"start_ms"?:int,"end_ms"?:int,"max_width"?:int}"#,
            gif,
        ),
        c(
            "video.from_images",
            "Video From Images",
            r#"{"paths":[str],"output":str.mp4,"ms_each"?:int,"fps"?:int}"#,
            from_images,
        ),
        c(
            "video.export",
            "Export MP4",
            r#"{"path":str,"output":str.mp4}"#,
            export,
        ),
    ]
}
