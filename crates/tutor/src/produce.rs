//! Production plan: turns takes and step markers into an output timeline with idle time cut,
//! chapter cards, smooth zoom shots, captions and click callouts. Pure data; the engine renders it.

use crate::perform::Performance;
use crate::player::{Marker, narration_ms};
use screenit_geom::{Point, Rect};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProduceOpts {
    pub zoom: bool,
    pub captions: bool,
    pub callouts: bool,
    pub trim_idle: bool,
    pub chapter_cards: bool,
    /// Zoom magnification for small targets.
    pub zoom_factor: f32,
    pub card_ms: u32,
    /// Quiet time kept after an action before idle trimming starts.
    pub keep_after_ms: u32,
}

impl Default for ProduceOpts {
    fn default() -> Self {
        Self {
            zoom: true,
            captions: true,
            callouts: true,
            trim_idle: true,
            chapter_cards: true,
            zoom_factor: 1.6,
            card_ms: 1800,
            keep_after_ms: 900,
        }
    }
}

/// Where an output moment comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Card { chapter: usize },
    Clip { take: usize, ms: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Seg {
    out_start: u64,
    out_end: u64,
    src: SegSrc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SegSrc {
    Card { chapter: usize },
    Clip { take: usize, from: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caption {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChapterMark {
    pub title: String,
    pub start_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    pub start_ms: u64,
    pub end_ms: u64,
    pub rect: Rect,
    pub ease_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Callout {
    pub at_ms: u64,
    pub point: Point,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    segs: Vec<Seg>,
    pub total_ms: u64,
    pub video: Rect,
    pub shots: Vec<Shot>,
    pub captions: Vec<Caption>,
    pub callouts: Vec<Callout>,
    pub chapters: Vec<ChapterMark>,
    pub titles: Vec<String>,
}

const CALLOUT_MS: u64 = 700;

/// Parts of a take (source ms ranges) that stay in the video.
fn kept_ranges(duration: u64, markers: &[Marker], opts: &ProduceOpts) -> Vec<(u64, u64)> {
    let mut cuts: Vec<(u64, u64)> = Vec::new();
    if opts.trim_idle {
        for m in markers {
            let hold = u64::from(opts.keep_after_ms).max(narration_ms(&m.narration));
            let from = m.act_ms.saturating_add(hold);
            let to = m.end_ms.saturating_sub(300);
            if to > from + 400 {
                cuts.push((from, to.min(duration)));
            }
        }
    }
    cuts.retain(|(a, b)| b > a);
    cuts.sort_unstable();
    let mut out = Vec::new();
    let mut at = 0u64;
    for (a, b) in cuts {
        if a > at {
            out.push((at, a));
        }
        at = at.max(b);
    }
    if duration > at {
        out.push((at, duration));
    }
    out
}

fn smooth(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Plan {
    /// Build the timeline for `perf`.
    pub fn build(perf: &Performance, opts: &ProduceOpts) -> Plan {
        let mut plan = Plan {
            video: Rect::new(0, 0, perf.region.w, perf.region.h),
            titles: perf.takes.iter().map(|t| t.title.clone()).collect(),
            ..Plan::default()
        };
        let mut t = 0u64;
        for (ti, take) in perf.takes.iter().enumerate() {
            plan.chapters.push(ChapterMark {
                title: take.title.clone(),
                start_ms: t,
            });
            if opts.chapter_cards {
                let end = t + u64::from(opts.card_ms);
                plan.segs.push(Seg {
                    out_start: t,
                    out_end: end,
                    src: SegSrc::Card { chapter: ti },
                });
                t = end;
            }
            for (a, b) in kept_ranges(take.duration_ms, &take.markers, opts) {
                plan.segs.push(Seg {
                    out_start: t,
                    out_end: t + (b - a),
                    src: SegSrc::Clip { take: ti, from: a },
                });
                t += b - a;
            }
            for m in &take.markers {
                plan.add_marker(ti, m, perf.region, opts);
            }
        }
        plan.total_ms = t;
        plan
    }

    fn map(&self, take: usize, ms: u64) -> Option<u64> {
        self.segs.iter().find_map(|s| match s.src {
            SegSrc::Clip { take: t, from } if t == take => {
                let len = s.out_end - s.out_start;
                (ms >= from && ms <= from + len).then(|| s.out_start + (ms - from))
            }
            _ => None,
        })
    }

    fn add_marker(&mut self, take: usize, m: &Marker, region: Rect, opts: &ProduceOpts) {
        let (Some(start), Some(end)) = (
            self.map(take, m.start_ms),
            self.map(take, m.end_ms.max(m.start_ms)),
        ) else {
            return;
        };
        if opts.captions && !m.narration.trim().is_empty() {
            let end_c = end.max(start + narration_ms(&m.narration).min(8000));
            self.captions.push(Caption {
                start_ms: start,
                end_ms: end_c,
                text: m.narration.trim().to_string(),
            });
        }
        let Some(r) = m.rect else { return };
        let local = Rect::new(
            r.x.saturating_sub(region.x),
            r.y.saturating_sub(region.y),
            r.w,
            r.h,
        );
        let at = self.map(take, m.act_ms).unwrap_or(start);
        if opts.callouts && matches!(m.kind.as_str(), "click" | "double_click" | "right_click") {
            self.callouts.push(Callout {
                at_ms: at,
                point: Point::new(
                    local.x.saturating_add((local.w / 2) as i32),
                    local.y.saturating_add((local.h / 2) as i32),
                ),
            });
        }
        if opts.zoom {
            if let Some(view) = zoom_rect(self.video, local, opts.zoom_factor) {
                self.shots.push(Shot {
                    start_ms: at.saturating_sub(500).max(start.saturating_sub(300)),
                    end_ms: end.max(at + 900),
                    rect: view,
                    ease_ms: 450,
                });
            }
        }
    }

    /// Source for an output time, `None` past the end.
    pub fn locate(&self, out_ms: u64) -> Option<Source> {
        let s = self
            .segs
            .iter()
            .find(|s| out_ms >= s.out_start && out_ms < s.out_end)?;
        Some(match s.src {
            SegSrc::Card { chapter } => Source::Card { chapter },
            SegSrc::Clip { take, from } => Source::Clip {
                take,
                ms: from + (out_ms - s.out_start),
            },
        })
    }

    /// Visible part of the frame at `out_ms` (the whole frame when no shot is active).
    pub fn view(&self, out_ms: u64) -> Rect {
        let full = self.video;
        let mut best: Option<(f64, Rect)> = None;
        for s in &self.shots {
            if out_ms < s.start_ms || out_ms >= s.end_ms {
                continue;
            }
            let e = s.ease_ms.max(1) as f64;
            let into = (out_ms - s.start_ms) as f64 / e;
            let left = (s.end_ms - out_ms) as f64 / e;
            let f = smooth(into.min(left).min(1.0));
            if best.is_none_or(|(bf, _)| f > bf) {
                best = Some((f, s.rect));
            }
        }
        let Some((f, r)) = best else { return full };
        let l = |a: i32, b: i32| (f64::from(a) + (f64::from(b) - f64::from(a)) * f).round() as i32;
        let lu = |a: u32, b: u32| {
            (f64::from(a) + (f64::from(b) - f64::from(a)) * f)
                .round()
                .max(2.0) as u32
        };
        Rect::new(
            l(full.x, r.x),
            l(full.y, r.y),
            lu(full.w, r.w),
            lu(full.h, r.h),
        )
    }

    pub fn caption(&self, out_ms: u64) -> Option<&Caption> {
        self.captions
            .iter()
            .find(|c| out_ms >= c.start_ms && out_ms < c.end_ms)
    }

    /// Active click rings: position and progress 0..1.
    pub fn callouts_at(&self, out_ms: u64) -> Vec<(Point, f32)> {
        self.callouts
            .iter()
            .filter(|c| out_ms >= c.at_ms && out_ms < c.at_ms + CALLOUT_MS)
            .map(|c| (c.point, (out_ms - c.at_ms) as f32 / CALLOUT_MS as f32))
            .collect()
    }

    pub fn srt(&self) -> String {
        let mut s = String::new();
        for (i, c) in self.captions.iter().enumerate() {
            s.push_str(&format!(
                "{}\n{} --> {}\n{}\n\n",
                i + 1,
                srt_time(c.start_ms),
                srt_time(c.end_ms),
                c.text.replace("\r\n", "\n").trim()
            ));
        }
        s
    }

    /// `mm:ss Title` lines, ready for a video description.
    pub fn chapter_list(&self) -> String {
        self.chapters
            .iter()
            .map(|c| {
                format!(
                    "{:02}:{:02} {}",
                    c.start_ms / 60_000,
                    c.start_ms / 1000 % 60,
                    c.title
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn srt_time(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// Zoomed view centred on `target`, or `None` when the target is already large.
fn zoom_rect(full: Rect, target: Rect, factor: f32) -> Option<Rect> {
    let f = f64::from(factor);
    if !f.is_finite() || f <= 1.05 || full.is_empty() || target.is_empty() {
        return None;
    }
    if target.area().saturating_mul(5) > full.area() {
        return None;
    }
    let w = ((f64::from(full.w) / f).round() as u32).max(2);
    let h = ((f64::from(full.h) / f).round() as u32).max(2);
    let cx = i64::from(target.x) + i64::from(target.w / 2);
    let cy = i64::from(target.y) + i64::from(target.h / 2);
    let x = (cx - i64::from(w / 2)).clamp(0, i64::from(full.w - w));
    let y = (cy - i64::from(h / 2)).clamp(0, i64::from(full.h - h));
    Some(Rect::new(x as i32, y as i32, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perform::Take;

    fn marker(id: &str, start: u64, act: u64, end: u64, rect: Option<Rect>, n: &str) -> Marker {
        Marker {
            step_id: id.into(),
            chapter: 0,
            kind: "click".into(),
            narration: n.into(),
            start_ms: start,
            act_ms: act,
            end_ms: end,
            rect,
            text: String::new(),
        }
    }

    fn perf() -> Performance {
        Performance {
            title: "T".into(),
            region: Rect::new(100, 50, 1000, 600),
            takes: vec![
                Take {
                    chapter: 0,
                    title: "One".into(),
                    path: "a.mp4".into(),
                    duration_ms: 10_000,
                    markers: vec![
                        marker(
                            "s1",
                            700,
                            1200,
                            2000,
                            Some(Rect::new(500, 300, 80, 30)),
                            "Click it",
                        ),
                        marker("s2", 2000, 2500, 9000, None, ""),
                    ],
                },
                Take {
                    chapter: 1,
                    title: "Two".into(),
                    path: "b.mp4".into(),
                    duration_ms: 3000,
                    markers: vec![],
                },
            ],
        }
    }

    #[test]
    fn trims_idle_and_adds_cards() {
        let p = Plan::build(&perf(), &ProduceOpts::default());
        assert!(p.total_ms < 10_000 + 3000 + 3600, "{}", p.total_ms);
        assert!(p.total_ms > 3000);
        assert_eq!(p.chapters.len(), 2);
        assert_eq!(p.chapters[0].start_ms, 0);
        assert!(matches!(p.locate(0), Some(Source::Card { chapter: 0 })));
        assert!(matches!(p.locate(1900), Some(Source::Clip { take: 0, .. })));
        assert_eq!(p.locate(p.total_ms), None);
        // the long idle tail of s2 is cut: second chapter starts well before 9s of clip time
        assert!(p.chapters[1].start_ms < 9_000, "{}", p.chapters[1].start_ms);
    }

    #[test]
    fn without_trim_or_cards_time_is_preserved() {
        let o = ProduceOpts {
            trim_idle: false,
            chapter_cards: false,
            ..ProduceOpts::default()
        };
        assert_eq!(Plan::build(&perf(), &o).total_ms, 13_000);
    }

    #[test]
    fn captions_callouts_and_zoom_follow_the_marker() {
        let o = ProduceOpts {
            chapter_cards: false,
            trim_idle: false,
            ..ProduceOpts::default()
        };
        let p = Plan::build(&perf(), &o);
        assert_eq!(p.captions.len(), 1);
        assert!(p.caption(1000).is_some());
        assert!(p.caption(9000).is_none());
        assert_eq!(p.callouts.len(), 1);
        assert_eq!(p.callouts[0].point, Point::new(440, 265));
        assert_eq!(p.callouts_at(1300).len(), 1);
        assert_eq!(p.view(100), p.video);
        let mid = p.view(1500);
        assert!(mid.w < 1000 && mid.w >= 625, "{mid:?}");
        assert!(Rect::new(0, 0, 1000, 600).intersect(&mid) == Some(mid));
    }

    #[test]
    fn srt_and_chapter_list_format() {
        let p = Plan::build(&perf(), &ProduceOpts::default());
        let srt = p.srt();
        assert!(srt.starts_with("1\n00:00:"), "{srt}");
        assert!(srt.contains(" --> "));
        assert!(srt.contains("Click it"));
        assert!(p.chapter_list().starts_with("00:00 One\n"));
        assert_eq!(srt_time(3_723_456), "01:02:03,456");
    }

    #[test]
    fn big_targets_do_not_zoom_and_bad_factors_are_safe() {
        let full = Rect::new(0, 0, 1000, 600);
        assert!(zoom_rect(full, Rect::new(0, 0, 900, 500), 1.6).is_none());
        assert!(zoom_rect(full, Rect::new(10, 10, 20, 20), f32::NAN).is_none());
        assert!(zoom_rect(full, Rect::new(0, 0, 0, 0), 2.0).is_none());
        let r = zoom_rect(full, Rect::new(990, 590, 10, 10), 4.0).unwrap();
        assert_eq!(r.right(), 1000);
        assert_eq!(r.bottom(), 600);
        assert!(Plan::build(&Performance::default(), &ProduceOpts::default()).total_ms == 0);
    }
}
