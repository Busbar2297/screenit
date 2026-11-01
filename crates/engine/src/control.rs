//! Control protocol (see docs/control-protocol.md): one JSON request in, one JSON response out.
//! Transport (loopback TCP, stdio) lives in the app; this is the pure, testable part.

use crate::{CommandError, Session};
use serde_json::{Value, json};

fn err(id: &Value, code: &str, msg: String) -> Value {
    json!({"id": id, "ok": false, "error": {"code": code, "message": msg}})
}

/// Handle one request: `{"id":..,"method":"engine.execute","params":{"command":..,"params":{..}}}`.
/// Methods: `engine.execute`, `engine.commands`, `engine.snapshot` (flattened PNG, base64-free path).
pub fn handle_request(s: &mut Session, req: &Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = req.get("method").and_then(Value::as_str) else {
        return err(&id, "bad_request", "missing `method`".into());
    };
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "engine.commands" => json!({"id": id, "ok": true, "result": s.command_list()}),
        "engine.execute" => {
            let Some(cmd) = params.get("command").and_then(Value::as_str) else {
                return err(&id, "bad_request", "missing `params.command`".into());
            };
            let args = params.get("params").cloned().unwrap_or(Value::Null);
            match s.dispatch(cmd, &args) {
                Ok(v) => json!({"id": id, "ok": true, "result": v}),
                Err(e) => {
                    let code = match &e {
                        CommandError::Unknown(_) => "unknown_command",
                        CommandError::Params(_) => "bad_params",
                        CommandError::Disabled(_) => "disabled",
                        CommandError::Failed(_) => "failed",
                        CommandError::Panic(_) => "internal",
                    };
                    err(&id, code, e.to_string())
                }
            }
        }
        other => err(&id, "unknown_method", format!("unknown method `{other}`")),
    }
}

/// Parse one line of text and answer with one line of JSON.
pub fn handle_line(s: &mut Session, line: &str) -> String {
    match serde_json::from_str::<Value>(line) {
        Ok(v) => handle_request(s, &v).to_string(),
        Err(e) => err(&Value::Null, "bad_json", e.to_string()).to_string(),
    }
}
