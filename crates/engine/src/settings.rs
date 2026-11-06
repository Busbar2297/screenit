//! Persisted user settings (`settings.json`).
//!
//! Every field has a default, unknown keys are ignored and a corrupt file falls back to
//! defaults, so loading can never fail the app.

use crate::{CommandError, Session};
use screenit_capture::Preset;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const SETTINGS_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Locale code (`en`, `ar`, ...). Empty means English.
    pub language: String,
    pub start_with_windows: bool,
    pub start_minimized: bool,
    /// `editor` opens the editor after a capture, `silent` saves and stays out of the way.
    pub after_capture: String,
    pub copy_to_clipboard: bool,
    pub include_cursor: bool,
    /// Empty means the default `<Pictures>\screenIt`.
    pub save_dir: String,
}

impl Default for General {
    fn default() -> Self {
        Self {
            language: "en".into(),
            start_with_windows: false,
            start_minimized: true,
            after_capture: "editor".into(),
            copy_to_clipboard: false,
            include_cursor: false,
            save_dir: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingSettings {
    pub fps: u32,
    /// `h264` or `h265`.
    pub codec: String,
    /// `native`, `1080p`, `1440p` or `2160p`.
    pub output: String,
    pub microphone: bool,
    pub system_audio: bool,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            fps: 30,
            codec: "h264".into(),
            output: "native".into(),
            microphone: false,
            system_audio: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OcrSettings {
    /// `windows`, `llm` or `windows_then_llm`.
    pub engine: String,
    /// `auto`, `en` or `ar`.
    pub language: String,
    pub llm_url: String,
    pub llm_model: String,
    /// Stored in the settings file in plain text; empty for LM Studio's default.
    pub llm_api_key: String,
    pub llm_timeout_secs: u32,
}

impl Default for OcrSettings {
    fn default() -> Self {
        Self {
            engine: "windows".into(),
            language: "auto".into(),
            llm_url: "http://localhost:1234/v1".into(),
            llm_model: String::new(),
            llm_api_key: String::new(),
            llm_timeout_secs: 60,
        }
    }
}

/// Local speech servers (OpenAI-compatible `/v1/audio/...`). Empty URLs switch the feature off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeechSettings {
    /// Speech to text, e.g. `http://localhost:8000/v1` (whisper.cpp server, Speaches).
    pub stt_url: String,
    pub stt_model: String,
    /// Spoken language hint (`en`, `ar`); empty to detect.
    pub stt_language: String,
    /// Text to speech server; empty means the built-in Windows voice.
    pub tts_url: String,
    pub tts_model: String,
    pub tts_voice: String,
    /// Stored in the settings file in plain text.
    pub api_key: String,
    pub timeout_secs: u32,
}

impl Default for SpeechSettings {
    fn default() -> Self {
        Self {
            stt_url: String::new(),
            stt_model: String::new(),
            stt_language: String::new(),
            tts_url: String::new(),
            tts_model: String::new(),
            tts_voice: String::new(),
            api_key: String::new(),
            timeout_secs: 300,
        }
    }
}

impl SpeechSettings {
    pub fn stt_config(&self) -> screenit_record::speech::SpeechConfig {
        screenit_record::speech::SpeechConfig {
            url: self.stt_url.trim().to_string(),
            model: self.stt_model.clone(),
            voice: String::new(),
            api_key: self.api_key.clone(),
            timeout_secs: self.timeout_secs,
        }
    }

    pub fn tts_config(&self) -> screenit_record::speech::SpeechConfig {
        screenit_record::speech::SpeechConfig {
            url: self.tts_url.trim().to_string(),
            model: self.tts_model.clone(),
            voice: self.tts_voice.clone(),
            api_key: self.api_key.clone(),
            timeout_secs: self.timeout_secs,
        }
    }
}

/// Hide secrets (passwords, keys and tokens in captures).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacySettings {
    /// Cover secrets in every new capture before it is copied or saved.
    pub auto_hide_secrets: bool,
    /// Also ask the language model (LM Studio) from the Hide secrets button.
    pub use_llm: bool,
    /// `pixelate` or `blur`.
    pub mode: String,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            auto_hide_secrets: false,
            use_llm: false,
            mode: "pixelate".into(),
        }
    }
}

