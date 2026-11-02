//! Capture model: selections, zones, presets, hotkeys, multi-area composition, freehand masks,
//! scrolling stitcher and capture-time effects. OS access is behind the `ScreenSource` trait.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use screenit_algo as algo;
use screenit_doc::Metadata;
use screenit_geom::{Point, PointF, Rect, Size, virtual_screen};
use screenit_raster::Raster;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("invalid selection: {0}")]
    BadSelection(String),
    #[error("no such monitor {0}")]
    NoMonitor(u32),
    #[error("no such window {0}")]
    NoWindow(u64),
    #[error("platform capture failed: {0}")]
    Platform(String),
    #[error("nothing to capture")]
    Empty,
    #[error(transparent)]
    Raster(#[from] screenit_raster::RasterError),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub rect: Rect,
    pub dpi: u32,
    pub primary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub process: String,
    pub rect: Rect,
    pub minimized: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorInfo {
    pub pos: Point,
}

/// Everything the engine needs from the OS to take screenshots. Implemented by `screenit-platform`
/// and by `testkit::FakeScreen`.
pub trait ScreenSource: Send + Sync {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError>;
    /// Visible top-level windows, topmost first.
    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError>;
    /// Pixels of a rectangle in virtual-screen coordinates.
    fn grab(&self, rect: Rect) -> Result<Raster, CaptureError>;
    /// Like grab, for a steady stream of frames (recording). Sources may use a faster path that
    /// is only worthwhile when called repeatedly; the default is grab.
    fn grab_stream(&self, rect: Rect) -> Result<Raster, CaptureError> {
        self.grab(rect)
    }
    fn grab_window(&self, id: u64) -> Result<Raster, CaptureError>;
    fn cursor(&self) -> Option<CursorInfo>;
    fn foreground(&self) -> Option<WindowInfo>;
    /// Scroll the content under `at` by `notches` wheel notches (positive = down).
    fn scroll(&self, _at: Point, _notches: i32) -> Result<(), CaptureError> {
        Err(CaptureError::Platform(
            "scrolling is not supported by this screen source".into(),
        ))
    }
    /// Deepest UI element (child window / control) under the point.
    fn object_at(&self, _p: Point) -> Option<WindowInfo> {
        None
    }
    /// Open popup menus, topmost first.
    fn menus(&self) -> Vec<WindowInfo> {
        Vec::new()
    }
}

/// One recognised line of text with its bounding box in image coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OcrLine {
    pub text: String,
    pub rect: Rect,
}

/// Text recognition, implemented by the platform layer (Windows.Media.Ocr) and by test fakes.
pub trait Ocr: Send + Sync {
    fn recognize(&self, image: &Raster) -> Result<Vec<OcrLine>, CaptureError>;

    /// Recognise with a language hint (`auto`, `en`, `ar`, any BCP-47 tag). Engines without
    /// language selection ignore it.
    fn recognize_lang(&self, image: &Raster, lang: &str) -> Result<Vec<OcrLine>, CaptureError> {
        let _ = lang;
        self.recognize(image)
    }

