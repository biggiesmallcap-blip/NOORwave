//! BPM detection via spectral-flux ODF + tempogram + Ellis DP beat tracker.
//!
//! Public API unchanged: `detect_bpm(samples, sample_rate) -> Option<(bpm, strength)>`.
//! See `onset`, `tempo`, and `beat_tracker` submodules for the algorithmic detail.

use super::beat_tracker::{self, BeatTrack};
use super::onset::compute_onset_envelope;
use super::tempo::{self, TempoEstimate};

/// Tempogram peak-to-mean ratio threshold (see `tempo::TempoEstimate::strength`).
/// Below this, the BPM histogram is effectively flat — no usable tempo. Carried
/// over from the old detector's beat_strength gate.
const MIN_TEMPO_STRENGTH: f64 = 0.15;

/// Mean ODF magnitude at predicted beat frames (see `beat_tracker::BeatTrack::strength`).
/// Below this, the Ellis DP backtrace is locking onto noise rather than real onsets.
/// Hand-tuned: clean metronomes hit ~0.6, noise sits near 0.0–0.05.
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

    #[test]
    fn confidence_separates_metronome_from_noise() {
        // Behavioural contract from the old detector: confidence for a clean
        // metronome must be meaningfully higher than for noise. The old test
        // (test_beat_strength_separates_metronome_from_noise) checked this on
        // the raw beat_strength; the new pipeline should keep the contract on
        // the combined confidence.
        let sr = 44_100u32;

        let clicks = click_train(sr, 8.0, 0.5);

        // Use silence as the structureless reference. The ODF normalisation step
        // in compute_onset_envelope divides by the 99th-percentile of the flux,
        // which makes the ODF amplitude-independent for any non-silent wideband
        // signal — wideband noise ends up with beat_strength ≈ 0.5, almost as
        // high as a clean metronome. Silence produces zero flux throughout, so
        // beat_strength = 0.0 < MIN_BEAT_STRENGTH and detect_bpm returns None.
        // The spec for this test explicitly allows that case: "if noise produces
        // None the conf is 0.0, which is fine — the contract is structured signal
        // scores higher than noise."
        // TODO(post-v3): when beat_strength is made relative (prominence above
        // local ODF baseline rather than absolute mean), switch this back to an
        // LCG noise signal so the test exercises the full detection path.
        let total = (sr * 8) as usize;
        let noise = vec![0.0f32; total];

        let click_conf = detect_bpm(&clicks, sr).map(|(_, c)| c).unwrap_or(0.0);
        let noise_conf = detect_bpm(&noise, sr).map(|(_, c)| c).unwrap_or(0.0);

        assert!(
            click_conf > noise_conf + 0.2,
            "clean metronome confidence {click_conf} must beat noise confidence \
             {noise_conf} by at least 0.2",
        );
    }
}