/// Updates from GitHub Releases.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Look for a newer release at startup and once a day.
    pub auto_check: bool,
    /// Download a newer release in the background (installing always waits for a click).
    pub auto_download: bool,
    /// A version the user chose to ignore; empty for none.
    pub skipped_version: String,
    /// Unix time of the last check, 0 for never.
    pub last_check_unix: u64,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_check: true,
            auto_download: true,
            skipped_version: String::new(),
            last_check_unix: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub general: General,
    pub recording: RecordingSettings,
    pub ocr: OcrSettings,
    pub privacy: PrivacySettings,
    pub speech: SpeechSettings,
    pub updates: UpdateSettings,
    /// `None` means the built-in presets.
    pub presets: Option<Vec<Preset>>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            general: General::default(),
            recording: RecordingSettings::default(),
            ocr: OcrSettings::default(),
            privacy: PrivacySettings::default(),
            speech: SpeechSettings::default(),
            updates: UpdateSettings::default(),
            presets: None,
        }
    }
}

fn one_of(what: &str, v: &str, allowed: &[&str]) -> Result<(), CommandError> {
    if allowed.contains(&v) {
        Ok(())
    } else {
        Err(CommandError::Params(format!(
            "{what} must be one of: {}",
            allowed.join(", ")
        )))
    }
}

impl Settings {
    /// Read a settings file; anything wrong yields defaults.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let mut s: Settings = serde_json::from_str(&text).unwrap_or_default();
        s.sanitize();
        s
    }

    /// Atomic write: temp file then rename.
    pub fn save(&self, path: &Path) -> Result<(), CommandError> {
        let io = |e: std::io::Error| CommandError::Failed(format!("settings: {e}"));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| CommandError::Failed(format!("settings: {e}")))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }

    /// Replace invalid values with defaults (used for hand-edited files).
    pub fn sanitize(&mut self) {
        let d = Settings::default();
        if self.validate().is_ok() {
            return;
        }
        if one_of("", &self.general.after_capture, &["editor", "silent"]).is_err() {
            self.general.after_capture = d.general.after_capture.clone();
        }
        if one_of("", &self.recording.codec, &["h264", "h265"]).is_err() {
            self.recording.codec = d.recording.codec.clone();
        }
        if one_of(
            "",
            &self.recording.output,
            &["native", "1080p", "1440p", "2160p"],
        )
        .is_err()
        {
            self.recording.output = d.recording.output.clone();
        }
        if one_of(
            "",
            &self.ocr.engine,
            &["windows", "llm", "windows_then_llm"],
        )
        .is_err()
        {
            self.ocr.engine = d.ocr.engine.clone();
        }
        if one_of("", &self.ocr.language, &["auto", "en", "ar"]).is_err() {
            self.ocr.language = d.ocr.language.clone();
        }
        if one_of("", &self.privacy.mode, &["pixelate", "blur"]).is_err() {
            self.privacy.mode = d.privacy.mode.clone();
        }
        if !(1..=120).contains(&self.recording.fps) {
            self.recording.fps = d.recording.fps;
        }
        if !(1..=600).contains(&self.ocr.llm_timeout_secs) {
            self.ocr.llm_timeout_secs = d.ocr.llm_timeout_secs;
        }
        if !(1..=1800).contains(&self.speech.timeout_secs) {
            self.speech.timeout_secs = d.speech.timeout_secs;
        }
        if self.updates.skipped_version.len() > 32 {
            self.updates.skipped_version.clear();
        }
    }

    pub fn validate(&self) -> Result<(), CommandError> {
        one_of(
            "general.after_capture",
            &self.general.after_capture,
            &["editor", "silent"],
        )?;
        one_of("recording.codec", &self.recording.codec, &["h264", "h265"])?;
        one_of(
            "recording.output",
            &self.recording.output,
            &["native", "1080p", "1440p", "2160p"],
        )?;
        one_of(
            "ocr.engine",
            &self.ocr.engine,
            &["windows", "llm", "windows_then_llm"],
        )?;
        one_of("ocr.language", &self.ocr.language, &["auto", "en", "ar"])?;
        one_of("privacy.mode", &self.privacy.mode, &["pixelate", "blur"])?;
        if !(1..=120).contains(&self.recording.fps) {
            return Err(CommandError::Params("recording.fps must be 1..=120".into()));
        }
        if !(1..=600).contains(&self.ocr.llm_timeout_secs) {
            return Err(CommandError::Params(
                "ocr.llm_timeout_secs must be 1..=600".into(),
            ));
        }
        if !(1..=1800).contains(&self.speech.timeout_secs) {
            return Err(CommandError::Params(
                "speech.timeout_secs must be 1..=1800".into(),
            ));
        }
        if self.updates.skipped_version.len() > 32 {
            return Err(CommandError::Params(
                "updates.skipped_version is too long".into(),
            ));
        }
        if self.general.language.len() > 16
            || !self
                .general
                .language
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(CommandError::Params(
                "general.language is not a locale code".into(),
            ));
        }
        Ok(())
    }

    pub fn is_silent(&self) -> bool {
        self.general.after_capture == "silent"
    }

    /// Should a finished capture open the editor? A preset that opts out always wins; the
    /// global `silent` mode turns the editor off for everything else.
    pub fn opens_editor(&self, preset_open_in_editor: Option<bool>) -> bool {
        !self.is_silent() && preset_open_in_editor.unwrap_or(true)
    }
}

