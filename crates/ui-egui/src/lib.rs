//! egui front end. A thin shell: every action goes through `Session::dispatch`, the UI only
//! decides which command to call and renders the document the engine owns.

mod canvas;
mod chrome;
mod fonts;
mod icons;
mod library_panel;
mod overlay;
mod recording;
mod scrolling;
mod selection;
mod settings_panel;
mod theme;
mod toast;

use icons::Icon;

use eframe::egui;
use screenit_capture::{FrozenScreen, Selection};
use screenit_engine::{Session, control};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

/// Things that happen outside the UI thread.
pub enum UiEvent {
    /// Run a capture preset by name (global hotkey / tray menu).
    RunPreset(String),
    /// Bring the editor window to the front.
    Show,
    /// One control-protocol line and where to send the answer.
    Control(String, Sender<String>),
    /// A message for the status line (e.g. a hotkey could not be registered).
    Notice(String),
    Quit,
}

/// Lets background threads wake the UI loop.
#[derive(Clone)]
pub struct Waker(egui::Context);

impl Waker {
    pub fn wake(&self) {
        self.0.request_repaint();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Text,
    Step,
    Blur,
    Highlight,
    Pen,
    Crop,
}

impl Tool {
    /// (tool, label, icon, single-key shortcut)
    pub const ALL: [(Tool, &'static str, Icon, &'static str); 11] = [
        (Tool::Select, "Select", Icon::Select, "V"),
        (Tool::Arrow, "Arrow", Icon::Arrow, "A"),
        (Tool::Line, "Line", Icon::Line, "L"),
        (Tool::Rect, "Rectangle", Icon::Rect, "R"),
        (Tool::Ellipse, "Ellipse", Icon::Ellipse, "E"),
        (Tool::Text, "Text", Icon::Text, "T"),
        (Tool::Step, "Step", Icon::Step, "S"),
        (Tool::Highlight, "Highlight", Icon::Highlight, "H"),
        (Tool::Blur, "Blur", Icon::Blur, "B"),
        (Tool::Pen, "Pen", Icon::Pen, "P"),
        (Tool::Crop, "Crop", Icon::Crop, "C"),
    ];

    pub(crate) fn uses_color(self) -> bool {
        !matches!(
            self,
            Tool::Select | Tool::Blur | Tool::Highlight | Tool::Crop
        )
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            Tool::Select => "Click an object to select it. Drag to move, resize or rotate it.",
            Tool::Arrow => "Drag from the tail to the tip.",
            Tool::Line => "Drag to draw a straight line.",
            Tool::Rect => "Drag to draw a rectangle.",
            Tool::Ellipse => "Drag to draw an ellipse.",
            Tool::Text => "Drag to draw a text box, then type in it.",
            Tool::Step => "Click to drop the next numbered step.",
            Tool::Highlight => "Drag over the area to highlight.",
            Tool::Blur => "Drag over sensitive content to blur it.",
            Tool::Pen => "Draw freehand.",
            Tool::Crop => "Drag the area to keep.",
        }
    }
}
/// Which screen the single window is showing: Snagit-style small capture bar, the editor, or the library.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum View {
    Compact,
    Editor,
    Library,
    Settings,
    About,
}

const COMPACT_SIZE: [f32; 2] = [1000.0, 252.0];
const EDITOR_SIZE: [f32; 2] = [1280.0, 820.0];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pending {
    Overlay,
    /// The overlay, picking the area to record.
    RecordOverlay,
    /// The overlay, picking the area to read text from.
    TextOverlay,
    /// The overlay, picking the area to capture by scrolling.
    ScrollOverlay,
    Fullscreen,
    Preset,
}

/// What the capture overlay's pick is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum OverlayMode {
    Capture,
    Record,
    Text,
    Scroll,
}

impl Pending {
    fn is_overlay(self) -> bool {
        matches!(
            self,
            Pending::Overlay
                | Pending::RecordOverlay
                | Pending::TextOverlay
                | Pending::ScrollOverlay
        )
    }
}

