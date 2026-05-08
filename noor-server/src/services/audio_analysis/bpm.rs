//! BPM detection via spectral-flux ODF + tempogram + Ellis DP beat tracker.
//!
//! Public API unchanged: `detect_bpm(samples, sample_rate) -> Option<(bpm, strength)>`.
//! See `onset`, `tempo`, and `beat_tracker` submodules for the algorithmic detail.

use super::beat_tracker::{self, BeatTrack};
use super::onset::compute_onset_envelope;
use super::tempo::{self, TempoEstimate};

const MIN_TEMPO_STRENGTH: f64 = 0.15;
const MIN_BEAT_STRENGTH: f64 = 0.10;

pub fn detect_bpm(samples: &[f32], sample_rate: u32) -> Option<(f64, f64)> {
    let env = compute_onset_envelope(samples, sample_rate)?;
    let TempoEstimate { bpm, strength } = tempo::estimate_tempo(&env)?;
    if strength < MIN_TEMPO_STRENGTH {
        return None;
    }

    let BeatTrack { strength: beat_strength, .. } = beat_tracker::track_beats(&env, bpm)?;
    if beat_strength < MIN_BEAT_STRENGTH {
        return None;
    }

    // Combine the two confidence scores: geometric mean keeps either failing
    // from ever producing a high final score.
    let combined = (strength * beat_strength).sqrt();
    Some((bpm, combined))
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
        let (bpm, conf) = detect_bpm(&s, 44100).expect("should detect");
        assert!((bpm - 120.0).abs() < 3.0, "got {}", bpm);
        assert!(conf > 0.2, "confidence too low: {}", conf);
    }

    #[test]
    fn detects_174_bpm_dnb_not_half() {
        let s = click_train(44100, 8.0, 60.0 / 174.0);
        let (bpm, _) = detect_bpm(&s, 44100).expect("should detect");
        assert!((bpm - 174.0).abs() < 4.0, "DnB regression: got {}", bpm);
    }

    #[test]
    fn reggae_eighths_resolve_to_quarter() {
        // The original user-reported bug: equal-energy onsets every eighth note
        // should yield the quarter-note BPM, not the doubled value.
        let s = click_train(44100, 8.0, 30.0 / 80.0); // eighths at quarter=80
        let (bpm, _) = detect_bpm(&s, 44100).expect("should detect");
        assert!(
            (bpm - 80.0).abs() < 3.0,
            "reggae regression (Pressure Drop bug): got {}",
            bpm,
        );
    }
}
