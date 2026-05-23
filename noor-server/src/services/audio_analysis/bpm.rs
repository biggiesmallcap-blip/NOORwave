//! BPM detection via the vendored `madmom_beats_port_core` crate
//! (Rust port of madmom's RNN beat tracker + DBN downbeat decoder).
//!
//! Pipeline:
//!   1. If the input isn't 44.1kHz mono, linearly resample it. The BLSTM is
//!      trained on 44.1kHz; passing other rates fails config validation.
//!   2. Run `analyze_with_model_data` with the model JSON / NPZ embedded into
//!      the binary via `include_str!` / `include_bytes!` — no disk I/O, no
//!      runtime model lookup.
//!   3. Derive a global BPM from the median inter-beat interval (more robust
//!      than the mean against missing or doubled beats at the head/tail).
//!   4. Report mean beat confidence as the second tuple value.
//!
//! Public API unchanged: `detect_bpm(samples, sample_rate) -> Option<(bpm, confidence)>`.

use madmom_beats_port_core::{CoreConfig, analyze_with_model_data};

/// Model JSON (graph + layer metadata). 68 KB.
const MODEL_JSON: &str =
    include_str!("../../../vendor/madmom_beats_port_core/models/downbeats_blstm.json");
/// Model weights (NPZ archive). 3.3 MB. Bumps the binary by that much; in
/// practice negligible.
const MODEL_WEIGHTS: &[u8] =
    include_bytes!("../../../vendor/madmom_beats_port_core/models/downbeats_blstm_weights.npz");

/// Target sample rate for the BLSTM. Hard-coded by the model.
const TARGET_SAMPLE_RATE: u32 = 44_100;
/// Minimum clip length the model can analyse meaningfully. Below this the DBN
/// emits zero beats and the inter-beat-interval calculation collapses.
const MIN_SAMPLES: usize = TARGET_SAMPLE_RATE as usize * 4; // 4 seconds