pub struct UiApp {
    pub(crate) session: Session,
    events: Receiver<UiEvent>,
    pub(crate) tex: Option<DocTex>,
    doc_rev: u64,
    tex_rev: u64,
    pub(crate) tool: Tool,
    pub(crate) stroke: egui::Color32,
    pub(crate) width: f32,
    pub(crate) zoom: f32,
    pub(crate) status: String,
    pub(crate) drag: Option<egui::Pos2>,
    pub(crate) pen: Vec<[f32; 2]>,
    pub(crate) selected: Option<u64>,
    /// Text box being typed into.
    pub(crate) editing: Option<u64>,
    /// What the current drag does to the selected object.
    pub(crate) gesture: Option<selection::Gesture>,
    /// Counts gestures and edits so each one is a single undo step.
    pub(crate) gesture_seq: u32,
    /// The selection whose style the property panel last loaded.
    sel_synced: Option<u64>,
    pub(crate) overlay: Option<overlay::Overlay>,
    pending: Option<(Instant, Pending, Option<String>)>,
    pub(crate) view: View,
    tray: bool,
    lib_query: String,
    pub(crate) thumbs: HashMap<u64, Option<egui::TextureHandle>>,
    video: Option<library_panel::VideoPlayer>,
    /// Physical window position to return to after a capture / when leaving the tray.
    saved_pos: Option<(i32, i32)>,
    home: Option<(i32, i32)>,
    /// Retries for putting the exe icon on the taskbar button once the window exists.
    icon_tries: u8,
    /// Fit the document to the canvas on the next frame (new capture, Ctrl+0).
    pub(crate) fit: bool,
    pub(crate) rec_opts: recording::RecOptions,
    recording: Option<recording::RecUi>,
    /// A scrolling capture being taken, one step per frame.
    scroll: Option<scrolling::ScrollRun>,
    /// Last file written (capture, export, recording): drives "Show in folder".
    pub(crate) last_saved: Option<std::path::PathBuf>,
    /// The dark title bar has been applied to the native window.
    framed: bool,
    quit: bool,
    /// Park in the tray on the first frame.
    start_hidden: bool,
    pub(crate) win_langs: Option<Vec<String>>,
    pub(crate) llm_models: Vec<String>,
    pub(crate) llm_msg: String,
    pub(crate) llm_rx: Option<Receiver<Result<Vec<String>, String>>>,
    pub(crate) ocr_rx: Option<Receiver<Result<screenit_engine::OcrOutput, String>>>,
    /// A Hide secrets scan running on a worker thread.
    pub(crate) secret_rx: Option<Receiver<Result<screenit_engine::ScanResult, String>>>,
    pub(crate) settings_tab: settings_panel::Tab,
    /// Grabbed text opened from the library, shown instead of the canvas.
    pub(crate) reading: Option<String>,
    pub(crate) toast: Option<toast::Notice>,
    /// Language whose fonts are installed (empty until the first frame).
    applied_lang: String,
    /// View to return to from About.
    about_from: View,
    /// The previous frame could be maximized, so leaving it restores a normal window.
    editor_chrome: bool,
}

impl UiApp {
    pub fn new(session: Session, events: Receiver<UiEvent>) -> Self {
        let rec_opts = recording::RecOptions {
            mic: session.settings.recording.microphone,
            system: session.settings.recording.system_audio,
            ..recording::RecOptions::default()
        };
        Self {
            session,
            events,
            tex: None,
            doc_rev: 1,
            tex_rev: 0,
            tool: Tool::Arrow,
            stroke: egui::Color32::from_rgb(230, 30, 30),
            width: 3.0,
            zoom: 1.0,
            status: screenit_i18n::t("status.ready"),
            drag: None,
            pen: Vec::new(),
            selected: None,
            editing: None,
            gesture: None,
            gesture_seq: 0,
            sel_synced: None,
            overlay: None,
            pending: None,
            view: View::Compact,
            tray: false,
            lib_query: String::new(),
            thumbs: HashMap::new(),
            video: None,
            saved_pos: None,
            home: None,
            icon_tries: 0,
            fit: true,
            rec_opts,
            recording: None,
            scroll: None,
            last_saved: None,
            framed: false,
            quit: false,
            start_hidden: false,
            win_langs: None,
            llm_models: Vec::new(),
            llm_msg: String::new(),
            llm_rx: None,
            ocr_rx: None,
            secret_rx: None,
            settings_tab: settings_panel::Tab::General,
            reading: None,
            toast: None,
            applied_lang: String::new(),
            about_from: View::Compact,
            editor_chrome: false,
        }
    }

