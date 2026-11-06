//! Auto-polish plan: turns the event log of a recording into an output timeline with idle time
//! cut, typing sped up, smooth zoom shots on clicks, click rings, shortcut badges, a smoothed
//! cursor and an optional reframing to another aspect ratio. Pure data; the engine renders it.

use crate::events::{EventKind, EventLog, InputEvent};
use screenit_geom::{Point, Rect};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aspect {
    #[default]
    Native,
    /// 1:1
    Square,
    /// 9:16 (TikTok, Shorts, Reels)
    Vertical,
    /// 16:9
    Wide,
}

impl Aspect {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "native" => Self::Native,
            "square" => Self::Square,
            "vertical" => Self::Vertical,
            "wide" => Self::Wide,
            _ => return None,
        })
    }

    /// Width / height, `None` for native.
    pub fn ratio(self) -> Option<f64> {
        match self {
            Self::Native => None,
            Self::Square => Some(1.0),
            Self::Vertical => Some(9.0 / 16.0),
            Self::Wide => Some(16.0 / 9.0),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    /// Leave the captured cursor alone.
    #[default]
    Native,
    /// Draw a soft disc under the pointer.
    Disc,
    /// Draw a ring around the pointer.
    Ring,
}

impl CursorStyle {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "native" => Self::Native,
            "disc" => Self::Disc,
            "ring" => Self::Ring,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PolishOpts {
    pub zoom: bool,
    /// Magnification when the target is small.
    pub zoom_factor: f32,
    pub click_rings: bool,
    pub cursor: CursorStyle,
    pub typing_speedup: bool,
    pub speedup_factor: f32,
    pub trim_idle: bool,
    /// Stretches with no input longer than this are cut.
    pub idle_min_ms: u32,
    /// Quiet time kept on each side of a cut.
    pub keep_ms: u32,
    pub shortcut_badges: bool,
    pub aspect: Aspect,
    /// Margin around the video, percent of the shorter side.
    pub padding_pct: u8,
    /// Corner radius, percent of the shorter side.
    pub corner_pct: u8,
    /// Extra source ranges (start_ms, end_ms) to cut (silences, filler words).
    pub extra_cuts: Vec<(u64, u64)>,
}

impl Default for PolishOpts {
    fn default() -> Self {
        Self {
            zoom: true,
            zoom_factor: 1.7,
            click_rings: true,
            cursor: CursorStyle::Native,
            typing_speedup: true,
            speedup_factor: 3.0,
            trim_idle: true,
            idle_min_ms: 2500,
            keep_ms: 600,
            shortcut_badges: true,
            aspect: Aspect::Native,
            padding_pct: 0,
            corner_pct: 0,
            extra_cuts: Vec::new(),
        }
    }
}

impl PolishOpts {
    /// Reject values that would make the plan meaningless.
    pub fn validate(&self) -> Result<(), String> {
        if !self.zoom_factor.is_finite() || !(1.0..=8.0).contains(&self.zoom_factor) {
            return Err("zoom_factor must be 1..=8".into());
        }
        if !self.speedup_factor.is_finite() || !(1.0..=16.0).contains(&self.speedup_factor) {
            return Err("speedup_factor must be 1..=16".into());
        }
        if self.extra_cuts.len() > 100_000 {
            return Err("too many extra cuts".into());
        }
        if self.padding_pct > 25 || self.corner_pct > 25 {
            return Err("padding_pct and corner_pct must be 0..=25".into());
        }
        Ok(())
    }
}

/// A stretch of source time that appears in the output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seg {
    pub out_start: u64,
    pub out_end: u64,
    pub src_start: u64,
    /// Playback speed times 1000 (1000 = normal).
    pub speed_milli: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    pub start_ms: u64,
    pub end_ms: u64,
    pub rect: Rect,
    pub ease_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Badge {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// A subtitle line on the output timeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caption {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Click {
    pub at_ms: u64,
    pub point: Point,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PolishPlan {
    pub segs: Vec<Seg>,
    pub total_ms: u64,
    /// Source frame in recording pixels (origin 0,0).
    pub video: Rect,
    /// Output frame size in pixels before padding is applied.
    pub shots: Vec<Shot>,
    pub badges: Vec<Badge>,
    pub clicks: Vec<Click>,
    pub captions: Vec<Caption>,
    pub opts: PolishOpts,
    #[serde(skip)]
    moves: Vec<(u64, Point)>,
}

const RING_MS: u64 = 700;
const BADGE_MS: u64 = 1400;
const CLUSTER_MS: u64 = 1500;
const CLUSTER_PX: i64 = 250;
const TYPE_GAP_MS: u64 = 700;
const TYPE_MIN_MS: u64 = 1500;
const CURSOR_FADE_AFTER_MS: u64 = 2500;

fn smooth(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Grow `r` to the aspect ratio `ratio` (w/h), around its centre, inside `bounds`.
pub fn fit_aspect(r: Rect, ratio: f64, bounds: Rect) -> Rect {
    if !ratio.is_finite() || ratio <= 0.0 || bounds.is_empty() || r.is_empty() {
        return bounds;
    }
    let (mut w, mut h) = (f64::from(r.w), f64::from(r.h));
    if w / h < ratio {
        w = h * ratio;
    } else {
        h = w / ratio;
    }
    let (bw, bh) = (f64::from(bounds.w), f64::from(bounds.h));
    if w > bw {
        h *= bw / w;
        w = bw;
    }
    if h > bh {
        w *= bh / h;
        h = bh;
    }
    let (w, h) = ((w.round() as u32).max(2), (h.round() as u32).max(2));
    let cx = i64::from(r.x) + i64::from(r.w / 2);
    let cy = i64::from(r.y) + i64::from(r.h / 2);
    let x = (cx - i64::from(w / 2)).clamp(
        i64::from(bounds.x),
        (i64::from(bounds.x) + i64::from(bounds.w) - i64::from(w)).max(i64::from(bounds.x)),
    );
    let y = (cy - i64::from(h / 2)).clamp(
        i64::from(bounds.y),
        (i64::from(bounds.y) + i64::from(bounds.h) - i64::from(h)).max(i64::from(bounds.y)),
    );
    Rect::new(x as i32, y as i32, w, h)
}

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
    let x = (cx - i64::from(w / 2)).clamp(0, i64::from(full.w.saturating_sub(w)));
    let y = (cy - i64::from(h / 2)).clamp(0, i64::from(full.h.saturating_sub(h)));
    Some(Rect::new(x as i32, y as i32, w, h))
}

type Spans = Vec<(u64, u64)>;

/// Source ranges that are cut, and ranges that play fast.
fn ranges(events: &[InputEvent], duration: u64, o: &PolishOpts) -> (Spans, Spans) {
    let mut cuts = Vec::new();
    if o.trim_idle {
        let mut last = 0u64;
        let mut times: Vec<u64> = events.iter().map(|e| e.t_ms.min(duration)).collect();
        times.push(duration);
        for t in times {
            let gap = t.saturating_sub(last);
            if gap > u64::from(o.idle_min_ms) {
                let from = last.saturating_add(u64::from(o.keep_ms));
                let to = t.saturating_sub(u64::from(o.keep_ms));
                if to > from {
                    cuts.push((from, to));
                }
            }
            last = last.max(t);
        }
    }
    cuts.extend(o.extra_cuts.iter().copied().filter(|(a, b)| b > a));
    let mut fast = Vec::new();
    if o.typing_speedup && o.speedup_factor > 1.05 {
        let typing: Vec<u64> = events
            .iter()
            .filter(|e| e.kind == EventKind::Type)
            .map(|e| e.t_ms)
            .collect();
        let mut i = 0;
        while i < typing.len() {
            let mut j = i;
            while j + 1 < typing.len() && typing[j + 1].saturating_sub(typing[j]) <= TYPE_GAP_MS {
                j += 1;
            }
            let (a, b) = (typing[i], typing[j]);
            if b.saturating_sub(a) >= TYPE_MIN_MS {
                fast.push((a, (b + 300).min(duration)));
            }
            i = j + 1;
        }
    }
    (cuts, fast)
}

fn inside(r: &[(u64, u64)], t: u64) -> bool {
    r.iter().any(|(a, b)| t >= *a && t < *b)
}

impl PolishPlan {
    /// Build the timeline for `log` over a recording `duration_ms` long.
    pub fn build(log: &EventLog, duration_ms: u64, video: Rect, opts: &PolishOpts) -> Self {
        let mut plan = Self {
            video,
            opts: opts.clone(),
            ..Self::default()
        };
        let duration = duration_ms.max(log.duration_ms.min(duration_ms.max(1)));
        let (cuts, fast) = ranges(&log.events, duration, opts);
        let mut pts: Vec<u64> = vec![0, duration];
        for (a, b) in cuts.iter().chain(fast.iter()) {
            pts.push((*a).min(duration));
            pts.push((*b).min(duration));
        }
        pts.sort_unstable();
        pts.dedup();
        let fast_milli = (opts.speedup_factor.clamp(1.0, 16.0) * 1000.0) as u32;
        let mut t = 0u64;
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b <= a {
                continue;
            }
            let mid = a + (b - a) / 2;
            if inside(&cuts, mid) {
                continue;
            }
            let speed = if inside(&fast, mid) { fast_milli } else { 1000 };
            let len = (b - a) * 1000 / u64::from(speed.max(1000));
            let len = len.max(1);
            if let Some(last) = plan.segs.last_mut() {
                if last.speed_milli == speed
                    && last.src_start + (last.out_end - last.out_start) * u64::from(speed) / 1000
                        == a
                {
                    last.out_end = t + len;
                    t += len;
                    continue;
                }
            }
            plan.segs.push(Seg {
                out_start: t,
                out_end: t + len,
                src_start: a,
                speed_milli: speed,
            });
            t += len;
        }
        plan.total_ms = t;
        plan.moves = log
            .events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::Move | EventKind::Down | EventKind::Up))
            .map(|e| (e.t_ms, Point::new(e.x - log.region.x, e.y - log.region.y)))
            .collect();
        plan.add_events(log);
        plan
    }

    /// Output time of a source time; a cut moment lands on the next kept moment.
    pub fn src_to_out(&self, src: u64) -> Option<u64> {
        for s in &self.segs {
            let len = (s.out_end - s.out_start) * u64::from(s.speed_milli) / 1000;
            if src < s.src_start {
                return Some(s.out_start);
            }
            if src <= s.src_start + len {
                return Some(
                    s.out_start + (src - s.src_start) * 1000 / u64::from(s.speed_milli.max(1)),
                );
            }
        }
        None
    }

    /// Cut and re-time audio (interleaved 48 kHz stereo) to follow the output timeline. Sped-up
    /// stretches are resampled (the pitch rises); missing source audio becomes silence.
    pub fn retime_audio(&self, pcm: &[i16]) -> Vec<i16> {
        let ch = usize::from(crate::audio::CHANNELS);
        let rate = u64::from(crate::audio::RATE);
        let mut out = Vec::new();
        for s in &self.segs {
            let frames = (s.out_end - s.out_start) * rate / 1000;
            let base = s.src_start * rate / 1000;
            for i in 0..frames {
                let src = (base + i * u64::from(s.speed_milli) / 1000) as usize;
                for c in 0..ch {
                    out.push(pcm.get(src * ch + c).copied().unwrap_or(0));
                }
            }
        }
        out
    }

    /// Place subtitle lines (source times) on the output timeline; lines that fall entirely in
    /// a cut disappear.
    pub fn set_captions(&mut self, lines: &[crate::speech::Line]) {
        let mut out = Vec::new();
        for l in lines.iter().take(100_000) {
            let Some(a) = self.src_to_out(l.start_ms) else {
                continue;
            };
            let b = self.src_to_out(l.end_ms).unwrap_or(self.total_ms);
            if b > a && !l.text.trim().is_empty() {
                out.push(Caption {
                    start_ms: a,
                    end_ms: b,
                    text: l.text.clone(),
                });
            }
        }
        self.captions = out;
    }

    /// The caption showing at output time `t`.
    pub fn caption(&self, t: u64) -> Option<&Caption> {
        self.captions
            .iter()
            .find(|c| t >= c.start_ms && t < c.end_ms)
    }

    /// Source time shown at an output time, `None` past the end.
    pub fn out_to_src(&self, out_ms: u64) -> Option<u64> {
        let s = self
            .segs
            .iter()
            .find(|s| out_ms >= s.out_start && out_ms < s.out_end)?;
        Some(s.src_start + (out_ms - s.out_start) * u64::from(s.speed_milli) / 1000)
    }

    fn add_events(&mut self, log: &EventLog) {
        let o = self.opts.clone();
        let region = log.region;
        // Clicks: merge those close in time and space into one shot.
        let mut last: Option<(u64, Point)> = None;
        for e in log.clicks() {
            let Some(at) = self.src_to_out(e.t_ms) else {
                continue;
            };
            let local = Point::new(e.x - region.x, e.y - region.y);
            if o.click_rings {
                self.clicks.push(Click {
                    at_ms: at,
                    point: local,
                });
            }
            if !o.zoom {
                continue;
            }
            let near = last.is_some_and(|(t, p)| {
                e.t_ms.saturating_sub(t) < CLUSTER_MS
                    && (i64::from(p.x) - i64::from(local.x)).abs()
                        + (i64::from(p.y) - i64::from(local.y)).abs()
                        < CLUSTER_PX
            });
            last = Some((e.t_ms, local));
            if near {
                if let Some(s) = self.shots.last_mut() {
                    s.end_ms = at + 900;
                }
                continue;
            }
            let target = e
                .rect
                .map(|r| {
                    Rect::new(
                        r.x.saturating_sub(region.x),
                        r.y.saturating_sub(region.y),
                        r.w,
                        r.h,
                    )
                })
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| {
                    Rect::new(
                        local.x.saturating_sub(40),
                        local.y.saturating_sub(20),
                        80,
                        40,
                    )
                });
            if let Some(view) = zoom_rect(self.video, target, o.zoom_factor) {
                let view = match o.aspect.ratio() {
                    Some(r) => fit_aspect(view, r, self.video),
                    None => view,
                };
                self.shots.push(Shot {
                    start_ms: at.saturating_sub(500),
                    end_ms: at + 1400,
                    rect: view,
                    ease_ms: 450,
                });
            }
        }
        if o.shortcut_badges {
            for e in log.events.iter().filter(|e| e.kind == EventKind::Key) {
                if let Some(at) = self.src_to_out(e.t_ms) {
                    self.badges.push(Badge {
                        start_ms: at,
                        end_ms: at + BADGE_MS,
                        text: e.chord.clone(),
                    });
                }
            }
        }
    }

    /// Smoothed pointer position at a source time, local to the recorded area.
    fn pointer_src(&self, src: u64) -> Option<Point> {
        let i = self.moves.partition_point(|(t, _)| *t <= src);
        let before = i.checked_sub(1).and_then(|k| self.moves.get(k));
        let after = self.moves.get(i);
        match (before, after) {
            (Some((t0, p0)), Some((t1, p1))) if t1 > t0 => {
                let f = smooth((src - t0) as f64 / (t1 - t0) as f64);
                let l = |a: i32, b: i32| {
                    (f64::from(a) + (f64::from(b) - f64::from(a)) * f).round() as i32
                };
                Some(Point::new(l(p0.x, p1.x), l(p0.y, p1.y)))
            }
            (Some((_, p)), _) | (None, Some((_, p))) => Some(*p),
            _ => None,
        }
    }

    /// Pointer position and opacity (0..1) at an output time. Fades when the pointer rests.
    pub fn cursor_at(&self, out_ms: u64) -> Option<(Point, f32)> {
        let src = self.out_to_src(out_ms)?;
        let p = self.pointer_src(src)?;
        let i = self.moves.partition_point(|(t, _)| *t <= src);
        let still_since = self.moves.get(..i).and_then(|m| {
            m.iter()
                .rev()
                .find(|(_, q)| {
                    (i64::from(q.x) - i64::from(p.x)).abs()
                        + (i64::from(q.y) - i64::from(p.y)).abs()
                        > 6
                })
                .map(|(t, _)| *t)
        });
        let rest = src.saturating_sub(still_since.unwrap_or(0));
        let alpha = if rest > CURSOR_FADE_AFTER_MS {
            (1.0 - (rest - CURSOR_FADE_AFTER_MS) as f32 / 600.0).clamp(0.0, 1.0)
        } else {
            1.0
        };
        Some((p, alpha))
    }

    /// Visible part of the source frame at `out_ms`.
    pub fn view(&self, out_ms: u64) -> Rect {
        let full = self.video;
        let base = match self.opts.aspect.ratio() {
            None => full,
            Some(ratio) => {
                // follow the pointer, averaged over the last 400 ms so the frame glides
                let mut sx = 0i64;
                let mut sy = 0i64;
                let mut n = 0i64;
                for k in 0..5u64 {
                    if let Some((p, _)) = self.cursor_at(out_ms.saturating_sub(k * 100)) {
                        sx += i64::from(p.x);
                        sy += i64::from(p.y);
                        n += 1;
                    }
                }
                let c = if n > 0 {
                    Point::new((sx / n) as i32, (sy / n) as i32)
                } else {
                    Point::new(full.x + (full.w / 2) as i32, full.y + (full.h / 2) as i32)
                };
                // largest window of that ratio inside the frame, centred on the pointer
                let around = Rect::new(c.x, c.y, 1, 1);
                let big = fit_aspect(Rect::new(full.x, full.y, full.w, full.h), ratio, full);
                let win = Rect::new(
                    around.x - (big.w / 2) as i32,
                    around.y - (big.h / 2) as i32,
                    big.w,
                    big.h,
                );
                fit_aspect(win, ratio, full)
            }
        };
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
        let Some((f, r)) = best else { return base };
        let l = |a: i32, b: i32| (f64::from(a) + (f64::from(b) - f64::from(a)) * f).round() as i32;
        let lu = |a: u32, b: u32| {
            (f64::from(a) + (f64::from(b) - f64::from(a)) * f)
                .round()
                .max(2.0) as u32
        };
        Rect::new(
            l(base.x, r.x),
            l(base.y, r.y),
            lu(base.w, r.w),
            lu(base.h, r.h),
        )
    }

    /// Active click rings: position and progress 0..1.
    pub fn rings_at(&self, out_ms: u64) -> Vec<(Point, f32)> {
        self.clicks
            .iter()
            .filter(|c| out_ms >= c.at_ms && out_ms < c.at_ms + RING_MS)
            .map(|c| (c.point, (out_ms - c.at_ms) as f32 / RING_MS as f32))
            .collect()
    }

    pub fn badge(&self, out_ms: u64) -> Option<&Badge> {
        self.badges
            .iter()
            .rev()
            .find(|b| out_ms >= b.start_ms && out_ms < b.end_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(t: u64, kind: EventKind, x: i32, y: i32) -> InputEvent {
        InputEvent {
            t_ms: t,
            kind,
            x,
            y,
            button: 0,
            chord: String::new(),
            rect: None,
        }
    }

    fn video() -> Rect {
        Rect::new(0, 0, 1000, 600)
    }

    fn log(events: Vec<InputEvent>, dur: u64) -> EventLog {
        EventLog {
            region: Rect::new(0, 0, 1000, 600),
            duration_ms: dur,
            events,
        }
    }

    #[test]
    fn idle_is_cut_and_typing_is_sped_up() {
        let mut e = vec![ev(0, EventKind::Move, 10, 10)];
        // typing 2 s at 100 ms spacing
        for i in 0..21 {
            e.push(ev(1000 + i * 100, EventKind::Type, 10, 10));
        }
        // nothing from 3.1 s until 12 s
        e.push(ev(12_000, EventKind::Move, 20, 20));
        let p = PolishPlan::build(&log(e, 13_000), 13_000, video(), &PolishOpts::default());
        assert!(p.total_ms < 13_000 - 5_000, "{}", p.total_ms);
        assert!(p.segs.iter().any(|s| s.speed_milli == 3000));
        // the typing seconds shrink: 2.3 s at 3x
        let fast = p.segs.iter().find(|s| s.speed_milli == 3000).unwrap();
        assert!(fast.out_end - fast.out_start < 1000);
        assert_eq!(p.out_to_src(p.total_ms), None);
    }

    #[test]
    fn nothing_is_cut_when_disabled() {
        let o = PolishOpts {
            trim_idle: false,
            typing_speedup: false,
            ..PolishOpts::default()
        };
        let p = PolishPlan::build(&log(vec![], 5000), 5000, video(), &o);
        assert_eq!(p.total_ms, 5000);
        assert_eq!(p.out_to_src(1234), Some(1234));
        assert_eq!(p.view(100), video());
    }

    #[test]
    fn clicks_zoom_and_cluster() {
        let mut d = ev(1000, EventKind::Down, 500, 300);
        d.rect = Some(Rect::new(480, 290, 60, 24));
        let mut d2 = ev(1400, EventKind::Down, 510, 300);
        d2.rect = d.rect;
        let o = PolishOpts {
            trim_idle: false,
            ..PolishOpts::default()
        };
        let p = PolishPlan::build(&log(vec![d, d2], 4000), 4000, video(), &o);
        assert_eq!(p.clicks.len(), 2);
        assert_eq!(p.shots.len(), 1, "close clicks share one zoom");
        let mid = p.view(1200);
        assert!(mid.w < 1000 && mid.w >= 580, "{mid:?}");
        assert!(video().intersect(&mid) == Some(mid));
        assert_eq!(p.rings_at(1100).len(), 1);
        assert_eq!(p.view(3900), video());
    }

    #[test]
    fn big_targets_do_not_zoom_and_bad_factors_are_safe() {
        assert!(zoom_rect(video(), Rect::new(0, 0, 900, 500), 1.7).is_none());
        assert!(zoom_rect(video(), Rect::new(10, 10, 20, 20), f32::NAN).is_none());
        assert!(zoom_rect(video(), Rect::new(0, 0, 0, 0), 2.0).is_none());
        let r = zoom_rect(video(), Rect::new(990, 590, 10, 10), 4.0).unwrap();
        assert_eq!(r.right(), 1000);
        assert_eq!(r.bottom(), 600);
        let bad = PolishOpts {
            zoom_factor: 0.0,
            ..PolishOpts::default()
        };
        assert!(bad.validate().is_err());
        assert!(PolishOpts::default().validate().is_ok());
        let p = PolishPlan::build(
            &EventLog::default(),
            0,
            Rect::default(),
            &PolishOpts::default(),
        );
        assert_eq!(p.total_ms, 0);
    }

    #[test]
    fn shortcuts_become_badges() {
        let mut k = ev(500, EventKind::Key, 0, 0);
        k.chord = "ctrl+s".into();
        let o = PolishOpts {
            trim_idle: false,
            ..PolishOpts::default()
        };
        let p = PolishPlan::build(&log(vec![k], 3000), 3000, video(), &o);
        assert_eq!(p.badge(600).map(|b| b.text.as_str()), Some("ctrl+s"));
        assert!(p.badge(2500).is_none());
    }

    #[test]
    fn cursor_glides_and_fades_when_still() {
        let e = vec![
            ev(0, EventKind::Move, 0, 0),
            ev(1000, EventKind::Move, 200, 100),
        ];
        let o = PolishOpts {
            trim_idle: false,
            ..PolishOpts::default()
        };
        let p = PolishPlan::build(&log(e, 6000), 6000, video(), &o);
        let (mid, a) = p.cursor_at(500).unwrap();
        assert!(mid.x > 60 && mid.x < 140, "{mid:?}");
        assert!(a > 0.99);
        let (_, faded) = p.cursor_at(5500).unwrap();
        assert!(faded < 0.1, "{faded}");
    }

    #[test]
    fn vertical_reframing_follows_the_pointer_inside_the_frame() {
        let e = vec![
            ev(0, EventKind::Move, 100, 300),
            ev(2000, EventKind::Move, 900, 300),
        ];
        let o = PolishOpts {
            aspect: Aspect::Vertical,
            trim_idle: false,
            ..PolishOpts::default()
        };
        let p = PolishPlan::build(&log(e, 3000), 3000, video(), &o);
        let a = p.view(0);
        let b = p.view(2900);
        for v in [a, b] {
            assert!(video().intersect(&v) == Some(v), "{v:?}");
            let r = f64::from(v.w) / f64::from(v.h);
            assert!((r - 9.0 / 16.0).abs() < 0.02, "{r}");
        }
        assert!(b.x > a.x + 200, "{a:?} {b:?}");
    }

    #[test]
    fn audio_follows_cuts_and_speed() {
        let o = PolishOpts {
            typing_speedup: false,
            ..PolishOpts::default()
        };
        // 1 s of audio where each frame's value is its millisecond
        let rate = crate::audio::RATE as usize / 1000;
        let pcm: Vec<i16> = (0..12_000 * rate)
            .flat_map(|f| {
                let v = (f / rate) as i16;
                [v, v]
            })
            .collect();
        let mut e = vec![ev(0, EventKind::Move, 1, 1)];
        e.push(ev(8000, EventKind::Move, 9, 9));
        let p = PolishPlan::build(&log(e, 12_000), 12_000, video(), &o);
        let a = p.retime_audio(&pcm);
        assert_eq!(a.len() as u64, p.total_ms * 48 * 2);
        // the first kept second is untouched; the cut means later audio is from later source
        assert_eq!(a[0], 0);
        assert!(p.total_ms < 12_000);
        assert!(PolishPlan::default().retime_audio(&pcm).is_empty());
    }

    #[test]
    fn extra_cuts_and_captions_follow_the_timeline() {
        let o = PolishOpts {
            trim_idle: false,
            typing_speedup: false,
            extra_cuts: vec![(2000, 3000)],
            ..PolishOpts::default()
        };
        let mut p = PolishPlan::build(&log(vec![], 6000), 6000, video(), &o);
        assert_eq!(p.total_ms, 5000);
        let line = |a, b, t: &str| crate::speech::Line {
            start_ms: a,
            end_ms: b,
            text: t.into(),
        };
        p.set_captions(&[
            line(500, 1500, "one"),
            line(2200, 2800, "gone"),
            line(3500, 4500, "two"),
            line(9000, 9500, "late"),
        ]);
        assert_eq!(p.captions.len(), 2);
        assert_eq!(p.caption(1000).map(|c| c.text.as_str()), Some("one"));
        assert_eq!(p.caption(2700).map(|c| c.text.as_str()), Some("two"));
        assert!(p.caption(2000).is_none());
        let bad = PolishOpts {
            extra_cuts: vec![(0, 1); 100_001],
            ..PolishOpts::default()
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn plan_survives_a_json_round_trip() {
        let p = PolishPlan::build(&log(vec![], 2000), 2000, video(), &PolishOpts::default());
        let j = serde_json::to_string(&p).unwrap();
        let q: PolishPlan = serde_json::from_str(&j).unwrap();
        assert_eq!(q.total_ms, p.total_ms);
        assert!(Aspect::parse("vertical").is_some() && Aspect::parse("x").is_none());
        assert!(CursorStyle::parse("ring").is_some() && CursorStyle::parse("x").is_none());
    }
}
