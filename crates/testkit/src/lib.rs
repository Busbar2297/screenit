//! Test support: golden-image comparison, deterministic patterns, and fake platform services.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use screenit_capture::{CaptureError, CursorInfo, MonitorInfo, ScreenSource, WindowInfo};
use screenit_geom::{Point, Rect};
use screenit_raster::Raster;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Deterministic test image: a gradient with a checker band and a bright marker, so crops,
/// flips and rotations are all distinguishable.
pub fn pattern(w: u32, h: u32) -> Raster {
    let mut r = Raster::new(w, h, [0, 0, 0, 255]).unwrap();
    for y in 0..h {
        for x in 0..w {
            let checker = ((x / 8) + (y / 8)) % 2 == 0;
            let px = [
                (x * 255 / w.max(1)) as u8,
                (y * 255 / h.max(1)) as u8,
                if checker { 200 } else { 60 },
                255,
            ];
            r.put(x as i32, y as i32, px);
        }
    }
    r.fill_rect(Rect::new(0, 0, 4.min(w), 4.min(h)), [255, 255, 255, 255]);
    r
}

/// Compare `actual` to `<manifest_dir>/tests/golden/<name>.png`. `SCREENIT_BLESS=1` (re)writes it.
/// `tolerance` is the allowed mean absolute channel difference (0.0 = exact).
pub fn check_golden(manifest_dir: &str, actual: &Raster, name: &str, tolerance: f64) {
    let path: PathBuf = Path::new(manifest_dir)
        .join("tests")
        .join("golden")
        .join(format!("{name}.png"));
    if std::env::var("SCREENIT_BLESS").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = screenit_codecs::encode(actual, screenit_codecs::Format::Png, 100).unwrap();
        std::fs::write(&path, bytes).unwrap();
        return;
    }
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|_| panic!("missing golden {path:?}; run `cargo xtask bless`"));
    let expected = screenit_codecs::decode(&bytes).unwrap();
    let diff = actual.mean_abs_diff(&expected).unwrap_or_else(|| {
        panic!(
            "golden {name}: size {}x{} != expected {}x{}",
            actual.width(),
            actual.height(),
            expected.width(),
            expected.height()
        )
    });
    assert!(
        diff <= tolerance,
        "golden {name}: mean abs diff {diff:.4} exceeds tolerance {tolerance}"
    );
}

#[macro_export]
macro_rules! assert_image_eq {
    ($actual:expr, $name:expr, $tol:expr) => {
        $crate::check_golden(env!("CARGO_MANIFEST_DIR"), &$actual, $name, $tol)
    };
}

/// A virtual desktop backed by a raster, with configurable windows.
pub struct FakeScreen {
    pub desktop: Raster,
    pub windows: Vec<WindowInfo>,
    pub cursor: Option<Point>,
    /// Optional scrollable viewport: grabbing exactly iew returns content at the current offset.
    pub scroller: Mutex<Option<FakeScroller>>,
}

pub struct FakeScroller {
    pub content: Raster,
    pub view: Rect,
    pub offset: i32,
    pub px_per_notch: i32,
}

impl FakeScreen {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            desktop: pattern(w, h),
            windows: Vec::new(),
            cursor: None,
            scroller: Mutex::new(None),
        }
    }
    pub fn with_window(mut self, id: u64, title: &str, rect: Rect) -> Self {
        self.windows.push(WindowInfo {
            id,
            title: title.into(),
            process: "fixture.exe".into(),
            rect,
            minimized: false,
        });
        self
    }
    pub fn with_scroller(self, content: Raster, view: Rect, px_per_notch: i32) -> Self {
        *self.scroller.lock().unwrap() = Some(FakeScroller {
            content,
            view,
            offset: 0,
            px_per_notch,
        });
        self
    }
    pub fn shared(self) -> Arc<dyn ScreenSource> {
        Arc::new(self)
    }
}