    /// Run a command; the document revision advances so the canvas texture refreshes.
    pub(crate) fn run(&mut self, id: &str, params: Value) -> Option<Value> {
        match self.session.dispatch(id, &params) {
            Ok(v) => {
                self.doc_rev += 1;
                Some(v)
            }
            Err(e) => {
                self.status = format!("{id}: {e}");
                None
            }
        }
    }

    pub(crate) fn stroke_hex(&self) -> String {
        let c = self.stroke;
        format!("#{:02x}{:02x}{:02x}{:02x}", c.r(), c.g(), c.b(), c.a())
    }

    pub(crate) fn start_capture(
        &mut self,
        ctx: &egui::Context,
        what: Pending,
        preset: Option<String>,
    ) {
        // Move the editor off-screen (minimised windows stop receiving frames) so it is not in the shot.
        if self.saved_pos.is_none() && !self.tray {
            self.saved_pos = screenit_platform::window::main_position();
            screenit_platform::window::move_main(-32000, -32000);
        }
        self.pending = Some((Instant::now() + Duration::from_millis(350), what, preset));
        ctx.request_repaint_after(Duration::from_millis(400));
    }

    /// Resize the single window for a view and switch to it.
    pub(crate) fn enter_view(&mut self, ctx: &egui::Context, view: View) {
        self.view = view;
        let [w, h] = match view {
            View::Compact => COMPACT_SIZE,
            View::Settings => [960.0, 660.0],
            View::About => [720.0, 560.0],
            View::Editor | View::Library => EDITOR_SIZE,
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
    }

    /// Open About, remembering the view the Back button returns to.
    pub(crate) fn open_about(&mut self, ctx: &egui::Context) {
        if self.view != View::About {
            self.about_from = self.view;
        }
        self.enter_view(ctx, View::About);
    }

    /// Maximize is live in the editor and the library. Windows 11 draws a dimmed square on a
    /// fixed window, so those two views are resizable and the capture bar stays fixed.
    fn sync_maximize_button(&mut self, ctx: &egui::Context) {
        let grow = matches!(self.view, View::Editor | View::Library);
        if self.editor_chrome && !grow {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
        }
        self.editor_chrome = grow;
        ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(grow));
        ctx.send_viewport_cmd(egui::ViewportCommand::EnableButtons {
            close: true,
            minimized: true,
            maximize: grow,
        });
    }

    /// Park the window in the tray: no taskbar button, off-screen, still processing events.
    fn send_to_tray(&mut self) {
        if let Some(p) = screenit_platform::window::main_position().filter(|p| p.0 > -30000) {
            self.home = Some(p);
        }
        screenit_platform::window::enter_tray();
        self.tray = true;
        self.saved_pos = None;
    }

