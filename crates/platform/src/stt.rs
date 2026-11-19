//! Transcription with the recognizer built into Windows (SAPI 5 dictation), offline.
//! Used when no speech-to-text server is configured. Accuracy is basic and it only knows the
//! languages installed under Settings > Time & language > Speech; a Whisper-style server is
//! better when one is available.

use screenit_record::RecordError;
use screenit_record::speech::{SpeechConfig, SpeechToText, Transcript, Word, normalize_wav};
use std::time::{Duration, Instant};
use windows::Win32::Media::Speech::{
    ISpRecoContext, ISpRecoResult, ISpRecognizer, ISpStream, SPEI_END_SR_STREAM, SPEI_RECOGNITION,
    SPEVENT, SPFM_OPEN_READONLY, SPRECORESULTTIMES, SPRS_ACTIVE, SpInprocRecognizer, SpStream,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::core::{IUnknown, Interface, PCWSTR, PWSTR};

pub struct WinStt;

fn plat<E: std::fmt::Display>(e: E) -> RecordError {
    RecordError::Platform(format!("windows speech recognition: {e}"))
}

/// Longest audio accepted (two hours of 16 kHz mono 16-bit).
const MAX_BYTES: usize = 16_000 * 2 * 7_200;
/// SAPI's "flag check" bits that must accompany an event interest mask.
const FLAGCHECK: u64 = (1 << 30) | (1 << 33);

fn interest(ids: &[i32]) -> u64 {
    ids.iter().fold(FLAGCHECK, |m, id| m | (1u64 << id))
}

/// Spread a phrase's words over its time span, in proportion to their length.
fn spread_words(text: &str, start_ms: u64, end_ms: u64, out: &mut Vec<Word>) {
    let words: Vec<&str> = text.split_whitespace().collect();
    let total: usize = words.iter().map(|w| w.chars().count().max(1)).sum();
    if total == 0 {
        return;
    }
    let span = end_ms.saturating_sub(start_ms).max(words.len() as u64 * 50);
    let mut done = 0usize;
    for w in words {
        let len = w.chars().count().max(1);
        let a = start_ms + span * done as u64 / total as u64;
        done += len;
        let b = start_ms + span * done as u64 / total as u64;
        out.push(Word {
            start_ms: a,
            end_ms: b.max(a + 1),
            text: w.to_string(),
        });
    }
}

struct TempFile(std::path::PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// One finished phrase pulled out of a recognition event.
fn phrase_of(ev: &SPEVENT) -> Option<(String, u64, u64)> {
    if ev.lParam.0 == 0 {
        return None;
    }
    // SAFETY: for a recognition event `lParam` is an `ISpRecoResult` the event queue handed us
    // one reference to; `from_raw` takes that reference over so it is released on drop.
    let unk = unsafe { IUnknown::from_raw(ev.lParam.0 as *mut core::ffi::c_void) };
    let result: ISpRecoResult = unk.cast().ok()?;
    let mut text = PWSTR::null();
    // SAFETY: `text` receives a CoTaskMem string that is freed below.
    unsafe {
        result
            .GetText(u32::MAX, u32::MAX, true, &mut text, None)
            .ok()?;
    }
    // SAFETY: `text` is a valid NUL-terminated string from GetText, or null.
    let s = if text.is_null() {
        String::new()
    } else {
        unsafe { text.to_string().unwrap_or_default() }
    };
    if !text.is_null() {
        // SAFETY: allocated by COM in GetText.
        unsafe { CoTaskMemFree(Some(text.0 as *const _)) };
    }
    let mut times = SPRECORESULTTIMES::default();
    // SAFETY: plain out-struct.
    let have = unsafe { result.GetResultTimes(&mut times) }.is_ok();
    // 16 kHz mono 16-bit is 32 bytes per millisecond.
    let end = ev.ullAudioStreamOffset / 32;
    let len = if have { times.ullLength / 10_000 } else { 0 };
    let start = end.saturating_sub(len);
    Some((s, start, end.max(start + 1)))
}

fn recognize(wav: &[u8]) -> Result<Transcript, RecordError> {
    if wav.len() > MAX_BYTES + 44 {
        return Err(plat(
            "the recording is too long for the built-in recognizer",
        ));
    }
    let dir = std::env::temp_dir();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file = TempFile(dir.join(format!("screenit-stt-{}-{stamp}.wav", std::process::id())));
    std::fs::write(&file.0, wav).map_err(plat)?;
    let wide: Vec<u16> = file
        .0
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let audio_secs = (wav.len() as u64 / 32_000).max(1);
    let deadline = Instant::now() + Duration::from_secs(60 + audio_secs * 3);

    // SAFETY: COM calls on objects created and used on this thread only; every pointer passed is
    // a live local; the event loop ends at the stream end, an error or the deadline.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let rec: ISpRecognizer =
            CoCreateInstance(&SpInprocRecognizer, None, CLSCTX_ALL).map_err(|_| {
                plat("no recognizer is installed (Settings > Time & language > Speech)")
            })?;
        let stream: ISpStream = CoCreateInstance(&SpStream, None, CLSCTX_ALL).map_err(plat)?;
        stream
            .BindToFile(PCWSTR(wide.as_ptr()), SPFM_OPEN_READONLY, None, None, 0)
            .map_err(plat)?;
        rec.SetInput(&stream, false).map_err(plat)?;
        let ctx: ISpRecoContext = rec.CreateRecoContext().map_err(plat)?;
        ctx.SetNotifyWin32Event().map_err(plat)?;
        let mask = interest(&[SPEI_RECOGNITION.0, SPEI_END_SR_STREAM.0]);
        ctx.SetInterest(mask, mask).map_err(plat)?;
        let grammar = ctx.CreateGrammar(1).map_err(plat)?;
        grammar
            .LoadDictation(PCWSTR::null(), Default::default())
            .map_err(plat)?;
        grammar.SetDictationState(SPRS_ACTIVE).map_err(plat)?;

        let mut words: Vec<Word> = Vec::new();
        let mut text = String::new();
        'wait: loop {
            if Instant::now() > deadline {
                return Err(plat("timed out"));
            }
            let _ = ctx.WaitForNotifyEvent(500);
            loop {
                let mut ev = SPEVENT::default();
                let mut got = 0u32;
                if ctx.GetEvents(1, &mut ev, &mut got).is_err() || got == 0 {
                    break;
                }
                let id = ev._bitfield & 0xffff;
                if id == SPEI_END_SR_STREAM.0 {
                    break 'wait;
                }
                if id == SPEI_RECOGNITION.0
                    && let Some((s, a, b)) = phrase_of(&ev)
                    && !s.trim().is_empty()
                {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(s.trim());
                    spread_words(&s, a, b, &mut words);
                }
            }
        }
        let _ = grammar.SetDictationState(Default::default());
        Ok(Transcript {
            language: String::new(),
            text,
            words,
        })
    }
}

