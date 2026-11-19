//! Built-in transcription with OpenAI Whisper (MIT weights), run on the CPU in pure Rust
//! (candle). Models are downloaded once from Hugging Face into the app's data folder and then
//! work offline. Greedy decoding with Whisper's timestamp rules; word times are spread over
//! each timed segment.

use candle_core::{Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::whisper::{self as m, Config, audio, model::Whisper as Model};
use screenit_record::RecordError;
use screenit_record::speech::{
    LocalModel, LocalSpeech, SpeechConfig, SpeechToText, Transcript, Word,
};
use screenit_update::UpdateHost;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

mod mel;
mod tok;

use tok::Tok;

/// `(name, approximate download in MB, note)`.
pub const MODELS: [(&str, u64, &str); 3] = [
    ("tiny", 151, "fastest, rough"),
    ("base", 290, "good balance"),
    ("small", 967, "most accurate, slower"),
];
pub const DEFAULT_MODEL: &str = "base";
const FILES: [&str; 3] = ["config.json", "tokenizer.json", "model.safetensors"];
const MAX_FILE: u64 = 1_200_000_000;
/// Longest audio accepted (three hours).
const MAX_SECONDS: usize = 3 * 3600;
/// Tokens sampled per 30 s window at most.
const SAMPLE_LEN: usize = 224;

fn plat<E: std::fmt::Display>(e: E) -> RecordError {
    RecordError::Platform(format!("whisper: {e}"))
}

struct Loaded {
    name: String,
    model: Model,
    tok: Tok,
    filters: Vec<f32>,
}

pub struct Whisper {
    dir: PathBuf,
    net: Arc<dyn UpdateHost>,
    loaded: Mutex<Option<Loaded>>,
}

fn known(model: &str) -> Result<&'static str, RecordError> {
    MODELS
        .iter()
        .find(|(n, _, _)| n.eq_ignore_ascii_case(model.trim()))
        .map(|(n, _, _)| *n)
        .ok_or_else(|| {
            RecordError::BadConfig(format!(
                "unknown Whisper model `{model}` (tiny, base, small)"
            ))
        })
}

impl Whisper {
    /// Models live in `dir/whisper-<name>/`.
    pub fn new(dir: PathBuf, net: Arc<dyn UpdateHost>) -> Self {
        Self {
            dir,
            net,
            loaded: Mutex::new(None),
        }
    }

    fn model_dir(&self, name: &str) -> PathBuf {
        self.dir.join(format!("whisper-{name}"))
    }

    fn is_installed(&self, name: &str) -> bool {
        let d = self.model_dir(name);
        FILES.iter().all(|f| d.join(f).is_file())
    }

    fn load(&self, name: &str) -> Result<Loaded, RecordError> {
        let d = self.model_dir(name);
        if !self.is_installed(name) {
            return Err(RecordError::BadConfig(format!(
                "the Whisper `{name}` model is not downloaded yet (Settings > Speech)"
            )));
        }
        let cfg: Config =
            serde_json::from_str(&std::fs::read_to_string(d.join("config.json")).map_err(plat)?)
                .map_err(plat)?;
        let tok = Tok::load(&d.join("tokenizer.json"), cfg.vocab_size)?;
        let bytes = std::fs::read(d.join("model.safetensors")).map_err(plat)?;
        let vb =
            VarBuilder::from_buffered_safetensors(bytes, m::DTYPE, &Device::Cpu).map_err(plat)?;
        let filters = mel::filters(cfg.num_mel_bins);
        let model = Model::load(&vb, cfg).map_err(plat)?;
        Ok(Loaded {
            name: name.to_string(),
            model,
            tok,
            filters,
        })
    }
}

impl LocalSpeech for Whisper {
    fn models(&self) -> Vec<LocalModel> {
        MODELS
            .iter()
            .map(|(n, mb, note)| LocalModel {
                name: (*n).to_string(),
                size_mb: *mb,
                note: (*note).to_string(),
                installed: self.is_installed(n),
            })
            .collect()
    }

    fn installed(&self, model: &str) -> bool {
        known(model).is_ok_and(|n| self.is_installed(n))
    }