    /// Bring the window back (from the tray or from the off-screen capture position).
    fn restore_window(&mut self, ctx: &egui::Context) {
        if self.tray {
            let (x, y) = self.home.unwrap_or((120, 80));
            screenit_platform::window::leave_tray(x, y);
            self.tray = false;
            self.saved_pos = None;
        } else if let Some((x, y)) = self.saved_pos.take() {
            screenit_platform::window::move_main(x, y);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        screenit_platform::window::activate_main();
    }

    /// After a capture ends without opening the editor (or is cancelled): stay out of the way.
    fn settle_after_capture(&mut self) {
        if self.tray {
            // Already parked off-screen; nothing to restore.
            self.saved_pos = None;
        } else if let Some((x, y)) = self.saved_pos.take() {
            screenit_platform::window::move_main(x, y);
        }
    }
    fn run_pending(&mut self, ctx: &egui::Context) {
        let Some((at, what, preset)) = self.pending.clone() else {
            return;
        };
        if Instant::now() < at {
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        }
        self.pending = None;
        let mut show = true;
        match what {
            Pending::Fullscreen => {
                if let Some(v) = self.run("capture.fullscreen", json!({})) {
                    self.show_capture(&v);
                }
                show = self.session.settings.opens_editor(None);
            }
            Pending::Preset => {
                let name = preset.unwrap_or_default();
                let preset_open = self
                    .session
                    .presets
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| p.open_in_editor);
                show = self.session.settings.opens_editor(preset_open);
                if let Some(v) = self.run("capture.preset.run", json!({"name": name})) {
                    self.show_capture(&v);
                }
            }
            Pending::Overlay
            | Pending::RecordOverlay
            | Pending::TextOverlay
            | Pending::ScrollOverlay => match self.session.screen().map(|s| {
                FrozenScreen::snapshot_with(
                    s.as_ref(),
                    self.session.settings.privacy.auto_hide_secrets,
                )
            }) {
                Ok(Ok(frozen)) => {
                    let mut ov = overlay::Overlay::new(ctx, Arc::new(frozen));
                    ov.record = what == Pending::RecordOverlay;
                    ov.text = what == Pending::TextOverlay;
                    ov.scroll = what == Pending::ScrollOverlay;
                    self.overlay = Some(ov);
                    show = false;
                }
                Ok(Err(e)) => self.status = format!("capture failed: {e}"),
                Err(e) => self.status = e.to_string(),
            },
        }
        if show {
            if self.session.doc.is_some() && !what.is_overlay() {
                self.enter_view(ctx, View::Editor);
            }
            self.restore_window(ctx);
        } else if !what.is_overlay() {
            self.settle_after_capture();
        }
    }

    /// Capture `selection` from the frozen overlay image, then restore the editor.
    pub(crate) fn finish_overlay(
        &mut self,
        ctx: &egui::Context,
        frozen: Arc<FrozenScreen>,
        sel: Option<Selection>,
        mode: OverlayMode,
    ) {
        self.overlay = None;
        if mode == OverlayMode::Text {
            self.finish_text_pick(ctx, &frozen, sel);
            return;
        }
        if mode == OverlayMode::Scroll {
            self.begin_scroll(&frozen, sel);
            return;
        }
        if mode == OverlayMode::Record {
            let rect = match sel {
                Some(Selection::Region { rect }) => Some(rect),
                Some(Selection::Window { id }) => frozen
                    .windows_snapshot()
                    .iter()
                    .find(|w| w.id == id)
                    .map(|w| w.rect),
                _ => None,
            };
            match rect {
                Some(r) => self.begin_record(ctx, r, frozen.bounds()),
                None => self.settle_after_capture(),
            }
            return;
        }
        if let Some(sel) = sel {
            let live = self.session.services.screen.replace(frozen);
            let r = match sel {
                Selection::Region { rect } => self.run(
                    "capture.region",
                    json!({"x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h}),
                ),
                Selection::Window { id } => self.run("capture.window", json!({"id": id})),
                _ => None,
            };
            self.session.services.screen = live;
            if let Some(v) = r {
                self.show_capture(&v);
                if self.session.settings.opens_editor(None) {
                    self.enter_view(ctx, View::Editor);
                    self.restore_window(ctx);
                } else {
                    self.settle_after_capture();
                }
                return;
            }
        }
        self.settle_after_capture();
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                UiEvent::RunPreset(_) if self.is_recording() => {
                    self.status = screenit_i18n::t("status.recording_busy");
                }
                UiEvent::RunPreset(name) => {
                    let all_in_one = self
                        .session
                        .presets
                        .iter()
                        .find(|p| p.name == name)
                        .is_some_and(|p| matches!(&p.selection, Selection::Region { rect } if rect.is_empty()));
                    if all_in_one {
                        self.start_capture(ctx, Pending::Overlay, None);
                    } else {
                        self.start_capture(ctx, Pending::Preset, Some(name));
                    }
                }
                UiEvent::Show => {
                    if self.view == View::Compact && self.session.doc.is_some() {
                        self.enter_view(ctx, View::Editor);
                    }
                    self.restore_window(ctx);
                }
                UiEvent::Control(line, reply) => {
                    let out = control::handle_line(&mut self.session, &line);
                    self.doc_rev += 1;
                    let _ = reply.send(out);
                }
                UiEvent::Notice(msg) => self.status = msg,
                UiEvent::Quit => self.quit = true,
            }
        }
    }

