/// Audio feature extraction: energy, LUFS, spectral centroid, instrumental detection.
///
/// Functions:
/// - `compute_energy`: RMS normalised to [0,1]
/// - `compute_lufs`: ITU-R BS.1770-4 gated loudness measurement
/// - `compute_stft_features`: spectral centroid and energy-band features
/// - `detect_instrumental_from`: vocal energy ratio heuristic
/// - `compute_danceability_from`: bass energy + BPM factor heuristic
use rustfft::FftPlanner;
use std::f64::consts::PI;

// ─── Energy ──────────────────────────────────────────────────────────────────

/// Compute normalised energy (RMS / 0.7, clamped to [0,1]).
pub fn compute_energy(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
    let rms = (sum_sq / samples.len() as f64).sqrt();
    (rms / 0.7).clamp(0.0, 1.0)
}

// ─── LUFS (ITU-R BS.1770-4) ─────────────────────────────────────────────────

/// 48kHz biquad coefficients for K-weighting high-shelf filter.
const KWEIGHT_B_48K: [f64; 3] = [1.535124032934, -2.691688794606, 1.198393328494];
const KWEIGHT_A_48K: [f64; 3] = [1.0, -1.690659293182, 0.732479864847];

/// 48kHz biquad coefficients for 38Hz high-pass filter.
const HP_B_48K: [f64; 3] = [1.0, -2.0, 1.0];
const HP_A_48K: [f64; 3] = [1.0, -1.990047454834, 0.990072250366];

/// Compute gated loudness in LUFS (ITU-R BS.1770-4).
/// Returns `None` if samples too short for meaningful measurement.
pub fn compute_lufs(samples: &[f32], sample_rate: u32) -> Option<f64> {
    if sample_rate == 0 || samples.len() < sample_rate as usize {
        return None; // need at least 1 second
    }

    // Rescale coefficients from 48kHz to actual sample rate (via bilinear transform)
    let (kb, ka) = if sample_rate == 48000 {
        (KWEIGHT_B_48K, KWEIGHT_A_48K)
    } else {
        rescale_biquad_coefficients(KWEIGHT_B_48K, KWEIGHT_A_48K, 48_000.0, sample_rate as f64)
    };
    let (hb, ha) = if sample_rate == 48000 {
        (HP_B_48K, HP_A_48K)
    } else {
        rescale_biquad_coefficients(HP_B_48K, HP_A_48K, 48_000.0, sample_rate as f64)
    };

    // Apply biquad filters in f64 throughout (Bug 5: avoid f32 round-trip between stages).
    let filtered = biquad_process_f64(samples.iter().map(|&s| s as f64), kb, ka);
    let filtered = biquad_process_f64(filtered.iter().copied(), hb, ha);

    let block_size = (sample_rate as f64 * 0.4) as usize; // 400ms
    let hop = (block_size as f64 * 0.25) as usize; // 75% overlap = 25% hop

    if block_size < 2 || hop < 1 || samples.len() < block_size {
        return None;
    }

    let num_blocks = (samples.len().saturating_sub(block_size)) / hop + 1;
    let mut block_loudness: Vec<f64> = Vec::with_capacity(num_blocks);

    for i in 0..num_blocks {
        let start = i * hop;
        let end = (start + block_size).min(filtered.len());
        if end <= start {
            continue;
        }

        let sum_sq: f64 = filtered[start..end].iter().map(|s| s.powi(2)).sum::<f64>();
        let mean_sq = sum_sq / (end - start) as f64;

        if mean_sq > 1e-12 {
            // Convert to LUFS: -0.691 + 10*log10(mean_sq)
            let lufs = -0.691 + 10.0 * mean_sq.log10();
            block_loudness.push(lufs);
        }
    }

    if block_loudness.is_empty() {
        return None;
    }

    // Absolute gate: -70 LUFS
    let absolute_gate = -70.0;
    let gated: Vec<f64> = block_loudness
        .iter()
        .filter(|&&l| l > absolute_gate)
        .copied()
        .collect();

    if gated.is_empty() {
        return None;
    }

    // First pass: compute integrated loudness with absolute gate
    let integrated: f64 = {
        let sum_linear: f64 = gated.iter().map(|l| 10.0_f64.powf(l / 10.0)).sum();
        let mean_linear = sum_linear / gated.len() as f64;
        if mean_linear <= 1e-12 {
            return None;
        }
        -0.691 + 10.0 * mean_linear.log10()
    };

    // Relative gate: integrated - 10 LUFS
    let relative_gate = integrated - 10.0;
    let gated_relative: Vec<f64> = block_loudness
        .iter()
        .filter(|&&l| l > relative_gate)
        .copied()
        .collect();

    if gated_relative.is_empty() {
        // Fall back to absolute-gated result
        return Some(integrated);
    }

    // Second pass: recompute with relative gate
    let sum_linear: f64 = gated_relative.iter().map(|l| 10.0_f64.powf(l / 10.0)).sum();
    let mean_linear = sum_linear / gated_relative.len() as f64;

    if mean_linear <= 1e-12 {
        return Some(integrated);
    }

    let final_lufs = -0.691 + 10.0 * mean_linear.log10();
    Some(final_lufs)
}

