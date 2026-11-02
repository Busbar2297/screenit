//! System commands: command listing, preferences.

use crate::args::Args;
use crate::{CmdResult, CommandSpec, Session, always};
use serde_json::{Value, json};

fn list(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    Ok(s.command_list())
}

fn get(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let f = &s.prefs;
    Ok(json!({
        "auto_save_dir": f.auto_save_dir, "copy_to_clipboard": f.copy_to_clipboard,
        "include_cursor": f.include_cursor, "default_step_radius": f.default_step_radius,
    }))
}

fn set(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(
        p,
        &[
            "auto_save_dir",
            "copy_to_clipboard",
            "include_cursor",
            "default_step_radius",
        ],
    )?;
    if let Some(d) = a.opt_str("auto_save_dir")? {
        s.prefs.auto_save_dir = if d.is_empty() { None } else { Some(d.into()) };
    }
    s.prefs.copy_to_clipboard = a.bool("copy_to_clipboard", s.prefs.copy_to_clipboard)?;
    s.prefs.include_cursor = a.bool("include_cursor", s.prefs.include_cursor)?;
    if let Some(r) = a.opt_f32("default_step_radius")? {
        s.prefs.default_step_radius = r.clamp(4.0, 200.0);
    }
    get(s, &Value::Null)
}

fn settings_get(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    serde_json::to_value(&s.settings).map_err(|e| crate::CommandError::Failed(e.to_string()))
}

fn settings_set(s: &mut Session, p: &Value) -> CmdResult {
    let a = Args::new(p, &["path", "value"])?;
    let path = a.str("path")?.to_string();
    let value = p.get("value").cloned().unwrap_or(Value::Null);
    s.set_setting(&path, value)?;
    settings_get(s, &Value::Null)
}

fn settings_reset(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    let presets = s.settings.presets.clone();
    s.settings = crate::settings::Settings {
        presets,
        ..Default::default()
    };
    s.apply_settings();
    s.persist_settings()?;
    settings_get(s, &Value::Null)
}

fn settings_open(s: &mut Session, p: &Value) -> CmdResult {
    Args::new(p, &[])?;
    Ok(json!({"path": s.settings_path}))
}

pub fn specs() -> Vec<CommandSpec> {
    let c = |id, label, params, run| CommandSpec {
        id,
        label,
        menu: "System",
        shortcut: "",
        params,
        enabled: always,
        run,
    };
    vec![
        c("command.list", "List Commands", "{}", list),
        c("settings.get", "Get Settings", "{}", settings_get),
        c(
            "settings.set",
            "Set Setting",
            r#"{"path":str,"value":any}"#,
            settings_set,
        ),
        c("settings.reset", "Reset Settings", "{}", settings_reset),
        c(
            "settings.open",
            "Settings File Location",
            "{}",
            settings_open,
        ),
        c("prefs.get", "Get Preferences", "{}", get),
        c(
            "prefs.set",
            "Set Preferences",
            r#"{"auto_save_dir"?:str,"copy_to_clipboard"?:bool,"include_cursor"?:bool,"default_step_radius"?:num}"#,
            set,
        ),
    ]
}