impl ScreenSource for FakeScreen {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        Ok(vec![MonitorInfo {
            id: 0,
            name: "FAKE1".into(),
            rect: self.desktop.rect(),
            dpi: 96,
            primary: true,
        }])
    }
    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
        Ok(self.windows.clone())
    }
    fn grab(&self, rect: Rect) -> Result<Raster, CaptureError> {
        if let Some(sc) = self.scroller.lock().unwrap().as_ref() {
            if rect == sc.view {
                let part = Rect::new(0, sc.offset, sc.view.w, sc.view.h);
                return sc.content.crop(part).ok_or(CaptureError::Empty);
            }
        }
        let r = rect
            .intersect(&self.desktop.rect())
            .ok_or(CaptureError::Empty)?;
        self.desktop.crop(r).ok_or(CaptureError::Empty)
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
        self.cursor.map(|pos| CursorInfo { pos })
    }
    fn foreground(&self) -> Option<WindowInfo> {
        self.windows.first().cloned()
    }
    fn scroll(&self, _at: Point, notches: i32) -> Result<(), CaptureError> {
        let mut g = self.scroller.lock().unwrap();
        let sc = g
            .as_mut()
            .ok_or_else(|| CaptureError::Platform("nothing scrolls here".into()))?;
        let max = sc.content.height() as i32 - sc.view.h as i32;
        sc.offset = (sc.offset + notches * sc.px_per_notch).clamp(0, max.max(0));
        Ok(())
    }
    fn object_at(&self, p: Point) -> Option<WindowInfo> {
        self.windows.iter().find(|w| w.rect.contains(p)).cloned()
    }
    fn menus(&self) -> Vec<WindowInfo> {
        self.windows
            .iter()
            .filter(|w| w.title.starts_with("Menu"))
            .cloned()
            .collect()
    }
}

/// In-memory clipboard that records what was set.
#[derive(Default)]
pub struct FakeClipboard {
    pub image: Mutex<Option<Raster>>,
    pub text: Mutex<Option<String>>,
}

impl screenit_engine::Clipboard for FakeClipboard {
    fn set_image(&self, img: &Raster) -> Result<(), String> {
        *self.image.lock().map_err(|e| e.to_string())? = Some(img.clone());
        Ok(())
    }
    fn get_image(&self) -> Result<Raster, String> {
        self.image
            .lock()
            .map_err(|e| e.to_string())?
            .clone()
            .ok_or_else(|| "clipboard has no image".to_string())
    }
    fn set_text(&self, text: &str) -> Result<(), String> {
        *self.text.lock().map_err(|e| e.to_string())? = Some(text.to_string());
        Ok(())
    }
    fn get_text(&self) -> Result<String, String> {
        self.text
            .lock()
            .map_err(|e| e.to_string())?
            .clone()
            .ok_or_else(|| "clipboard has no text".to_string())
    }
}

/// Recorder that produces no video, only a deterministic result; exercises the state machine.
#[derive(Default)]
pub struct FakeRecorder {
    cfg: Option<screenit_record::RecordConfig>,
}

impl screenit_record::Recorder for FakeRecorder {
    fn start(
        &mut self,
        cfg: &screenit_record::RecordConfig,
    ) -> Result<(), screenit_record::RecordError> {
        self.cfg = Some(cfg.clone());
        Ok(())
    }
    fn pause(&mut self) -> Result<(), screenit_record::RecordError> {
        Ok(())
    }
    fn resume(&mut self) -> Result<(), screenit_record::RecordError> {
        Ok(())
    }
    fn attach_webcam(
        &mut self,
        _cam: std::sync::Arc<dyn screenit_record::CameraSource>,
        _layout: screenit_record::PipLayout,
    ) -> Result<(), screenit_record::RecordError> {
        Ok(())
    }
    fn set_pip_layout(
        &mut self,
        _layout: screenit_record::PipLayout,
    ) -> Result<(), screenit_record::RecordError> {
        Ok(())
    }
    fn stop(&mut self) -> Result<screenit_record::RecordingResult, screenit_record::RecordError> {
        let cfg = self
            .cfg
            .take()
            .ok_or_else(|| screenit_record::RecordError::Platform("not started".into()))?;
        let (width, height) = cfg.encode_size();
        Ok(screenit_record::RecordingResult {
            has_audio: cfg.mic.is_some() || cfg.system_audio,
            path: cfg.output,
            duration_ms: 0,
            frames: 0,
            width,
            height,
        })
    }
}