/// Rescale digital biquad coefficients from `from_sr` to `to_sr` via bilinear transform.
///
/// Approach:
/// 1. Inverse bilinear at `from_sr` (T1 = 1/from_sr) to obtain the underlying
///    continuous-time biquad in the variable u = s·T1/2:
///       B(u) = B0 + B1·u + B2·u²,   A(u) = A0 + A1·u + A2·u²
///    where
///       B0 = b0 + b1 + b2,   B1 = 2(b0 − b2),   B2 = b0 − b1 + b2
///       A0 = a0 + a1 + a2,   A1 = 2(a0 − a2),   A2 = a0 − a1 + a2
/// 2. Re-bilinear at `to_sr` (T2 = 1/to_sr). Substituting u = r·(1 − z⁻¹)/(1 + z⁻¹)
///    with r = T1/T2 = to_sr/from_sr and clearing (1 + z⁻¹)² yields a new biquad.
///
/// The ratio `r` acts as the prewarp factor `k = tan(π·fc/from_sr) / tan(π·fc/to_sr)`
/// in the small-frequency limit and is exact for the all-purpose rescaling used here.
pub fn rescale_biquad_coefficients(
    b: [f64; 3],
    a: [f64; 3],
    from_sr: f64,
    to_sr: f64,
) -> ([f64; 3], [f64; 3]) {
    if (from_sr - to_sr).abs() < f64::EPSILON {
        return (b, a);
    }

    // Step 1 — inverse bilinear at from_sr → continuous-time (u = s·T1/2) biquad
    let b_cap = [b[0] + b[1] + b[2], 2.0 * (b[0] - b[2]), b[0] - b[1] + b[2]];
    let a_cap = [a[0] + a[1] + a[2], 2.0 * (a[0] - a[2]), a[0] - a[1] + a[2]];

    // Step 2 — re-bilinear at to_sr
    let r = to_sr / from_sr;
    let r2 = r * r;

    let new_b = [
        b_cap[0] + b_cap[1] * r + b_cap[2] * r2,
        2.0 * (b_cap[0] - b_cap[2] * r2),
        b_cap[0] - b_cap[1] * r + b_cap[2] * r2,
    ];
    let new_a = [
        a_cap[0] + a_cap[1] * r + a_cap[2] * r2,
        2.0 * (a_cap[0] - a_cap[2] * r2),
        a_cap[0] - a_cap[1] * r + a_cap[2] * r2,
    ];

    // Normalise so a0 = 1.0
    let a0 = new_a[0];
    if a0.abs() < 1e-20 {
        // Degenerate — return original rather than produce NaNs
        return (b, a);
    }
    (
        [new_b[0] / a0, new_b[1] / a0, new_b[2] / a0],
        [1.0, new_a[1] / a0, new_a[2] / a0],
    )
}

/// Apply a biquad filter, keeping the signal in f64 throughout.
/// b = [b0, b1, b2], a = [a0, a1, a2] (a0 normalised internally)
fn biquad_process_f64<I: IntoIterator<Item = f64>>(
    samples: I,
    b: [f64; 3],
    a: [f64; 3],
) -> Vec<f64> {
    let iter = samples.into_iter();
    let (lower, _) = iter.size_hint();
    let mut output = Vec::with_capacity(lower);

    let a0 = a[0];
    let b0 = b[0] / a0;
    let b1 = b[1] / a0;
    let b2 = b[2] / a0;
    let a1 = a[1] / a0;
    let a2 = a[2] / a0;

    let mut x1 = 0.0f64;
    let mut x2 = 0.0f64;
    let mut y1 = 0.0f64;
    let mut y2 = 0.0f64;

    for x in iter {
        let y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
        output.push(y);
        x2 = x1;
        x1 = x;
        y2 = y1;
        y1 = y;
    }

    output
}

