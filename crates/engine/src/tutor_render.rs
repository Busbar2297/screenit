//! Renders a tutorial production plan to an MP4: reads chapter clips, applies the zoom view,
//! draws click rings, burns captions and chapter cards, and hands the frames to the writer.

use screenit_doc::Color;
use screenit_geom::{PointF, Rect};
use screenit_raster::Raster;
use screenit_record::{RecordError, RecordingResult, VideoReader, VideoWriter};
use screenit_tutor::perform::Performance;
use screenit_tutor::produce::{Plan, Source};
use screenit_vector::{draw_text_box, layout_text};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

fn plat(e: impl std::fmt::Display) -> RecordError {
    RecordError::Platform(e.to_string())
}

/// A dark card with the chapter number and title.
pub fn chapter_card(w: u32, h: u32, index: usize, title: &str) -> Result<Raster, RecordError> {
    let mut r = Raster::new(w, h, [20, 24, 33, 255]).map_err(plat)?;
    let small = (h as f32 / 24.0).max(12.0);
    let big = (h as f32 / 11.0).max(18.0);
    let box_w = w as f32 * 0.8;
    let x = w as f32 * 0.1;
    draw_text_box(
        &mut r,
        &format!("Chapter {}", index + 1),
        PointF::new(x, h as f32 * 0.33),
        small,
        box_w,
        Color([150, 170, 210, 255]),
    );
    draw_text_box(
        &mut r,
        title,
        PointF::new(x, h as f32 * 0.33 + small * 1.6),
        big,
        box_w,
        Color::WHITE,
    );
    Ok(r)
}

/// Burn a caption into the lower part of the frame on a translucent bar.
pub fn burn_caption(r: &mut Raster, text: &str) {
    let (w, h) = (r.width(), r.height());
    let size = (h as f32 / 26.0).max(12.0);
    let wrap = w as f32 * 0.84;
    let lay = layout_text(text, size, wrap);
    let lines = lay.lines.len().clamp(1, 4) as f32;
    let text_h = lay.line_height * lines;
    let pad = size * 0.6;
    let bar_h = (text_h + pad * 2.0) as i32;
    let y0 = (h as i32 - bar_h - (h as f32 * 0.04) as i32).max(0);
    let x0 = ((w as f32 - lay.width.min(wrap)) / 2.0 - pad).max(0.0) as i32;
    let bar_w = (lay.width.min(wrap) + pad * 2.0) as i32;
    for y in y0..(y0 + bar_h).min(h as i32) {
        for x in x0..(x0 + bar_w).min(w as i32) {
            r.blend(x, y, [0, 0, 0, 255], 175);
        }
    }
    draw_text_box(
        r,
        text,
        PointF::new(x0 as f32 + pad, y0 as f32 + pad),
        size,
        wrap,
        Color::WHITE,
    );
}

/// An expanding, fading ring at `(cx, cy)`; `progress` is 0..1.
pub fn draw_ring(r: &mut Raster, cx: i32, cy: i32, progress: f32, scale: f32) {
    let p = progress.clamp(0.0, 1.0);
    let radius = (12.0 + 30.0 * p) * scale.clamp(0.5, 4.0);
    let alpha = (255.0 * (1.0 - p)) as u8;
    let reach = radius as i32 + 3;
    for y in cy - reach..=cy + reach {
        for x in cx - reach..=cx + reach {
            let d = (((x - cx).pow(2) + (y - cy).pow(2)) as f32).sqrt();
            let off = (d - radius).abs();
            if off <= 2.0 {
                let cov = ((2.0 - off) / 2.0 * f32::from(alpha)) as u8;
                r.blend(x, y, [255, 214, 0, 255], cov);
            }
        }
    }
}

