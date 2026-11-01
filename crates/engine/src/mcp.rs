//! Model Context Protocol server logic (stdio JSON-RPC 2.0). Every engine command is a tool;
//! `.` in ids becomes `_` in tool names. Transport lives in the CLI (`screenit-cli mcp`).

use crate::{CommandError, Session};
use serde_json::{Value, json};

const PROTOCOL: &str = "2024-11-05";

fn tool_name(id: &str) -> String {
    id.replace('.', "_")
}

fn rpc_err(id: &Value, code: i64, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
}

/// Handle one JSON-RPC message. Returns `None` for notifications (no response expected).
pub fn handle_mcp(s: &mut Session, req: &Value) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let Some(id) = id else {
        return None; // notification, e.g. notifications/initialized
    };
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let ok = |r: Value| Some(json!({"jsonrpc": "2.0", "id": id, "result": r}));
    match method {
        "initialize" => ok(json!({
            "protocolVersion": PROTOCOL,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "screenit", "version": env!("CARGO_PKG_VERSION")},
        })),
        "ping" => ok(json!({})),
        "tools/list" => {
            let tools: Vec<Value> = s
                .registry()
                .iter()
                .map(|c| {
                    json!({
                        "name": tool_name(c.id),
                        "description": format!("{} (params: {})", c.label, c.params),
                        "inputSchema": {"type": "object", "additionalProperties": true},
                    })
                })
                .collect();
            ok(json!({"tools": tools}))
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(rpc_err(&id, -32602, "missing tool name"));
            };
            let Some(cmd) = s
                .registry()
                .iter()
                .find(|c| tool_name(c.id) == name)
                .map(|c| c.id)
            else {
                return Some(rpc_err(&id, -32602, &format!("unknown tool `{name}`")));
            };
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match s.dispatch(cmd, &args) {
                Ok(v) => ok(
                    json!({"content": [{"type": "text", "text": v.to_string()}], "isError": false}),
                ),
                Err(e) => {
                    let kind = match &e {
                        CommandError::Unknown(_) => "unknown_command",
                        CommandError::Params(_) => "bad_params",
                        CommandError::Disabled(_) => "disabled",
                        CommandError::Failed(_) => "failed",
                        CommandError::Panic(_) => "internal",
                    };
                    ok(
                        json!({"content": [{"type": "text", "text": format!("{kind}: {e}")}], "isError": true}),
                    )
                }
            }
        }
        other => Some(rpc_err(&id, -32601, &format!("method not found: {other}"))),
    }
}

/// One line in, optional one line out.
pub fn handle_mcp_line(s: &mut Session, line: &str) -> Option<String> {
    match serde_json::from_str::<Value>(line) {
        Ok(v) => handle_mcp(s, &v).map(|r| r.to_string()),
        Err(e) => Some(rpc_err(&Value::Null, -32700, &format!("parse error: {e}")).to_string()),
    }
}
