//! Audio analysis and clean-up on interleaved 48 kHz stereo samples: silence detection, filler
//! word ranges from a transcript, range merging, mixing and RNNoise-based noise reduction.

use crate::audio::{CHANNELS, RATE};
use crate::speech::Transcript;

/// Words cut by default (lower case, no punctuation).
pub const FILLERS: [&str; 9] = ["um", "uh", "uhm", "umm", "er", "erm", "hmm", "mm", "ah"];

fn frames_per_ms() -> usize {
    (RATE / 1000) as usize
}

/// Source ranges `(start_ms, end_ms)` of silence at least `min_ms` long. A range is shrunk by
/// `keep_ms` on each side so cuts leave a natural pause. `threshold_db` is dBFS (e.g. -45).
pub fn silent_ranges(pcm: &[i16], min_ms: u32, keep_ms: u32, threshold_db: f32) -> Vec<(u64, u64)> {
    let ch = usize::from(CHANNELS);
    let win = 20 * frames_per_ms();
    let limit = 32768.0 * 10f64.powf(f64::from(threshold_db.clamp(-90.0, -5.0)) / 20.0);
    let total_frames = pcm.len() / ch;
    let mut out = Vec::new();
    let mut run: Option<u64> = None;
    let n_win = total_frames.div_ceil(win);
    for w in 0..=n_win {
        let quiet = if w < n_win {
            let a = w * win * ch;
            let b = ((w + 1) * win * ch).min(pcm.len());
            let s = pcm.get(a..b).unwrap_or(&[]);
            let sum: f64 = s.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
            let rms = (sum / s.len().max(1) as f64).sqrt();
            rms < limit
        } else {
            false
        };
        let t = (w * 20) as u64;
        match (quiet, run) {
            (true, None) => run = Some(t),
            (false, Some(start)) => {
                let end = t.min((total_frames / frames_per_ms()) as u64);
                run = None;
                if end.saturating_sub(start) >= u64::from(min_ms) {
                    let (a, b) = (
                        start + u64::from(keep_ms),
                        end.saturating_sub(u64::from(keep_ms)),
                    );
                    if b > a {
                        out.push((a, b));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn norm(w: &str) -> String {
    w.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Ranges of filler words (`fillers`, or [`FILLERS`] when empty) with a little slack each side.
pub fn filler_ranges(t: &Transcript, fillers: &[String]) -> Vec<(u64, u64)> {
    let list: Vec<String> = if fillers.is_empty() {
        FILLERS.iter().map(|s| (*s).to_string()).collect()
    } else {
        fillers.iter().map(|f| norm(f)).collect()
    };
    t.words
        .iter()
        .filter(|w| list.contains(&norm(&w.text)))
        .map(|w| (w.start_ms.saturating_sub(40), w.end_ms.saturating_add(40)))
        .collect()
}

/// Sort and join overlapping or touching ranges.
pub fn merge_ranges(mut r: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    r.retain(|(a, b)| b > a);
    r.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::with_capacity(r.len());
    for (a, b) in r {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Add `clip` (stereo, 48 kHz) into `base` starting at `at_ms`, growing `base` with silence when
/// the clip runs past the end only up to `max_len` samples.
pub fn mix_in(base: &mut Vec<i16>, clip: &[i16], at_ms: u64, max_len: usize) {
    let ch = usize::from(CHANNELS);
    let start = (at_ms as usize)
        .saturating_mul(frames_per_ms())
        .saturating_mul(ch);
    let end = start.saturating_add(clip.len()).min(max_len);
    if end <= start {
        return;
    }
    if base.len() < end {
        base.resize(end, 0);
    }
    for (i, v) in clip.iter().take(end - start).enumerate() {
        let s = i32::from(base[start + i]) + i32::from(*v);
        base[start + i] = s.clamp(-32768, 32767) as i16;
    }
}

/// Reduce steady background noise (fans, hiss, room tone) with the RNNoise model.
pub fn denoise(pcm: &[i16]) -> Vec<i16> {
    use nnnoiseless::{DenoiseState, FRAME_SIZE};
    let ch = usize::from(CHANNELS);
    let frames = pcm.len() / ch;
    let mut out = vec![0i16; frames * ch];
    for c in 0..ch {
        let mut st = DenoiseState::new();
        let mut input = [0f32; FRAME_SIZE];
        let mut output = [0f32; FRAME_SIZE];
        let mut pos = 0usize;
        let mut first = true;
        while pos < frames {
            let n = FRAME_SIZE.min(frames - pos);
            input.fill(0.0);
            for (i, slot) in input.iter_mut().take(n).enumerate() {
                *slot = f32::from(pcm[(pos + i) * ch + c]);
            }
            st.process_frame(&mut output, &input);
            // the model's first output frame is a fade-in artefact: keep the input instead
            for i in 0..n {
                let v = if first { input[i] } else { output[i] };
                out[(pos + i) * ch + c] = v.round().clamp(-32768.0, 32767.0) as i16;
            }
            first = false;
            pos += n;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::Word;

    fn tone(ms: usize, amp: i16) -> Vec<i16> {
        (0..ms * 48 * 2)
            .map(|i| if (i / 2 / 12) % 2 == 0 { amp } else { -amp })
            .collect()
    }

    #[test]
    fn finds_long_silences_and_keeps_a_margin() {
        let mut pcm = tone(1000, 8000);
        pcm.extend(vec![0i16; 3000 * 48 * 2]);
        pcm.extend(tone(1000, 8000));
        let r = silent_ranges(&pcm, 1500, 300, -45.0);
        assert_eq!(r.len(), 1);
        assert!((r[0].0 as i64 - 1300).abs() <= 20, "{r:?}");
        assert!((r[0].1 as i64 - 3700).abs() <= 20, "{r:?}");
        // too short to matter, and an all-quiet clip
        assert!(silent_ranges(&pcm, 5000, 0, -45.0).is_empty());
        assert_eq!(silent_ranges(&vec![0; 48_000 * 2], 500, 0, -45.0).len(), 1);
        assert!(silent_ranges(&[], 100, 0, -45.0).is_empty());
        assert!(silent_ranges(&pcm, 100, 0, f32::NAN).len() <= 1);
    }

    #[test]
    fn fillers_and_merging() {
        let w = |a, b, t: &str| Word {
            start_ms: a,
            end_ms: b,
            text: t.into(),
        };
        let t = Transcript {
            words: vec![
                w(0, 200, "So,"),
                w(300, 500, "Um,"),
                w(900, 1000, "uh"),
                w(1010, 1100, "go"),
            ],
            ..Transcript::default()
        };
        let f = filler_ranges(&t, &[]);
        assert_eq!(f, vec![(260, 540), (860, 1040)]);
        assert_eq!(filler_ranges(&t, &["go".into()]), vec![(970, 1140)]);
        assert_eq!(
            merge_ranges(vec![(10, 20), (5, 12), (30, 30), (20, 25), (40, 50)]),
            vec![(5, 25), (40, 50)]
        );
    }

    #[test]
    fn mixing_clamps_and_respects_the_cap() {
        let mut base = vec![30_000i16; 48 * 2 * 10];
        let cap = base.len();
        mix_in(&mut base, &[10_000; 200], 0, cap);
        assert_eq!(base[0], 32767);
        assert_eq!(base[300], 30_000);
        let mut b = Vec::new();
        mix_in(&mut b, &[5; 100], 1, 150 * 2);
        assert!(b.len() <= 300 && b.len() > 96);
        mix_in(&mut b, &[5; 100], 10_000_000, 1000);
        assert!(b.len() <= 300);
    }

    #[test]
    fn denoise_keeps_length_and_reduces_noise() {
        // deterministic hiss
        let mut x = 12345u32;
        let hiss: Vec<i16> = (0..48_000 * 2)
            .map(|_| {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                ((x >> 16) as i32 % 600 - 300) as i16
            })
            .collect();
        let d = denoise(&hiss);
        assert_eq!(d.len(), hiss.len());
        let e = |v: &[i16]| v.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / v.len() as f64;
        assert!(e(&d[48_000..]) < e(&hiss[48_000..]) * 0.9);
        assert!(denoise(&[]).is_empty());
        assert_eq!(denoise(&[1, 2, 3]).len(), 2);
    }
}
