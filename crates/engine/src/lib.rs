//! Engine: `Session` owns the document, history, library and injected services; every user-visible
//! action is a command in the registry and is invoked through `Session::dispatch`.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod advanced_cmds;
pub use advanced_cmds::{OcrJob, OcrOutput};
mod args;
mod capture_cmds;
mod commands;
pub mod control;
mod doc_cmds;
mod editor_cmds;
mod fx_cmds;
mod image_cmds;
mod library_cmds;
pub mod mcp;
mod ml_cmds;
mod record_cmds;
mod secrets;
mod speech_cmds;
pub use secrets::{Found, ScanResult, SecretScan};
mod polish_cmds;
mod polish_render;
pub mod settings;
mod system_cmds;
mod text_cmds;
mod tutor_cmds;
mod tutor_render;
pub use tutor_cmds::TutorShared;
mod update_cmds;
mod video_cmds;

pub use args::Args;
pub use commands::all_commands;

use screenit_capture::{Preset, ScreenSource, Selection};
use screenit_doc::CaptureDoc;
use screenit_geom::Rect;
use screenit_library::Library;
use screenit_ops::History;
use screenit_raster::Raster;
use screenit_record::{
    RecordConfig, Recorder, RecordingResult, Session as RecSession, VideoReader,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("unknown command `{0}`")]
    Unknown(String),
    #[error("{0}")]
    Params(String),
    #[error("command unavailable: {0}")]
    Disabled(String),
    #[error("{0}")]
    Failed(String),
    #[error("internal error (recovered): {0}")]
    Panic(String),
}

impl CommandError {
    pub fn failed(e: impl std::fmt::Display) -> Self {
        CommandError::Failed(e.to_string())
    }
}

pub type CmdResult = Result<Value, CommandError>;

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
    fn unix_secs(&self) -> i64;
    fn sleep_ms(&self, ms: u32);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
    fn unix_secs(&self) -> i64 {
        (self.now_ms() / 1000) as i64
    }
    fn sleep_ms(&self, ms: u32) {
        std::thread::sleep(std::time::Duration::from_millis(u64::from(ms.min(60_000))));
    }
}

pub trait Clipboard: Send + Sync {
    fn set_image(&self, img: &Raster) -> Result<(), String>;
    fn get_image(&self) -> Result<Raster, String>;
    fn set_text(&self, text: &str) -> Result<(), String>;
    fn get_text(&self) -> Result<String, String>;
}

pub type RecorderFactory = Box<dyn Fn() -> Box<dyn Recorder> + Send>;

/// Platform services injected by the app (real Win32 in `screenit-platform`, fakes in tests).
pub struct Services {
    pub screen: Option<Arc<dyn ScreenSource>>,
    pub clipboard: Option<Arc<dyn Clipboard>>,
    pub clock: Arc<dyn Clock>,
    pub recorder: Option<RecorderFactory>,
    pub video: Option<Arc<dyn VideoReader>>,
    pub video_writer: Option<Arc<dyn screenit_record::VideoWriter>>,
    /// Plays a recording's audio track in step with the in-app video transport.
    pub player: Option<Arc<dyn screenit_record::TrackPlayer>>,
    pub ocr: Option<Arc<dyn screenit_capture::Ocr>>,
    pub llm: Option<Arc<dyn screenit_capture::LlmOcr>>,
    pub camera: Option<Arc<dyn screenit_record::CameraSource>>,
    pub devices: Option<Arc<dyn screenit_record::AvDevices>>,
    /// Looks for, downloads and installs newer releases from GitHub.
    pub updater: Option<Arc<screenit_update::Updater>>,
    /// UI Automation, synthetic input and app launching for AI tutorials.
    pub ui: Option<Arc<dyn screenit_tutor::UiTree>>,
    pub input: Option<Arc<dyn screenit_tutor::InputDriver>>,
    pub launcher: Option<Arc<dyn screenit_tutor::AppLauncher>>,
    /// Chat model that plans and repairs tutorial scripts.
    pub chat: Option<Arc<dyn screenit_tutor::LlmChat>>,
    /// Speech to text and text to speech (local server, or the OS voice for narration).
    pub stt: Option<Arc<dyn screenit_record::speech::SpeechToText>>,
    pub tts: Option<Arc<dyn screenit_record::speech::TextToSpeech>>,
    /// The OS voice used when no text-to-speech server is configured.
    pub tts_fallback: Option<Arc<dyn screenit_record::speech::TextToSpeech>>,
    /// Clock and sleeper for tutorial runs (virtual time in tests).
    pub pace: Arc<dyn screenit_tutor::Pace>,
}