    fn download(&self, model: &str, progress: &dyn Fn(u64, u64)) -> Result<(), RecordError> {
        let name = known(model)?;
        let d = self.model_dir(name);
        std::fs::create_dir_all(&d).map_err(plat)?;
        let total = MODELS
            .iter()
            .find(|(n, _, _)| *n == name)
            .map_or(0, |(_, mb, _)| mb * 1_048_576);
        let mut base = 0u64;
        for f in FILES {
            let dest = d.join(f);
            if dest.is_file() {
                continue;
            }
            let url = format!("https://huggingface.co/openai/whisper-{name}/resolve/main/{f}");
            let at = base;
            self.net
                .download(&url, &dest, MAX_FILE, &|done, _| {
                    progress((at + done).min(total.max(at + done)), total)
                })
                .map_err(|e| plat(format!("download of {f} failed: {e}")))?;
            base += std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        }
        Ok(())
    }
}

/// 16-bit mono samples of a WAV made by `wav_16k_mono`, as floats.
fn samples_of(wav: &[u8]) -> Result<Vec<f32>, RecordError> {
    if wav.len() <= 44 || wav.get(0..4) != Some(b"RIFF") {
        return Err(RecordError::BadConfig("no audio to transcribe".into()));
    }
    let start = wav
        .get(12..wav.len().min(512))
        .and_then(|h| h.windows(4).position(|w| w == b"data"))
        .map_or(44, |i| 12 + i + 8);
    let body = wav.get(start..).unwrap_or(&[]);
    if body.len() / 2 > MAX_SECONDS * m::SAMPLE_RATE {
        return Err(RecordError::BadConfig(
            "the recording is too long to transcribe (3 hours at most)".into(),
        ));
    }
    Ok(body
        .chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0)
        .collect())
}

/// (start, end, text tokens); times in 20 ms timestamp steps.
type Segment = (u32, u32, Vec<u32>);

/// Timed pieces of one window: `(start token, end token, text tokens)`, plus how far the window
/// was fully consumed (in timestamp steps of 20 ms), when it ended mid-segment.
pub(crate) fn segments(sampled: &[u32], ts_begin: u32, eot: u32) -> (Vec<Segment>, Option<u32>) {
    let toks: Vec<u32> = sampled.iter().copied().take_while(|t| *t != eot).collect();
    let mut out = Vec::new();
    let mut start: Option<u32> = None;
    let mut text: Vec<u32> = Vec::new();
    let mut last_end: Option<u32> = None;
    for t in &toks {
        if *t >= ts_begin {
            let at = t - ts_begin;
            match start {
                None => start = Some(at),
                Some(s) => {
                    if !text.is_empty() {
                        out.push((s, at, std::mem::take(&mut text)));
                    }
                    last_end = Some(at);
                    start = None;
                }
            }
        } else if *t < eot {
            text.push(*t);
        }
    }
    // A segment opened but never closed: keep its text up to the window end.
    let single_ending = toks.len() >= 2
        && toks.last().is_some_and(|t| *t >= ts_begin)
        && toks.get(toks.len() - 2).is_some_and(|t| *t < ts_begin);
    if out.is_empty() && !text.is_empty() {
        out.push((start.unwrap_or(0), 1500, text));
        return (out, None);
    }
    if let Some(s) = start
        && !text.is_empty()
    {
        out.push((s, 1500, text));
        return (out, None);
    }
    let seek = if single_ending { None } else { last_end };
    (out, seek)
}

