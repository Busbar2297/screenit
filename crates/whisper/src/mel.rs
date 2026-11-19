//! Mel filter bank for Whisper's front end: librosa's Slaney mel scale with Slaney area
//! normalisation, 16 kHz, 400-point FFT (201 bins), as used to train the models.

const SR: f64 = 16_000.0;
const N_FFT: usize = 400;
const BINS: usize = N_FFT / 2 + 1;

fn hz_to_mel(f: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = 6.4f64.ln() / 27.0;
    if f >= min_log_hz {
        min_log_mel + (f / min_log_hz).ln() / logstep
    } else {
        f / f_sp
    }
}

fn mel_to_hz(m: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = 6.4f64.ln() / 27.0;
    if m >= min_log_mel {
        min_log_hz * (logstep * (m - min_log_mel)).exp()
    } else {
        m * f_sp
    }
}

/// `n_mels x 201` weights, row-major.
pub(crate) fn filters(n_mels: usize) -> Vec<f32> {
    let n_mels = n_mels.clamp(1, 256);
    let top = hz_to_mel(SR / 2.0);
    let hz: Vec<f64> = (0..n_mels + 2)
        .map(|i| mel_to_hz(top * i as f64 / (n_mels + 1) as f64))
        .collect();
    let mut out = vec![0f32; n_mels * BINS];
    for i in 0..n_mels {
        let (lo, mid, hi) = (hz[i], hz[i + 1], hz[i + 2]);
        let norm = 2.0 / (hi - lo).max(f64::MIN_POSITIVE);
        for k in 0..BINS {
            let f = k as f64 * SR / N_FFT as f64;
            let up = (f - lo) / (mid - lo).max(f64::MIN_POSITIVE);
            let down = (hi - f) / (hi - mid).max(f64::MIN_POSITIVE);
            out[i * BINS + k] = (up.min(down).max(0.0) * norm) as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_has_the_right_shape_and_matches_librosa() {
        let f = filters(80);
        assert_eq!(f.len(), 80 * 201);
        assert!(f.iter().all(|v| v.is_finite() && *v >= 0.0));
        // librosa.filters.mel(sr=16000, n_fft=400, n_mels=80)[0][1] = 0.02486259
        assert!((f[1] - 0.024_862_59).abs() < 1e-4, "{}", f[1]);
        // Every filter has some weight.
        for i in 0..80 {
            assert!(f[i * 201..(i + 1) * 201].iter().any(|v| *v > 0.0), "{i}");
        }
        assert_eq!(filters(0).len(), 201);
    }

    #[test]
    fn mel_scale_round_trips() {
        for hz in [0.0, 300.0, 999.0, 1000.0, 4000.0, 8000.0] {
            assert!((mel_to_hz(hz_to_mel(hz)) - hz).abs() < 1e-6);
        }
    }
}
