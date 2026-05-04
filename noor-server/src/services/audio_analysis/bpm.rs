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
        if bpm % 2 == 0 {
            if let Some(&c_half) = orig_corr.get(&(bpm / 2)) {
                *c += 0.5 * c_half;
            }
        }
    }

    // Step 6: Gaussian prior centred at 120 BPM (sigma=30)
    let prior_mean = 120.0f64;
    let prior_sigma = 30.0f64;
    for (_, c) in &mut corr {
        let bpm = (*c).abs(); // use absolute for prior application
        let prior = (-0.5 * ((bpm as f64 - prior_mean) / prior_sigma).powi(2)).exp();
        *c *= prior;
    }

    // Step 7: Find argmax
    let (best_bpm, best_corr) = corr
        .iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .copied()
        .unwrap_or((120, 0.0));

    // Normalise beat_strength to [0,1]
    let max_corr = corr.iter().map(|(_, c)| c.abs()).fold(0.0f64, f64::max);
    let beat_strength = if max_corr > 1e-12 {
        best_corr.abs() / max_corr
    } else {
        0.0
    };

    // Gate: reject low-confidence BPM (beat_strength < 0.15 → None)
    if beat_strength < 0.15 {
        return None;
    }
    Some((best_bpm as f64, beat_strength))
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
}