/// Whisper's decoding rules on one step's logits.
pub(crate) fn apply_rules(logits: &mut [f32], sampled: &[u32], tok: &Tok, suppress: &[u32]) {
    let ts = tok.ts_begin as usize;
    let n = logits.len();
    let kill = |i: usize, l: &mut [f32]| {
        if let Some(v) = l.get_mut(i) {
            *v = f32::NEG_INFINITY;
        }
    };
    for &s in suppress
        .iter()
        .chain([tok.no_ts, tok.sot, tok.transcribe, tok.translate].iter())
    {
        kill(s as usize, logits);
    }
    for (_, id) in &tok.langs {
        kill(*id as usize, logits);
    }
    let is_ts = |t: &u32| *t >= tok.ts_begin;
    if sampled.is_empty() {
        // Start with a timestamp no later than one second.
        for i in 0..ts.min(n) {
            kill(i, logits);
        }
        for i in (ts + 51).min(n)..n {
            kill(i, logits);
        }
        return;
    }
    let last_ts = sampled.last().is_some_and(is_ts);
    let penult_ts = sampled.len() < 2 || sampled.get(sampled.len() - 2).is_some_and(is_ts);
    if last_ts {
        if penult_ts {
            for i in ts.min(n)..n {
                kill(i, logits);
            }
        } else {
            for i in 0..(tok.eot as usize).min(n) {
                kill(i, logits);
            }
        }
    }
    if let Some(last) = sampled.iter().rev().find(|t| is_ts(t)) {
        let upto = if last_ts && !penult_ts {
            *last
        } else {
            last + 1
        };
        for i in ts.min(n)..(upto as usize).min(n) {
            kill(i, logits);
        }
    }
    // Prefer a timestamp when timestamps together are likelier than any single text token.
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if !max.is_finite() {
        return;
    }
    let sum: f32 = logits.iter().map(|v| (v - max).exp()).sum();
    let lse = max + sum.max(f32::MIN_POSITIVE).ln();
    let ts_mass: f32 = logits
        .get(ts.min(n)..)
        .unwrap_or(&[])
        .iter()
        .map(|v| (v - lse).exp())
        .sum();
    let text_max = logits
        .get(..ts.min(n))
        .unwrap_or(&[])
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    if ts_mass > 0.0 && ts_mass.ln() > text_max - lse {
        for i in 0..ts.min(n) {
            kill(i, logits);
        }
    }
}

fn argmax(v: &[f32]) -> u32 {
    let mut best = (0usize, f32::NEG_INFINITY);
    for (i, x) in v.iter().enumerate() {
        if *x > best.1 {
            best = (i, *x);
        }
    }
    best.0 as u32
}

fn spread(text: &str, start_ms: u64, end_ms: u64, out: &mut Vec<Word>) {
    let words: Vec<&str> = text.split_whitespace().collect();
    let total: usize = words.iter().map(|w| w.chars().count().max(1)).sum();
    if total == 0 {
        return;
    }
    let span = end_ms.saturating_sub(start_ms).max(words.len() as u64 * 50);
    let mut done = 0usize;
    for w in words {
        let a = start_ms + span * done as u64 / total as u64;
        done += w.chars().count().max(1);
        let b = start_ms + span * done as u64 / total as u64;
        out.push(Word {
            start_ms: a,
            end_ms: b.max(a + 1),
            text: w.to_string(),
        });
    }
}

fn last_logits(
    model: &mut Model,
    tokens: &[u32],
    feats: &Tensor,
    flush: bool,
) -> candle_core::Result<Vec<f32>> {
    let t = Tensor::new(tokens, &Device::Cpu)?.unsqueeze(0)?;
    let ys = model.decoder.forward(&t, feats, flush)?;
    let (_, len, _) = ys.dims3()?;
    model
        .decoder
        .final_linear(&ys.i((..1, len.saturating_sub(1)..))?)?
        .i(0)?
        .i(0)?
        .to_vec1::<f32>()
}

