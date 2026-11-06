//! Renders a [`PolishPlan`] over a recording into an MP4: cuts and speeds the picture and sound,
//! applies the zoom view, draws the pointer style, click rings and shortcut badges, then lays the
//! result on a padded, rounded canvas.

use crate::tutor_render::draw_ring;
use screenit_doc::Color;
use screenit_geom::{PointF, Rect};
use screenit_raster::Raster;
use screenit_record::polish::{CursorStyle, PolishPlan, fit_aspect};
use screenit_record::{RecordError, RecordingResult, VideoReader, VideoWriter};
use screenit_vector::{draw_text_box, layout_text};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const BACKGROUND: [u8; 4] = [24, 28, 40, 255];

fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

fn plat(e: impl std::fmt::Display) -> RecordError {
    RecordError::Platform(e.to_string())
}

/// Everything one render needs.
pub struct PolishJob<'a> {
    pub reader: &'a dyn VideoReader,
    pub writer: &'a dyn VideoWriter,
    pub video: &'a Path,
    pub plan: &'a PolishPlan,
    pub fps: u32,
    pub out: &'a Path,
    pub abort: &'a AtomicBool,
    /// Reduce background noise in the recording's own sound.
    pub denoise: bool,
    /// Spoken clips (output start ms, 48 kHz stereo samples) mixed over the sound.
    pub narration: &'a [(u64, Vec<i16>)],
    /// (frames done, frames total).
    pub on_frame: &'a dyn Fn(u32, u32),
}

/// Size of the finished frames for a recording of `src_w` x `src_h`.
pub fn output_size(plan: &PolishPlan, src_w: u32, src_h: u32) -> (u32, u32) {
    let full = Rect::new(0, 0, src_w.max(2), src_h.max(2));
    match plan.opts.aspect.ratio() {
        None => (even(src_w), even(src_h)),
        Some(r) => {
            let big = fit_aspect(full, r, full);
            (even(big.w), even(big.h))
        }
    }
}

/// Space around the picture and corner radius, in pixels.
fn frame_metrics(plan: &PolishPlan, w: u32, h: u32) -> (u32, u32) {
    let short = w.min(h);
    let pad = short * u32::from(plan.opts.padding_pct.min(25)) / 100;
    let inner_short = short.saturating_sub(pad * 2).max(2);
    let radius = inner_short * u32::from(plan.opts.corner_pct.min(25)) / 100;
    (pad, radius)
}

/// The part `view` of `frame` (in recording pixels, `video` is the recorded area) at `w` x `h`.
fn crop_view(
    frame: Raster,
    video: Rect,
    view: Rect,
    w: u32,
    h: u32,
) -> Result<Raster, RecordError> {
    let sx = f64::from(frame.width()) / f64::from(video.w.max(1));
    let sy = f64::from(frame.height()) / f64::from(video.h.max(1));
    let src = if view == video {
        frame
    } else {
        let v = Rect::new(
            (f64::from(view.x) * sx).round() as i32,
            (f64::from(view.y) * sy).round() as i32,
            ((f64::from(view.w) * sx).round() as u32).max(2),
            ((f64::from(view.h) * sy).round() as u32).max(2),
        );
        frame
            .crop(v)
            .ok_or_else(|| plat("zoom view is outside the frame"))?
    };
    if src.width() == w && src.height() == h {
        Ok(src)
    } else {
        screenit_algo::resize(&src, w, h).map_err(plat)
    }
}

/// A rounded keycap-style label for a shortcut, top centre.
pub fn draw_badge(r: &mut Raster, text: &str) {
    let (w, h) = (r.width(), r.height());
    let size = (h as f32 / 22.0).max(12.0);
    let lay = layout_text(
        &text.replace('+', " + ").to_uppercase(),
        size,
        w as f32 * 0.8,
    );
    let pad = size * 0.7;
    let bw = (lay.width + pad * 2.0) as i32;
    let bh = (lay.line_height + pad) as i32;
    let x0 = ((w as i32 - bw) / 2).max(0);
    let y0 = (h as f32 * 0.05) as i32;
    for y in y0..(y0 + bh).min(h as i32) {
        for x in x0..(x0 + bw).min(w as i32) {
            r.blend(x, y, [20, 24, 36, 255], 215);
        }
    }
    draw_text_box(
        r,
        &text.replace('+', " + ").to_uppercase(),
        PointF::new(x0 as f32 + pad, y0 as f32 + pad * 0.5),
        size,
        w as f32 * 0.8,
        Color::WHITE,
    );
}