impl SpeechToText for WinStt {
    fn transcribe(
        &self,
        wav: &[u8],
        _language: Option<&str>,
        _cfg: &SpeechConfig,
    ) -> Result<Transcript, RecordError> {
        if wav.len() < 64 {
            return Err(RecordError::BadConfig("no audio to transcribe".into()));
        }
        recognize(&normalize_wav(wav))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenit_record::speech::{TextToSpeech, wav_16k_mono};

    #[test]
    fn empty_audio_is_an_error() {
        assert!(
            WinStt
                .transcribe(&[], None, &SpeechConfig::default())
                .is_err()
        );
    }

    #[test]
    fn words_cover_the_span_in_order() {
        let mut w = Vec::new();
        spread_words("one two three", 1000, 2000, &mut w);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].start_ms, 1000);
        assert!(w.windows(2).all(|p| p[0].end_ms <= p[1].start_ms + 1));
        assert!(w[2].end_ms <= 2001);
        let mut none = Vec::new();
        spread_words("   ", 0, 10, &mut none);
        assert!(none.is_empty());
    }

    /// Needs the Windows voice and recognizer; skipped quietly where either is missing.
    #[test]
    fn hears_what_the_windows_voice_says() {
        let Ok(pcm) = crate::tts::WinTts.speak(
            "Hello and welcome. Today we will click the save button.",
            &SpeechConfig::default(),
        ) else {
            eprintln!("no voice available");
            return;
        };
        let wav = wav_16k_mono(&pcm);
        match WinStt.transcribe(&wav, None, &SpeechConfig::default()) {
            Ok(t) => {
                eprintln!("heard: {:?} {:?}", t.text, t.words);
                let lower = t.text.to_lowercase();
                assert!(
                    lower.contains("welcome") || lower.contains("save"),
                    "{lower}"
                );
                assert!(t.words.iter().all(|w| w.end_ms > w.start_ms));
            }
            Err(e) => eprintln!("no recognizer available: {e}"),
        }
    }
}