fn run(l: &mut Loaded, pcm: &[f32], language: Option<&str>) -> candle_core::Result<Transcript> {
    let cfg = l.model.config.clone();
    let n_mels = cfg.num_mel_bins;
    let mel = audio::pcm_to_mel(&cfg, pcm, &l.filters);
    let frames = mel.len() / n_mels.max(1);
    let mel = Tensor::from_vec(mel, (1, n_mels, frames), &Device::Cpu)?;
    let content = pcm.len() / m::HOP_LENGTH;
    let tok = &l.tok;
    let mut lang_tok: Option<u32> = language
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .and_then(|s| tok.lang(&s));
    let mut words = Vec::new();
    let mut text = String::new();
    let mut seek = 0usize;
    while seek < content {
        let size = m::N_FRAMES.min(frames.saturating_sub(seek));
        if size == 0 {
            break;
        }
        let seg = mel.narrow(2, seek, size)?;
        let feats = l.model.encoder.forward(&seg, true)?;
        let lang = match lang_tok {
            Some(t) => t,
            None => {
                let logits = last_logits(&mut l.model, &[tok.sot], &feats, true)?;
                let best = tok
                    .langs
                    .iter()
                    .max_by(|a, b| {
                        let va = logits.get(a.1 as usize).copied().unwrap_or(f32::MIN);
                        let vb = logits.get(b.1 as usize).copied().unwrap_or(f32::MIN);
                        va.total_cmp(&vb)
                    })
                    .map_or(tok.sot, |(_, id)| *id);
                lang_tok = Some(best);
                best
            }
        };
        let prompt = [tok.sot, lang, tok.transcribe];
        let mut tokens: Vec<u32> = prompt.to_vec();
        let mut no_speech = 0f32;
        for i in 0..SAMPLE_LEN {
            let t = Tensor::new(tokens.as_slice(), &Device::Cpu)?.unsqueeze(0)?;
            let ys = l.model.decoder.forward(&t, &feats, i == 0)?;
            if i == 0
                && let Some(ns) = tok.no_speech
            {
                let first = l.model.decoder.final_linear(&ys.i(..1)?)?.i(0)?.i(0)?;
                let p = candle_nn::ops::softmax(&first, 0)?.to_vec1::<f32>()?;
                no_speech = p.get(ns as usize).copied().unwrap_or(0.0);
            }
            let (_, len, _) = ys.dims3()?;
            let mut logits = l
                .model
                .decoder
                .final_linear(&ys.i((..1, len.saturating_sub(1)..))?)?
                .i(0)?
                .i(0)?
                .to_vec1::<f32>()?;
            apply_rules(
                &mut logits,
                &tokens[prompt.len()..],
                tok,
                &cfg.suppress_tokens,
            );
            let next = argmax(&logits);
            tokens.push(next);
            if next == tok.eot || tokens.len() >= cfg.max_target_positions.saturating_sub(1) {
                break;
            }
        }
        let sampled = &tokens[prompt.len()..];
        let (segs, ends_at) = segments(sampled, tok.ts_begin, tok.eot);
        let offset_ms = (seek * m::HOP_LENGTH * 1000 / m::SAMPLE_RATE) as u64;
        if no_speech < 0.6 || segs.iter().any(|s| s.2.len() > 3) {
            for (a, b, ids) in &segs {
                let s = tok.decode(ids);
                let s = s.trim();
                if s.is_empty() {
                    continue;
                }
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(s);
                spread(
                    s,
                    offset_ms + u64::from(*a) * 20,
                    offset_ms + u64::from(*b).max(u64::from(*a) + 1) * 20,
                    &mut words,
                );
            }
        }
        // Timestamps step 20 ms = 2 mel frames; never move less than one second.
        let step = ends_at
            .map(|e| (e as usize) * 2)
            .filter(|f| *f >= 100)
            .unwrap_or(size);
        seek += step;
    }
    let language = lang_tok
        .and_then(|id| tok.langs.iter().find(|(_, i)| *i == id))
        .map(|(n, _)| n.clone())
        .unwrap_or_default();
    Ok(Transcript {
        language,
        text,
        words,
    })
}

impl SpeechToText for Whisper {
    fn transcribe(
        &self,
        wav: &[u8],
        language: Option<&str>,
        cfg: &SpeechConfig,
    ) -> Result<Transcript, RecordError> {
        let pcm = samples_of(wav)?;
        let name = known(if cfg.model.trim().is_empty() {
            DEFAULT_MODEL
        } else {
            &cfg.model
        })?;
        let mut g = self
            .loaded
            .lock()
            .map_err(|_| plat("model lock poisoned"))?;
        if g.as_ref().is_none_or(|l| l.name != name) {
            *g = None;
            *g = Some(self.load(name)?);
        }
        let l = g.as_mut().ok_or_else(|| plat("model not loaded"))?;
        l.model.reset_kv_cache();
        run(l, &pcm, language).map_err(plat)
    }
}