// ─── Unified STFT pass ───────────────────────────────────────────────────────
//
// Spectral centroid, instrumental detection, and danceability all want the same
// 4096/2048 Hann STFT. Doing one pass that accumulates everything is ~3× faster
// than running three independent STFTs.

const STFT_FFT_SIZE: usize = 4096;
const STFT_HOP: usize = 2048;

pub struct StftFeatures {
    pub centroid_hz: Option<f64>,
    pub vocal_ratio: f64,
    pub bass_ratio: f64,
    pub total_energy: f64,
}

/// One STFT pass that accumulates all feature statistics derived from the
/// 4096-point Hann magnitude/power spectrum.
pub fn compute_stft_features(samples: &[f32], sample_rate: u32) -> Option<StftFeatures> {
    if sample_rate == 0 || samples.len() < STFT_FFT_SIZE {
        return None;
    }

    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(STFT_FFT_SIZE);

    let num_frames = samples.len().saturating_sub(STFT_FFT_SIZE) / STFT_HOP + 1;
    if num_frames == 0 {
        return None;
    }

    let hann: Vec<f64> = (0..STFT_FFT_SIZE)
        .map(|n| 0.5 * (1.0 - (2.0 * PI * n as f64 / (STFT_FFT_SIZE - 1) as f64).cos()))
        .collect();

    let mut fft_input = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0); STFT_FFT_SIZE];
    let mut centroid_sum = 0.0f64;
    let mut centroid_valid_frames = 0usize;
    let mut vocal_energy = 0.0f64;
    let mut bass_energy = 0.0f64;
    let mut total_energy = 0.0f64;

    for frame_idx in 0..num_frames {
        let start = frame_idx * STFT_HOP;

        for i in 0..STFT_FFT_SIZE {
            fft_input[i] =
                rustfft::num_complex::Complex::new(samples[start + i] * hann[i] as f32, 0.0);
        }

        fft.process(&mut fft_input);

        let mut weighted_sum = 0.0f64;
        let mut magnitude_sum = 0.0f64;

        for bin in 1..(STFT_FFT_SIZE / 2) {
            let freq = bin as f64 * sample_rate as f64 / STFT_FFT_SIZE as f64;
            let magnitude = fft_input[bin].norm() as f64;
            let power = magnitude * magnitude;

            weighted_sum += freq * magnitude;
            magnitude_sum += magnitude;

            total_energy += power;
            if (20.0..=250.0).contains(&freq) {
                bass_energy += power;
            }
            if (300.0..=3400.0).contains(&freq) {
                vocal_energy += power;
            }
        }

        if magnitude_sum > 1e-12 {
            centroid_sum += weighted_sum / magnitude_sum;
            centroid_valid_frames += 1;
        }
    }

    let centroid_hz = if centroid_valid_frames > 0 {
        Some(centroid_sum / centroid_valid_frames as f64)
    } else {
        None
    };

    let (vocal_ratio, bass_ratio) = if total_energy > 1e-12 {
        (vocal_energy / total_energy, bass_energy / total_energy)
    } else {
        (0.0, 0.0)
    };

    Some(StftFeatures {
        centroid_hz,
        vocal_ratio,
        bass_ratio,
        total_energy,
    })
}

// ─── Spectral Centroid ───────────────────────────────────────────────────────

// ─── Instrumental Detection ─────────────────────────────────────────────────

/// Detect whether a track is likely instrumental.
/// Uses vocal energy ratio in 300-3400 Hz range.
/// Returns `Some(true)` if vocal_ratio < 0.12, `Some(false)` if > 0.22, `None` otherwise.
pub fn detect_instrumental_from(stft: &StftFeatures) -> Option<bool> {
    if stft.total_energy < 1e-12 {
        return None;
    }
    if stft.vocal_ratio < 0.12 {
        Some(true)
    } else if stft.vocal_ratio > 0.22 {
        Some(false)
    } else {
        None
    }
}

// ─── Danceability ───────────────────────────────────────────────────────────

