//! BPM detection via libaubio (aubio-rs).
//!
//! Public API unchanged: `detect_bpm(samples, sample_rate) -> Option<(bpm, confidence)>`.
//!
//! Replaces the previous hand-rolled ODF + autocorrelation + Ellis DP beat
//! tracker chain. aubio uses Davies/Plumbley 2007 (spectral flux onset + beat
//! tracking). Not state-of-the-art (Madmom/TempoCNN beat it) but battle-tested
//! and avoids the threshold-tuning rabbit hole of the hand-rolled detector.

use aubio_rs::{OnsetMode, Tempo};

/// FFT window for the spectral-flux onset detector. 1024 samples ≈ 23 ms at
/// 44.1 kHz, the standard for MIR tempo work.
const BUF_SIZE: usize = 1024;
/// Hop between successive frames. 512 = 50% overlap, also MIR standard.
const HOP_SIZE: usize = 512;
/// Reject results outside this band — outliers are almost always artifacts.
const MIN_BPM: f64 = 40.0;
const MAX_BPM: f64 = 240.0;
/// Aubio confidence floor. Empirically silence/noise sit near 0; a clean beat
/// hits 0.3+. Hand-tuned to admit real songs while filtering pure noise.
const MIN_CONFIDENCE: f64 = 0.05;

pub fn detect_bpm(samples: &[f32], sample_rate: u32) -> Option<(f64, f64)> {
    if samples.len() < BUF_SIZE * 2 || sample_rate == 0 {
        return None;
    }

    let mut tempo = Tempo::new(OnsetMode::SpecFlux, BUF_SIZE, HOP_SIZE, sample_rate).ok()?;

    // Feed audio in HOP_SIZE-aligned chunks; the trailing partial chunk is
    // discarded (≤ 12 ms at 44.1 kHz, well below detection resolution).
    for chunk in samples.chunks_exact(HOP_SIZE) {
        let _ = tempo.do_result(chunk);
    }

    let bpm = tempo.get_bpm() as f64;
    let confidence = tempo.get_confidence() as f64;

    if !bpm.is_finite()
        || bpm < MIN_BPM
        || bpm > MAX_BPM
        || !confidence.is_finite()
        || confidence < MIN_CONFIDENCE
    {
        return None;
    }

    Some((bpm, confidence))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click_train(sr: u32, seconds: f64, period_seconds: f64) -> Vec<f32> {
        let total = (sr as f64 * seconds) as usize;
        let period = (sr as f64 * period_seconds) as usize;
        let mut out = vec![0.0f32; total];
        let mut t = 0usize;
        while t < total {
            for j in 0..32 {
                if t + j < out.len() {
                    out[t + j] = 1.0;
                }
            }
            t += period;
        }
        out
    }

    #[test]
    fn rejects_short_input() {
        assert!(detect_bpm(&vec![0.0f32; 1000], 44100).is_none());
    }

    #[test]
    fn rejects_silence() {
        assert!(detect_bpm(&vec![0.0f32; 44100 * 8], 44100).is_none());
    }

    #[test]
    fn detects_120_bpm() {
        let s = click_train(44100, 8.0, 0.5);
        let (bpm, _) = detect_bpm(&s, 44100).expect("should detect");
        assert!((bpm - 120.0).abs() < 3.0, "got {}", bpm);
    }
}
