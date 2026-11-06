//! Speech through a local OpenAI-compatible server: `/v1/audio/transcriptions` (whisper.cpp
//! server, Speaches, ...) and `/v1/audio/speech`.

use super::{auth, base_url, err};
use screenit_capture::{CaptureError, LlmConfig};
use screenit_record::RecordError;
use screenit_record::speech::{
    SpeechConfig, SpeechToText, TextToSpeech, Transcript, Word, pcm_from_wav,
};
use serde_json::{Value, json};
use std::time::Duration;

/// Largest transcript JSON accepted.
const MAX_JSON: u64 = 20_000_000;
/// Largest narration WAV accepted.
const MAX_AUDIO: u64 = 120_000_000;
/// Longest narration text sent in one request.
const MAX_SPEECH_CHARS: usize = 4_000;

pub struct HttpSpeech;

fn rec(e: CaptureError) -> RecordError {
    RecordError::Platform(e.to_string())
}

fn llm_cfg(cfg: &SpeechConfig) -> LlmConfig {
    LlmConfig {
        url: cfg.url.clone(),
        model: cfg.model.clone(),
        api_key: cfg.api_key.clone(),
        timeout_secs: cfg.timeout_secs.max(1),
    }
}

fn agent(cfg: &LlmConfig) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(u64::from(
            cfg.timeout_secs.clamp(1, 1800),
        ))))
        .http_status_as_error(false)
        .build()
        .into()
}

fn said(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let msg = v["error"]["message"]
        .as_str()
        .or_else(|| v["error"].as_str())
        .or_else(|| v["message"].as_str())
        .unwrap_or(body);
    msg.chars().filter(|c| !c.is_control()).take(300).collect()
}

/// A `multipart/form-data` body: text fields plus one file.
pub fn multipart(boundary: &str, fields: &[(&str, String)], file: (&str, &str, &[u8])) -> Vec<u8> {
    let mut b = Vec::with_capacity(file.2.len() + 512);
    for (k, v) in fields {
        let k: String = k.chars().filter(|c| c.is_ascii_graphic()).collect();
        b.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                .as_bytes(),
        );
    }
    b.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: audio/wav\r\n\r\n",
            file.0, file.1
        )
        .as_bytes(),
    );
    b.extend_from_slice(file.2);
    b.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    b
}

fn ms(v: &Value) -> Option<u64> {
    let s = v.as_f64()?;
    if (0.0..86_400.0 * 10.0).contains(&s) {
        Some((s * 1000.0).round() as u64)
    } else {
        None
    }
}

/// Parse a `verbose_json` transcription reply. Word timestamps are used when present;
/// otherwise each segment's text is spread evenly across the segment.
pub fn parse_transcript(body: &str) -> Result<Transcript, RecordError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| RecordError::Platform(format!("speech response: {e}")))?;
    let text = v
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| RecordError::Platform("speech response: no text".into()))?
        .trim()
        .to_string();
    let language = v
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(32)
        .collect();
    let mut words: Vec<Word> = Vec::new();
    fn word_of(w: &Value) -> Option<Word> {
        let t = w
            .get("word")
            .or_else(|| w.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let (a, b) = (w.get("start").and_then(ms)?, w.get("end").and_then(ms)?);
        (!t.is_empty()).then(|| Word {
            start_ms: a,
            end_ms: b.max(a),
            text: t.to_string(),
        })
    }
    let collect = |list: &Value| -> Vec<Word> {
        list.as_array()
            .map(|l| l.iter().filter_map(word_of).take(200_000).collect())
            .unwrap_or_default()
    };
    if let Some(list) = v.get("words") {
        words = collect(list);
    }
    if words.is_empty()
        && let Some(segs) = v.get("segments").and_then(Value::as_array)
    {
        for s in segs {
            if let Some(list) = s.get("words") {
                words.extend(collect(list));
                continue;
            }
            let st = s.get("text").and_then(Value::as_str).unwrap_or("");
            let toks: Vec<&str> = st.split_whitespace().collect();
            if let (false, Some(a), Some(b)) = (
                toks.is_empty(),
                s.get("start").and_then(ms),
                s.get("end").and_then(ms),
            ) {
                let b = b.max(a);
                let n = toks.len() as u64;
                for (i, t) in toks.iter().enumerate() {
                    let i = i as u64;
                    words.push(Word {
                        start_ms: a + (b - a) * i / n,
                        end_ms: a + (b - a) * (i + 1) / n,
                        text: (*t).to_string(),
                    });
                }
            }
        }
    }
    Ok(Transcript {
        language,
        text,
        words,
    })
}

