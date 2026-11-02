//! egui front end. A thin shell: every action goes through `Session::dispatch`, the UI only
//! decides which command to call and renders the document the engine owns.

mod canvas;
mod chrome;
mod icons;
mod library_panel;
mod overlay;
mod recording;
mod settings_panel;
mod theme;

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
            Tool::Select => "Click an object to select it, drag to move it.",
            Tool::Arrow => "Drag from the tail to the tip.",
            Tool::Line => "Drag to draw a straight line.",
            Tool::Rect => "Drag to draw a rectangle.",
            Tool::Ellipse => "Drag to draw an ellipse.",
            Tool::Text => "Click where the text should go.",
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
}

const COMPACT_SIZE: [f32; 2] = [812.0, 252.0];
const EDITOR_SIZE: [f32; 2] = [1280.0, 820.0];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pending {
    Overlay,
    /// The overlay, picking the area to record.
    RecordOverlay,
    Fullscreen,
    Preset,
}

impl Pending {
    fn is_overlay(self) -> bool {
        matches!(self, Pending::Overlay | Pending::RecordOverlay)
    }
}

pub struct UiApp {
    pub(crate) session: Session,
    events: Receiver<UiEvent>,
    pub(crate) tex: Option<egui::TextureHandle>,
    doc_rev: u64,
    tex_rev: u64,
    pub(crate) tool: Tool,
    pub(crate) stroke: egui::Color32,
    pub(crate) width: f32,
    pub(crate) text: String,
    pub(crate) zoom: f32,
    pub(crate) status: String,
    pub(crate) drag: Option<egui::Pos2>,
    pub(crate) pen: Vec<[f32; 2]>,
    pub(crate) selected: Option<u64>,
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
    /// Fit the document to the canvas on the next frame (new capture, Ctrl+0).
    pub(crate) fit: bool,
    pub(crate) rec_opts: recording::RecOptions,
    recording: Option<recording::RecUi>,
    /// Last file written (capture, export, recording): drives "Show in folder".
    pub(crate) last_saved: Option<std::path::PathBuf>,
    /// The dark title bar has been applied to the native window.
    framed: bool,
    quit: bool,
    pub(crate) settings_tab: settings_panel::Tab,
}

impl UiApp {
    pub fn new(session: Session, events: Receiver<UiEvent>) -> Self {
        Self {
            session,
            events,
            tex: None,
            doc_rev: 1,
            tex_rev: 0,
            tool: Tool::Arrow,
            stroke: egui::Color32::from_rgb(230, 30, 30),
            width: 3.0,
            text: "Text".into(),
            zoom: 1.0,
            status: "Ready. Press Print Screen to capture.".into(),
            drag: None,
            pen: Vec::new(),
            selected: None,
            overlay: None,
            pending: None,
            view: View::Compact,
            tray: false,
            lib_query: String::new(),
            thumbs: HashMap::new(),
            video: None,
            saved_pos: None,
            home: None,
            fit: true,
            rec_opts: recording::RecOptions::default(),
            recording: None,
            last_saved: None,
            framed: false,
            quit: false,
            settings_tab: settings_panel::Tab::General,
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
        let [w, h] = if view == View::Compact {
            COMPACT_SIZE
        } else if view == View::Settings {
            [960.0, 660.0]
        } else {
            EDITOR_SIZE
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
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
                if self.run("capture.fullscreen", json!({})).is_some() {
                    self.status = "Captured the full screen".into();
                }
            }
            Pending::Preset => {
                let name = preset.unwrap_or_default();
                show = self
                    .session
                    .presets
                    .iter()
                    .find(|p| p.name == name)
                    .is_none_or(|p| p.open_in_editor);
                if let Some(v) = self.run("capture.preset.run", json!({"name": name})) {
                    self.show_capture(&v);
                }
            }
            Pending::Overlay | Pending::RecordOverlay => match self
                .session
                .screen()
                .map(|s| FrozenScreen::snapshot(s.as_ref()))
            {
                Ok(Ok(frozen)) => {
                    let mut ov = overlay::Overlay::new(ctx, Arc::new(frozen));
                    ov.record = what == Pending::RecordOverlay;
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
        record: bool,
    ) {
        self.overlay = None;
        if record {
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
                self.enter_view(ctx, View::Editor);
                self.restore_window(ctx);
                return;
            }
        }
        self.settle_after_capture();
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                UiEvent::RunPreset(_) if self.is_recording() => {
                    self.status = "Stop the recording before taking a capture".into();
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
        self.tex = self.session.doc.as_ref().map(|d| {
            let img = screenit_compose::flatten(d);
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.data(),
            );
            ctx.load_texture(
                "document",
                ci,
                egui::TextureOptions {
                    magnification: egui::TextureFilter::Nearest,
                    minification: egui::TextureFilter::Linear,
                    ..Default::default()
                },
            )
        });
        if self.tex.as_ref().map(|t| t.size()) != old_size {
            self.fit = true;
        }
    }
}

impl UiApp {
    /// Status line for a capture result, e.g. `Captured 2560 × 1600 · saved to C:\…\capture-1.scit`.
    fn show_capture(&mut self, v: &Value) {
        let (w, h) = (
            v["width"].as_u64().unwrap_or(0),
            v["height"].as_u64().unwrap_or(0),
        );
        match v["saved"].as_str() {
            Some(p) => self.saved_to(&format!("Captured {w} × {h} ·"), p),
            None => self.status = format!("Captured {w} × {h}"),
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
    filter: (&str, &str),
) -> Option<String> {
    let dir = screenit_platform::shell::output_dir();
    let _ = std::fs::create_dir_all(&dir);
    screenit_platform::shell::save_dialog(title, &dir, default_name, filter)
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
        self.drain_events(ctx);
        self.run_pending(ctx);
        if self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.refresh_texture(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        overlay::drive(self, &ctx);
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
                egui::Panel::left("tools")
                    .frame(bar(theme::SURFACE))
                    .exact_size(58.0)
                    .resizable(false)
                    .show(ui, |ui| self.tool_palette(ui));
                egui::Panel::right("props")
                    .frame(theme::panel_frame(theme::SURFACE, 14))
                    .exact_size(236.0)
                    .resizable(false)
                    .show(ui, |ui| self.properties(ui));
                egui::CentralPanel::default()
                    .frame(theme::panel_frame(theme::CANVAS, 0))
                    .show(ui, |ui| canvas::show(self, ui));
            }
        }
    }
}
/// Start the UI. `setup` runs once the window exists and returns the event receiver, so hotkey,
/// tray and control-channel threads can be given a `Waker`.
pub fn run(
    session: Session,
    setup: impl FnOnce(Waker) -> Receiver<UiEvent> + 'static,
) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("screenIt")
            .with_inner_size(COMPACT_SIZE)
            .with_min_inner_size([560.0, 180.0]),
        ..Default::default()
    };
    eframe::run_native(
        "screenIt",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let rx = setup(Waker(cc.egui_ctx.clone()));
            Ok(Box::new(UiApp::new(session, rx)))
        }),
    )
}