pub fn detect_bpm(samples: &[f32], sample_rate: u32) -> Option<(f64, f64)> {
    if sample_rate == 0 || samples.is_empty() {
        return None;
    }

    let resampled = if sample_rate == TARGET_SAMPLE_RATE {
        std::borrow::Cow::Borrowed(samples)
    } else {
        std::borrow::Cow::Owned(resample_linear(samples, sample_rate, TARGET_SAMPLE_RATE))
    };
    if resampled.len() < MIN_SAMPLES {
        return None;
    }

    let config = CoreConfig::default();
    let result = analyze_with_model_data(
        resampled.as_ref(),
        TARGET_SAMPLE_RATE,
        &config,
        MODEL_JSON,
        MODEL_WEIGHTS,
    )
    .ok()?;

    bpm_from_analysis(&result.beat_times, &result.beat_confidences)
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BeatGridAnalysis {
    pub bpm: f64,
    pub confidence: f64,
    pub beats_seconds: Vec<f32>,
    pub downbeats_seconds: Vec<f32>,
}

pub fn analyze_beat_grid(samples: &[f32], sample_rate: u32) -> Option<BeatGridAnalysis> {
    if sample_rate == 0 || samples.is_empty() {
        return None;
    }

    let resampled = if sample_rate == TARGET_SAMPLE_RATE {
        std::borrow::Cow::Borrowed(samples)
    } else {
        std::borrow::Cow::Owned(resample_linear(samples, sample_rate, TARGET_SAMPLE_RATE))
    };
    if resampled.len() < MIN_SAMPLES {
        return None;
    }

    let config = CoreConfig::default();
    let result = analyze_with_model_data(
        resampled.as_ref(),
        TARGET_SAMPLE_RATE,
        &config,
        MODEL_JSON,
        MODEL_WEIGHTS,
    )
    .ok()?;
    let (bpm, confidence) = bpm_from_analysis(&result.beat_times, &result.beat_confidences)?;
    let downbeats_seconds = result
        .beat_times
        .iter()
        .zip(result.beat_numbers.iter())
        .filter_map(|(time, beat_number)| (*beat_number == 1).then_some(*time))
        .collect::<Vec<_>>();

    Some(BeatGridAnalysis {
        bpm,
        confidence,
        beats_seconds: result.beat_times,
        downbeats_seconds,
    })
}

/// Compute (bpm, mean_confidence) from beat-time + per-beat-confidence arrays.
/// Returned as `pub(crate)` so the unit tests can exercise it without spinning
/// up the full model.
pub(crate) fn bpm_from_analysis(beat_times: &[f32], confidences: &[f32]) -> Option<(f64, f64)> {
    if beat_times.len() < 4 {
        return None;
    }
    let mut intervals: Vec<f64> = beat_times
        .windows(2)
        .map(|w| (w[1] - w[0]) as f64)
        .filter(|d| *d > 0.0)
        .collect();
    if intervals.len() < 3 {
        return None;
    }
    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = intervals[intervals.len() / 2];
    if median <= 0.0 {
        return None;
    }
    let bpm = 60.0 / median;
    let confidence: f64 = if confidences.is_empty() {
        0.0
    } else {
        confidences.iter().map(|c| *c as f64).sum::<f64>() / confidences.len() as f64
    };
    if !bpm.is_finite() || !(30.0..=240.0).contains(&bpm) {
        return None;
    }
    Some((bpm, confidence))
}

/// Linear-interpolation resampler. Beat tracking cares about low-frequency
/// periodicity (rhythm), not high-frequency fidelity, so linear interp is
/// adequate here and avoids pulling rubato setup into this module.
fn resample_linear(input: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if src_rate == dst_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = dst_rate as f64 / src_rate as f64;
    let out_len = ((input.len() as f64) * ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    let last_idx = input.len() - 1;
    for i in 0..out_len {
        let src_pos = (i as f64) / ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - (src_idx as f64);
        if src_idx >= last_idx {
            out.push(input[last_idx]);
        } else {
            let a = input[src_idx] as f64;
            let b = input[src_idx + 1] as f64;
            out.push((a + (b - a) * frac) as f32);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bpm_from_uniform_beats() {
        // Beats at 120 BPM = every 0.5 s for 8 seconds = 16 beats.
        let beats: Vec<f32> = (0..16).map(|i| 0.5 * (i as f32)).collect();
        let confs = vec![0.9; beats.len()];
        let (bpm, conf) = bpm_from_analysis(&beats, &confs).expect("should detect");
        assert!((bpm - 120.0).abs() < 1.0, "got {}", bpm);
        assert!((conf - 0.9).abs() < 1e-6);
    }

    #[test]
    fn bpm_uses_median_robust_to_outliers() {
        // 119, 120, 121, 122, 123 BPM intervals plus one wildly missed beat.
        // Median should pick 120 BPM, mean would skew.
        let mut beats = vec![0.0f32];
        let intervals = [0.5044, 0.5, 0.4959, 0.4918, 1.5]; // last interval = missed beat
        let mut t = 0.0f32;
        for d in intervals {
            t += d;
            beats.push(t);
        }
        let confs = vec![0.9; beats.len()];
        let (bpm, _) = bpm_from_analysis(&beats, &confs).expect("should detect");
        assert!((bpm - 120.0).abs() < 3.0, "got {}", bpm);
    }

    #[test]
    fn bpm_rejects_too_few_beats() {
        let beats = vec![0.0f32, 0.5, 1.0]; // 3 beats → 2 intervals, below the 3-interval floor
        let confs = vec![0.9; 3];
        assert!(bpm_from_analysis(&beats, &confs).is_none());
    }

    #[test]
    fn bpm_rejects_out_of_range_tempo() {
        // 300 BPM is outside the [30, 240] sanity band.
        let beats: Vec<f32> = (0..20).map(|i| 0.2 * (i as f32)).collect();
        let confs = vec![0.9; beats.len()];
        assert!(bpm_from_analysis(&beats, &confs).is_none());
    }

    #[test]
    fn resample_identity_when_rates_match() {
        let input = vec![1.0, 2.0, 3.0, 4.0];
        let out = resample_linear(&input, 44_100, 44_100);
        assert_eq!(out, input);
    }

    #[test]
    fn resample_48k_to_44_1k_length_within_one_sample() {
        let input = vec![0.0f32; 48_000];
        let out = resample_linear(&input, 48_000, 44_100);
        let expected = 44_100.0;
        assert!(
            (out.len() as f32 - expected).abs() <= 1.0,
            "got {}",
            out.len()
        );
    }

    #[test]
    fn rejects_short_clip() {
        let s = vec![0.0f32; 1000];
        assert!(detect_bpm(&s, 44_100).is_none());
    }

    #[test]
    fn rejects_zero_sample_rate() {
        let s = vec![0.0f32; 44_100 * 8];
        assert!(detect_bpm(&s, 0).is_none());
    }

    #[test]
    fn beat_grid_analysis_rejects_short_clip() {
        let s = vec![0.0f32; 1000];
        assert!(analyze_beat_grid(&s, 44_100).is_none());
    }

    #[test]
    fn beat_grid_analysis_rejects_zero_sample_rate() {
        let s = vec![0.0f32; 44_100 * 8];
        assert!(analyze_beat_grid(&s, 0).is_none());
    }

    #[test]
    fn bpm_detect_bpm_still_works_for_existing_callers() {
        let beats: Vec<f32> = (0..16).map(|i| 0.5 * (i as f32)).collect();
        let confs = vec![0.8; beats.len()];
        let (bpm, conf) = bpm_from_analysis(&beats, &confs).expect("bpm");
        assert!((bpm - 120.0).abs() < 1.0);
        assert!((conf - 0.8).abs() < 1e-6);
    }

    /// End-to-end smoke test: synthesize a 120 BPM click train at 44.1kHz,
    /// run it through the full pipeline (features → BLSTM → DBN), and confirm
    /// the recovered tempo lands within ±5 BPM of the target.
    /// Marked `#[ignore]` because loading the 3.3 MB model + running inference
    /// takes ~5 s; we don't want to slow every `cargo test` invocation.
    /// Run with `cargo test detects_120_bpm_click_train -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn detects_120_bpm_click_train() {
        let sr = 44_100u32;
        let secs = 10usize;
        let mut samples = vec![0.0f32; sr as usize * secs];
        let period = (sr as f64 * 60.0 / 120.0) as usize; // every 0.5 s
        for start in (0..samples.len()).step_by(period) {
            for j in 0..64 {
                if start + j < samples.len() {
                    samples[start + j] = 1.0;
                }
            }
        }
        let (bpm, _conf) = detect_bpm(&samples, sr).expect("should detect");
        assert!((bpm - 120.0).abs() < 5.0, "got {} (target 120)", bpm);
    }
}