/// A subtitle: wrapped white text on a dark bar near the bottom edge.
pub fn draw_caption(r: &mut Raster, text: &str) {
    let (w, h) = (r.width(), r.height());
    let size = (h as f32 / 24.0).max(12.0);
    let wrap = w as f32 * 0.86;
    let lay = layout_text(text, size, wrap);
    let pad = size * 0.5;
    let bw = (lay.width + pad * 2.0) as i32;
    let bh = (lay.height() + pad) as i32;
    let x0 = ((w as i32 - bw) / 2).max(0);
    let y0 = (h as i32 - bh - (h as f32 * 0.05) as i32).max(0);
    for y in y0..(y0 + bh).min(h as i32) {
        for x in x0..(x0 + bw).min(w as i32) {
            r.blend(x, y, [10, 12, 20, 255], 200);
        }
    }
    draw_text_box(
        r,
        text,
        PointF::new(x0 as f32 + pad, y0 as f32 + pad * 0.5),
        size,
        wrap,
        Color::WHITE,
    );
}

fn disc(r: &mut Raster, cx: i32, cy: i32, radius: f32, px: [u8; 4], alpha: f32) {
    let reach = radius.ceil() as i32 + 1;
    for y in cy - reach..=cy + reach {
        for x in cx - reach..=cx + reach {
            let d = (((x - cx).pow(2) + (y - cy).pow(2)) as f32).sqrt();
            let cov = (radius - d).clamp(0.0, 1.0) * alpha;
            if cov > 0.0 {
                r.blend(x, y, px, (cov * 255.0) as u8);
            }
        }
    }
}

/// Lay `inner` on a background canvas with `pad` pixels around it and rounded corners.
fn on_canvas(inner: &Raster, w: u32, h: u32, pad: u32, radius: u32) -> Result<Raster, RecordError> {
    if pad == 0 && radius == 0 {
        return Ok(inner.clone());
    }
    let mut canvas = Raster::new(w, h, BACKGROUND).map_err(plat)?;
    let (px, py) = (pad as i32, pad as i32);
    // soft drop shadow: a darker, offset copy of the footprint
    let off = (pad / 4).max(2) as i32;
    canvas.fill_rect(
        Rect::new(px + off, py + off, inner.width(), inner.height()),
        [8, 10, 16, 255],
    );
    canvas.copy_from(inner, px, py);
    let rad = radius.min(inner.width() / 2).min(inner.height() / 2) as i32;
    for (cx, cy, sx, sy) in [
        (px + rad, py + rad, -1, -1),
        (px + inner.width() as i32 - rad - 1, py + rad, 1, -1),
        (px + rad, py + inner.height() as i32 - rad - 1, -1, 1),
        (
            px + inner.width() as i32 - rad - 1,
            py + inner.height() as i32 - rad - 1,
            1,
            1,
        ),
    ] {
        for dy in 0..=rad {
            for dx in 0..=rad {
                let (x, y) = (cx + sx * dx, cy + sy * dy);
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                let outside = (d - rad as f32 + 0.5).clamp(0.0, 1.0);
                if outside > 0.0 {
                    canvas.blend(x, y, BACKGROUND, (outside * 255.0) as u8);
                }
            }
        }
    }
    Ok(canvas)
}

/// One finished frame at output time `t`.
fn frame_at_time(
    job: &PolishJob<'_>,
    t: u64,
    size: (u32, u32),
    metrics: (u32, u32),
) -> Result<Raster, RecordError> {
    let plan = job.plan;
    let (w, h) = size;
    let (pad, radius) = metrics;
    let (iw, ih) = (
        even(w.saturating_sub(pad * 2)),
        even(h.saturating_sub(pad * 2)),
    );
    let src = plan
        .out_to_src(t)
        .ok_or_else(|| plat("time outside the plan"))?;
    let frame = job.reader.frame_at(job.video, src)?;
    let view = plan.view(t);
    let mut img = crop_view(frame, plan.video, view, iw, ih)?;
    let map = |x: i32, y: i32| {
        (
            (f64::from(x - view.x) * f64::from(iw) / f64::from(view.w.max(1))) as i32,
            (f64::from(y - view.y) * f64::from(ih) / f64::from(view.h.max(1))) as i32,
        )
    };
    let scale = iw as f32 / 1280.0;
    match plan.opts.cursor {
        CursorStyle::Native => {}
        style => {
            if let Some((p, alpha)) = plan.cursor_at(t) {
                let (cx, cy) = map(p.x, p.y);
                match style {
                    CursorStyle::Disc => {
                        disc(
                            &mut img,
                            cx,
                            cy,
                            22.0 * scale.clamp(0.5, 4.0),
                            [255, 214, 0, 255],
                            0.38 * alpha,
                        );
                    }
                    CursorStyle::Ring => {
                        if alpha > 0.4 {
                            draw_ring(&mut img, cx, cy, 0.3, scale);
                        }
                    }
                    CursorStyle::Native => {}
                }
            }
        }
    }
    for (p, prog) in plan.rings_at(t) {
        let (cx, cy) = map(p.x, p.y);
        draw_ring(&mut img, cx, cy, prog, scale);
    }
    if let Some(b) = plan.badge(t) {
        draw_badge(&mut img, &b.text);
    }
    if let Some(c) = plan.caption(t) {
        draw_caption(&mut img, &c.text);
    }
    on_canvas(&img, w, h, pad, radius)
}

