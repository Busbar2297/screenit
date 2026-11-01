//! Audio timeline for recordings: several capture sources (microphone, system loopback) feed
//! 16-bit stereo PCM at [`RATE`]; the mixer sums them and hands out chunks stamped on the video
//! clock. Sources that fall silent (loopback delivers nothing while no sound plays) are padded
//! with zeros, and sources that run ahead are trimmed, so audio never drifts from the picture.

use std::collections::VecDeque;

pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;
/// Audio is emitted this far behind the video clock so late capture packets still make it in.
pub const LATENCY_MS: u64 = 120;
/// A source queue longer than this (beyond what is due) is trimmed from the front.
const MAX_QUEUE_MS: u64 = 400;
const SAMPLES_PER_MS: u64 = RATE as u64 * CHANNELS as u64 / 1000;

/// One encoded-ready chunk: interleaved stereo samples and its place on the timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioChunk {
    pub ts_100ns: i64,
    pub dur_100ns: i64,
    pub pcm: Vec<i16>,
}

#[derive(Debug, Default)]
pub struct AudioMixer {
    queues: Vec<VecDeque<i16>>,
    /// Frames (one sample per channel) handed out so far.
    written: u64,
}

impl AudioMixer {
    pub fn new(sources: usize) -> Self {
        Self {
            queues: vec![VecDeque::new(); sources],
            written: 0,
        }
    }

    /// Append interleaved stereo samples from `source`; unknown sources are ignored.
    pub fn push(&mut self, source: usize, pcm: &[i16]) {
        if let Some(q) = self.queues.get_mut(source) {
            q.extend(pcm.iter().copied());
        }
    }

    /// Drop everything queued (used while paused so paused audio never reaches the file).
    pub fn discard(&mut self) {
        for q in &mut self.queues {
            q.clear();
        }
    }

    /// The next chunk due at video time `media_ms`, or `None` if less than 10 ms is due.
    pub fn take_due(&mut self, media_ms: u64) -> Option<AudioChunk> {
        let target = media_ms
            .saturating_sub(LATENCY_MS)
            .saturating_mul(u64::from(RATE))
            / 1000;
        let due = target.saturating_sub(self.written);
        if due < u64::from(RATE / 100) {
            return None;
        }
        let frames = due.min(u64::from(RATE)) as usize;
        let samples = frames * usize::from(CHANNELS);
        let mut acc = vec![0i32; samples];
        let max = (MAX_QUEUE_MS * SAMPLES_PER_MS) as usize + samples;
        for q in &mut self.queues {
            if q.len() > max {
                let extra = q.len() - max;
                q.drain(..extra);
            }
            let n = q.len().min(samples);
            for (a, v) in acc.iter_mut().zip(q.drain(..n)) {
                *a += i32::from(v);
            }
        }
        let pcm = acc
            .into_iter()
            .map(|v| v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16)
            .collect();
        let ts_100ns = (self.written.saturating_mul(10_000_000) / u64::from(RATE)) as i64;
        let dur_100ns = (frames as u64 * 10_000_000 / u64::from(RATE)) as i64;
        self.written += frames as u64;
        Some(AudioChunk {
            ts_100ns,
            dur_100ns,
            pcm,
        })
    }

    /// Everything still owed up to `media_ms`, latency included (end of recording). At most a few
    /// seconds are produced; a bogus clock cannot make this loop forever.
    pub fn flush(&mut self, media_ms: u64) -> Vec<AudioChunk> {
        let mut out = Vec::new();
        while out.len() < 8 {
            match self.take_due(media_ms.saturating_add(LATENCY_MS)) {
                Some(c) => out.push(c),
                None => break,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Vec<i16> {
        vec![100; (n * SAMPLES_PER_MS) as usize]
    }

    #[test]
    fn nothing_due_inside_the_latency_window() {
        let mut m = AudioMixer::new(1);
        m.push(0, &ms(50));
        assert!(m.take_due(LATENCY_MS).is_none());
        assert!(m.take_due(LATENCY_MS + 5).is_none());
    }

    #[test]
    fn chunks_are_contiguous_and_follow_the_clock() {
        let mut m = AudioMixer::new(1);
        m.push(0, &ms(1000));
        let a = m.take_due(LATENCY_MS + 100).unwrap();
        assert_eq!(a.ts_100ns, 0);
        assert_eq!(a.dur_100ns, 100 * 10_000);
        assert_eq!(a.pcm.len() as u64, 100 * SAMPLES_PER_MS);
        let b = m.take_due(LATENCY_MS + 250).unwrap();
        assert_eq!(b.ts_100ns, a.dur_100ns);
        assert_eq!(b.dur_100ns, 150 * 10_000);
    }

    #[test]
    fn silent_source_is_padded_with_zeros() {
        let mut m = AudioMixer::new(2);
        m.push(0, &ms(20));
        let c = m.take_due(LATENCY_MS + 40).unwrap();
        let half = c.pcm.len() / 2;
        assert!(c.pcm[..half].iter().all(|&v| v == 100));
        assert!(c.pcm[half..].iter().all(|&v| v == 0));
    }

    #[test]
    fn sources_are_summed_with_clipping() {
        let mut m = AudioMixer::new(2);
        m.push(0, &vec![30_000; 960]);
        m.push(1, &vec![30_000; 960]);
        let c = m.take_due(LATENCY_MS + 10).unwrap();
        assert!(c.pcm.iter().all(|&v| v == i16::MAX));
    }

    #[test]
    fn runaway_queue_is_trimmed_from_the_front() {
        let mut m = AudioMixer::new(1);
        m.push(0, &vec![1; (2000 * SAMPLES_PER_MS) as usize]);
        m.push(0, &vec![2; (10 * SAMPLES_PER_MS) as usize]);
        let _ = m.take_due(LATENCY_MS + 10).unwrap();
        // Only ~MAX_QUEUE_MS of backlog survives, so the newest samples arrive soon.
        let rest = m.flush(LATENCY_MS + 10 + MAX_QUEUE_MS);
        assert!(rest.iter().flat_map(|c| c.pcm.iter()).any(|&v| v == 2));
    }

    #[test]
    fn discard_drops_paused_audio_and_huge_jumps_are_chunked() {
        let mut m = AudioMixer::new(1);
        m.push(0, &ms(100));
        m.discard();
        let c = m.take_due(LATENCY_MS + 100).unwrap();
        assert!(c.pcm.iter().all(|&v| v == 0));
        let big = m.take_due(u64::MAX / 1000).unwrap();
        assert_eq!(big.dur_100ns, 10_000_000);
        m.push(7, &ms(10)); // unknown source: ignored, no panic
    }
}
