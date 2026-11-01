//! Recording model: session state machine, PiP layout, cursor effects, screen-draw fading,
//! edit lists (trim/cut/combine) and the platform traits. No OS code and no encoder here.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod audio;

use screenit_geom::{Point, PointF, Rect, RectF};
use screenit_raster::Raster;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RecordError {
    #[error("invalid transition from {from:?}: {action}")]
    BadTransition {
        from: RecState,
        action: &'static str,
    },
    #[error("invalid config: {0}")]
    BadConfig(String),
    #[error("invalid edit: {0}")]
    BadEdit(String),
    #[error("platform recording failed: {0}")]
    Platform(String),
}

// ------------------------------------------------------------------ session state machine

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecState {
    Idle,
    Countdown { remaining_ms: u32 },
    Recording,
    Paused,
    Stopped,
}

/// Deterministic session clock: callers pass timestamps, so tests need no real time.
#[derive(Clone, Debug)]
pub struct Session {
    state: RecState,
    started_at: Option<u64>,
    paused_total: u64,
    paused_at: Option<u64>,
    stopped_at: Option<u64>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self {
            state: RecState::Idle,
            started_at: None,
            paused_total: 0,
            paused_at: None,
            stopped_at: None,
        }
    }
    pub fn state(&self) -> RecState {
        self.state
    }

    pub fn start(&mut self, now_ms: u64, countdown_ms: u32) -> Result<(), RecordError> {
        if self.state != RecState::Idle {
            return Err(RecordError::BadTransition {
                from: self.state,
                action: "start",
            });
        }
        if countdown_ms > 0 {
            self.state = RecState::Countdown {
                remaining_ms: countdown_ms,
            };
        } else {
            self.state = RecState::Recording;
            self.started_at = Some(now_ms);
        }
        Ok(())
    }

    /// Advance the countdown; begins recording when it reaches zero.
    pub fn tick_countdown(&mut self, now_ms: u64, elapsed_ms: u32) -> Result<(), RecordError> {
        match self.state {
            RecState::Countdown { remaining_ms } => {
                if elapsed_ms >= remaining_ms {
                    self.state = RecState::Recording;
                    self.started_at = Some(now_ms);
                } else {
                    self.state = RecState::Countdown {
                        remaining_ms: remaining_ms - elapsed_ms,
                    };
                }
                Ok(())
            }
            s => Err(RecordError::BadTransition {
                from: s,
                action: "tick_countdown",
            }),
        }
    }

    pub fn pause(&mut self, now_ms: u64) -> Result<(), RecordError> {
        if self.state != RecState::Recording {
            return Err(RecordError::BadTransition {
                from: self.state,
                action: "pause",
            });
        }
        self.state = RecState::Paused;
        self.paused_at = Some(now_ms);
        Ok(())
    }

    pub fn resume(&mut self, now_ms: u64) -> Result<(), RecordError> {
        if self.state != RecState::Paused {
            return Err(RecordError::BadTransition {
                from: self.state,
                action: "resume",
            });
        }
        if let Some(p) = self.paused_at.take() {
            self.paused_total += now_ms.saturating_sub(p);
        }
        self.state = RecState::Recording;
        Ok(())
    }

    pub fn stop(&mut self, now_ms: u64) -> Result<(), RecordError> {
        match self.state {
            RecState::Recording | RecState::Paused => {
                if let Some(p) = self.paused_at.take() {
                    self.paused_total += now_ms.saturating_sub(p);
                }
                self.stopped_at = Some(now_ms);
                self.state = RecState::Stopped;
                Ok(())
            }
            RecState::Countdown { .. } => {
                self.state = RecState::Idle;
                Ok(())
            }
            s => Err(RecordError::BadTransition {
                from: s,
                action: "stop",
            }),
        }
    }

    /// Recorded duration excluding paused time.
    pub fn elapsed_ms(&self, now_ms: u64) -> u64 {
        let Some(start) = self.started_at else {
            return 0;
        };
        let end = match self.state {
            RecState::Stopped => self.stopped_at.unwrap_or(now_ms),
            RecState::Paused => self.paused_at.unwrap_or(now_ms),
            _ => now_ms,
        };
        end.saturating_sub(start).saturating_sub(self.paused_total)
    }
}