/// Render the plan. `job.on_frame(done, total)` reports progress.
pub fn render_polish(job: &PolishJob<'_>) -> Result<RecordingResult, String> {
    let fps = job.fps.clamp(1, 60);
    let info = job.reader.probe(job.video).map_err(|e| e.to_string())?;
    let size = output_size(job.plan, info.width, info.height);
    let metrics = frame_metrics(job.plan, size.0, size.1);
    let total = job.plan.total_ms.max(1);
    let n = (total * u64::from(fps) / 1000).clamp(1, 400_000);
    let mut audio = if info.has_audio {
        job.plan
            .retime_audio(&job.reader.audio_pcm(job.video).map_err(|e| e.to_string())?)
    } else {
        Vec::new()
    };
    if job.denoise {
        audio = screenit_record::audio_fx::denoise(&audio);
    }
    if !job.narration.is_empty() {
        let len = (total as usize).saturating_mul(48 * 2);
        if audio.len() < len {
            audio.resize(len, 0);
        }
        for (at, clip) in job.narration {
            screenit_record::audio_fx::mix_in(&mut audio, clip, *at, len);
        }
    }
    let mut i = 0u64;
    let mut frames = std::iter::from_fn(|| {
        if i >= n {
            return None;
        }
        if job.abort.load(Ordering::SeqCst) {
            return Some(Err(plat("aborted")));
        }
        let t = (i * 1000 / u64::from(fps)).min(total - 1);
        (job.on_frame)(
            u32::try_from(i).unwrap_or(u32::MAX),
            u32::try_from(n).unwrap_or(u32::MAX),
        );
        i += 1;
        Some(frame_at_time(job, t, size, metrics))
    });
    job.writer
        .write_with_audio(job.out, size.0, size.1, fps, &mut frames, &audio)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenit_record::VideoInfo;
    use screenit_record::events::{EventKind, EventLog, InputEvent};
    use screenit_record::polish::{Aspect, PolishOpts};
    use std::sync::Mutex;

    struct Solid;
    impl VideoReader for Solid {
        fn probe(&self, _: &Path) -> Result<VideoInfo, RecordError> {
            Ok(VideoInfo {
                duration_ms: 4000,
                width: 320,
                height: 180,
                has_audio: true,
            })
        }
        fn frame_at(&self, _: &Path, _ms: u64) -> Result<Raster, RecordError> {
            Raster::new(320, 180, [200, 200, 200, 255]).map_err(plat)
        }
        fn audio_pcm(&self, _: &Path) -> Result<Vec<i16>, RecordError> {
            Ok(vec![1000; 48_000 * 2 * 4])
        }
    }

    #[derive(Default)]
    struct Sink {
        got: Mutex<(u64, u32, u32, usize, Vec<Raster>)>,
    }
    impl VideoWriter for Sink {
        fn write(
            &self,
            out: &Path,
            w: u32,
            h: u32,
            fps: u32,
            frames: &mut dyn Iterator<Item = Result<Raster, RecordError>>,
        ) -> Result<RecordingResult, RecordError> {
            self.write_with_audio(out, w, h, fps, frames, &[])
        }
        fn write_with_audio(
            &self,
            out: &Path,
            w: u32,
            h: u32,
            _fps: u32,
            frames: &mut dyn Iterator<Item = Result<Raster, RecordError>>,
            audio: &[i16],
        ) -> Result<RecordingResult, RecordError> {
            let mut n = 0;
            let mut keep = Vec::new();
            for f in frames {
                let f = f?;
                if n == 0 || n == 15 {
                    keep.push(f);
                }
                n += 1;
            }
            *self.got.lock().map_err(plat)? = (n, w, h, audio.len(), keep);
            Ok(RecordingResult {
                path: out.to_path_buf(),
                duration_ms: 0,
                frames: n,
                width: w,
                height: h,
                has_audio: !audio.is_empty(),
            })
        }
    }

    fn plan(o: &PolishOpts) -> PolishPlan {
        let ev = |t, k, x, y| InputEvent {
            t_ms: t,
            kind: k,
            x,
            y,
            button: 0,
            chord: String::new(),
            rect: None,
        };
        let log = EventLog {
            region: Rect::new(0, 0, 320, 180),
            duration_ms: 4000,
            events: vec![
                ev(0, EventKind::Move, 10, 10),
                ev(500, EventKind::Down, 160, 90),
                ev(3900, EventKind::Move, 20, 20),
            ],
        };
        PolishPlan::build(&log, 4000, Rect::new(0, 0, 320, 180), o)
    }

    fn run(o: &PolishOpts) -> (RecordingResult, Sink, PolishPlan) {
        let p = plan(o);
        let sink = Sink::default();
        let abort = AtomicBool::new(false);
        let res = render_polish(&PolishJob {
            reader: &Solid,
            writer: &sink,
            video: Path::new("a.mp4"),
            plan: &p,
            fps: 10,
            out: Path::new("b.mp4"),
            abort: &abort,
            denoise: false,
            narration: &[],
            on_frame: &|_, _| {},
        })
        .unwrap();
        (res, sink, p)
    }

    #[test]
    fn renders_every_frame_with_matching_audio() {
        let o = PolishOpts {
            trim_idle: false,
            ..PolishOpts::default()
        };
        let (res, sink, p) = run(&o);
        assert_eq!((res.width, res.height), (320, 180));
        assert!(res.has_audio);
        let got = sink.got.lock().unwrap();
        assert_eq!(got.0, 40);
        assert_eq!(got.3 as u64, p.total_ms * 48 * 2);
    }

    #[test]
    fn vertical_with_padding_has_rounded_background_corners() {
        let o = PolishOpts {
            trim_idle: false,
            zoom: false,
            click_rings: false,
            aspect: Aspect::Vertical,
            padding_pct: 10,
            corner_pct: 20,
            ..PolishOpts::default()
        };
        let (res, sink, _) = run(&o);
        assert_eq!((res.width, res.height), (100, 180));
        let got = sink.got.lock().unwrap();
        let f = &got.4[0];
        assert_eq!(f.get(0, 0), Some(BACKGROUND));
        // the picture's own corner is rounded off, its middle is picture
        assert_eq!(f.get(50, 90), Some([200, 200, 200, 255]));
        let (pad, _) = frame_metrics(&plan(&o), 100, 180);
        let c = f.get(pad as i32, pad as i32).unwrap();
        assert!(c[0] < 100, "corner shows the background: {c:?}");
    }

    /// Budget `polish_1080p_frame_ms` (40 ms in release). The bound here is loose so a debug
    /// build on a busy machine does not flake; it catches order-of-magnitude regressions.
    #[test]
    fn a_zoomed_padded_1080p_frame_stays_within_budget() {
        let frame = Raster::new(1920, 1080, [120, 130, 140, 255]).unwrap();
        let video = Rect::new(0, 0, 1920, 1080);
        let view = Rect::new(400, 300, 1120, 630);
        let t = std::time::Instant::now();
        for _ in 0..3 {
            let img = crop_view(frame.clone(), video, view, 1920, 1080).unwrap();
            on_canvas(&img, 1920, 1080, 60, 24).unwrap();
        }
        let per = t.elapsed().as_millis() / 3;
        assert!(per < 1500, "{per} ms per frame");
    }

    #[test]
    fn badge_ring_and_disc_draw_pixels_and_abort_stops() {
        let mut r = Raster::new(320, 180, [0, 0, 0, 255]).unwrap();
        draw_badge(&mut r, "ctrl+s");
        assert!(r.data().chunks(4).any(|p| p[0] > 200));
        let mut c = Raster::new(320, 180, [0, 0, 0, 255]).unwrap();
        draw_caption(
            &mut c,
            "A long subtitle line that has to wrap onto a second line",
        );
        assert!(c.data().chunks(4).any(|p| p[0] > 200));
        assert_eq!(c.get(160, 5), Some([0, 0, 0, 255]), "the top stays clear");
        draw_caption(&mut c, "");
        let mut r = Raster::new(40, 40, [0, 0, 0, 255]).unwrap();
        disc(&mut r, 20, 20, 10.0, [255, 255, 255, 255], 1.0);
        assert_eq!(r.get(20, 20), Some([255, 255, 255, 255]));
        disc(&mut r, -500, 900, 10.0, [255; 4], 1.0);
        let p = plan(&PolishOpts::default());
        let sink = Sink::default();
        let abort = AtomicBool::new(true);
        let res = render_polish(&PolishJob {
            reader: &Solid,
            writer: &sink,
            video: Path::new("a.mp4"),
            plan: &p,
            fps: 10,
            out: Path::new("b.mp4"),
            abort: &abort,
            denoise: false,
            narration: &[],
            on_frame: &|_, _| {},
        });
        assert!(res.is_err());
    }
}
