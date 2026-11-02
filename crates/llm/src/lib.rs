//! OCR through a local vision language model (LM Studio, or any OpenAI-compatible server).
//!
//! Plain HTTP with strict timeouts and size caps. Every failure is an `Err`, never a panic.

use screenit_capture::{CaptureError, LlmConfig, LlmOcr};
use screenit_codecs::{Format, encode};
use screenit_raster::Raster;
use serde_json::{Value, json};
use std::time::Duration;

/// Longest image side sent to the model.
const MAX_SIDE: u32 = 2048;
/// Largest response body accepted.
const MAX_RESPONSE: u64 = 2_000_000;

const PROMPT: &str = "Transcribe all text in this image exactly as written, preserving line \
breaks and the original languages (Arabic and English). Reply with the text only, no commentary.";

pub struct LmStudio;

fn err<E: std::fmt::Display>(what: &str, e: E) -> CaptureError {
    CaptureError::Platform(format!("llm {what}: {e}"))
}

fn base_url(cfg: &LlmConfig) -> Result<String, CaptureError> {
    let u = cfg.url.trim().trim_end_matches('/');
    if !(u.starts_with("http://") || u.starts_with("https://")) || u.len() > 300 {
        return Err(err("config", "the server URL must start with http://"));
    }
    Ok(u.to_string())
}

/// `Authorization` header value; LM Studio ignores the key, other servers need a real one.
fn auth(cfg: &LlmConfig) -> String {
    let k: String = cfg
        .api_key
        .chars()
        .filter(|c| !c.is_control())
        .take(512)
        .collect();
    let k = k.trim();
    format!("Bearer {}", if k.is_empty() { "lm-studio" } else { k })
}

fn agent(cfg: &LlmConfig) -> ureq::Agent {
    let secs = u64::from(cfg.timeout_secs.clamp(1, 600));
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(secs)))
        .build()
        .into()
}

pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        let ch = |shift: u32| char::from(T[((n >> shift) & 63) as usize]);
        out.push(ch(18));
        out.push(ch(12));
        out.push(if c.len() > 1 { ch(6) } else { '=' });
        out.push(if c.len() > 2 { ch(0) } else { '=' });
    }
    out
}

fn fit(image: &Raster) -> Result<Raster, CaptureError> {
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 {
        return Err(CaptureError::Empty);
    }
    let longest = w.max(h);
    if longest <= MAX_SIDE {
        return Ok(image.clone());
    }
    let scale = f64::from(MAX_SIDE) / f64::from(longest);
    let nw = ((f64::from(w) * scale) as u32).max(1);
    let nh = ((f64::from(h) * scale) as u32).max(1);
    Ok(screenit_algo::resize(image, nw, nh)?)
}

/// Build the chat-completions request body (public so tests can inspect it).
pub fn request_body(image: &Raster, cfg: &LlmConfig) -> Result<Value, CaptureError> {
    let png = encode(&fit(image)?, Format::Png, 100).map_err(|e| err("encode", e))?;
    let url = format!("data:image/png;base64,{}", base64(&png));
    let mut body = json!({
        "temperature": 0,
        "max_tokens": 4096,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": PROMPT},
                {"type": "image_url", "image_url": {"url": url}},
            ],
        }],
    });
    if !cfg.model.trim().is_empty() {
        body["model"] = json!(cfg.model.trim());
    }
    Ok(body)
}

/// Pull the reply text out of a chat-completions response.
pub fn parse_reply(body: &str) -> Result<String, CaptureError> {
    let v: Value = serde_json::from_str(body).map_err(|e| err("response", e))?;
    let text = v
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| err("response", "no text in the reply"))?;
    Ok(text.trim().to_string())
}

pub fn parse_models(body: &str) -> Result<Vec<String>, CaptureError> {
    let v: Value = serde_json::from_str(body).map_err(|e| err("response", e))?;
    let list = v
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| err("response", "no model list"))?;
    Ok(list
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect())
}