/// Compute danceability from bass energy ratio, BPM, and beat strength.
///
/// Formula:
///   bass_ratio = energy(20-250 Hz) / total_energy
///   bpm_factor = if bpm in 100..160 { 1.0 } else { 0.6 }
///   danceability = clamp(bass_ratio * 1.5 * bpm_factor * beat_strength, 0, 1)
pub fn compute_danceability_from(
    stft: &StftFeatures,
    bpm: Option<f64>,
    beat_strength: Option<f64>,
) -> Option<f64> {
    if stft.total_energy < 1e-12 {
        return None;
    }
    let bpm = bpm.unwrap_or(120.0);
    let beat_strength = beat_strength.unwrap_or(0.5);
    let bpm_factor = if (100.0..=160.0).contains(&bpm) {
        1.0
    } else {
        0.6
    };
    Some((stft.bass_ratio * 1.5 * bpm_factor * beat_strength).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_energy_silence() {
        let samples = vec![0.0f32; 44100];
        assert!((compute_energy(&samples) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_energy_full_scale() {
        let samples = vec![1.0f32; 44100];
        let energy = compute_energy(&samples);
        assert!((energy - 1.0).abs() < 1e-6); // 1.0 / 0.7 > 1.0, clamped
    }

    #[test]
    fn test_energy_mid_scale() {
        // RMS of 0.7 should give energy ~1.0
        let samples = vec![0.7f32; 44100];
        let energy = compute_energy(&samples);
        assert!((energy - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_lufs_empty() {
        assert!(compute_lufs(&[], 48000).is_none());
    }

    #[test]
    fn test_lufs_too_short() {
        let samples = vec![0.0f32; 100];
        assert!(compute_lufs(&samples, 48000).is_none());
    }

    #[test]
    fn test_instrumental_silence() {
        let samples = vec![0.0f32; 48000];
        // Silence should return None (total_energy ~0)
        let stft = compute_stft_features(&samples, 48000).expect("STFT must compute");
        assert!(detect_instrumental_from(&stft).is_none());
    }

    #[test]
    fn test_danceability_empty() {
        let dance = compute_stft_features(&[], 44100)
            .as_ref()
            .and_then(|stft| compute_danceability_from(stft, Some(120.0), Some(0.5)));
        assert!(dance.is_none());
    }

    #[test]
    fn test_danceability_short_clip_returns_none_not_panic() {
        // Bug 4 regression: previously panicked on samples between 256 and 4096
        // because the guard checked < 256 but the FFT needed 4096.
        let samples = vec![0.1f32; 1000];
        let dance = compute_stft_features(&samples, 44100)
            .as_ref()
            .and_then(|stft| compute_danceability_from(stft, Some(120.0), Some(0.5)));
        assert!(dance.is_none());
        let samples = vec![0.1f32; 4095];
        let dance = compute_stft_features(&samples, 44100)
            .as_ref()
            .and_then(|stft| compute_danceability_from(stft, Some(120.0), Some(0.5)));
        assert!(dance.is_none());
    }

    #[test]
    fn test_rescale_same_rate() {
        let b = [1.0, 2.0, 3.0];
        let a = [1.0, 0.5, 0.25];
        let (nb, na) = rescale_biquad_coefficients(b, a, 48_000.0, 48_000.0);
        assert_eq!(nb, b);
        assert_eq!(na, a);
    }

    #[test]
    fn test_rescale_44100_lufs_kweight_stable() {
        // Rescale the 48kHz K-weight biquad to 44.1kHz — coefficients must remain finite.
        let (b, a) = rescale_biquad_coefficients(KWEIGHT_B_48K, KWEIGHT_A_48K, 48_000.0, 44_100.0);
        for v in b.iter().chain(a.iter()) {
            assert!(v.is_finite(), "coefficient must be finite, got {v}");
        }
        assert!((a[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn test_lufs_44100_runs() {
        // 2s of 1kHz sine @ 44.1kHz — LUFS should compute without panic.
        let sr = 44_100u32;
        let samples: Vec<f32> = (0..(sr * 2) as usize)
            .map(|n| 0.5 * (2.0 * PI * 1000.0 * n as f64 / sr as f64).sin() as f32)
            .collect();
        let lufs = compute_lufs(&samples, sr);
        assert!(lufs.is_some());
        let l = lufs.unwrap();
        assert!(l.is_finite(), "LUFS must be finite");
    }

    #[test]
    fn test_lufs_96000_runs() {
        // 2s of 1kHz sine @ 96kHz — LUFS should compute without panic.
        let sr = 96_000u32;
        let samples: Vec<f32> = (0..(sr * 2) as usize)
            .map(|n| 0.5 * (2.0 * PI * 1000.0 * n as f64 / sr as f64).sin() as f32)
            .collect();
        let lufs = compute_lufs(&samples, sr);
        assert!(lufs.is_some());
        let l = lufs.unwrap();
        assert!(l.is_finite(), "LUFS must be finite");
    }

    #[test]
    fn test_lufs_within_expected_range_for_known_sine() {
        // 0.5-amplitude 1kHz sine: mean-square = 0.125 → ~-9 dBFS.
        // K-weighting adds a few dB at 1kHz; result should land in [-12, -3] LUFS.
        let sr = 48_000u32;
        let samples: Vec<f32> = (0..(sr * 2) as usize)
            .map(|n| 0.5 * (2.0 * PI * 1000.0 * n as f64 / sr as f64).sin() as f32)
            .collect();
        let lufs = compute_lufs(&samples, sr).expect("LUFS must compute");
        assert!(
            (-12.0..=-3.0).contains(&lufs),
            "LUFS for 0.5-amp 1kHz sine should be in [-12, -3], got {lufs}"
        );
    }

    #[test]
    fn test_instrumental_classifies_pure_bass_as_instrumental() {
        // 60 Hz fundamental + harmonics that stay below the 300 Hz vocal floor.
        let sr = 48_000u32;
        let total = (sr * 2) as usize;
        let samples: Vec<f32> = (0..total)
            .map(|n| {
                let t = n as f64 / sr as f64;
                let s = (2.0 * PI * 60.0 * t).sin()
                    + 0.5 * (2.0 * PI * 120.0 * t).sin()
                    + 0.25 * (2.0 * PI * 180.0 * t).sin();
                (s * 0.3) as f32
            })
            .collect();
        let stft = compute_stft_features(&samples, sr).expect("STFT must compute");
        assert_eq!(
            detect_instrumental_from(&stft),
            Some(true),
            "low-frequency-only signal should be classified instrumental"
        );
    }

    #[test]
    fn test_instrumental_classifies_vocal_band_signal_as_vocal() {
        // 1 kHz sine sits squarely in the 300-3400 Hz vocal band.
        let sr = 48_000u32;
        let total = (sr * 2) as usize;
        let samples: Vec<f32> = (0..total)
            .map(|n| 0.3 * (2.0 * PI * 1000.0 * n as f64 / sr as f64).sin() as f32)
            .collect();
        let stft = compute_stft_features(&samples, sr).expect("STFT must compute");
        assert_eq!(
            detect_instrumental_from(&stft),
            Some(false),
            "vocal-band-only signal should be classified vocal"
        );
    }

    #[test]
    fn test_danceability_bassy_click_track_is_high() {
        // 128 BPM clicks with strong bass content (60 Hz envelope).
        let sr = 44_100u32;
        let total = (sr * 6) as usize;
        let click_period = (sr as f64 * 60.0 / 128.0) as usize;
        let mut samples = vec![0.0f32; total];
        for i in (0..total).step_by(click_period) {
            for j in 0..(sr / 30) as usize {
                if i + j < samples.len() {
                    let t = j as f64 / sr as f64;
                    samples[i + j] = 0.5 * (2.0 * PI * 60.0 * t).sin() as f32;
                }
            }
        }
        let stft = compute_stft_features(&samples, sr).expect("STFT must compute");
        let dance = compute_danceability_from(&stft, Some(128.0), Some(0.8))
            .expect("should compute danceability");
        assert!(
            dance > 0.3,
            "bassy 128 BPM click track should score >0.3, got {dance}"
        );
    }

    #[test]
    fn test_spectral_centroid_tracks_dominant_freq() {
        // 5 kHz sine should produce a high centroid (>2 kHz).
        let sr = 48_000u32;
        let total = (sr * 2) as usize;
        let samples: Vec<f32> = (0..total)
            .map(|n| 0.3 * (2.0 * PI * 5000.0 * n as f64 / sr as f64).sin() as f32)
            .collect();
        let centroid = compute_stft_features(&samples, sr)
            .and_then(|stft| stft.centroid_hz)
            .expect("centroid must compute");
        assert!(
            (3000.0..=7000.0).contains(&centroid),
            "5 kHz sine should yield centroid in [3 kHz, 7 kHz], got {centroid}"
        );
    }
}
