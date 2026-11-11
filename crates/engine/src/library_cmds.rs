//! Library commands: list/search, tag, delete, open, recent, tags.

use crate::args::Args;
use crate::{CmdResult, CommandError, CommandSpec, Session, always};
use screenit_library::{Kind, Query, SortBy};
use serde_json::{Value, json};

fn list(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["text", "kind", "app", "tag", "from", "to", "sort"])?;
    let kind = match a.opt_str("kind")? {
        None => None,
        Some("image") => Some(Kind::Image),
        Some("video") => Some(Kind::Video),
        Some("gif") => Some(Kind::Gif),
        Some("text") => Some(Kind::Text),
        Some(o) => {
            return Err(CommandError::Params(format!(
                "unknown kind `{o}` (image|video|gif|text)"
            )));
        }
    };
    let sort = match a.opt_str("sort")? {
        None | Some("date_desc") => SortBy::DateDesc,
        Some("date_asc") => SortBy::DateAsc,
        Some("title") => SortBy::Title,
        Some("app") => SortBy::App,
        Some(o) => return Err(CommandError::Params(format!("unknown sort `{o}`"))),
    };
    let q = Query {
        text: a.opt_str("text")?.map(str::to_string),
        kind,
        app: a.opt_str("app")?.map(str::to_string),
        tag: a.opt_str("tag")?.map(str::to_string),
        from: a.opt_f32("from")?.map(|v| v as i64),
        to: a.opt_f32("to")?.map(|v| v as i64),
        sort,
    };
    Ok(json!({"entries": s.library.query(&q)}))
}

fn recent(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["count"])?;
    Ok(json!({"entries": s.library.recent(a.opt_u32("count")?.unwrap_or(10).min(1000) as usize)}))
}

fn tag(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["id", "tag", "remove"])?;
    let (id, t) = (a.u64("id")?, a.str("tag")?);
    if a.bool("remove", false)? {
        s.library.remove_tag(id, t).map_err(CommandError::failed)?;
    } else {
        s.library.add_tag(id, t).map_err(CommandError::failed)?;
    }
    Ok(Value::Null)
}

fn tags(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    Ok(json!({"tags": s.library.tags()}))
}

fn delete(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["id", "delete_file"])?;
    let e = s
        .library
        .remove(a.u64("id")?)
        .map_err(CommandError::failed)?;
    if a.bool("delete_file", false)? {
        std::fs::remove_file(&e.path).map_err(CommandError::failed)?;
    }
    Ok(Value::Null)
}

fn open(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["id"])?;
    let path = s
        .library
        .get(a.u64("id")?)
        .map(|e| e.path.to_string_lossy().into_owned())
        .ok_or_else(|| CommandError::Failed("no such library entry".into()))?;
    s.dispatch("doc.open", &json!({"path": path}))
}

fn share(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["id", "mode", "dest_dir"])?;
    let e = s
        .library
        .get(a.u64("id")?)
        .cloned()
        .ok_or_else(|| CommandError::Failed("no such library entry".into()))?;
    match a.opt_str("mode")?.unwrap_or("file") {
        "file" => {
            let dir = std::path::Path::new(a.str("dest_dir")?);
            std::fs::create_dir_all(dir).map_err(CommandError::failed)?;
            let name = e
                .path
                .file_name()
                .ok_or_else(|| CommandError::Failed("entry has no file name".into()))?;
            let dest = dir.join(name);
            std::fs::copy(&e.path, &dest).map_err(CommandError::failed)?;
            Ok(json!({"path": dest}))
        }
        "clipboard" => {
            copy_to_clipboard(s, &e)?;
            Ok(json!({"copied": true}))
        }
        o => Err(CommandError::Params(format!(
            "unknown mode `{o}` (file|clipboard)"
        ))),
    }
}

fn copy_to_clipboard(s: &Session, e: &screenit_library::Entry) -> Result<(), CommandError> {
    let clip = s
        .services
        .clipboard
        .clone()
        .ok_or_else(|| CommandError::Disabled("no clipboard available".into()))?;
    if e.kind == Kind::Text {
        let text = if e.body.is_empty() {
            std::fs::read_to_string(&e.path).map_err(CommandError::failed)?
        } else {
            e.body.clone()
        };
        return clip.set_text(&text).map_err(CommandError::Failed);
    }
    let img = if e.kind == Kind::Video {
        s.services
            .video
            .clone()
            .ok_or_else(|| CommandError::Disabled("no video reader available".into()))?
            .frame_at(&e.path, 0)
            .map_err(CommandError::failed)?
    } else if screenit_format::is_project(&e.path) {
        let d = screenit_format::load(&e.path).map_err(CommandError::failed)?;
        screenit_compose::flatten(&d)
    } else {
        let bytes = std::fs::read(&e.path).map_err(CommandError::failed)?;
        screenit_codecs::decode(&bytes).map_err(CommandError::failed)?
    };
    clip.set_image(&img).map_err(CommandError::Failed)
}

fn open_dir(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["dir"])?;
    let dir = std::path::Path::new(a.str("dir")?);
    std::fs::create_dir_all(dir).map_err(CommandError::failed)?;
    s.library = screenit_library::Library::open(dir).map_err(CommandError::failed)?;
    Ok(json!({"entries": s.library.len()}))
}

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, params, run| CommandSpec {
        id,
        label,
        menu: "Library",
        shortcut: "",
        params,
        enabled: always,
        run,
    };
    vec![
        c(
            "library.list",
            "Search Library",
            r#"{"text"?,"kind"?:image|video|gif|text,"app"?,"tag"?,"from"?:unix,"to"?:unix,"sort"?:date_desc|date_asc|title|app}"#,
            list,
        ),
        c(
            "library.recent",
            "Recent Captures",
            r#"{"count"?:int}"#,
            recent,
        ),
        c(
            "library.tag",
            "Tag Capture",
            r#"{"id":int,"tag":str,"remove"?:bool}"#,
            tag,
        ),
        c("library.tags", "List Tags", "{}", tags),
        c(
            "library.delete",
            "Delete Capture",
            r#"{"id":int,"delete_file"?:bool}"#,
            delete,
        ),
        c("library.open", "Open From Library", r#"{"id":int}"#, open),
        c(
            "library.share",
            "Share Library Item",
            r#"{"id":int,"mode"?:file|clipboard,"dest_dir"?:str}"#,
            share,
        ),
        c(
            "library.open_dir",
            "Open Library Folder",
            r#"{"dir":str}"#,
            open_dir,
        ),
    ]
}