impl Default for Services {
    fn default() -> Self {
        Self {
            screen: None,
            clipboard: None,
            clock: Arc::new(SystemClock),
            recorder: None,
            video: None,
            video_writer: None,
            player: None,
            ocr: None,
            llm: None,
            camera: None,
            devices: None,
            updater: None,
            ui: None,
            input: None,
            launcher: None,
            chat: None,
            stt: None,
            tts: None,
            tts_fallback: None,
            pace: Arc::new(screenit_tutor::RealPace),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Prefs {
    pub auto_save_dir: Option<PathBuf>,
    pub copy_to_clipboard: bool,
    pub include_cursor: bool,
    pub default_step_radius: f32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            auto_save_dir: None,
            copy_to_clipboard: false,
            include_cursor: false,
            default_step_radius: 14.0,
        }
    }
}

pub struct ActiveRecording {
    pub recorder: Box<dyn Recorder>,
    pub session: RecSession,
    pub config: RecordConfig,
    pub pip: Option<screenit_record::PipLayout>,
}

pub struct Session {
    pub services: Services,
    pub doc: Option<CaptureDoc>,
    pub history: History,
    pub library: Library,
    pub presets: Vec<Preset>,
    pub prefs: Prefs,
    pub settings: settings::Settings,
    pub settings_path: Option<PathBuf>,
    /// Applies start-with-Windows `(enabled, minimized)`; returns false on failure.
    pub on_autostart: Option<Arc<dyn Fn(bool, bool) -> bool + Send + Sync>>,
    /// Where captures go when the `save_dir` setting is empty.
    pub default_save_dir: Option<PathBuf>,
    pub last_region: Option<Rect>,
    pub scroll_frames: Option<(Rect, Vec<Raster>)>,
    pub recording: Option<ActiveRecording>,
    pub last_recording: Option<RecordingResult>,
    pub capture_counter: u32,
    pub tutor: Arc<TutorShared>,
    registry: Vec<CommandSpec>,
}

pub type EnabledFn = fn(&Session) -> Option<&'static str>;
pub type RunFn = fn(&mut Session, &Value) -> CmdResult;

#[derive(Clone)]
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: &'static str,
    pub shortcut: &'static str,
    /// Human/agent readable parameter description, e.g. `{"x":int,"y":int}`.
    pub params: &'static str,
    /// `None` when enabled, otherwise the reason it is disabled.
    pub enabled: EnabledFn,
    pub run: RunFn,
}

pub fn always(_: &Session) -> Option<&'static str> {
    None
}
pub fn need_doc(s: &Session) -> Option<&'static str> {
    if s.doc.is_none() {
        Some("no document open")
    } else {
        None
    }
}
pub fn need_screen(s: &Session) -> Option<&'static str> {
    if s.services.screen.is_none() {
        Some("no screen source available")
    } else {
        None
    }
}

impl Session {
    pub fn new(services: Services) -> Self {
        Self {
            services,
            doc: None,
            history: History::default(),
            library: Library::in_memory(),
            presets: screenit_capture::default_presets(),
            prefs: Prefs::default(),
            settings: settings::Settings::default(),
            settings_path: None,
            on_autostart: None,
            default_save_dir: None,
            last_region: None,
            scroll_frames: None,
            recording: None,
            last_recording: None,
            capture_counter: 0,
            tutor: Arc::new(TutorShared::default()),
            registry: all_commands(),
        }
    }

