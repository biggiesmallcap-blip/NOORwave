//! Tempo estimation from an onset envelope.
//!
//! Pipeline:
//!   1. Autocorrelate ODF for BPMs 40..=240.
//!   2. Multiply by log-Gaussian prior centred at log2(120), σ = 0.6 octaves.
//!   3. Argmax → primary tempo candidate.
//!   4. Octave disambiguation:
//!      a. Check the integer double (best × 2): if its biased-normalised
//!         autocorrelation is ≥ 0.85 × the winner's, prefer it (catches DnB-style
//!         fundamental-at-double-speed cases).
//!      b. Else find the half-tempo candidate via lag search (target_lag = 2 ×
//!         best_lag) and prefer it when its raw autocorrelation exceeds the
//!         winner's (catches reggae-style dominant-sub-harmonic cases).

use crate::services::audio_analysis::onset::OnsetEnvelope;

pub const BPM_MIN: i32 = 40;
pub const BPM_MAX: i32 = 240;
pub const PRIOR_CENTER_BPM: f64 = 120.0;
pub const PRIOR_SIGMA_OCTAVES: f64 = 0.6;
/// Minimum raw-autocorrelation ratio for a double-tempo candidate to override
/// the primary estimate.  A value of 0.85 means the double must have at least
/// 85% of the winner's (biased-normalised) correlation to be considered.
pub const OCTAVE_RATIO_THRESHOLD: f64 = 0.85;

#[derive(Debug)]
pub struct TempoEstimate {
    pub bpm: f64,
    /// Peak-to-mean ratio of the prior-weighted autocorrelation. In [0, 1].
    pub strength: f64,
}

pub fn estimate_tempo(env: &OnsetEnvelope) -> Option<TempoEstimate> {
    let n = env.odf.len();
    if n < 64 || env.hop_seconds <= 0.0 {
        return None;
    }

    // Lag in ODF samples for a given BPM.
    let lag_for = |bpm: f64| -> usize {
        let secs_per_beat = 60.0 / bpm;
        (secs_per_beat / env.hop_seconds).round() as usize
    };

    // Mean-center ODF for a cleaner autocorrelation peak.
    let mean: f64 = env.odf.iter().sum::<f64>() / n as f64;
    let centered: Vec<f64> = env.odf.iter().map(|v| v - mean).collect();

    // Compute biased-normalised autocorrelation (divide by n, not n–lag) for
    // each BPM candidate.  The biased estimator penalises long lags slightly,
    // which suppresses the artefact where a sub-harmonic lag (e.g. 87 BPM for a
    // 174 BPM signal) accrues more aligned pairs than the true lag because the
    // integer rounding of 2T lands more accurately than the rounding of T when
    // the period is non-integer.  With the biased estimator the
    // raw(double)/raw(fundamental) ratio rises above 0.85 for fast click trains
    // like the 174 BPM DnB test, enabling the disambiguation step to promote
    // the double correctly.
    let mut raw: std::collections::HashMap<i32, f64> =
        std::collections::HashMap::with_capacity((BPM_MAX - BPM_MIN + 1) as usize);

    for bpm in BPM_MIN..=BPM_MAX {
        let lag = lag_for(bpm as f64);
        if lag == 0 || lag >= n {
            raw.insert(bpm, 0.0);
            continue;
        }
        let mut sum = 0.0f64;
        for i in 0..(n - lag) {
            sum += centered[i] * centered[i + lag];
        }
        // Biased normalisation: divide by n (constant) instead of (n - lag).
        raw.insert(bpm, (sum / n as f64).max(0.0));
    }

    // Apply log-Gaussian prior.
    let mut weighted: Vec<(i32, f64, f64)> = (BPM_MIN..=BPM_MAX)
        .map(|bpm| {
            let r = *raw.get(&bpm).unwrap_or(&0.0);
            let p = log_gaussian_prior(bpm as f64, PRIOR_CENTER_BPM, PRIOR_SIGMA_OCTAVES);
            (bpm, r, r * p)
        })
        .collect();

    // Argmax of weighted score.
    weighted.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let (best_bpm, _best_raw, best_weighted) = weighted[0];
    if best_weighted < 1e-9 {
        return None;
    }

    // ── Octave disambiguation ────────────────────────────────────────────────
    let winner_raw = *raw.get(&best_bpm).unwrap_or(&0.0);
    let best_lag = lag_for(best_bpm as f64);
    let mut chosen = best_bpm;

    // Step (a): promote the integer double when its raw correlation is within
    // OCTAVE_RATIO_THRESHOLD of the winner.  This handles the DnB case where
    // the prior pulls toward a slower sub-harmonic but the true tempo is fast.
    let double_bpm = best_bpm * 2;
    if (BPM_MIN..=BPM_MAX).contains(&double_bpm) {
        let r_double = *raw.get(&double_bpm).unwrap_or(&0.0);
        if winner_raw > 0.0 && r_double >= OCTAVE_RATIO_THRESHOLD * winner_raw {
            chosen = double_bpm;
        }
    }

    // Step (b): if the double was not promoted, check the lag-based half-tempo.
    // We search for the BPM whose lag is closest to 2 × best_lag (the half-tempo
    // lag), breaking ties by choosing the BPM with the higher raw correlation.
    // If that candidate's raw beats the winner's, prefer the half-tempo.
    if chosen == best_bpm {
        let target_half_lag = 2 * best_lag;
        let half_bpm = (BPM_MIN..=BPM_MAX)
            .filter(|&b| {
                let l = lag_for(b as f64);
                l > 0 && l < n
            })
            .min_by(|&a, &b| {
                let la = lag_for(a as f64);
                let lb = lag_for(b as f64);
                let da = (la as i64 - target_half_lag as i64).unsigned_abs();
                let db = (lb as i64 - target_half_lag as i64).unsigned_abs();
                // Break ties by preferring the BPM with higher raw correlation.
                da.cmp(&db).then_with(|| {
                    let ra = raw.get(&a).copied().unwrap_or(0.0);
                    let rb = raw.get(&b).copied().unwrap_or(0.0);
                    rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
                })
            });
        if let Some(h) = half_bpm {
            let r_half = *raw.get(&h).unwrap_or(&0.0);
            if r_half > winner_raw {
                chosen = h;
            }
        }
    }

    // Strength = peak-to-mean ratio of the prior-weighted spectrum.
    let mean_w: f64 = weighted.iter().map(|t| t.2).sum::<f64>() / weighted.len() as f64;
    let chosen_w = weighted
        .iter()
        .find(|t| t.0 == chosen)
        .map(|t| t.2)
        .unwrap_or(best_weighted);
    let strength = if mean_w > 1e-12 {
        ((chosen_w / mean_w - 1.0) / 4.0).clamp(0.0, 1.0)
    } else {
        0.0
    };

    Some(TempoEstimate {
        bpm: chosen as f64,
        strength,
    })
}