    /// BCP-47 tags of the installed recognition languages (empty when unknown).
    fn languages(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Where a language-model OCR server lives (an OpenAI-compatible API such as LM Studio).
#[derive(Clone, Debug, PartialEq)]
pub struct LlmConfig {
    pub url: String,
    pub model: String,
    /// Bearer token for servers that require one (LM Studio allows an empty key).
    pub api_key: String,
    pub timeout_secs: u32,
}

/// OCR through a vision language model. Returns plain text only (no boxes).
pub trait LlmOcr: Send + Sync {
    fn transcribe(&self, image: &Raster, cfg: &LlmConfig) -> Result<String, CaptureError>;
    /// Model ids the server offers (also a connection test).
    fn models(&self, cfg: &LlmConfig) -> Result<Vec<String>, CaptureError>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Selection {
    Region { rect: Rect },
    Window { id: u64 },
    Fullscreen { monitor: Option<u32> },
    Fixed { size: Size, at: Point },
    LastRegion,
    MultiArea { rects: Vec<Rect> },
    Freehand { points: Vec<Point> },
}

pub struct CaptureOutput {
    pub image: Raster,
    pub metadata: Metadata,
    /// Region of the virtual screen that was captured (used for "repeat last region").
    pub region: Option<Rect>,
}

const MAX_AREAS: usize = 64;

/// Topmost visible window containing the point (snap-to-window under the crosshair).
pub fn window_at(windows: &[WindowInfo], p: Point) -> Option<&WindowInfo> {
    windows.iter().find(|w| !w.minimized && w.rect.contains(p))
}

/// Even-odd polygon containment.
fn in_polygon(pts: &[PointF], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = pts.len().saturating_sub(1);
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[j]);
        if (a.y > y) != (b.y > y) && x < (b.x - a.x) * (y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn base_metadata(src: &dyn ScreenSource, source: &str, monitor: u32, dpi: u32) -> Metadata {
    let fg = src.foreground();
    Metadata {
        app: fg.as_ref().map(|w| w.process.clone()).unwrap_or_default(),
        title: fg.map(|w| w.title).unwrap_or_default(),
        url: None,
        captured_at: chrono_now(),
        monitor,
        dpi,
        source: source.to_string(),
        tags: Vec::new(),
    }
}

fn chrono_now() -> String {
    // RFC 3339 seconds-resolution timestamp from the system clock.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

fn monitor_for(monitors: &[MonitorInfo], r: Rect) -> (u32, u32) {
    monitors
        .iter()
        .filter_map(|m| m.rect.intersect(&r).map(|i| (i.area(), m)))
        .max_by_key(|(a, _)| *a)
        .map_or((0, 96), |(_, m)| (m.id, m.dpi))
}

pub fn capture(
    src: &dyn ScreenSource,
    sel: &Selection,
    last: Option<Rect>,
) -> Result<CaptureOutput, CaptureError> {
    let monitors = src.monitors()?;
    let screen = virtual_screen(&monitors.iter().map(|m| m.rect).collect::<Vec<_>>());
    let clip = |r: Rect| -> Result<Rect, CaptureError> {
        r.intersect(&screen)
            .ok_or_else(|| CaptureError::BadSelection("outside the screen".into()))
    };
    match sel {
        Selection::Region { rect } => {
            let r = clip(*rect)?;
            let (mon, dpi) = monitor_for(&monitors, r);
            Ok(CaptureOutput {
                image: src.grab(r)?,
                metadata: base_metadata(src, "region", mon, dpi),
                region: Some(r),
            })
        }
        Selection::LastRegion => {
            let r =
                clip(last.ok_or_else(|| CaptureError::BadSelection("no previous region".into()))?)?;
            let (mon, dpi) = monitor_for(&monitors, r);
            Ok(CaptureOutput {
                image: src.grab(r)?,
                metadata: base_metadata(src, "last_region", mon, dpi),
                region: Some(r),
            })
        }
        Selection::Fixed { size, at } => {
            if size.w == 0 || size.h == 0 {
                return Err(CaptureError::BadSelection("zero size".into()));
            }
            let r = clip(Rect::new(at.x, at.y, size.w, size.h))?;
            let (mon, dpi) = monitor_for(&monitors, r);
            Ok(CaptureOutput {
                image: src.grab(r)?,
                metadata: base_metadata(src, "fixed", mon, dpi),
                region: Some(r),
            })
        }
        Selection::Fullscreen { monitor } => {
            let (r, mon, dpi) = match monitor {
                Some(id) => {
                    let m = monitors
                        .iter()
                        .find(|m| m.id == *id)
                        .ok_or(CaptureError::NoMonitor(*id))?;
                    (m.rect, m.id, m.dpi)
                }
                None => (screen, 0, monitors.first().map_or(96, |m| m.dpi)),
            };
            Ok(CaptureOutput {
                image: src.grab(r)?,
                metadata: base_metadata(src, "fullscreen", mon, dpi),
                region: Some(r),
            })
        }
        Selection::Window { id } => {
            let win = src
                .windows()?
                .into_iter()
                .find(|w| w.id == *id)
                .ok_or(CaptureError::NoWindow(*id))?;
            let (mon, dpi) = monitor_for(&monitors, win.rect);
            let mut md = base_metadata(src, "window", mon, dpi);
            md.title = win.title.clone();
            md.app = win.process.clone();
            Ok(CaptureOutput {
                image: src.grab_window(*id)?,
                metadata: md,
                region: Some(win.rect),
            })
        }
        Selection::MultiArea { rects } => {
            if rects.is_empty() || rects.len() > MAX_AREAS {
                return Err(CaptureError::BadSelection("need 1..=64 areas".into()));
            }
            let clipped: Vec<Rect> = rects.iter().filter_map(|r| r.intersect(&screen)).collect();
            if clipped.is_empty() {
                return Err(CaptureError::Empty);
            }
            let bounds = clipped.iter().fold(Rect::default(), |a, r| a.union(r));
            let mut out = Raster::new(bounds.w, bounds.h, [0; 4])?;
            for r in &clipped {
                let img = src.grab(*r)?;
                out.copy_from(&img, r.x - bounds.x, r.y - bounds.y);
            }
            let (mon, dpi) = monitor_for(&monitors, bounds);
            Ok(CaptureOutput {
                image: out,
                metadata: base_metadata(src, "multi_area", mon, dpi),
                region: Some(bounds),
            })
        }
        Selection::Freehand { points } => {
            if points.len() < 3 || points.len() > 100_000 {
                return Err(CaptureError::BadSelection(
                    "freehand needs 3..=100000 points".into(),
                ));
            }
            let (mut a, mut b) = (points[0], points[0]);
            for p in points {
                a = Point::new(a.x.min(p.x), a.y.min(p.y));
                b = Point::new(b.x.max(p.x), b.y.max(p.y));
            }
            let r = clip(Rect::from_points(a, b))?;
            let mut img = src.grab(r)?;
            let poly: Vec<PointF> = points
                .iter()
                .map(|p| PointF::new((p.x - r.x) as f32 + 0.5, (p.y - r.y) as f32 + 0.5))
                .collect();
            for y in 0..img.height() as i32 {
                for x in 0..img.width() as i32 {
                    if !in_polygon(&poly, x as f32 + 0.5, y as f32 + 0.5) {
                        img.put(x, y, [0, 0, 0, 0]);
                    }
                }
            }
            let (mon, dpi) = monitor_for(&monitors, r);
            Ok(CaptureOutput {
                image: img,
                metadata: base_metadata(src, "freehand", mon, dpi),
                region: Some(r),
            })
        }
    }
}

/// Draw the cursor (a simple arrow glyph) into a capture taken at `origin`.
pub fn composite_cursor(img: &mut Raster, origin: Point, cursor: CursorInfo) {
    let (cx, cy) = (cursor.pos.x - origin.x, cursor.pos.y - origin.y);
    let shape: [(i32, i32); 10] = [
        (0, 0),
        (0, 14),
        (4, 11),
        (7, 17),
        (9, 16),
        (6, 10),
        (11, 10),
        (0, 0),
        (0, 0),
        (0, 0),
    ];
    for dy in 0..18 {
        let width = if dy < 14 { dy * 2 / 3 + 1 } else { 0 };
        for dx in 0..width.min(12) {
            img.put(cx + dx, cy + dy, [0, 0, 0, 255]);
        }
    }
    let _ = shape;
}

// ---------------------------------------------------------------- scrolling stitcher

fn row_eq(a: &Raster, ya: u32, b: &Raster, yb: u32) -> bool {
    let w = a.width() as usize * 4;
    let (ia, ib) = (ya as usize * w, yb as usize * w);
    match (a.data().get(ia..ia + w), b.data().get(ib..ib + w)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn rows_vary(r: &Raster, y0: u32, n: u32) -> bool {
    (y0 + 1..y0 + n).any(|y| !row_eq(r, y0, r, y))
}

/// Stitch frames of a vertical scroll. Each new frame is matched against the bottom of the result by
/// exact row comparison (screen captures are pixel-exact); identical frames are skipped.
pub fn stitch_vertical(frames: &[Raster]) -> Result<Raster, CaptureError> {
    let first = frames.first().ok_or(CaptureError::Empty)?;
    let w = first.width();
    let mut acc = first.clone();
    for f in &frames[1..] {
        if f.width() != w {
            return Err(CaptureError::BadSelection("frames differ in width".into()));
        }
        let h = f.height();
        let mut shift = None;
        for s in 0..h {
            let overlap = h - s;
            let top = acc.height().saturating_sub(overlap);
            if acc.height() < overlap {
                continue;
            }
            let matches = (0..overlap).all(|i| row_eq(&acc, top + i, f, i));
            if matches && (overlap < 2 || rows_vary(f, 0, overlap)) {
                shift = Some(s);
                break;
            }
        }
        match shift {
            Some(0) => {}
            Some(s) => {
                let mut next = Raster::new(w, acc.height() + s, [0; 4])?;
                next.copy_from(&acc, 0, 0);
                next.copy_from(f, 0, (acc.height() + s - h) as i32);
                acc = next;
            }
            None => {
                let mut next = Raster::new(w, acc.height() + h, [0; 4])?;
                next.copy_from(&acc, 0, 0);
                next.copy_from(f, 0, acc.height() as i32);
                acc = next;
            }
        }
    }
    Ok(acc)
}

fn transpose(r: &Raster) -> Result<Raster, CaptureError> {
    let mut out = Raster::new(r.height(), r.width(), [0; 4])?;
    for y in 0..r.height() as i32 {
        for x in 0..r.width() as i32 {
            if let Some(p) = r.get(x, y) {
                out.put(y, x, p);
            }
        }
    }
    Ok(out)
}

/// Horizontal scroll variant of [`stitch_vertical`].
pub fn stitch_horizontal(frames: &[Raster]) -> Result<Raster, CaptureError> {
    let t: Result<Vec<Raster>, _> = frames.iter().map(transpose).collect();
    transpose(&stitch_vertical(&t?)?)
}

// ---------------------------------------------------------------- effects

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Effects {
    pub border: Option<(u32, [u8; 4])>,
    pub corner_radius: Option<u32>,
    pub scale_percent: Option<u32>,
    pub shadow: bool,
}

/// Apply capture-time effects in a fixed order: scale, corner rounding, border, shadow.
pub fn apply_effects(img: &Raster, fx: &Effects) -> Result<Raster, CaptureError> {
    let mut out = img.clone();
    if let Some(p) = fx.scale_percent {
        if p == 0 || p > 800 {
            return Err(CaptureError::BadSelection(
                "scale must be 1..=800 percent".into(),
            ));
        }
        if p != 100 {
            let w = (u64::from(out.width()) * u64::from(p) / 100).max(1) as u32;
            let h = (u64::from(out.height()) * u64::from(p) / 100).max(1) as u32;
            out = algo::resize(&out, w, h)?;
        }
    }
    if let Some(rad) = fx.corner_radius {
        round_corners(&mut out, rad);
    }
    if let Some((w, c)) = fx.border {
        out = algo::border(&out, w, c)?;
    }
    if fx.shadow {
        out = algo::drop_shadow(&out, 4, 6, [0, 0, 0, 140])?;
    }
    Ok(out)
}

fn round_corners(r: &mut Raster, radius: u32) {
    let rad = radius.min(r.width() / 2).min(r.height() / 2) as i32;
    if rad == 0 {
        return;
    }
    let (w, h) = (r.width() as i32, r.height() as i32);
    for y in 0..rad {
        for x in 0..rad {
            let (dx, dy) = ((rad - x) as f32 - 0.5, (rad - y) as f32 - 0.5);
            if (dx * dx + dy * dy).sqrt() > rad as f32 {
                for (px, py) in [
                    (x, y),
                    (w - 1 - x, y),
                    (x, h - 1 - y),
                    (w - 1 - x, h - 1 - y),
                ] {
                    r.put(px, py, [0, 0, 0, 0]);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- presets and hotkeys

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Image,
    Video,
    AllInOne,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub mode: Mode,
    pub selection: Selection,
    pub delay_ms: u32,
    pub include_cursor: bool,
    pub copy_to_clipboard: bool,
    pub open_in_editor: bool,
    pub hotkey: Option<String>,
    pub effects: Effects,
}

impl Preset {
    pub fn new(name: &str, selection: Selection) -> Self {
        Self {
            name: name.to_string(),
            mode: Mode::Image,
            selection,
            delay_ms: 0,
            include_cursor: false,
            copy_to_clipboard: false,
            open_in_editor: true,
            hotkey: None,
            effects: Effects::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hotkey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub win: bool,
    /// Windows virtual-key code.
    pub vk: u32,
}

/// Parse `Ctrl+Shift+S`, `Alt+F9`, `PrintScreen`, ... into modifiers + virtual-key code.
pub fn parse_hotkey(s: &str) -> Result<Hotkey, CaptureError> {
    let bad = |m: &str| CaptureError::BadSelection(format!("hotkey `{s}`: {m}"));
    let mut hk = Hotkey {
        ctrl: false,
        shift: false,
        alt: false,
        win: false,
        vk: 0,
    };
    let parts: Vec<&str> = s.split('+').map(str::trim).collect();
    let Some((key, mods)) = parts.split_last() else {
        return Err(bad("empty"));
    };
    for m in mods {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => hk.ctrl = true,
            "shift" => hk.shift = true,
            "alt" => hk.alt = true,
            "win" | "super" => hk.win = true,
            _ => return Err(bad("unknown modifier")),
        }
    }
    let k = key.to_ascii_lowercase();
    hk.vk = match k.as_str() {
        "printscreen" | "prtsc" | "snapshot" => 0x2C,
        "space" => 0x20,
        "enter" => 0x0D,
        "escape" | "esc" => 0x1B,
        _ if k.len() == 1 && k.is_ascii() => {
            let c = k.as_bytes().first().copied().unwrap_or(0);
            if c.is_ascii_alphanumeric() {
                u32::from(c.to_ascii_uppercase())
            } else {
                return Err(bad("unknown key"));
            }
        }
        _ if k.starts_with('f') => {
            let n: u32 = k[1..].parse().map_err(|_| bad("unknown key"))?;
            if !(1..=24).contains(&n) {
                return Err(bad("function key out of range"));
            }
            0x6F + n
        }
        _ => return Err(bad("unknown key")),
    };
    if hk.vk != 0x2C && !(hk.ctrl || hk.alt || hk.win || hk.shift) && !k.starts_with('f') {
        return Err(bad("needs a modifier"));
    }
    Ok(hk)
}

/// Detect duplicate hotkeys across presets; returns (preset a, preset b) conflicts.
pub fn hotkey_conflicts(presets: &[Preset]) -> Vec<(String, String)> {
    let mut seen: Vec<(Hotkey, &str)> = Vec::new();
    let mut out = Vec::new();
    for p in presets {
        let Some(h) = p.hotkey.as_deref().and_then(|s| parse_hotkey(s).ok()) else {
            continue;
        };
        if let Some((_, other)) = seen.iter().find(|(k, _)| *k == h) {
            out.push(((*other).to_string(), p.name.clone()));
        } else {
            seen.push((h, &p.name));
        }
    }
    out
}

pub fn default_presets() -> Vec<Preset> {
    let mut all_in_one = Preset::new(
        "All-in-One",
        Selection::Region {
            rect: Rect::default(),
        },
    );
    all_in_one.mode = Mode::AllInOne;
    all_in_one.hotkey = Some("PrintScreen".into());
    let mut fullscreen = Preset::new("Fullscreen", Selection::Fullscreen { monitor: None });
    fullscreen.hotkey = Some("Ctrl+Shift+F".into());
    vec![
        all_in_one,
        fullscreen,
        Preset::new("Last region", Selection::LastRegion),
    ]
}

/// A frozen snapshot of the whole virtual screen, served as a `ScreenSource`. The region overlay
/// shows this image while the user selects, and captures are cropped from it, so what the user
/// sees is exactly what is captured (and the overlay itself never appears in the result).
pub struct FrozenScreen {
    origin: Point,
    image: Raster,
    monitors: Vec<MonitorInfo>,
    windows: Vec<WindowInfo>,
    cursor: Option<CursorInfo>,
    foreground: Option<WindowInfo>,
}

impl FrozenScreen {
    pub fn snapshot(src: &dyn ScreenSource) -> Result<Self, CaptureError> {
        let monitors = src.monitors()?;
        let bounds = virtual_screen(&monitors.iter().map(|m| m.rect).collect::<Vec<_>>());
        let image = src.grab(bounds)?;
        Ok(Self {
            origin: Point::new(bounds.x, bounds.y),
            image,
            monitors,
            windows: src.windows().unwrap_or_default(),
            cursor: src.cursor(),
            foreground: src.foreground(),
        })
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.image.width(),
            self.image.height(),
        )
    }

    pub fn image(&self) -> &Raster {
        &self.image
    }

    pub fn windows_snapshot(&self) -> &[WindowInfo] {
        &self.windows
    }
}

impl ScreenSource for FrozenScreen {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        Ok(self.monitors.clone())
    }
    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
        Ok(self.windows.clone())
    }
    fn grab(&self, rect: Rect) -> Result<Raster, CaptureError> {
        let r = rect.intersect(&self.bounds()).ok_or(CaptureError::Empty)?;
        let local = Rect::new(r.x - self.origin.x, r.y - self.origin.y, r.w, r.h);
        self.image.crop(local).ok_or(CaptureError::Empty)
    }
    fn grab_window(&self, id: u64) -> Result<Raster, CaptureError> {
        let w = self
            .windows
            .iter()
            .find(|w| w.id == id)
            .ok_or(CaptureError::NoWindow(id))?;
        self.grab(w.rect)
    }
    fn cursor(&self) -> Option<CursorInfo> {
        self.cursor
    }
    fn foreground(&self) -> Option<WindowInfo> {
        self.foreground.clone()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        screen: Raster,
    }

    impl Fake {
        fn new() -> Self {
            let mut screen = Raster::new(200, 100, [10, 10, 10, 255]).unwrap();
            screen.fill_rect(Rect::new(20, 20, 30, 30), [255, 0, 0, 255]);
            Self { screen }
        }
    }

    impl ScreenSource for Fake {
        fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
            Ok(vec![MonitorInfo {
                id: 1,
                name: "A".into(),
                rect: Rect::new(0, 0, 200, 100),
                dpi: 96,
                primary: true,
            }])
        }
        fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
            Ok(vec![WindowInfo {
                id: 7,
                title: "Win".into(),
                process: "app.exe".into(),
                rect: Rect::new(20, 20, 30, 30),
                minimized: false,
            }])
        }
        fn grab(&self, rect: Rect) -> Result<Raster, CaptureError> {
            self.screen.crop(rect).ok_or(CaptureError::Empty)
        }
        fn grab_window(&self, id: u64) -> Result<Raster, CaptureError> {
            let w = self
                .windows()?
                .into_iter()
                .find(|w| w.id == id)
                .ok_or(CaptureError::NoWindow(id))?;
            self.grab(w.rect)
        }
        fn cursor(&self) -> Option<CursorInfo> {
            Some(CursorInfo {
                pos: Point::new(5, 5),
            })
        }
        fn foreground(&self) -> Option<WindowInfo> {
            self.windows().ok()?.into_iter().next()
        }
    }

    #[test]
    fn region_is_clipped_to_screen() {
        let f = Fake::new();
        let out = capture(
            &f,
            &Selection::Region {
                rect: Rect::new(150, 50, 500, 500),
            },
            None,
        )
        .unwrap();
        assert_eq!((out.image.width(), out.image.height()), (50, 50));
        assert!(
            capture(
                &f,
                &Selection::Region {
                    rect: Rect::new(900, 900, 5, 5)
                },
                None
            )
            .is_err()
        );
    }

    #[test]
    fn window_capture_has_metadata() {
        let f = Fake::new();
        let out = capture(&f, &Selection::Window { id: 7 }, None).unwrap();
        assert_eq!(out.metadata.title, "Win");
        assert_eq!(out.image.get(0, 0), Some([255, 0, 0, 255]));
        assert!(capture(&f, &Selection::Window { id: 99 }, None).is_err());
    }

    #[test]
    fn fullscreen_and_last_region() {
        let f = Fake::new();
        let out = capture(&f, &Selection::Fullscreen { monitor: None }, None).unwrap();
        assert_eq!(out.image.width(), 200);
        let again = capture(&f, &Selection::LastRegion, Some(Rect::new(20, 20, 30, 30))).unwrap();
        assert_eq!(again.image.get(1, 1), Some([255, 0, 0, 255]));
        assert!(capture(&f, &Selection::LastRegion, None).is_err());
        assert!(capture(&f, &Selection::Fullscreen { monitor: Some(9) }, None).is_err());
    }

    #[test]
    fn multi_area_composes_on_transparent_canvas() {
        let f = Fake::new();
        let sel = Selection::MultiArea {
            rects: vec![Rect::new(20, 20, 10, 10), Rect::new(100, 60, 10, 10)],
        };
        let out = capture(&f, &sel, None).unwrap();
        assert_eq!((out.image.width(), out.image.height()), (90, 50));
        assert_eq!(out.image.get(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(out.image.get(40, 5), Some([0, 0, 0, 0]));
        assert_eq!(out.image.get(85, 45), Some([10, 10, 10, 255]));
        assert!(capture(&f, &Selection::MultiArea { rects: vec![] }, None).is_err());
    }

    #[test]
    fn freehand_masks_outside_polygon() {
        let f = Fake::new();
        let tri = vec![Point::new(20, 20), Point::new(49, 20), Point::new(20, 49)];
        let out = capture(&f, &Selection::Freehand { points: tri }, None).unwrap();
        assert_eq!(out.image.get(1, 1), Some([255, 0, 0, 255]));
        assert_eq!(out.image.get(28, 28).map(|p| p[3]), Some(0));
        assert!(
            capture(
                &f,
                &Selection::Freehand {
                    points: vec![Point::new(0, 0)]
                },
                None
            )
            .is_err()
        );
    }

    #[test]
    fn window_snap_picks_topmost() {
        let f = Fake::new();
        let ws = f.windows().unwrap();
        assert_eq!(window_at(&ws, Point::new(25, 25)).map(|w| w.id), Some(7));
        assert!(window_at(&ws, Point::new(0, 0)).is_none());
    }

    fn page(h: u32, offset: u32) -> Raster {
        // Unique content per absolute row so overlaps are unambiguous.
        let mut r = Raster::new(8, h, [0, 0, 0, 255]).unwrap();
        for y in 0..h {
            let v = (y + offset) as u8;
            for x in 0..8 {
                r.put(x, y as i32, [v, v.wrapping_mul(3), x as u8 * 7, 255]);
            }
        }
        r
    }

    #[test]
    fn stitch_reassembles_scroll() {
        let frames = vec![page(20, 0), page(20, 12), page(20, 24), page(20, 24)];
        let out = stitch_vertical(&frames).unwrap();
        assert_eq!(out.height(), 44);
        assert_eq!(out, page(44, 0));
    }

    #[test]
    fn stitch_no_overlap_appends_and_rejects_mismatch() {
        let out = stitch_vertical(&[page(10, 0), page(10, 100)]).unwrap();
        assert_eq!(out.height(), 20);
        assert!(stitch_vertical(&[]).is_err());
        assert!(stitch_vertical(&[page(10, 0), Raster::new(4, 4, [0; 4]).unwrap()]).is_err());
    }

    #[test]
    fn stitch_horizontal_works() {
        let t = |r: &Raster| transpose(r).unwrap();
        let frames = vec![t(&page(20, 0)), t(&page(20, 12))];
        let out = stitch_horizontal(&frames).unwrap();
        assert_eq!(out, t(&page(32, 0)));
    }

    #[test]
    fn effects_apply_in_order() {
        let img = Raster::new(20, 20, [255, 255, 255, 255]).unwrap();
        let fx = Effects {
            border: Some((2, [0, 0, 0, 255])),
            corner_radius: Some(5),
            scale_percent: Some(200),
            shadow: false,
        };
        let out = apply_effects(&img, &fx).unwrap();
        assert_eq!((out.width(), out.height()), (44, 44));
        assert!(
            apply_effects(
                &img,
                &Effects {
                    scale_percent: Some(0),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn hotkeys_parse_and_conflict() {
        let h = parse_hotkey("Ctrl+Shift+S").unwrap();
        assert!(h.ctrl && h.shift && !h.alt);
        assert_eq!(h.vk, u32::from(b'S'));
        assert_eq!(parse_hotkey("PrintScreen").unwrap().vk, 0x2C);
        assert_eq!(parse_hotkey("alt+f9").unwrap().vk, 0x78);
        for bad in ["", "Ctrl+", "Hyper+X", "S", "F99", "Ctrl+é", "+++"] {
            assert!(parse_hotkey(bad).is_err(), "{bad}");
        }
        let mut a = Preset::new("a", Selection::LastRegion);
        let mut b = Preset::new("b", Selection::LastRegion);
        a.hotkey = Some("Ctrl+1".into());
        b.hotkey = Some("ctrl+1".into());
        assert_eq!(
            hotkey_conflicts(&[a, b]),
            vec![("a".to_string(), "b".to_string())]
        );
        assert!(hotkey_conflicts(&default_presets()).is_empty());
    }

    #[test]
    fn cursor_compositing_stays_in_bounds() {
        let mut img = Raster::new(10, 10, [255; 4]).unwrap();
        composite_cursor(
            &mut img,
            Point::new(0, 0),
            CursorInfo {
                pos: Point::new(2, 2),
            },
        );
        assert_eq!(img.get(2, 2), Some([0, 0, 0, 255]));
        composite_cursor(
            &mut img,
            Point::new(0, 0),
            CursorInfo {
                pos: Point::new(-50, 99),
            },
        );
    }

    #[test]
    fn frozen_screen_serves_crops_in_virtual_coordinates() {
        struct Two;
        impl ScreenSource for Two {
            fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
                Ok(vec![
                    MonitorInfo {
                        id: 0,
                        name: "L".into(),
                        rect: Rect::new(-10, 0, 10, 10),
                        dpi: 96,
                        primary: false,
                    },
                    MonitorInfo {
                        id: 1,
                        name: "R".into(),
                        rect: Rect::new(0, 0, 10, 10),
                        dpi: 96,
                        primary: true,
                    },
                ])
            }
            fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
                Ok(vec![])
            }
            fn grab(&self, rect: Rect) -> Result<Raster, CaptureError> {
                let mut r = Raster::new(rect.w, rect.h, [0, 0, 0, 255])?;
                for y in 0..rect.h {
                    for x in 0..rect.w {
                        r.put(
                            x as i32,
                            y as i32,
                            [
                                (rect.x + x as i32 + 10) as u8,
                                (rect.y + y as i32) as u8,
                                0,
                                255,
                            ],
                        );
                    }
                }
                Ok(r)
            }
            fn grab_window(&self, id: u64) -> Result<Raster, CaptureError> {
                Err(CaptureError::NoWindow(id))
            }
            fn cursor(&self) -> Option<CursorInfo> {
                None
            }
            fn foreground(&self) -> Option<WindowInfo> {
                None
            }
        }
        let f = FrozenScreen::snapshot(&Two).unwrap();
        assert_eq!(f.bounds(), Rect::new(-10, 0, 20, 10));
        let g = f.grab(Rect::new(-3, 2, 4, 2)).unwrap();
        assert_eq!(
            g.get(0, 0),
            Some([7, 2, 0, 255]),
            "virtual x=-3 maps to marker 7"
        );
        assert_eq!(g.get(3, 1), Some([10, 3, 0, 255]));
        assert!(f.grab(Rect::new(100, 100, 5, 5)).is_err());
    }
}

fn is_rtl_char(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Windows OCR reports the words of a right-to-left line in left-to-right (visual) order.
/// Reverse each run of consecutive RTL words so the text reads in logical order; Latin words,
/// numbers and punctuation keep their place.
pub fn rtl_logical_order(line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    if !words.iter().any(|w| w.chars().any(is_rtl_char)) {
        return line.to_string();
    }
    let mut out: Vec<&str> = Vec::with_capacity(words.len());
    let mut run: Vec<&str> = Vec::new();
    for w in words {
        if w.chars().any(is_rtl_char) {
            run.push(w);
        } else {
            out.extend(run.drain(..).rev());
            out.push(w);
        }
    }
    out.extend(run.drain(..).rev());
    out.join(" ")
}

#[cfg(test)]
mod rtl_tests {
    use super::rtl_logical_order;

    #[test]
    fn reverses_arabic_runs_only() {
        let visual = "\u{628}\u{627}\u{644}\u{639}\u{627}\u{644}\u{645} \u{645}\u{631}\u{62D}\u{628}\u{627} - screenIt";
        let logical = "\u{645}\u{631}\u{62D}\u{628}\u{627} \u{628}\u{627}\u{644}\u{639}\u{627}\u{644}\u{645} - screenIt";
        assert_eq!(rtl_logical_order(visual), logical);
        assert_eq!(rtl_logical_order("Total: 1,250 USD"), "Total: 1,250 USD");
        assert_eq!(rtl_logical_order(""), "");
    }
}
