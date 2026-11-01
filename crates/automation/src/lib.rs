//! Control channel transport: loopback TCP, one JSON request per line, one JSON reply per line.
//! The port and a per-run token are written to `control.json` in the data directory; every
//! request must carry that token (`"token": "..."`), so only processes that can read the user's
//! profile can drive the app. The handler decides how requests reach the engine.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct ControlServer {
    pub port: u16,
    pub token: String,
    pub info_path: PathBuf,
}

fn make_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())
}

/// Start serving on an ephemeral loopback port. `handler` maps a request line to a reply line.
pub fn start(
    data_dir: &Path,
    handler: impl Fn(String) -> String + Send + Sync + 'static,
) -> std::io::Result<ControlServer> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let token = make_token();
    std::fs::create_dir_all(data_dir)?;
    let info_path = data_dir.join("control.json");
    std::fs::write(
        &info_path,
        json!({"port": port, "token": token, "pid": std::process::id()}).to_string(),
    )?;
    let handler = Arc::new(handler);
    let tok = token.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let (h, t) = (handler.clone(), tok.clone());
            std::thread::spawn(move || serve_conn(conn, &t, &*h));
        }
    });
    Ok(ControlServer {
        port,
        token,
        info_path,
    })
}

fn serve_conn(conn: TcpStream, token: &str, handler: &(dyn Fn(String) -> String + Send + Sync)) {
    let Ok(mut out) = conn.try_clone() else {
        return;
    };
    for line in BufReader::new(conn).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(v) if v.get("token").and_then(Value::as_str) == Some(token) => handler(line),
            Ok(v) => json!({"id": v.get("id"), "ok": false, "error": {"code": "unauthorized", "message": "missing or wrong token"}}).to_string(),
            Err(e) => json!({"id": null, "ok": false, "error": {"code": "bad_json", "message": e.to_string()}}).to_string(),
        };
        if writeln!(out, "{reply}").and_then(|()| out.flush()).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_token_and_forwards_valid_requests() {
        let dir = std::env::temp_dir().join(format!("scit-auto-{}", std::process::id()));
        let s = start(&dir, |l| {
            format!(
                "{{\"echo\":{}}}",
                serde_json::to_string(&l).unwrap_or_default()
            )
        })
        .unwrap();
        let mut c = TcpStream::connect((Ipv4Addr::LOCALHOST, s.port)).unwrap();
        let mut r = BufReader::new(c.try_clone().unwrap());
        writeln!(c, "{{\"id\":1,\"method\":\"engine.commands\"}}").unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert!(line.contains("unauthorized"), "{line}");
        writeln!(
            c,
            "{{\"id\":2,\"token\":\"{}\",\"method\":\"engine.commands\"}}",
            s.token
        )
        .unwrap();
        line.clear();
        r.read_line(&mut line).unwrap();
        assert!(line.contains("echo"), "{line}");
        std::fs::remove_dir_all(dir).ok();
    }
}