impl Session {
    /// Push settings into the live preferences.
    pub fn apply_settings(&mut self) {
        let g = self.settings.general.clone();
        self.prefs.copy_to_clipboard = g.copy_to_clipboard;
        self.prefs.include_cursor = g.include_cursor;
        self.prefs.auto_save_dir = if g.save_dir.trim().is_empty() {
            self.default_save_dir.clone()
        } else {
            Some(PathBuf::from(g.save_dir.trim()))
        };
    }

    /// Persist when a settings file is configured. Failures are reported to the caller.
    pub fn persist_settings(&mut self) -> Result<(), CommandError> {
        self.settings.presets = Some(self.presets.clone());
        match self.settings_path.clone() {
            Some(p) => self.settings.save(&p),
            None => Ok(()),
        }
    }

    /// Load settings from `path` and remember it for later saves.
    pub fn use_settings_file(&mut self, path: PathBuf) {
        self.settings = Settings::load(&path);
        if let Some(p) = self.settings.presets.clone().filter(|p| !p.is_empty()) {
            self.presets = p;
        }
        self.settings_path = Some(path);
        self.apply_settings();
    }

    /// Swap in validated settings, run the start-with-Windows hook when it changed, and save.
    fn install_settings(&mut self, next: Settings) -> Result<(), CommandError> {
        let old = (
            self.settings.general.start_with_windows,
            self.settings.general.start_minimized,
        );
        let new = (
            next.general.start_with_windows,
            next.general.start_minimized,
        );
        self.settings = next;
        self.apply_settings();
        self.persist_settings()?;
        if old != new
            && let Some(hook) = self.on_autostart.clone()
            && !hook(new.0, new.1)
        {
            return Err(CommandError::Failed(
                "could not update the Windows startup entry".into(),
            ));
        }
        Ok(())
    }

    /// Replace all settings at once (the Settings form); invalid input is rejected.
    pub fn replace_settings(&mut self, next: Settings) -> Result<(), CommandError> {
        next.validate()?;
        self.install_settings(next)
    }