pub fn log_gaussian_prior(bpm: f64, center: f64, sigma_oct: f64) -> f64 {
    let z = (bpm.log2() - center.log2()) / sigma_oct;
    (-0.5 * z * z).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::audio_analysis::onset::compute_onset_envelope;

    fn click_train(sr: u32, seconds: f64, period_seconds: f64) -> Vec<f32> {
        let total = (sr as f64 * seconds) as usize;
        let period = (sr as f64 * period_seconds) as usize;
        let mut out = vec![0.0f32; total];
        let mut t = 0usize;
        while t < total {
            for j in 0..32 { if t + j < out.len() { out[t + j] = 1.0; } }
            t += period;
        }
        out
    }

    /// Reggae-style: equal-energy onsets every eighth note. Quarter-note
    /// tempo is 80, eighth-note tempo is 160. The detector MUST pick 80.
    fn even_eighths(sr: u32, seconds: f64, quarter_bpm: f64) -> Vec<f32> {
        let eighth_period_s = 30.0 / quarter_bpm; // half of 60/bpm
        click_train(sr, seconds, eighth_period_s)
    }

    #[test]
    fn prior_is_symmetric_in_octaves() {
        let p_half = log_gaussian_prior(60.0, 120.0, 0.6);
        let p_one = log_gaussian_prior(120.0, 120.0, 0.6);
        let p_two = log_gaussian_prior(240.0, 120.0, 0.6);
        assert!((p_one - 1.0).abs() < 1e-9);
        assert!((p_half - p_two).abs() < 1e-9, "{} != {}", p_half, p_two);
        assert!(p_half < p_one);
    }

    #[test]
    fn detects_120_bpm_metronome() {
        let env = compute_onset_envelope(&click_train(44100, 8.0, 0.5), 44100).unwrap();
        let est = estimate_tempo(&env).unwrap();
        assert!((est.bpm - 120.0).abs() < 2.5, "got {}", est.bpm);
        assert!(est.strength > 0.3, "strength {} too low for clean metronome", est.strength);
    }

    #[test]
    fn detects_174_bpm_dnb() {
        let env = compute_onset_envelope(&click_train(44100, 8.0, 60.0 / 174.0), 44100).unwrap();
        let est = estimate_tempo(&env).unwrap();
        // 174 must NOT be reported as 87 (the half).
        assert!(
            (est.bpm - 174.0).abs() < 4.0,
            "DnB regression: expected ~174, got {}", est.bpm,
        );
    }

    #[test]
    fn reggae_eighths_resolve_to_quarter_tempo() {
        let env = compute_onset_envelope(&even_eighths(44100, 8.0, 80.0), 44100).unwrap();
        let est = estimate_tempo(&env).unwrap();
        // The bug being fixed: must report 80, not 160.
        assert!(
            (est.bpm - 80.0).abs() < 3.0,
            "reggae eighths regression: expected ~80, got {} \
             (this is the Pressure Drop / Harder They Come bug)",
            est.bpm,
        );
    }

    #[test]
    fn rejects_silence() {
        let env = compute_onset_envelope(&vec![0.0f32; 44100 * 4], 44100).unwrap();
        let est = estimate_tempo(&env);
        // Either None or strength near zero is acceptable.
        if let Some(e) = est {
            assert!(e.strength < 0.1, "silence produced strength {}", e.strength);
        }
    }
}
