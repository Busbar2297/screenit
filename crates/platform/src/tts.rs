//! Narration with the built-in Windows voices (Windows.Media.SpeechSynthesis), offline.
//! Used when no text-to-speech server is configured.

use screenit_record::RecordError;
use screenit_record::speech::{SpeechConfig, TextToSpeech, pcm_from_wav};
use windows::Media::SpeechSynthesis::SpeechSynthesizer;
use windows::Storage::Streams::DataReader;

pub struct WinTts;

fn plat<E: std::fmt::Display>(e: E) -> RecordError {
    RecordError::Platform(format!("tts: {e}"))
}

/// Longest text spoken in one call.
const MAX_CHARS: usize = 4_000;
/// Largest synthesized stream accepted (about 10 minutes of 48 kHz stereo).
const MAX_BYTES: u64 = 120_000_000;

impl TextToSpeech for WinTts {
    fn speak(&self, text: &str, cfg: &SpeechConfig) -> Result<Vec<i16>, RecordError> {
        let text: String = text.chars().take(MAX_CHARS).collect();
        if text.trim().is_empty() {
            return Err(RecordError::BadConfig("nothing to say".into()));
        }
        let synth = SpeechSynthesizer::new().map_err(plat)?;
        let want = cfg.voice.trim().to_lowercase();
        if !want.is_empty()
            && let Ok(voices) = SpeechSynthesizer::AllVoices()
        {
            for v in voices {
                let name = v
                    .DisplayName()
                    .map(|n| n.to_string().to_lowercase())
                    .unwrap_or_default();
                if name.contains(&want) {
                    synth.SetVoice(&v).map_err(plat)?;
                    break;
                }
            }
        }
        let stream = synth
            .SynthesizeTextToStreamAsync(&text.as_str().into())
            .map_err(plat)?
            .join()
            .map_err(plat)?;
        let size = stream.Size().map_err(plat)?;
        if size == 0 || size > MAX_BYTES {
            return Err(plat("the voice produced no usable audio"));
        }
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).map_err(plat)?)
            .map_err(plat)?;
        let n = reader
            .LoadAsync(size as u32)
            .map_err(plat)?
            .join()
            .map_err(plat)? as usize;
        let mut bytes = vec![0u8; n];
        reader.ReadBytes(&mut bytes).map_err(plat)?;
        pcm_from_wav(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_an_error() {
        assert!(WinTts.speak("  ", &SpeechConfig::default()).is_err());
    }

    /// Needs the Windows speech stack; skipped quietly where no voice is installed.
    #[test]
    fn speaks_with_the_installed_voice() {
        match WinTts.speak("Hello from screen it.", &SpeechConfig::default()) {
            Ok(pcm) => assert!(pcm.len() > 4800, "{}", pcm.len()),
            Err(e) => eprintln!("no voice available: {e}"),
        }
    }
}