    fn refresh_texture(&mut self, ctx: &egui::Context) {
        if self.tex_rev == self.doc_rev {
            return;
        }
        self.tex_rev = self.doc_rev;
        let old_size = self.tex.as_ref().map(|t| t.size());
        self.tex = self
            .session
            .doc
            .as_ref()
            .map(|d| DocTex::upload(ctx, &screenit_compose::flatten(d)));
        if self.tex.as_ref().map(|t| t.size()) != old_size {
            self.fit = true;
        }
    }
}

/// The flattened document on the GPU, cut into tiles so a very tall or wide image (a long
/// scrolling capture) never exceeds the graphics card's texture size limit.
#[derive(Clone)]
pub(crate) struct DocTex {
    size: [usize; 2],
    /// Pixel origin of each tile plus its texture.
    tiles: Vec<([usize; 2], egui::TextureHandle)>,
}

/// Longest side of one tile; every GL driver in use handles at least this.
const TILE: usize = 4096;

impl DocTex {
    fn upload(ctx: &egui::Context, img: &screenit_raster::Raster) -> Self {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let data = img.data();
        let opts = egui::TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Linear,
            ..Default::default()
        };
        let tile = TILE.min(ctx.input(|i| i.max_texture_side)).max(64);
        let mut tiles = Vec::new();
        for y in (0..h).step_by(tile) {
            for x in (0..w).step_by(tile) {
                let (tw, th) = (tile.min(w - x), tile.min(h - y));
                let mut px = Vec::with_capacity(tw * th * 4);
                for row in y..y + th {
                    let from = (row * w + x) * 4;
                    if let Some(s) = data.get(from..from + tw * 4) {
                        px.extend_from_slice(s);
                    }
                }
                if px.len() != tw * th * 4 {
                    continue;
                }
                let ci = egui::ColorImage::from_rgba_unmultiplied([tw, th], &px);
                tiles.push((
                    [x, y],
                    ctx.load_texture(format!("document-{x}-{y}"), ci, opts),
                ));
            }
        }
        Self {
            size: [w, h],
            tiles,
        }
    }

    pub(crate) fn size(&self) -> [usize; 2] {
        self.size
    }

    pub(crate) fn size_vec2(&self) -> egui::Vec2 {
        egui::vec2(self.size[0] as f32, self.size[1] as f32)
    }

    /// Paint the whole image into `rect`.
    pub(crate) fn paint(&self, painter: &egui::Painter, rect: egui::Rect) {
        let [w, h] = self.size;
        if w == 0 || h == 0 {
            return;
        }
        let k = egui::vec2(rect.width() / w as f32, rect.height() / h as f32);
        for ([x, y], t) in &self.tiles {
            let [tw, th] = t.size();
            let min = rect.min + egui::vec2(*x as f32 * k.x, *y as f32 * k.y);
            let r = egui::Rect::from_min_size(min, egui::vec2(tw as f32 * k.x, th as f32 * k.y));
            painter.image(
                t.id(),
                r,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    }
}

impl UiApp {
    /// Follow the language setting: switch the catalog and install matching fonts.
    fn sync_language(&mut self, ctx: &egui::Context) {
        let want = self.session.settings.general.language.clone();
        if want == self.applied_lang {
            return;
        }
        if let Err(e) = screenit_i18n::set_locale(&want) {
            self.status = screenit_i18n::tf("settings.translation_error", &[("error", &e)]);
        }
        fonts::install(ctx, &want);
        // The old status line was shaped for the previous language's font; restart it.
        if !self.applied_lang.is_empty() {
            self.status = screenit_i18n::t("status.ready");
        }
        self.applied_lang = want;
        ctx.request_repaint();
    }

    /// Status line for a capture result, e.g. `Captured 2560 × 1600 · saved to C:\…\capture-1.scit`.
    fn show_capture(&mut self, v: &Value) {
        let (w, h) = (
            v["width"].as_u64().unwrap_or(0),
            v["height"].as_u64().unwrap_or(0),
        );
        match v["saved"].as_str() {
            Some(p) => {
                self.status = screenit_i18n::tf(
                    "status.captured_saved",
                    &[("w", &w.to_string()), ("h", &h.to_string()), ("path", p)],
                );
                self.last_saved = Some(p.into());
            }
            None => {
                self.status = screenit_i18n::tf(
                    "status.captured",
                    &[("w", &w.to_string()), ("h", &h.to_string())],
                )
            }
        }
    }

    /// Report a written file with its full path and remember it for "Show in folder".
    pub(crate) fn saved_to(&mut self, what: &str, path: &str) {
        self.status = format!("{what} saved to {path}");
        self.last_saved = Some(path.into());
    }
}

/// Native Save As dialog starting in the screenIt folder; `None` when cancelled.
pub(crate) fn ask_save_path(
    title: &str,
    default_name: &str,
    filters: &[(&str, &str)],
) -> Option<String> {
    let dir = screenit_platform::shell::output_dir();
    let _ = std::fs::create_dir_all(&dir);
    screenit_platform::shell::save_dialog(title, &dir, default_name, filters)
        .map(|p| p.to_string_lossy().into_owned())
}

pub(crate) fn default_output(ext: &str) -> String {
    let dir = screenit_platform::shell::output_dir();
    let _ = std::fs::create_dir_all(&dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    dir.join(format!("capture-{stamp}.{ext}"))
        .to_string_lossy()
        .into_owned()
}

impl eframe::App for UiApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.framed {
            screenit_platform::window::dark_title_bar();
            self.framed = true;
        }
        if self.icon_tries < 5 {
            self.icon_tries += 1;
            screenit_platform::window::apply_exe_icon();
        }
        if self.start_hidden {
            self.start_hidden = false;
            self.send_to_tray();
        }
        let (minimized, close) = ctx.input(|i| {
            let v = i.viewport();
            (v.minimized == Some(true), v.close_requested())
        });
        if close && !self.quit {
            // Closing the window keeps screenIt running in the tray (Quit is in the tray menu).
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.send_to_tray();
        } else if minimized
            && !self.tray
            && self.overlay.is_none()
            && self.pending.is_none()
            && self.recording.is_none()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            self.send_to_tray();
        }
        self.sync_language(ctx);
        self.sync_language(ctx);
        self.drain_events(ctx);
        self.poll_ocr_jobs();
        self.run_pending(ctx);
        self.drive_scroll(ctx);
        if self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.refresh_texture(ctx);
        self.sync_maximize_button(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        overlay::drive(self, &ctx);
        toast::drive(self, &ctx);
        self.drive_recording(&ctx);
        if self.view != View::Compact
            && ctx.input_mut(|i| {
                i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::B,
                )
            })
        {
            self.enter_view(&ctx, View::Compact);
        }
        let bar = |fill| theme::panel_frame(fill, 8);
        match self.view {
            View::Compact => {
                egui::CentralPanel::default()
                    .frame(theme::panel_frame(theme::BG, 14))
                    .show(ui, |ui| self.capture_bar(ui, &ctx));
            }
            View::Settings => {
                egui::Panel::top("topbar")
                    .frame(bar(theme::SURFACE))
                    .show(ui, |ui| self.top_bar(ui, &ctx));
                egui::Panel::bottom("status")
                    .frame(theme::panel_frame(theme::SURFACE, 4))
                    .show(ui, |ui| self.status_bar(ui));
                egui::CentralPanel::default()
                    .frame(theme::panel_frame(theme::BG, 0))
                    .show(ui, |ui| self.settings_view(ui));
            }
            View::Library => {
                egui::Panel::top("topbar")
                    .frame(bar(theme::SURFACE))
                    .show(ui, |ui| self.top_bar(ui, &ctx));
                egui::Panel::bottom("status")
                    .frame(theme::panel_frame(theme::SURFACE, 4))
                    .show(ui, |ui| self.status_bar(ui));
                egui::CentralPanel::default()
                    .frame(theme::panel_frame(theme::BG, 18))
                    .show(ui, |ui| self.library_view(ui));
            }
            View::About => {
                egui::CentralPanel::default()
                    .frame(theme::panel_frame(theme::BG, 18))
                    .show(ui, |ui| self.about_view(ui, &ctx));
            }
            View::Editor => {
                self.shortcuts(&ctx);
                egui::Panel::top("topbar")
                    .frame(bar(theme::SURFACE))
                    .show(ui, |ui| self.top_bar(ui, &ctx));
                egui::Panel::bottom("status")
                    .frame(theme::panel_frame(theme::SURFACE, 4))
                    .show(ui, |ui| self.status_bar(ui));
                if self.video.is_some() {
                    egui::Panel::bottom("transport")
                        .frame(bar(theme::SURFACE))
                        .show(ui, |ui| self.video_bar(ui, &ctx));
                }
                egui::CentralPanel::default()
                    .frame(theme::panel_frame(theme::CANVAS, 0))
                    .show(ui, |ui| self.editor_columns(ui));
            }
        }
    }
}
/// Start the UI. `setup` runs once the window exists and returns the event receiver, so hotkey,
/// tray and control-channel threads can be given a `Waker`.
/// start_hidden starts parked in the tray (used for start-with-Windows).
pub fn run(
    session: Session,
    start_hidden: bool,
    setup: impl FnOnce(Waker) -> Receiver<UiEvent> + 'static,
) -> eframe::Result {
    let icon = screenit_platform::app_icon_rgba(256);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("screenIt")
        .with_inner_size(COMPACT_SIZE)
        .with_min_inner_size([560.0, 180.0])
        .with_resizable(false)
        .with_maximize_button(false)
        .with_icon(egui::IconData {
            rgba: icon,
            width: 256,
            height: 256,
        });
    if start_hidden {
        viewport = viewport.with_position([-32000.0, -32000.0]);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "screenIt",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let rx = setup(Waker(cc.egui_ctx.clone()));
            let mut app = UiApp::new(session, rx);
            app.start_hidden = start_hidden;
            Ok(Box::new(app))
        }),
    )
}

