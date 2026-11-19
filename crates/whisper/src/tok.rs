//! The parts of Whisper's byte-level BPE tokenizer that decoding needs: id to text, and the
//! special token ids, read from Hugging Face's `tokenizer.json`.

use screenit_record::RecordError;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

pub(crate) struct Tok {
    words: Vec<String>,
    bytes: HashMap<char, u8>,
    pub sot: u32,
    pub eot: u32,
    pub transcribe: u32,
    pub translate: u32,
    pub no_ts: u32,
    pub no_speech: Option<u32>,
    pub ts_begin: u32,
    /// `("en", id)` for every language token.
    pub langs: Vec<(String, u32)>,
}

fn bad(m: &str) -> RecordError {
    RecordError::Platform(format!("whisper tokenizer: {m}"))
}

/// GPT-2's printable stand-ins for raw bytes, reversed.
fn byte_decoder() -> HashMap<char, u8> {
    let mut bs: Vec<u32> = (33..=126).chain(161..=172).chain(174..=255).collect();
    let mut cs = bs.clone();
    let mut n = 0u32;
    for b in 0..256u32 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    bs.iter()
        .zip(cs)
        .filter_map(|(b, c)| Some((char::from_u32(c)?, u8::try_from(*b).ok()?)))
        .collect()
}

fn is_lang(s: &str) -> Option<&str> {
    let inner = s.strip_prefix("<|")?.strip_suffix("|>")?;
    (2..=3).contains(&inner.len()).then_some(())?;
    inner
        .chars()
        .all(|c| c.is_ascii_lowercase())
        .then_some(inner)
}

impl Tok {
    pub(crate) fn load(path: &Path, vocab_size: usize) -> Result<Self, RecordError> {
        let text = std::fs::read_to_string(path).map_err(|e| bad(&e.to_string()))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| bad(&e.to_string()))?;
        let size = vocab_size.clamp(1, 200_000);
        let mut words = vec![String::new(); size];
        let mut special: HashMap<String, u32> = HashMap::new();
        if let Some(map) = v["model"]["vocab"].as_object() {
            for (w, id) in map {
                if let Some(slot) = id.as_u64().and_then(|i| words.get_mut(i as usize)) {
                    *slot = w.clone();
                }
            }
        }
        for t in v["added_tokens"].as_array().into_iter().flatten() {
            if let (Some(id), Some(c)) = (t["id"].as_u64(), t["content"].as_str()) {
                if let Some(slot) = words.get_mut(id as usize) {
                    *slot = c.to_string();
                }
                special.insert(c.to_string(), id as u32);
            }
        }
        Self::from_parts(words, &special)
    }

    fn from_parts(words: Vec<String>, special: &HashMap<String, u32>) -> Result<Self, RecordError> {
        let need = |k: &str| {
            special
                .get(k)
                .copied()
                .ok_or_else(|| bad(&format!("missing {k}")))
        };
        let no_ts = need("<|notimestamps|>")?;
        let ts_begin = special.get("<|0.00|>").copied().unwrap_or(no_ts + 1);
        let mut langs: Vec<(String, u32)> = special
            .iter()
            .filter(|(_, id)| **id < ts_begin)
            .filter_map(|(k, id)| Some((is_lang(k)?.to_string(), *id)))
            .collect();
        langs.sort_by_key(|(_, id)| *id);
        Ok(Self {
            words,
            bytes: byte_decoder(),
            sot: need("<|startoftranscript|>")?,
            eot: need("<|endoftext|>")?,
            transcribe: need("<|transcribe|>")?,
            translate: need("<|translate|>")?,
            no_ts,
            no_speech: special
                .get("<|nospeech|>")
                .or_else(|| special.get("<|nocaptions|>"))
                .copied(),
            ts_begin,
            langs,
        })
    }

    pub(crate) fn lang(&self, code: &str) -> Option<u32> {
        self.langs
            .iter()
            .find(|(n, _)| n == code)
            .map(|(_, id)| *id)
    }

    /// Text of ordinary tokens (specials are skipped).
    pub(crate) fn decode(&self, ids: &[u32]) -> String {
        let mut out: Vec<u8> = Vec::new();
        for id in ids {
            if *id >= self.eot {
                continue;
            }
            let Some(w) = self.words.get(*id as usize) else {
                continue;
            };
            for c in w.chars() {
                if let Some(b) = self.bytes.get(&c) {
                    out.push(*b);
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[cfg(test)]
    pub(crate) fn fake() -> Self {
        let mut words: Vec<String> = (0..50_257).map(|_| String::new()).collect();
        words[10] = "Hello".into();
        words[11] = "\u{120}world".into();
        let special: HashMap<String, u32> = [
            ("<|endoftext|>", 50_257),
            ("<|startoftranscript|>", 50_258),
            ("<|en|>", 50_259),
            ("<|ar|>", 50_260),
            ("<|translate|>", 50_358),
            ("<|transcribe|>", 50_359),
            ("<|nospeech|>", 50_362),
            ("<|notimestamps|>", 50_363),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        words.resize(50_364, String::new());
        let mut t = Self::from_parts(words, &special).unwrap_or_else(|_| unreachable!());
        t.words.resize(50_364 + 1501, String::new());
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_byte_level_text_and_finds_specials() {
        let t = Tok::fake();
        assert_eq!(t.decode(&[10, 11]), "Hello world");
        assert_eq!(t.decode(&[10, t.eot, 99_999]), "Hello");
        assert_eq!(t.lang("ar"), Some(50_260));
        assert_eq!(t.ts_begin, 50_364);
        assert_eq!(t.langs.len(), 2);
        assert!(t.no_speech.is_some());
    }

    #[test]
    fn missing_or_garbage_files_are_errors() {
        let d = std::env::temp_dir().join(format!("scit-tok-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let f = d.join("t.json");
        std::fs::write(&f, "not json").unwrap();
        assert!(Tok::load(&f, 51_865).is_err());
        std::fs::write(&f, "{}").unwrap();
        assert!(Tok::load(&f, 51_865).is_err());
        assert!(Tok::load(&d.join("none.json"), 10).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