fn view_frame(
    frame: Raster,
    plan: &Plan,
    view: Rect,
    w: u32,
    h: u32,
) -> Result<Raster, RecordError> {
    let (fw, fh) = (frame.width(), frame.height());
    let sx = f64::from(fw) / f64::from(plan.video.w.max(1));
    let sy = f64::from(fh) / f64::from(plan.video.h.max(1));
    let full = view == plan.video;
    let src = if full {
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

/// Render `plan` over the takes of `perf` into `out`. `on_frame(done, total)` reports progress.
#[allow(clippy::too_many_arguments)]
pub fn render_video(
    reader: &dyn VideoReader,
    writer: &dyn VideoWriter,
    perf: &Performance,
    plan: &Plan,
    fps: u32,
    out: &Path,
    abort: &AtomicBool,
    on_frame: &dyn Fn(u32, u32),
) -> Result<RecordingResult, String> {
    let fps = fps.clamp(1, 60);
    let (w, h) = (even(perf.region.w), even(perf.region.h));
    let total = plan.total_ms.max(1);
    let n = (total * u64::from(fps) / 1000).clamp(1, 400_000);
    let mut i = 0u64;
    let mut card: Option<(usize, Raster)> = None;
    let mut frames = std::iter::from_fn(|| {
        if i >= n {
            return None;
        }
        if abort.load(Ordering::SeqCst) {
            return Some(Err(plat("aborted")));
        }
        let t = (i * 1000 / u64::from(fps)).min(total - 1);
        let done = u32::try_from(i).unwrap_or(u32::MAX);
        let total_frames = u32::try_from(n).unwrap_or(u32::MAX);
        on_frame(done, total_frames);
        i += 1;
        Some((|| -> Result<Raster, RecordError> {
            let mut img = match plan.locate(t) {
                Some(Source::Card { chapter }) => {
                    if card.as_ref().is_none_or(|(c, _)| *c != chapter) {
                        let title = plan.titles.get(chapter).map_or("", String::as_str);
                        card = Some((chapter, chapter_card(w, h, chapter, title)?));
                    }
                    return card
                        .as_ref()
                        .map(|(_, r)| r.clone())
                        .ok_or_else(|| plat("card"));
                }
                Some(Source::Clip { take, ms }) => {
                    let tk = perf.takes.get(take).ok_or_else(|| plat("missing take"))?;
                    let ms = ms.min(tk.duration_ms.saturating_sub(1));
                    let frame = reader.frame_at(&tk.path, ms)?;
                    view_frame(frame, plan, plan.view(t), w, h)?
                }
                None => Raster::new(w, h, [0, 0, 0, 255]).map_err(plat)?,
            };
            let view = plan.view(t);
            for (pt, prog) in plan.callouts_at(t) {
                let ox =
                    (f64::from(pt.x - view.x) * f64::from(w) / f64::from(view.w.max(1))) as i32;
                let oy =
                    (f64::from(pt.y - view.y) * f64::from(h) / f64::from(view.h.max(1))) as i32;
                draw_ring(&mut img, ox, oy, prog, w as f32 / 1280.0);
            }
            if let Some(c) = plan.caption(t) {
                burn_caption(&mut img, &c.text);
            }
            Ok(img)
        })())
    });
    writer
        .write(out, w, h, fps, &mut frames)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_and_caption_draw_pixels() {
        let card = chapter_card(320, 180, 0, "Hello").unwrap();
        assert!(card.data().chunks(4).any(|p| p[0] > 200 && p[1] > 200));
        let mut r = Raster::new(320, 180, [255, 255, 255, 255]).unwrap();
        burn_caption(&mut r, "Click the button");
        assert!(r.get(160, 150).is_some_and(|p| p[0] < 200), "bar darkens");
        assert_eq!(r.get(2, 2), Some([255, 255, 255, 255]));
    }

    #[test]
    fn ring_is_clipped_safely() {
        let mut r = Raster::new(40, 40, [0, 0, 0, 255]).unwrap();
        draw_ring(&mut r, -50, 500, 0.5, 1.0);
        draw_ring(&mut r, 20, 20, 0.0, 1.0);
        assert!(r.data().chunks(4).any(|p| p[0] > 100));
    }
}