/// A horizontal row that reads right to left in right-to-left languages.
pub(crate) fn h_row<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) {
    if screenit_i18n::is_rtl() {
        // `with_layout` would claim all remaining height in a vertical parent; reserve one row.
        let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
        ui.allocate_ui_with_layout(
            size,
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                add(ui);
            },
        );
    } else {
        ui.horizontal(|ui| {
            add(ui);
        });
    }
}

/// Where text starts: left, or right in right-to-left languages.
pub(crate) fn start_layout() -> egui::Layout {
    if screenit_i18n::is_rtl() {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    }
}

/// The opposite end of a row from where text starts (the "far" side).
pub(crate) fn far_layout() -> egui::Layout {
    if screenit_i18n::is_rtl() {
        egui::Layout::left_to_right(egui::Align::Center)
    } else {
        egui::Layout::right_to_left(egui::Align::Center)
    }
}

/// Top-down column whose items hug the start edge.
pub(crate) fn column_layout() -> egui::Layout {
    if screenit_i18n::is_rtl() {
        egui::Layout::top_down(egui::Align::Max)
    } else {
        egui::Layout::top_down(egui::Align::Min)
    }
}

/// `%LOCALAPPDATA%\screenIt\locales`: where edited translations live.
pub(crate) fn user_locales_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|p| std::path::PathBuf::from(p).join("screenIt").join("locales"))
}

#[cfg(test)]
mod doc_tex_tests {
    use super::*;

    #[test]
    fn tall_and_wide_images_are_cut_into_tiles_that_cover_every_pixel() {
        let ctx = egui::Context::default();
        let tile = ctx.input(|i| i.max_texture_side).min(TILE);
        let (w, h) = (tile as u32 + 5, 2 * tile as u32 + 7);
        let img = screenit_raster::Raster::new(w, h, [9, 9, 9, 255]).unwrap();
        let t = DocTex::upload(&ctx, &img);
        assert_eq!(t.size(), [w as usize, h as usize]);
        assert_eq!(t.tiles.len(), 2 * 3);
        let area: usize = t.tiles.iter().map(|(_, x)| x.size()[0] * x.size()[1]).sum();
        assert_eq!(area, w as usize * h as usize);
        assert!(
            t.tiles
                .iter()
                .all(|(_, x)| x.size().iter().all(|s| *s <= tile))
        );
    }
}