/// Paths of a model's files, for tests and the settings page.
pub fn model_files(dir: &Path, name: &str) -> Vec<PathBuf> {
    let d = dir.join(format!("whisper-{name}"));
    FILES.iter().map(|f| d.join(f)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok() -> Tok {
        Tok::fake()
    }

    #[test]
    fn segments_split_on_timestamp_pairs() {
        let t = tok();
        let ts = t.ts_begin;
        let s = [ts, 10, 11, ts + 50, ts + 50, 12, ts + 100, t.eot];
        let (segs, seek) = segments(&s, ts, t.eot);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], (0, 50, vec![10, 11]));
        assert_eq!(segs[1], (50, 100, vec![12]));
        assert_eq!(seek, None, "a single closing timestamp consumed the window");
        let open = [ts, 10, ts + 40, ts + 40, t.eot];
        assert_eq!(segments(&open, ts, t.eot).1, Some(40));
        let (segs, seek) = segments(&[10, 11, t.eot], ts, t.eot);
        assert_eq!(segs.len(), 1);
        assert_eq!(seek, None);
        assert!(segments(&[], ts, t.eot).0.is_empty());
    }

    #[test]
    fn rules_force_a_starting_timestamp_and_pairs() {
        let t = tok();
        let n = t.ts_begin as usize + 200;
        let mut l = vec![0f32; n];
        l[5] = 10.0;
        apply_rules(&mut l, &[], &t, &[]);
        assert!(argmax(&l) >= t.ts_begin && argmax(&l) <= t.ts_begin + 50);
        // After "ts text ts" only text or a later timestamp; after "ts ts" no timestamp.
        let mut l = vec![0f32; n];
        l[t.ts_begin as usize + 1] = 10.0;
        l[7] = 5.0;
        apply_rules(
            &mut l,
            &[t.ts_begin, 7, t.ts_begin + 3, t.ts_begin + 3],
            &t,
            &[],
        );
        assert_eq!(argmax(&l), 7);
        let mut bad = vec![f32::NAN; 3];
        apply_rules(&mut bad, &[1], &t, &[99_999]);
    }

    #[test]
    fn hostile_audio_is_an_error_not_a_panic() {
        assert!(samples_of(b"").is_err());
        assert!(samples_of(b"RIFFxxxxWAVE").is_err());
        assert!(known("huge").is_err());
        assert_eq!(known(" Base ").ok(), Some("base"));
    }

    #[test]
    fn words_spread_over_the_segment() {
        let mut w = Vec::new();
        spread("hello big world", 1000, 2000, &mut w);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].start_ms, 1000);
        assert!(w[2].end_ms <= 2001);
    }

    /// Downloads the tiny model (~150 MB) and transcribes the Windows voice. Run by hand:
    /// `cargo test -p screenit-whisper -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn transcribes_real_speech_with_the_tiny_model() {
        let dir = std::env::var("SCREENIT_WHISPER_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("screenit-whisper-test"));
        let wav = std::fs::read(dir.join("speech.wav")).unwrap();
        struct NoNet;
        impl UpdateHost for NoNet {
            fn get_text(&self, _: &str, _: u64) -> Result<String, String> {
                Err("offline".into())
            }
            fn download(
                &self,
                _: &str,
                _: &Path,
                _: u64,
                _: &dyn Fn(u64, u64),
            ) -> Result<(), String> {
                Err("offline".into())
            }
            fn launch_installer(&self, _: &Path) -> Result<(), String> {
                Err("no".into())
            }
        }
        let w = Whisper::new(dir, Arc::new(NoNet));
        let cfg = SpeechConfig {
            model: "tiny".into(),
            ..SpeechConfig::default()
        };
        let t = w.transcribe(&wav, Some("en"), &cfg).unwrap();
        eprintln!("{t:?}");
        assert!(t.text.to_lowercase().contains("save"), "{}", t.text);
    }
}
