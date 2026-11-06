//! Speech seams: turning recorded speech into text with word times (subtitles, filler cut) and
//! text into speech (narration). Backends talk to a local server (`screenit-llm`) or the OS;
//! this module holds the data types, the traits and the pure audio helpers they share.

use crate::RecordError;
use crate::audio::{CHANNELS, RATE};
use serde::{Deserialize, Serialize};

/// Where a speech server lives. `url` is its base, e.g. `http://localhost:8000/v1`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechConfig {
    pub url: String,
    pub model: String,
    pub voice: String,
    pub api_key: String,
    pub timeout_secs: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Word {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transcript {
    pub language: String,
    pub text: String,
    pub words: Vec<Word>,
}

/// A caption block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// Speech to text. `wav` is a 16-bit mono WAV (see [`wav_16k_mono`]).
pub trait SpeechToText: Send + Sync {
    fn transcribe(
        &self,
        wav: &[u8],
        language: Option<&str>,
        cfg: &SpeechConfig,
    ) -> Result<Transcript, RecordError>;
}

/// Text to speech. Returns interleaved 48 kHz stereo 16-bit samples.
pub trait TextToSpeech: Send + Sync {
    fn speak(&self, text: &str, cfg: &SpeechConfig) -> Result<Vec<i16>, RecordError>;
}

fn bad(m: &str) -> RecordError {
    RecordError::Platform(format!("speech: {m}"))
}

/// A 16 kHz mono 16-bit WAV from the recorder's 48 kHz stereo samples (what Whisper expects).
pub fn wav_16k_mono(pcm: &[i16]) -> Vec<u8> {
    let ch = usize::from(CHANNELS);
    let step = (RATE / 16_000) as usize;
    let mut mono: Vec<i16> = Vec::with_capacity(pcm.len() / (ch * step) + 1);
    for group in pcm.chunks(ch * step) {
        let sum: i64 = group.iter().map(|v| i64::from(*v)).sum();
        mono.push((sum / group.len().max(1) as i64) as i16);
    }
    let data_len = (mono.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + mono.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&16_000u32.to_le_bytes());
    out.extend_from_slice(&32_000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in mono {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Largest WAV accepted from a server (about 10 minutes of 48 kHz stereo).
const MAX_WAV: usize = 120_000_000;

/// Decode a 16-bit PCM WAV (mono or stereo, any rate) into 48 kHz stereo samples.
pub fn pcm_from_wav(bytes: &[u8]) -> Result<Vec<i16>, RecordError> {
    if bytes.len() > MAX_WAV {
        return Err(bad("audio reply is too large"));
    }
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err(bad("the reply is not a WAV file"));
    }
    let u16_at = |i: usize| -> Option<u16> {
        Some(u16::from_le_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]))
    };
    let u32_at = |i: usize| -> Option<u32> {
        Some(u32::from_le_bytes([
            *bytes.get(i)?,
            *bytes.get(i + 1)?,
            *bytes.get(i + 2)?,
            *bytes.get(i + 3)?,
        ]))
    };
    let mut at = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    while at + 8 <= bytes.len() {
        let id = bytes.get(at..at + 4).unwrap_or(&[]);
        let len = u32_at(at + 4).ok_or_else(|| bad("truncated WAV"))? as usize;
        let body = at + 8;
        let end = body.saturating_add(len).min(bytes.len());
        if id == b"fmt " {
            fmt = Some((
                u16_at(body).ok_or_else(|| bad("truncated WAV"))?,
                u16_at(body + 2).ok_or_else(|| bad("truncated WAV"))?,
                u32_at(body + 4).ok_or_else(|| bad("truncated WAV"))?,
                u16_at(body + 14).ok_or_else(|| bad("truncated WAV"))?,
            ));
        } else if id == b"data" {
            data = bytes.get(body..end);
            break;
        }
        at = body.saturating_add(len).saturating_add(len & 1);
    }
    let (tag, channels, rate, bits) = fmt.ok_or_else(|| bad("WAV has no format"))?;
    let data = data.ok_or_else(|| bad("WAV has no audio"))?;
    if tag != 1 || bits != 16 || !(1..=2).contains(&channels) || rate == 0 || rate > 192_000 {
        return Err(bad("only 16-bit PCM WAV mono or stereo is supported"));
    }
    let ch = usize::from(channels);
    let frames: Vec<[i16; 2]> = data
        .chunks_exact(2 * ch)
        .map(|f| {
            let l = i16::from_le_bytes([f[0], f[1]]);
            let r = if ch == 2 {
                i16::from_le_bytes([f[2], f[3]])
            } else {
                l
            };
            [l, r]
        })
        .collect();
    if frames.is_empty() {
        return Err(bad("WAV has no audio"));
    }
    let out_frames = (frames.len() as u64 * u64::from(RATE) / u64::from(rate)) as usize;
    let mut out = Vec::with_capacity(out_frames * 2);
    for i in 0..out_frames {
        let pos = i as f64 * f64::from(rate) / f64::from(RATE);
        let k = pos as usize;
        let frac = pos - k as f64;
        let a = frames.get(k).copied().unwrap_or([0, 0]);
        let b = frames.get(k + 1).copied().unwrap_or(a);
        for c in 0..2 {
            let v = f64::from(a[c]) + (f64::from(b[c]) - f64::from(a[c])) * frac;
            out.push(v.round() as i16);
        }
    }
    Ok(out)
}

impl Transcript {
    /// Group words into caption lines: a line ends at sentence punctuation, after `max_chars`,
    /// after `max_ms`, or at a pause longer than 700 ms.
    pub fn lines(&self, max_chars: usize, max_ms: u64) -> Vec<Line> {
        let mut out: Vec<Line> = Vec::new();
        let mut cur: Option<Line> = None;
        let mut last_end = 0u64;
        for w in &self.words {
            let t = w.text.trim();
            if t.is_empty() {
                continue;
            }
            let pause = cur.is_some() && w.start_ms.saturating_sub(last_end) > 700;
            if pause && let Some(l) = cur.take() {
                out.push(l);
            }
            let line = cur.get_or_insert_with(|| Line {
                start_ms: w.start_ms,
                end_ms: w.end_ms,
                text: String::new(),
            });
            if !line.text.is_empty() {
                line.text.push(' ');
            }
            line.text.push_str(t);
            line.end_ms = w.end_ms.max(line.start_ms);
            last_end = w.end_ms;
            let full = line.text.chars().count() >= max_chars
                || line.end_ms.saturating_sub(line.start_ms) >= max_ms;
            let sentence = t.ends_with(['.', '!', '?', '؟', '。']);
            if (full || sentence)
                && let Some(l) = cur.take()
            {
                out.push(l);
            }
        }
        out.extend(cur);
        out
    }
}

/// Lines as an SRT document.
pub fn srt(lines: &[Line]) -> String {
    let t = |ms: u64| {
        format!(
            "{:02}:{:02}:{:02},{:03}",
            ms / 3_600_000,
            ms / 60_000 % 60,
            ms / 1000 % 60,
            ms % 1000
        )
    };
    let mut s = String::new();
    for (i, l) in lines.iter().enumerate() {
        s.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            t(l.start_ms),
            t(l.end_ms),
            l.text.replace("\r\n", "\n").trim()
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(a: u64, b: u64, t: &str) -> Word {
        Word {
            start_ms: a,
            end_ms: b,
            text: t.into(),
        }
    }

    #[test]
    fn wav_round_trips_through_the_resampler() {
        // 100 ms of a constant stereo signal at 48 kHz
        let pcm = vec![1000i16; 4800 * 2];
        let wav = wav_16k_mono(&pcm);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 1600 * 2);
        let back = pcm_from_wav(&wav).unwrap();
        // 1600 mono frames at 16 kHz become 4800 stereo frames at 48 kHz
        assert!((back.len() as i64 - 9600).abs() <= 2, "{}", back.len());
        assert!(back.iter().all(|v| (*v - 1000).abs() <= 1));
    }

    #[test]
    fn hostile_wav_files_are_errors() {
        assert!(pcm_from_wav(b"").is_err());
        assert!(pcm_from_wav(b"RIFFxxxxWAVE").is_err());
        let mut wav = wav_16k_mono(&[0; 96]);
        wav[20] = 3; // float format
        assert!(pcm_from_wav(&wav).is_err());
        let mut wav = wav_16k_mono(&[0; 96]);
        wav.truncate(30);
        assert!(pcm_from_wav(&wav).is_err());
        let mut wav = wav_16k_mono(&[0; 96]);
        wav[24..28].copy_from_slice(&0u32.to_le_bytes()); // rate 0
        assert!(pcm_from_wav(&wav).is_err());
    }

    #[test]
    fn words_group_into_lines() {
        let t = Transcript {
            language: "en".into(),
            text: String::new(),
            words: vec![
                w(0, 300, "Open"),
                w(320, 600, "the"),
                w(620, 900, "menu."),
                w(950, 1200, "Then"),
                w(1210, 1500, "click"),
                // a long pause
                w(3000, 3300, "Done"),
            ],
        };
        let l = t.lines(80, 6000);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].text, "Open the menu.");
        assert_eq!(l[1].text, "Then click");
        assert_eq!((l[2].start_ms, l[2].end_ms), (3000, 3300));
        let short = t.lines(8, 6000);
        assert!(short.len() > 3);
        assert!(srt(&l).starts_with("1\n00:00:00,000 --> 00:00:00,900\nOpen the menu."));
        assert!(Transcript::default().lines(10, 10).is_empty());
    }
}
