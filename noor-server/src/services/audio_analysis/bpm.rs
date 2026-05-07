/// BPM detection using energy-flux onset detection + autocorrelation.
///
/// Algorithm:
/// 1. Compute energy per frame (10ms windows, 5ms hop)
/// 2. Half-wave energy flux (onset strength function)
/// 3. Autocorrelation of onset strength for candidate BPMs 60-200
/// 4. Octave folding to reinforce harmonics
/// 5. Gaussian prior centred at 120 BPM
/// 6. Gate: only return Some if beat_strength > 0.15

/// Detect BPM from mono audio samples.
/// Returns `Some((bpm, beat_strength))` or `None` if samples too short or beat_strength < 0.15.
pub fn detect_bpm(samples: &[f32], sample_rate: u32) -> Option<(f64, f64)> {
    if sample_rate == 0 || samples.len() < sample_rate as usize {
        return None; // need at least 1 second
    }

    let frame_size = (sample_rate / 100) as usize; // 10ms
    let hop_size = frame_size / 2; // 5ms

    if frame_size < 2 || hop_size < 1 {
        return None;
    }

    // Step 1: Energy per frame
    let num_frames = (samples.len().saturating_sub(frame_size)) / hop_size + 1;
    if num_frames < 2 {
        return None;
    }

    let mut energy = vec![0.0f64; num_frames];
    for n in 0..num_frames {
        let start = n * hop_size;
        let end = (start + frame_size).min(samples.len());
        let mut e = 0.0f64;
        for i in start..end {
            let s = samples[i] as f64;
            e += s * s;
        }
        energy[n] = e / (end - start) as f64;
    }

    // Step 2: Half-wave energy flux (onset strength)
    let num_onsets = num_frames - 1;
    let mut onset = vec![0.0f64; num_onsets];
    for n in 1..num_frames {
        let diff = energy[n] - energy[n - 1];
        onset[n - 1] = diff.max(0.0);
    }

    // Step 3: Normalise onset to [0,1]
    let max_onset = onset.iter().cloned().fold(0.0f64, f64::max);
    if max_onset < 1e-12 {
        return None; // no energy variation
    }
    for o in &mut onset {
        *o /= max_onset;
    }

    // Step 4: Autocorrelation for BPM 60..=200
    let mut corr: Vec<(i32, f64)> = (60..=200)
        .map(|bpm| {
            let lag =
                ((60.0 * sample_rate as f64) / (bpm as f64 * hop_size as f64)).round() as usize;
            let lag = lag.min(num_onsets.saturating_sub(1));
            if lag == 0 {
                return (bpm, 0.0f64);
            }
            let mut sum = 0.0f64;
            for n in 0..(num_onsets - lag) {
                sum += onset[n] * onset[n + lag];
            }
            (bpm, sum)
        })
        .collect();

    // Step 5: Octave fold
    let orig_corr: std::collections::HashMap<i32, f64> =
        corr.iter().map(|(b, c)| (*b, *c)).collect();

    for (bpm, c) in &mut corr {
        let bpm = *bpm;
        // Add contribution from bpm*2
        if let Some(&c2) = orig_corr.get(&(bpm * 2)) {
            *c += 0.5 * c2;
        }
        // Add contribution from bpm/2
        if bpm % 2 == 0
            && let Some(&c_half) = orig_corr.get(&(bpm / 2)) {
                *c += 0.5 * c_half;
            }
    }

    // Step 6: Gaussian prior centred at 120 BPM (sigma=30)
    apply_bpm_prior(&mut corr, 120.0, 30.0);

    // Step 7: Find argmax
    let (best_bpm, best_corr) = corr
        .iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .copied()
        .unwrap_or((120, 0.0));

    // Beat strength as peak-prominence: how much the winner stands above the mean.
    // Old code divided best_corr by max_corr — but those are equal by construction
    // (autocorrelation of half-wave-rectified onset is non-negative), so this was
    // always 1.0. Peak-to-mean ratio is the textbook prominence measure.
    let mean_corr = corr.iter().map(|(_, c)| *c).sum::<f64>() / corr.len() as f64;
    let beat_strength = if mean_corr > 1e-12 {
        ((best_corr / mean_corr - 1.0) / 4.0).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // Gate: reject low-confidence BPM (beat_strength < 0.15 → None)
    if beat_strength < 0.15 {
        return None;
    }
    Some((best_bpm as f64, beat_strength))
}

/// Multiply each BPM candidate's correlation by a Gaussian prior centred at `mean` with stddev `sigma`.
fn apply_bpm_prior(corr: &mut [(i32, f64)], mean: f64, sigma: f64) {
    for (bpm, c) in corr.iter_mut() {
        let bpm = *bpm as f64;
        let prior = (-0.5 * ((bpm - mean) / sigma).powi(2)).exp();
        *c *= prior;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_samples() {
        assert!(detect_bpm(&[], 44100).is_none());
    }

    #[test]
    fn test_zero_sample_rate() {
        let samples = vec![0.0f32; 44100];
        assert!(detect_bpm(&samples, 0).is_none());
    }

    #[test]
    fn test_too_short() {
        let samples = vec![0.0f32; 1000];
        assert!(detect_bpm(&samples, 44100).is_none());
    }

    #[test]
    fn test_silence() {
        let samples = vec![0.0f32; 44100 * 5];
        assert!(detect_bpm(&samples, 44100).is_none());
    }

    #[test]
    fn test_bpm_prior_pulls_toward_center() {
        // Bug 1 regression: prior must depend on BPM, not on correlation magnitude.
        // With a flat correlation of 1.0 across 60/120/180, the Gaussian prior at
        // mean=120, sigma=30 should leave 120 untouched and attenuate 60/180.
        let mut corr = vec![(60, 1.0_f64), (120, 1.0), (180, 1.0)];
        apply_bpm_prior(&mut corr, 120.0, 30.0);
        assert!(
            (corr[1].1 - 1.0).abs() < 1e-9,
            "prior at center should leave correlation unchanged, got {}",
            corr[1].1
        );
        assert!(
            corr[0].1 < 0.2 && corr[2].1 < 0.2,
            "prior 2σ away should attenuate to <0.2, got {} and {}",
            corr[0].1,
            corr[2].1
        );
    }

    #[test]
    fn test_bpm_detects_synthetic_120() {
        // 120 BPM impulse train at 44.1 kHz for 8 s. Click every 0.5 s.
        let sr = 44_100u32;
        let total_samples = (sr * 8) as usize;
        let mut samples = vec![0.0f32; total_samples];
        let click_period = sr as usize / 2; // 0.5 s = 120 BPM
        for i in (0..total_samples).step_by(click_period) {
            for j in 0..32 {
                if i + j < samples.len() {
                    samples[i + j] = 1.0;
                }
            }
        }
        let result = detect_bpm(&samples, sr).expect("should detect a BPM");
        assert!(
            (result.0 - 120.0).abs() < 5.0 || (result.0 - 60.0).abs() < 5.0,
            "expected ~120 BPM (or 60 octave), got {}",
            result.0
        );
    }

    #[test]
    fn test_beat_strength_separates_metronome_from_noise() {
        // Bug 3 regression: beat_strength must be a real signal, not always 1.0.
        let sr = 44_100u32;
        let total_samples = (sr * 8) as usize;

        // 1) clean 120 BPM click track
        let mut clicks = vec![0.0f32; total_samples];
        let click_period = sr as usize / 2;
        for i in (0..total_samples).step_by(click_period) {
            for j in 0..32 {
                if i + j < clicks.len() {
                    clicks[i + j] = 1.0;
                }
            }
        }

        // 2) deterministic pseudo-noise (LCG) — no periodic structure
        let mut state: u64 = 0xC0FFEE;
        let noise: Vec<f32> = (0..total_samples)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state as i64 as f32) / (i64::MAX as f32)
            })
            .collect();

        let click_bs = detect_bpm(&clicks, sr).map(|(_, bs)| bs).unwrap_or(0.0);
        let noise_bs = detect_bpm(&noise, sr).map(|(_, bs)| bs).unwrap_or(0.0);

        assert!(
            click_bs > noise_bs + 0.3,
            "metronome beat_strength {click_bs} should beat noise beat_strength {noise_bs} \
             by at least 0.3 (was both 1.0 before Bug 3 fix)"
        );
    }
}