impl LlmOcr for LmStudio {
    fn transcribe(&self, image: &Raster, cfg: &LlmConfig) -> Result<String, CaptureError> {
        let url = format!("{}/chat/completions", base_url(cfg)?);
        let body = request_body(image, cfg)?.to_string();
        let mut resp = agent(cfg)
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Authorization", &auth(cfg))
            .send(&body)
            .map_err(|e| err("request", e))?;
        let text = resp
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE)
            .read_to_string()
            .map_err(|e| err("read", e))?;
        parse_reply(&text)
    }

    fn models(&self, cfg: &LlmConfig) -> Result<Vec<String>, CaptureError> {
        let url = format!("{}/models", base_url(cfg)?);
        let mut resp = agent(cfg)
            .get(&url)
            .header("Authorization", &auth(cfg))
            .call()
            .map_err(|e| err("request", e))?;
        let text = resp
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE)
            .read_to_string()
            .map_err(|e| err("read", e))?;
        parse_models(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// One-shot fake server: answers each connection with the next canned response.
    fn serve(responses: Vec<(u16, String, u64)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body, delay_ms) in responses {
                let Ok((mut s, _)) = l.accept() else { break };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = s.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text
                            .to_ascii_lowercase()
                            .split("content-length:")
                            .nth(1)
                            .and_then(|r| r.trim().split("\r\n").next().map(str::to_string))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8_lossy(&buf).into_owned());
                std::thread::sleep(Duration::from_millis(delay_ms));
                let _ = write!(
                    s,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
            requests
        });
        (url, h)
    }

    fn cfg(url: &str) -> LlmConfig {
        LlmConfig {
            url: url.into(),
            model: "qwen2-vl".into(),
            api_key: String::new(),
            timeout_secs: 2,
        }
    }

    fn img() -> Raster {
        Raster::new(40, 20, [255; 4]).unwrap()
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn transcribes_through_a_fake_server() {
        let reply =
            json!({"choices":[{"message":{"content":"  مرحبا Hello\nline two "}}]}).to_string();
        let (url, h) = serve(vec![(200, reply, 0)]);
        let text = LmStudio.transcribe(&img(), &cfg(&url)).unwrap();
        assert_eq!(text, "مرحبا Hello\nline two");
        let req = &h.join().unwrap()[0];
        assert!(req.starts_with("POST /v1/chat/completions"));
        assert!(req.contains("data:image/png;base64,"));
        assert!(req.contains("qwen2-vl"));
    }

    #[test]
    fn sends_the_api_key_as_a_bearer_token() {
        let (url, h) = serve(vec![
            (200, r#"{"data":[]}"#.into(), 0),
            (200, r#"{"data":[]}"#.into(), 0),
        ]);
        let mut c = cfg(&url);
        c.api_key = "  sk-secret\r\nX: y ".into();
        LmStudio.models(&c).unwrap();
        c.api_key = String::new();
        LmStudio.models(&c).unwrap();
        let reqs = h.join().unwrap();
        assert!(
            reqs[0].contains("authorization: Bearer sk-secretX: y")
                || reqs[0].contains("Authorization: Bearer sk-secretX: y")
        );
        assert!(!reqs[0].contains("\r\nX: y"));
        assert!(
            reqs[1]
                .to_ascii_lowercase()
                .contains("authorization: bearer lm-studio")
        );
    }

    #[test]
    fn lists_models() {
        let (url, h) = serve(vec![(200, r#"{"data":[{"id":"a"},{"id":"b"}]}"#.into(), 0)]);
        assert_eq!(LmStudio.models(&cfg(&url)).unwrap(), vec!["a", "b"]);
        assert!(h.join().unwrap()[0].starts_with("GET /v1/models"));
    }

    #[test]
    fn malformed_and_error_responses_are_errors() {
        let (url, _h) = serve(vec![(200, "not json".into(), 0)]);
        assert!(LmStudio.transcribe(&img(), &cfg(&url)).is_err());
        let (url, _h) = serve(vec![(200, r#"{"choices":[]}"#.into(), 0)]);
        assert!(LmStudio.transcribe(&img(), &cfg(&url)).is_err());
        let (url, _h) = serve(vec![(500, "{}".into(), 0)]);
        assert!(LmStudio.transcribe(&img(), &cfg(&url)).is_err());
    }

    #[test]
    fn slow_server_times_out() {
        let (url, _h) = serve(vec![(200, "{}".into(), 3500)]);
        let mut c = cfg(&url);
        c.timeout_secs = 1;
        let t = std::time::Instant::now();
        assert!(LmStudio.transcribe(&img(), &c).is_err());
        assert!(t.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn server_down_and_bad_urls_are_errors() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", l.local_addr().unwrap());
        drop(l);
        assert!(LmStudio.models(&cfg(&url)).is_err());
        assert!(LmStudio.models(&cfg("ftp://x")).is_err());
        assert!(LmStudio.models(&cfg("")).is_err());
    }

    #[test]
    fn large_images_are_downscaled() {
        let big = Raster::new(5000, 100, [255; 4]).unwrap();
        let r = fit(&big).unwrap();
        assert_eq!(r.width(), 2048);
        assert!(r.height() >= 1);
    }
}