/// A session wired with a fake screen of the given size, a fake clipboard and a fake recorder.
pub fn fake_session(w: u32, h: u32) -> (screenit_engine::Session, Arc<FakeClipboard>) {
    let clip = Arc::new(FakeClipboard::default());
    let services = screenit_engine::Services {
        screen: Some(
            FakeScreen::new(w, h)
                .with_window(1, "Fixture", Rect::new(10, 10, w / 2, h / 2))
                .shared(),
        ),
        clipboard: Some(clip.clone()),
        recorder: Some(Box::new(|| Box::new(FakeRecorder::default()))),
        video_writer: Some(std::sync::Arc::new(FakeVideoWriter)),
        camera: Some(std::sync::Arc::new(FakeCamera::new(
            Raster::new(64, 36, [200, 30, 30, 255]).expect("raster"),
        ))),
        devices: Some(std::sync::Arc::new(FakeDevices)),
        ..Default::default()
    };
    (screenit_engine::Session::new(services), clip)
}

/// Video writer fake: consumes the frames and reports how many it saw (no file is written).
pub struct FakeVideoWriter;

impl screenit_record::VideoWriter for FakeVideoWriter {
    fn write(
        &self,
        out: &std::path::Path,
        width: u32,
        height: u32,
        fps: u32,
        frames: &mut dyn Iterator<Item = Result<Raster, screenit_record::RecordError>>,
    ) -> Result<screenit_record::RecordingResult, screenit_record::RecordError> {
        let mut n = 0u64;
        for f in frames {
            let f = f?;
            if f.width() != width || f.height() != height {
                return Err(screenit_record::RecordError::Platform(
                    "frame size mismatch".into(),
                ));
            }
            n += 1;
        }
        Ok(screenit_record::RecordingResult {
            path: out.to_path_buf(),
            duration_ms: n * 1000 / u64::from(fps.max(1)),
            frames: n,
            width,
            height,
            has_audio: false,
        })
    }
}

/// Camera fake producing a constant-colour frame.
pub struct FakeCamera {
    pub image: Raster,
    /// How many times the device was released.
    pub stops: std::sync::atomic::AtomicUsize,
}

impl FakeCamera {
    pub fn new(image: Raster) -> Self {
        Self {
            image,
            stops: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl screenit_record::CameraSource for FakeCamera {
    fn name(&self) -> String {
        "Fake Camera".into()
    }
    fn frame(&self) -> Option<Raster> {
        Some(self.image.clone())
    }
    fn stop(&self) {
        self.stops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Fixed device lists.
pub struct FakeDevices;

impl screenit_record::AvDevices for FakeDevices {
    fn microphones(&self) -> Vec<String> {
        vec!["Fake Microphone".into()]
    }
    fn cameras(&self) -> Vec<String> {
        vec!["Fake Camera".into()]
    }
}

/// Language-model OCR fake: replies with fixed text, or fails when None.
pub struct FakeLlm(pub Option<String>);

impl screenit_capture::LlmOcr for FakeLlm {
    fn transcribe(
        &self,
        _: &Raster,
        _: &screenit_capture::LlmConfig,
    ) -> Result<String, CaptureError> {
        self.0
            .clone()
            .ok_or_else(|| CaptureError::Platform("llm down".into()))
    }
    fn models(&self, _: &screenit_capture::LlmConfig) -> Result<Vec<String>, CaptureError> {
        self.0
            .as_ref()
            .map(|_| vec!["fake-model".into()])
            .ok_or_else(|| CaptureError::Platform("llm down".into()))
    }
}

/// OCR fake that returns fixed lines regardless of the image.
pub struct FakeOcr(pub Vec<String>);

impl screenit_capture::Ocr for FakeOcr {
    fn recognize(&self, image: &Raster) -> Result<Vec<screenit_capture::OcrLine>, CaptureError> {
        Ok(self
            .0
            .iter()
            .enumerate()
            .map(|(i, t)| screenit_capture::OcrLine {
                text: t.clone(),
                rect: Rect::new(0, i as i32 * 10, image.width(), 10),
            })
            .collect())
    }
}