    /// Set one value by dotted path, e.g. `general.after_capture`.
    pub fn set_setting(&mut self, path: &str, value: Value) -> Result<(), CommandError> {
        let mut root = serde_json::to_value(&self.settings)
            .map_err(|e| CommandError::Failed(e.to_string()))?;
        let mut cur = &mut root;
        let parts: Vec<&str> = path.split('.').collect();
        for (i, part) in parts.iter().enumerate() {
            let obj = cur
                .as_object_mut()
                .ok_or_else(|| CommandError::Params(format!("unknown setting `{path}`")))?;
            if i + 1 == parts.len() {
                if !obj.contains_key(*part) || *part == "version" || *part == "presets" {
                    return Err(CommandError::Params(format!("unknown setting `{path}`")));
                }
                obj.insert((*part).to_string(), value);
                break;
            }
            cur = obj
                .get_mut(*part)
                .ok_or_else(|| CommandError::Params(format!("unknown setting `{path}`")))?;
        }
        let next: Settings = serde_json::from_value(root)
            .map_err(|e| CommandError::Params(format!("bad value for `{path}`: {e}")))?;
        next.validate()?;
        self.install_settings(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("scit-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("settings.json")
    }

    #[test]
    fn speech_settings_default_off_and_sanitize() {
        let mut s = Settings::default();
        assert!(s.speech.stt_url.is_empty() && s.speech.stt_config().url.is_empty());
        s.speech.stt_url = "  http://x/v1 ".into();
        assert_eq!(s.speech.stt_config().url, "http://x/v1");
        s.speech.timeout_secs = 0;
        assert!(s.validate().is_err());
        s.sanitize();
        assert_eq!(s.speech.timeout_secs, 300);
        // an old file without the section still loads
        let old: Settings = serde_json::from_str(r#"{"version":1}"#).unwrap();
        assert_eq!(old.speech, SpeechSettings::default());
    }

    #[test]
    fn round_trip() {
        let p = tmp("rt");
        let mut s = Settings::default();
        s.general.after_capture = "silent".into();
        s.ocr.language = "ar".into();
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
    }

    #[test]
    fn corrupt_file_yields_defaults() {
        let p = tmp("corrupt");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{ not json").unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
    }

    #[test]
    fn unknown_keys_ignored_and_invalid_values_repaired() {
        let p = tmp("unk");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            r#"{"future":1,"general":{"after_capture":"bogus","language":"de"},"recording":{"fps":9999}}"#,
        )
        .unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.general.after_capture, "editor");
        assert_eq!(s.general.language, "de");
        assert_eq!(s.recording.fps, 30);
    }

    #[test]
    fn set_validates_and_persists() {
        let p = tmp("set");
        let mut s = Session::new(crate::Services::default());
        s.use_settings_file(p.clone());
        s.set_setting("general.after_capture", json!("silent"))
            .unwrap();
        assert!(s.settings.is_silent());
        assert_eq!(Settings::load(&p).general.after_capture, "silent");
        assert!(
            s.set_setting("general.after_capture", json!("nope"))
                .is_err()
        );
        assert!(s.set_setting("general.after_capture", json!(3)).is_err());
        assert!(s.set_setting("general.nothing", json!(1)).is_err());
        assert!(s.set_setting("version", json!(9)).is_err());
        assert!(s.set_setting("recording.fps", json!(0)).is_err());
        assert!(s.settings.is_silent(), "failed sets leave settings intact");
    }

    #[test]
    fn after_capture_mode_and_preset_override() {
        let mut s = Settings::default();
        assert!(s.opens_editor(None));
        assert!(s.opens_editor(Some(true)));
        assert!(!s.opens_editor(Some(false)));
        s.general.after_capture = "silent".into();
        assert!(!s.opens_editor(None));
        assert!(!s.opens_editor(Some(true)));
    }

    #[test]
    fn autostart_hook_runs_only_on_change() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = std::sync::Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let mut s = Session::new(crate::Services::default());
        s.on_autostart = Some(std::sync::Arc::new(move |_, _| {
            c.fetch_add(1, Ordering::SeqCst);
            true
        }));
        s.set_setting("general.copy_to_clipboard", json!(true))
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        s.set_setting("general.start_with_windows", json!(true))
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        s.on_autostart = Some(std::sync::Arc::new(|_, _| false));
        assert!(
            s.set_setting("general.start_minimized", json!(false))
                .is_err()
        );
    }

    #[test]
    fn settings_drive_prefs() {
        let mut s = Session::new(crate::Services::default());
        s.set_setting("general.copy_to_clipboard", json!(true))
            .unwrap();
        assert!(s.prefs.copy_to_clipboard);
        s.set_setting("general.save_dir", json!("D:/caps")).unwrap();
        assert_eq!(s.prefs.auto_save_dir, Some(PathBuf::from("D:/caps")));
    }
}