// ------------------------------------------------------------------ config and platform traits

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    H264,
    H265,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordConfig {
    pub region: Rect,
    pub window: Option<u64>,
    pub fps: u32,
    pub codec: Codec,
    pub mic: Option<String>,
    pub system_audio: bool,
    pub cursor_highlight: bool,
    pub click_ripples: bool,
    pub output: PathBuf,
}

impl RecordConfig {
    pub fn validate(&self) -> Result<(), RecordError> {
        let bad = |m: &str| Err(RecordError::BadConfig(m.to_string()));
        if self.region.is_empty() || self.region.w > 16_384 || self.region.h > 16_384 {
            return bad("region must be 1..=16384 on each side");
        }
        // H.264/H.265 encoders need even dimensions; the platform crops by one pixel if odd.
        if !(1..=120).contains(&self.fps) {
            return bad("fps must be 1..=120");
        }
        if self.output.as_os_str().is_empty()
            || self
                .output
                .extension()
                .is_none_or(|e| !e.eq_ignore_ascii_case("mp4"))
        {
            return bad("output must be an .mp4 path");
        }
        Ok(())
    }
    /// Encoder-friendly (even) frame size.
    pub fn encode_size(&self) -> (u32, u32) {
        (self.region.w & !1, self.region.h & !1)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingResult {
    pub path: PathBuf,
    pub duration_ms: u64,
    pub frames: u64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
}

/// Implemented by `screenit-platform` (Media Foundation) and by test fakes.
pub trait Recorder: Send {
    fn start(&mut self, cfg: &RecordConfig) -> Result<(), RecordError>;
    fn pause(&mut self) -> Result<(), RecordError>;
    fn resume(&mut self) -> Result<(), RecordError>;
    fn stop(&mut self) -> Result<RecordingResult, RecordError>;
    /// Composite a webcam into the recording. Optional capability.
    fn attach_webcam(
        &mut self,
        _cam: std::sync::Arc<dyn CameraSource>,
        _layout: PipLayout,
    ) -> Result<(), RecordError> {
        Err(RecordError::BadConfig(
            "this recorder cannot composite a webcam".into(),
        ))
    }
    /// Change the picture-in-picture layout (size, shape, corner, swap) mid-recording.
    fn set_pip_layout(&mut self, _layout: PipLayout) -> Result<(), RecordError> {
        Err(RecordError::BadConfig("no webcam attached".into()))
    }
}

/// A live camera. `frame` returns the most recent frame, if any has arrived; real cameras open
/// the device lazily on the first call.
pub trait CameraSource: Send + Sync {
    fn name(&self) -> String;
    fn frame(&self) -> Option<Raster>;
    /// Release the device (camera light off) until the next `frame` call.
    fn stop(&self) {}
}

/// Capture devices present on this machine, by friendly name.
pub trait AvDevices: Send + Sync {
    fn microphones(&self) -> Vec<String>;
    fn cameras(&self) -> Vec<String>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoInfo {
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
}

/// Decoding side, used by the video editor (frame grab, GIF export, thumbnails).
pub trait VideoReader: Send + Sync {
    fn probe(&self, path: &std::path::Path) -> Result<VideoInfo, RecordError>;
    fn frame_at(&self, path: &std::path::Path, ms: u64) -> Result<Raster, RecordError>;
}

/// Encodes a sequence of frames into an MP4 (video editing output).
pub trait VideoWriter: Send + Sync {
    fn write(
        &self,
        out: &std::path::Path,
        width: u32,
        height: u32,
        fps: u32,
        frames: &mut dyn Iterator<Item = Result<Raster, RecordError>>,
    ) -> Result<RecordingResult, RecordError>;
}

// ------------------------------------------------------------------ picture-in-picture

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipShape {
    Rectangle,
    RoundedRectangle,
    Circle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipLayout {
    pub shape: PipShape,
    /// Webcam width as a percentage of the screen width (5..=100).
    pub size_percent: u32,
    pub corner: Corner,
    pub margin: u32,
    /// When true, the webcam fills the frame and the screen becomes the inset.
    pub swapped: bool,
}

impl Default for PipLayout {
    fn default() -> Self {
        Self {
            shape: PipShape::RoundedRectangle,
            size_percent: 20,
            corner: Corner::BottomRight,
            margin: 16,
            swapped: false,
        }
    }
}

/// Webcam rectangle within a frame of `w`x`h`; `cam_aspect` is camera width/height.
pub fn pip_rect(w: u32, h: u32, cam_aspect: f32, l: &PipLayout) -> Rect {
    if l.swapped {
        return Rect::new(0, 0, w, h);
    }
    let aspect = if cam_aspect.is_finite() && cam_aspect > 0.1 {
        cam_aspect
    } else {
        16.0 / 9.0
    };
    let pw = (u64::from(w) * u64::from(l.size_percent.clamp(5, 100)) / 100).max(1) as u32;
    let ph = if l.shape == PipShape::Circle {
        pw
    } else {
        ((pw as f32 / aspect) as u32).max(1)
    };
    let (pw, ph) = (pw.min(w), ph.min(h));
    let m = l.margin.min(w.saturating_sub(pw)).min(h.saturating_sub(ph)) as i32;
    let (rx, ry) = ((w - pw) as i32, (h - ph) as i32);
    match l.corner {
        Corner::TopLeft => Rect::new(m, m, pw, ph),
        Corner::TopRight => Rect::new(rx - m, m, pw, ph),
        Corner::BottomLeft => Rect::new(m, ry - m, pw, ph),
        Corner::BottomRight => Rect::new(rx - m, ry - m, pw, ph),
    }
}

/// Inset rectangle of the screen when swapped (webcam full frame).
pub fn swapped_screen_rect(w: u32, h: u32, l: &PipLayout) -> Rect {
    pip_rect(
        w,
        h,
        w as f32 / h.max(1) as f32,
        &PipLayout {
            swapped: false,
            ..*l
        },
    )
}

/// Nearest-neighbour resize (webcam previews do not need filtering quality).
fn scale_to(src: &Raster, w: u32, h: u32) -> Raster {
    let mut out = Raster::new(w.max(1), h.max(1), [0, 0, 0, 255]).unwrap_or_else(|_| src.clone());
    let (sw, sh) = (src.width().max(1), src.height().max(1));
    for y in 0..out.height() {
        for x in 0..out.width() {
            let px = src.get(
                (x * sw / out.width()) as i32,
                (y * sh / out.height()) as i32,
            );
            out.put(x as i32, y as i32, px.unwrap_or([0, 0, 0, 255]));
        }
    }
    out
}

/// Whether pixel (x,y) of a `w`x`h` inset is inside the shape.
fn in_shape(shape: PipShape, x: u32, y: u32, w: u32, h: u32) -> bool {
    match shape {
        PipShape::Rectangle => true,
        PipShape::Circle => {
            let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            dx * dx + dy * dy <= cx.min(cy) * cx.min(cy)
        }
        PipShape::RoundedRectangle => {
            let r = (w.min(h) / 8).max(1) as f32;
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let nx = fx.clamp(r, w as f32 - r);
            let ny = fy.clamp(r, h as f32 - r);
            (fx - nx).powi(2) + (fy - ny).powi(2) <= r * r
        }
    }
}

/// Composite `cam` onto `screen` per `layout` (PiP inset, or full-frame webcam with the screen inset
/// when `layout.swapped`). Returns a new frame; inputs are untouched.
pub fn composite_webcam(screen: &Raster, cam: &Raster, layout: &PipLayout) -> Raster {
    let (w, h) = (screen.width(), screen.height());
    let aspect = cam.width() as f32 / cam.height().max(1) as f32;
    let (base, inset_src, rect, shape) = if layout.swapped {
        let r = swapped_screen_rect(w, h, layout);
        (scale_to(cam, w, h), screen, r, layout.shape)
    } else {
        (
            screen.clone(),
            cam,
            pip_rect(w, h, aspect, layout),
            layout.shape,
        )
    };
    let mut out = base;
    let inset = scale_to(inset_src, rect.w, rect.h);
    for y in 0..inset.height() {
        for x in 0..inset.width() {
            if in_shape(shape, x, y, inset.width(), inset.height()) {
                if let Some(px) = inset.get(x as i32, y as i32) {
                    out.put(rect.x + x as i32, rect.y + y as i32, px);
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------ cursor effects

/// Radius (px) and alpha (0..=255) of a click ripple `age_ms` after the click.
pub fn ripple_state(age_ms: u64, duration_ms: u64, max_radius: f32) -> Option<(f32, u8)> {
    if duration_ms == 0 || age_ms >= duration_ms {
        return None;
    }
    let t = age_ms as f32 / duration_ms as f32;
    Some((max_radius * t, ((1.0 - t) * 200.0) as u8))
}

/// Draw the cursor highlight disc and any active click ripples onto a recorded frame.
pub fn draw_cursor_effects(
    frame: &mut Raster,
    cursor: Point,
    highlight: bool,
    ripples: &[(Point, u64)],
    now_ms: u64,
) {
    let disc = |f: &mut Raster, c: Point, radius: f32, rgba: [u8; 4], ring: bool| {
        let r = radius.ceil() as i32 + 1;
        for dy in -r..=r {
            for dx in -r..=r {
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                let v = if ring {
                    (1.5 - (d - radius).abs()).clamp(0.0, 1.0)
                } else {
                    (radius + 0.5 - d).clamp(0.0, 1.0)
                };
                if v > 0.0 {
                    f.blend(c.x + dx, c.y + dy, rgba, (v * 255.0) as u8);
                }
            }
        }
    };
    if highlight {
        disc(frame, cursor, 28.0, [255, 235, 0, 90], false);
    }
    for (pos, t0) in ripples {
        if let Some((radius, alpha)) = ripple_state(now_ms.saturating_sub(*t0), 600, 40.0) {
            disc(frame, *pos, radius, [255, 60, 60, alpha], true);
        }
    }
}

/// Screen Draw objects fade out `fade_ms` after creation (Snagit default 3 s).
pub fn fade_alpha(age_ms: u64, fade_ms: u64) -> u8 {
    if fade_ms == 0 || age_ms >= fade_ms {
        return 0;
    }
    let remaining = fade_ms - age_ms;
    let tail = (fade_ms / 4).max(1);
    if remaining >= tail {
        255
    } else {
        (remaining * 255 / tail) as u8
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScreenDrawItem {
    pub from: PointF,
    pub to: PointF,
    pub kind: DrawKind,
    pub created_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawKind {
    Arrow,
    Square,
    Step(u32),
}

#[derive(Clone, Debug, Default)]
pub struct ScreenDraw {
    pub items: Vec<ScreenDrawItem>,
    pub fade_ms: u64,
    next_step: u32,
}

impl ScreenDraw {
    pub fn new(fade_ms: u64) -> Self {
        Self {
            items: Vec::new(),
            fade_ms,
            next_step: 1,
        }
    }
    pub fn add_arrow(&mut self, from: PointF, to: PointF, now: u64) {
        self.items.push(ScreenDrawItem {
            from,
            to,
            kind: DrawKind::Arrow,
            created_ms: now,
        });
    }
    pub fn add_square(&mut self, from: PointF, to: PointF, now: u64) {
        self.items.push(ScreenDrawItem {
            from,
            to,
            kind: DrawKind::Square,
            created_ms: now,
        });
    }
    pub fn add_step(&mut self, at: PointF, now: u64) {
        let n = self.next_step.max(1);
        self.next_step = n + 1;
        self.items.push(ScreenDrawItem {
            from: at,
            to: at,
            kind: DrawKind::Step(n),
            created_ms: now,
        });
    }
    /// Drop expired items; returns the items still visible with their alpha.
    pub fn visible(&mut self, now: u64) -> Vec<(ScreenDrawItem, u8)> {
        let fade = self.fade_ms;
        self.items
            .retain(|i| fade_alpha(now.saturating_sub(i.created_ms), fade) > 0);
        self.items
            .iter()
            .map(|i| {
                (
                    i.clone(),
                    fade_alpha(now.saturating_sub(i.created_ms), fade),
                )
            })
            .collect()
    }
    pub fn bounds(&self) -> Option<RectF> {
        self.items
            .iter()
            .map(|i| {
                RectF::new(
                    i.from.x.min(i.to.x),
                    i.from.y.min(i.to.y),
                    (i.from.x - i.to.x).abs(),
                    (i.from.y - i.to.y).abs(),
                )
            })
            .reduce(|a, b| {
                let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
                let (x1, y1) = ((a.x + a.w).max(b.x + b.w), (a.y + a.h).max(b.y + b.h));
                RectF::new(x0, y0, x1 - x0, y1 - y0)
            })
    }
}

// ------------------------------------------------------------------ edit list

/// A piece of a source clip kept in the output, in source milliseconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub clip: usize,
    pub start_ms: u64,
    pub end_ms: u64,
}

impl Segment {
    pub fn len(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditList {
    pub segments: Vec<Segment>,
}

impl EditList {
    pub fn from_clip(clip: usize, duration_ms: u64) -> Self {
        Self {
            segments: vec![Segment {
                clip,
                start_ms: 0,
                end_ms: duration_ms,
            }],
        }
    }

    pub fn total_ms(&self) -> u64 {
        self.segments.iter().map(Segment::len).sum()
    }

    /// Keep only output range [start, end).
    pub fn trim(&mut self, start: u64, end: u64) -> Result<(), RecordError> {
        let total = self.total_ms();
        if start >= end || end > total {
            return Err(RecordError::BadEdit(format!(
                "trim {start}..{end} outside 0..{total}"
            )));
        }
        self.remove_range(end, total)?;
        if start > 0 {
            self.remove_range(0, start)?;
        }
        Ok(())
    }

    /// Remove output range [from, to) and join the remainder.
    pub fn cut(&mut self, from: u64, to: u64) -> Result<(), RecordError> {
        let total = self.total_ms();
        if from >= to || to > total {
            return Err(RecordError::BadEdit(format!(
                "cut {from}..{to} outside 0..{total}"
            )));
        }
        if to - from == total {
            return Err(RecordError::BadEdit("cannot cut the whole video".into()));
        }
        self.remove_range(from, to)
    }

    fn remove_range(&mut self, from: u64, to: u64) -> Result<(), RecordError> {
        let mut out = Vec::new();
        let mut pos = 0u64;
        for s in &self.segments {
            let (a, b) = (pos, pos + s.len());
            pos = b;
            if b <= from || a >= to {
                out.push(s.clone());
                continue;
            }
            if a < from {
                out.push(Segment {
                    clip: s.clip,
                    start_ms: s.start_ms,
                    end_ms: s.start_ms + (from - a),
                });
            }
            if b > to {
                out.push(Segment {
                    clip: s.clip,
                    start_ms: s.start_ms + (to - a),
                    end_ms: s.end_ms,
                });
            }
        }
        self.segments = out;
        Ok(())
    }

    pub fn combine(&mut self, other: &EditList) {
        self.segments.extend(other.segments.iter().cloned());
    }

    /// Source (clip, ms) for an output time.
    pub fn map_time(&self, out_ms: u64) -> Option<(usize, u64)> {
        let mut pos = 0u64;
        for s in &self.segments {
            if out_ms < pos + s.len() {
                return Some((s.clip, s.start_ms + (out_ms - pos)));
            }
            pos += s.len();
        }
        None
    }
}

/// Frame timestamps (ms) for GIF export at `fps`.
pub fn gif_frame_times(duration_ms: u64, fps: u32) -> Vec<u64> {
    let fps = u64::from(fps.clamp(1, 30));
    let n = (duration_ms * fps / 1000).clamp(1, 10_000);
    (0..n).map(|i| i * 1000 / fps).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_lifecycle_excludes_paused_time() {
        let mut s = Session::new();
        s.start(1000, 0).unwrap();
        s.pause(2000).unwrap();
        s.resume(5000).unwrap();
        assert_eq!(s.elapsed_ms(6000), 2000);
        s.stop(7000).unwrap();
        assert_eq!(s.elapsed_ms(99_999), 3000);
        assert_eq!(s.state(), RecState::Stopped);
    }

    #[test]
    fn session_rejects_bad_transitions() {
        let mut s = Session::new();
        assert!(s.pause(0).is_err());
        assert!(s.resume(0).is_err());
        assert!(s.stop(0).is_err());
        s.start(0, 3000).unwrap();
        assert!(s.start(0, 0).is_err());
        s.tick_countdown(1000, 1000).unwrap();
        assert_eq!(s.state(), RecState::Countdown { remaining_ms: 2000 });
        s.tick_countdown(3000, 2500).unwrap();
        assert_eq!(s.state(), RecState::Recording);
        assert_eq!(s.elapsed_ms(4000), 1000);
        assert!(s.tick_countdown(0, 1).is_err());
    }

    #[test]
    fn cancel_countdown_returns_to_idle() {
        let mut s = Session::new();
        s.start(0, 3000).unwrap();
        s.stop(10).unwrap();
        assert_eq!(s.state(), RecState::Idle);
    }

    fn cfg() -> RecordConfig {
        RecordConfig {
            region: Rect::new(0, 0, 801, 601),
            window: None,
            fps: 30,
            codec: Codec::H264,
            mic: None,
            system_audio: false,
            cursor_highlight: false,
            click_ripples: false,
            output: "out.mp4".into(),
        }
    }

    #[test]
    fn config_validation() {
        assert!(cfg().validate().is_ok());
        assert_eq!(cfg().encode_size(), (800, 600));
        let mut c = cfg();
        c.fps = 0;
        assert!(c.validate().is_err());
        let mut c = cfg();
        c.region = Rect::new(0, 0, 0, 5);
        assert!(c.validate().is_err());
        let mut c = cfg();
        c.output = "a.avi".into();
        assert!(c.validate().is_err());
        let mut c = cfg();
        c.output = "".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn pip_corners_and_swap() {
        let l = PipLayout {
            size_percent: 25,
            margin: 10,
            ..Default::default()
        };
        let r = pip_rect(1000, 600, 16.0 / 9.0, &l);
        assert_eq!(r.w, 250);
        assert_eq!(r.right(), 990);
        assert_eq!(r.bottom(), 590);
        let tl = pip_rect(
            1000,
            600,
            1.0,
            &PipLayout {
                corner: Corner::TopLeft,
                ..l
            },
        );
        assert_eq!((tl.x, tl.y), (10, 10));
        let c = pip_rect(
            1000,
            600,
            1.0,
            &PipLayout {
                shape: PipShape::Circle,
                ..l
            },
        );
        assert_eq!(c.w, c.h);
        assert_eq!(
            pip_rect(1000, 600, 1.0, &PipLayout { swapped: true, ..l }),
            Rect::new(0, 0, 1000, 600)
        );
        let inset = swapped_screen_rect(1000, 600, &PipLayout { swapped: true, ..l });
        assert_eq!(inset.w, 250);
        let _ = pip_rect(0, 0, f32::NAN, &l);
        let _ = pip_rect(
            10,
            10,
            0.0,
            &PipLayout {
                size_percent: 0,
                margin: u32::MAX,
                ..l
            },
        );
    }

    #[test]
    fn ripples_expand_and_fade() {
        let (r0, a0) = ripple_state(0, 600, 40.0).unwrap();
        let (r1, a1) = ripple_state(300, 600, 40.0).unwrap();
        assert!(r1 > r0 && a1 < a0);
        assert!(ripple_state(600, 600, 40.0).is_none());
        assert!(ripple_state(5, 0, 40.0).is_none());
        let mut f = Raster::new(100, 100, [255; 4]).unwrap();
        draw_cursor_effects(
            &mut f,
            Point::new(50, 50),
            true,
            &[(Point::new(50, 50), 0)],
            300,
        );
        assert_ne!(f.get(50, 50), Some([255; 4]));
        draw_cursor_effects(&mut f, Point::new(-1000, 9999), true, &[], 0);
    }

    #[test]
    fn screen_draw_fades_after_three_seconds() {
        let mut d = ScreenDraw::new(3000);
        d.add_arrow(PointF::new(0.0, 0.0), PointF::new(10.0, 10.0), 0);
        d.add_step(PointF::new(5.0, 5.0), 1000);
        d.add_step(PointF::new(6.0, 6.0), 1000);
        assert_eq!(d.visible(500).len(), 3);
        assert_eq!(d.visible(2000).len(), 3);
        assert_eq!(d.visible(3500).len(), 2);
        assert_eq!(d.visible(4000).len(), 0);
        assert_eq!(fade_alpha(0, 3000), 255);
        assert!(fade_alpha(2900, 3000) < 255);
        assert_eq!(fade_alpha(10, 0), 0);
    }

    #[test]
    fn edit_list_trim_cut_combine() {
        let mut e = EditList::from_clip(0, 10_000);
        e.cut(2000, 4000).unwrap();
        assert_eq!(e.total_ms(), 8000);
        assert_eq!(e.map_time(1999), Some((0, 1999)));
        assert_eq!(e.map_time(2000), Some((0, 4000)));
        e.trim(1000, 7000).unwrap();
        assert_eq!(e.total_ms(), 6000);
        assert_eq!(e.map_time(0), Some((0, 1000)));
        let mut other = EditList::from_clip(1, 3000);
        other.trim(500, 1500).unwrap();
        e.combine(&other);
        assert_eq!(e.total_ms(), 7000);
        assert_eq!(e.map_time(6000), Some((1, 500)));
        assert_eq!(e.map_time(7000), None);
        assert!(e.cut(5, 5).is_err());
        assert!(e.trim(0, 99_999).is_err());
        assert!(EditList::from_clip(0, 100).cut(0, 100).is_err());
    }

    #[test]
    fn gif_times_are_bounded() {
        assert_eq!(gif_frame_times(1000, 10).len(), 10);
        assert_eq!(gif_frame_times(0, 10), vec![0]);
        assert!(gif_frame_times(u64::MAX / 2000, 30).len() <= 10_000);
    }
}

#[cfg(test)]
mod webcam_tests {
    use super::*;

    fn solid(w: u32, h: u32, px: [u8; 4]) -> Raster {
        Raster::new(w, h, px).unwrap()
    }

    #[test]
    fn pip_inset_lands_in_the_corner_and_shape_masks_corners() {
        let screen = solid(200, 100, [0, 0, 255, 255]);
        let cam = solid(64, 36, [255, 0, 0, 255]);
        let l = PipLayout {
            shape: PipShape::Circle,
            size_percent: 20,
            corner: Corner::TopLeft,
            margin: 10,
            swapped: false,
        };
        let out = composite_webcam(&screen, &cam, &l);
        assert_eq!(
            out.get(30, 30),
            Some([255, 0, 0, 255]),
            "circle centre is webcam"
        );
        assert_eq!(
            out.get(10, 10),
            Some([0, 0, 255, 255]),
            "circle corner is screen"
        );
        assert_eq!(out.get(190, 90), Some([0, 0, 255, 255]));
        assert_eq!(
            screen.get(30, 30),
            Some([0, 0, 255, 255]),
            "input untouched"
        );
    }

    #[test]
    fn swapped_layout_makes_the_webcam_full_frame() {
        let screen = solid(200, 100, [0, 0, 255, 255]);
        let cam = solid(64, 36, [255, 0, 0, 255]);
        let l = PipLayout {
            shape: PipShape::Rectangle,
            swapped: true,
            corner: Corner::BottomRight,
            margin: 0,
            size_percent: 25,
        };
        let out = composite_webcam(&screen, &cam, &l);
        assert_eq!(out.get(5, 5), Some([255, 0, 0, 255]));
        assert_eq!(
            out.get(195, 95),
            Some([0, 0, 255, 255]),
            "screen is the inset"
        );
    }
}