impl SpeechToText for HttpSpeech {
    fn transcribe(
        &self,
        wav: &[u8],
        language: Option<&str>,
        cfg: &SpeechConfig,
    ) -> Result<Transcript, RecordError> {
        let c = llm_cfg(cfg);
        let url = format!("{}/audio/transcriptions", base_url(&c).map_err(rec)?);
        let boundary = "screenitBoundary7f3a91c2d4";
        let mut fields = vec![
            ("response_format", "verbose_json".to_string()),
            ("timestamp_granularities[]", "word".to_string()),
            ("timestamp_granularities[]", "segment".to_string()),
            ("temperature", "0".to_string()),
        ];
        let model = cfg.model.trim();
        if !model.is_empty() {
            fields.push(("model", model.chars().filter(|c| !c.is_control()).collect()));
        }
        if let Some(l) = language.map(str::trim).filter(|l| !l.is_empty()) {
            let l: String = l
                .chars()
                .filter(char::is_ascii_alphabetic)
                .take(8)
                .collect();
            if !l.is_empty() {
                fields.push(("language", l));
            }
        }
        let body = multipart(boundary, &fields, ("file", "audio.wav", wav));
        let mut resp = agent(&c)
            .post(&url)
            .header(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .header("Authorization", &auth(&c))
            .send(&body[..])
            .map_err(|e| rec(err("request", e)))?;
        let code = resp.status().as_u16();
        let text = resp
            .body_mut()
            .with_config()
            .limit(MAX_JSON)
            .read_to_string()
            .map_err(|e| rec(err("read", e)))?;
        if code >= 400 {
            return Err(rec(err(
                "request",
                format!("http status {code}: {}", said(&text)),
            )));
        }
        parse_transcript(&text)
    }
}

impl TextToSpeech for HttpSpeech {
    fn speak(&self, text: &str, cfg: &SpeechConfig) -> Result<Vec<i16>, RecordError> {
        let c = llm_cfg(cfg);
        let url = format!("{}/audio/speech", base_url(&c).map_err(rec)?);
        let input: String = text.chars().take(MAX_SPEECH_CHARS).collect();
        if input.trim().is_empty() {
            return Err(RecordError::BadConfig("nothing to say".into()));
        }
        let mut body = json!({"input": input, "response_format": "wav"});
        if !cfg.model.trim().is_empty() {
            body["model"] = json!(cfg.model.trim());
        }
        if !cfg.voice.trim().is_empty() {
            body["voice"] = json!(cfg.voice.trim());
        }
        let mut resp = agent(&c)
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Authorization", &auth(&c))
            .send(&body.to_string())
            .map_err(|e| rec(err("request", e)))?;
        let code = resp.status().as_u16();
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(MAX_AUDIO)
            .read_to_vec()
            .map_err(|e| rec(err("read", e)))?;
        if code >= 400 {
            return Err(rec(err(
                "request",
                format!(
                    "http status {code}: {}",
                    said(&String::from_utf8_lossy(&bytes))
                ),
            )));
        }
        pcm_from_wav(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serve canned `(status, content-type, body)` replies, returning each raw request.
    fn serve(
        responses: Vec<(u16, &'static str, Vec<u8>)>,
    ) -> (String, std::thread::JoinHandle<Vec<Vec<u8>>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let mut reqs = Vec::new();
            for (status, ctype, body) in responses {
                let Ok((mut s, _)) = l.accept() else { break };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
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
                reqs.push(buf);
                let _ = write!(
                    s,
                    "HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(&body);
            }
            reqs
        });
        (url, h)
    }

    fn cfg(url: &str) -> SpeechConfig {
        SpeechConfig {
            url: url.into(),
            model: "whisper-1".into(),
            voice: "alloy".into(),
            api_key: String::new(),
            timeout_secs: 3,
        }
    }

    #[test]
    fn transcribes_through_a_fake_server() {
        let reply = json!({
            "language": "english", "text": " Open the menu. ",
            "words": [
                {"word": " Open", "start": 0.0, "end": 0.3},
                {"word": "the", "start": 0.32, "end": 0.5},
                {"word": "menu.", "start": 0.52, "end": 0.9}
            ]
        })
        .to_string();
        let (url, h) = serve(vec![(200, "application/json", reply.into_bytes())]);
        let wav = screenit_record::speech::wav_16k_mono(&[0i16; 960]);
        let t = HttpSpeech.transcribe(&wav, Some("en"), &cfg(&url)).unwrap();
        assert_eq!(t.text, "Open the menu.");
        assert_eq!(t.words.len(), 3);
        assert_eq!((t.words[0].text.as_str(), t.words[2].end_ms), ("Open", 900));
        let req = String::from_utf8_lossy(&h.join().unwrap()[0]).into_owned();
        assert!(req.starts_with("POST /v1/audio/transcriptions"), "{req}");
        assert!(req.contains("multipart/form-data; boundary="));
        assert!(req.contains("name=\"response_format\"") && req.contains("verbose_json"));
        assert!(req.contains("name=\"language\"") && req.contains("whisper-1"));
        assert!(req.contains("filename=\"audio.wav\"") && req.contains("RIFF"));
    }

    #[test]
    fn segments_without_words_are_spread_evenly() {
        let t = parse_transcript(
            r#"{"text":"a b c d","segments":[{"start":1.0,"end":2.0,"text":" a b c d"}]}"#,
        )
        .unwrap();
        assert_eq!(t.words.len(), 4);
        assert_eq!(t.words[0].start_ms, 1000);
        assert_eq!(t.words[3].end_ms, 2000);
    }

    #[test]
    fn bad_replies_are_errors() {
        assert!(parse_transcript("nope").is_err());
        assert!(parse_transcript("{}").is_err());
        let t = parse_transcript(r#"{"text":"x","words":[{"word":"x","start":-1,"end":"a"}]}"#)
            .unwrap();
        assert!(t.words.is_empty());
        let (url, _h) = serve(vec![(
            400,
            "application/json",
            br#"{"error":{"message":"model not loaded"}}"#.to_vec(),
        )]);
        let e = HttpSpeech.transcribe(&[0; 64], None, &cfg(&url));
        assert!(e.unwrap_err().to_string().contains("model not loaded"));
        assert!(HttpSpeech.transcribe(&[], None, &cfg("ftp://x")).is_err());
    }

    #[test]
    fn speaks_through_a_fake_server() {
        let wav = screenit_record::speech::wav_16k_mono(&[500i16; 4800 * 2]);
        let (url, h) = serve(vec![(200, "audio/wav", wav)]);
        let pcm = HttpSpeech.speak("Hello there", &cfg(&url)).unwrap();
        assert!(pcm.len() > 8000);
        let req = String::from_utf8_lossy(&h.join().unwrap()[0]).into_owned();
        assert!(req.starts_with("POST /v1/audio/speech"), "{req}");
        assert!(req.contains("Hello there") && req.contains("alloy") && req.contains("\"wav\""));
        assert!(HttpSpeech.speak("   ", &cfg(&url)).is_err());
        let (url, _h) = serve(vec![(200, "audio/mpeg", b"ID3....".to_vec())]);
        assert!(HttpSpeech.speak("hi", &cfg(&url)).is_err());
    }
}