    pub fn registry(&self) -> &[CommandSpec] {
        &self.registry
    }

    /// Run a command by id. Never panics: a panic inside a command is caught, reported as an
    /// error, and the document is left as it was before the command (history is atomic per step).
    pub fn dispatch(&mut self, id: &str, params: &Value) -> CmdResult {
        let spec = self
            .registry
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| CommandError::Unknown(suggest(id, &self.registry)))?;
        if let Some(why) = (spec.enabled)(self) {
            return Err(CommandError::Disabled(why.to_string()));
        }
        let run = spec.run;
        let backup = self.doc.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(self, params)));
        match result {
            Ok(r) => r,
            Err(p) => {
                self.doc = backup;
                let msg = p
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "panic".into());
                Err(CommandError::Panic(msg))
            }
        }
    }

    pub fn command_list(&self) -> Value {
        Value::Array(
            self.registry
                .iter()
                .map(|c| {
                    let why = (c.enabled)(self);
                    json!({
                        "id": c.id, "label": c.label, "menu": c.menu, "shortcut": c.shortcut,
                        "params": c.params, "enabled": why.is_none(), "disabledReason": why,
                        "needsDocument": why == Some("no document open"),
                    })
                })
                .collect(),
        )
    }

    pub fn doc_ref(&self) -> Result<&CaptureDoc, CommandError> {
        self.doc
            .as_ref()
            .ok_or_else(|| CommandError::Disabled("no document open".into()))
    }

    /// Run `f` on the document as exactly one history step (rolled back on error).
    pub fn edit<T>(
        &mut self,
        label: &str,
        coalesce: Option<&str>,
        f: impl FnOnce(&mut CaptureDoc) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let doc = self
            .doc
            .as_mut()
            .ok_or_else(|| CommandError::Disabled("no document open".into()))?;
        self.history.apply(doc, label, coalesce, f)
    }

    pub fn screen(&self) -> Result<Arc<dyn ScreenSource>, CommandError> {
        self.services
            .screen
            .clone()
            .ok_or_else(|| CommandError::Disabled("no screen source available".into()))
    }

    pub fn new_document(&mut self, doc: CaptureDoc) {
        self.doc = Some(doc);
        self.history = History::default();
        self.scroll_frames = None;
    }

    /// Run a preset's selection (used by hotkeys and `capture.preset.run`).
    pub fn selection_of(&self, preset: &Preset) -> Selection {
        preset.selection.clone()
    }
}

fn suggest(id: &str, reg: &[CommandSpec]) -> String {
    let near: Vec<&str> = reg
        .iter()
        .map(|c| c.id)
        .filter(|c| c.contains(id) || id.contains(c) || c.split('.').next() == id.split('.').next())
        .take(3)
        .collect();
    if near.is_empty() {
        id.to_string()
    } else {
        format!("{id}`; did you mean `{}", near.join("`, `"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_command_suggests_neighbours() {
        let mut s = Session::new(Services::default());
        let e = s
            .dispatch("capture.regio", &json!({}))
            .unwrap_err()
            .to_string();
        assert!(e.contains("capture."), "{e}");
    }

    #[test]
    fn disabled_without_document() {
        let mut s = Session::new(Services::default());
        assert!(matches!(
            s.dispatch("editor.arrow.add", &json!({})),
            Err(CommandError::Disabled(_))
        ));
    }

    #[test]
    fn registry_ids_are_unique_and_documented() {
        let s = Session::new(Services::default());
        let mut seen = std::collections::HashSet::new();
        for c in s.registry() {
            assert!(seen.insert(c.id), "duplicate id {}", c.id);
            assert!(
                !c.label.is_empty() && !c.params.is_empty(),
                "{} lacks label/params doc",
                c.id
            );
        }
        assert!(s.registry().len() > 40);
    }
}
