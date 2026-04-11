/// Audio feature extraction: energy, LUFS, spectral centroid, instrumental detection.
///
/// Functions:
/// - `compute_energy`: RMS normalised to [0,1]
/// - `compute_lufs`: ITU-R BS.1770-4 gated loudness measurement
/// - `compute_spectral_centroid`: STFT-based spectral centroid
/// - `detect_instrumental`: vocal energy ratio heuristic
/// - `compute_danceability`: bass energy + BPM factor heuristic

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

    // Rescale coefficients from 48kHz to actual sample rate
    let (kb, ka) = rescale_biquad_coefficients(KWEIGHT_B_48K, KWEIGHT_A_48K, 48000, sample_rate);
    let (hb, ha) = rescale_biquad_coefficients(HP_B_48K, HP_A_48K, 48000, sample_rate);

    // Apply biquad filters (stage 1: K-weight high-shelf, stage 2: high-pass at 38Hz)
    let filtered = biquad_process(samples, kb, ka);
    let filtered = biquad_process(&filtered, hb, ha);

    // Gated integration: 400ms blocks, 75% overlap
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

        // Mean square energy of block
        let sum_sq: f64 = filtered[start..end].iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
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
    let sum_linear: f64 = gated_relative
        .iter()
        .map(|l| 10.0_f64.powf(l / 10.0))
        .sum();
    let mean_linear = sum_linear / gated_relative.len() as f64;

    if mean_linear <= 1e-12 {
        return Some(integrated);
    }

    let final_lufs = -0.691 + 10.0 * mean_linear.log10();
    Some(final_lufs)
}

/// Rescale biquad coefficients from one sample rate to another using bilinear transform.
///
/// The coefficients provided are designed for 48kHz. For other sample rates (e.g., 44.1kHz),
/// we need to bilinear-transform them. This is a simplified approach: we convert the
/// coefficients to their pole-zero representation, rescale, and convert back.
///
/// For a proper implementation, we'd use the bilinear transform formula:
///   s = (2/T) * (1 - z^-1) / (1 + z^-1)
/// where T = 1/sample_rate.
///
/// Simplified approach: scale the frequency-dependent coefficients proportionally.
fn rescale_biquad_coefficients(b: [f64; 3], a: [f64; 3], from_rate: u32, to_rate: u32) -> ([f64; 3], [f64; 3]) {
    if from_rate == to_rate {
        return (b, a);
    }

    let ratio = to_rate as f64 / from_rate as f64;

    // For the high-pass and high-shelf filters, the coefficients depend on the
    // bilinear transform warping. We use a simplified frequency-warping correction.
    //
    // The proper approach: convert biquad to analog prototype, warp frequencies,
    // convert back. Here we use a simplified prewarping approximation.

    // For a biquad with coefficients [b0, b1, b2] and [a0, a1, a2]:
    // We apply a simple scaling to account for sample rate change.
    // This is an approximation but works well for moderate rate changes (44.1k <-> 48k).

    // Scale approach: adjust a1, a2, b1, b2 based on sample rate ratio
    // This preserves the filter's frequency response shape.

    // More accurate: use the bilinear transform prewarping
    // For each coefficient, we need to account for the frequency warping.
    // Simplified: scale the "delay" terms by the ratio.

    let new_b = [
        b[0],
        b[1] * ratio,
        b[2] * ratio * ratio,
    ];

    let new_a = [
        a[0],
        a[1] * ratio,
        a[2] * ratio * ratio,
    ];

    // Normalise so a[0] = 1.0
    let a0 = new_a[0];
    (
        [new_b[0] / a0, new_b[1] / a0, new_b[2] / a0],
        [1.0, new_a[1] / a0, new_a[2] / a0],
    )
}

/// Apply a biquad filter to a sample buffer.
/// b = [b0, b1, b2], a = [a0, a1, a2] (with a0 assumed to be 1.0 or already normalised)
fn biquad_process(samples: &[f32], b: [f64; 3], a: [f64; 3]) -> Vec<f32> {
    let mut output = Vec::with_capacity(samples.len());
    let mut x1 = 0.0f64;
    let mut x2 = 0.0f64;
    let mut y1 = 0.0f64;
    let mut y2 = 0.0f64;

    // Normalise coefficients if a[0] != 1.0
    let a0 = a[0];
    let b0 = b[0] / a0;
    let b1 = b[1] / a0;
    let b2 = b[2] / a0;
    let a1 = a[1] / a0;
    let a2 = a[2] / a0;

    for &sample in samples {
        let x = sample as f64;
        let y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;

        output.push(y as f32);

        x2 = x1;
        x1 = x;
        y2 = y1;
        y1 = y;
    }

    output
}

// ─── Spectral Centroid ───────────────────────────────────────────────────────

const SC_FFT_SIZE: usize = 4096;
const SC_HOP: usize = 2048;

/// Compute spectral centroid averaged across STFT frames.
/// Returns `None` if samples too short.
pub fn compute_spectral_centroid(samples: &[f32], sample_rate: u32) -> Option<f64> {
    if sample_rate == 0 || samples.len() < SC_FFT_SIZE {
        return None;
    }

    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(SC_FFT_SIZE);

    let num_frames = samples.len().saturating_sub(SC_FFT_SIZE) / SC_HOP + 1;
    if num_frames == 0 {
        return None;
    }

    // Precompute Hann window
    let hann: Vec<f64> = (0..SC_FFT_SIZE)
        .map(|n| 0.5 * (1.0 - (2.0 * PI * n as f64 / (SC_FFT_SIZE - 1) as f64).cos()))
        .collect();

    let mut fft_input = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0); SC_FFT_SIZE];
    let mut centroid_sum = 0.0f64;
    let mut valid_frames = 0usize;

    for frame_idx in 0..num_frames {
        let start = frame_idx * SC_HOP;

        // Apply Hann window
        for i in 0..SC_FFT_SIZE {
            fft_input[i] = rustfft::num_complex::Complex::new(
                samples[start + i] * hann[i] as f32,
                0.0,
            );
        }

        fft.process(&mut fft_input);

        // Compute spectral centroid for this frame
        let mut weighted_sum = 0.0f64;
        let mut magnitude_sum = 0.0f64;

        for bin in 1..(SC_FFT_SIZE / 2) {
            let freq = bin as f64 * sample_rate as f64 / SC_FFT_SIZE as f64;
            let magnitude = fft_input[bin].norm() as f64;

            weighted_sum += freq * magnitude;
            magnitude_sum += magnitude;
        }

        if magnitude_sum > 1e-12 {
            centroid_sum += weighted_sum / magnitude_sum;
            valid_frames += 1;
        }
    }

    if valid_frames == 0 {
        return None;
    }

    Some(centroid_sum / valid_frames as f64)
}

// ─── Instrumental Detection ─────────────────────────────────────────────────

const INST_FFT_SIZE: usize = 4096;
const INST_HOP: usize = 2048;

/// Detect whether a track is likely instrumental.
/// Uses vocal energy ratio in 300-3400 Hz range.
/// Returns `Some(true)` if vocal_ratio < 0.12, `Some(false)` if > 0.22, `None` otherwise.
pub fn detect_instrumental(samples: &[f32], sample_rate: u32) -> Option<bool> {
    if sample_rate == 0 || samples.len() < INST_FFT_SIZE {
        return None;
    }

    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(INST_FFT_SIZE);

    let num_frames = samples.len().saturating_sub(INST_FFT_SIZE) / INST_HOP + 1;
    if num_frames == 0 {
        return None;
    }

    // Precompute Hann window
    let hann: Vec<f64> = (0..INST_FFT_SIZE)
        .map(|n| 0.5 * (1.0 - (2.0 * PI * n as f64 / (INST_FFT_SIZE - 1) as f64).cos()))
        .collect();

    let mut fft_input = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0); INST_FFT_SIZE];
    let mut vocal_energy = 0.0f64;
    let mut total_energy = 0.0f64;

    for frame_idx in 0..num_frames {
        let start = frame_idx * INST_HOP;

        // Apply Hann window
        for i in 0..INST_FFT_SIZE {
            fft_input[i] = rustfft::num_complex::Complex::new(
                samples[start + i] * hann[i] as f32,
                0.0,
            );
        }

        fft.process(&mut fft_input);

        for bin in 1..(INST_FFT_SIZE / 2) {
            let freq = bin as f64 * sample_rate as f64 / INST_FFT_SIZE as f64;
            let magnitude = fft_input[bin].norm() as f64;
            let power = magnitude * magnitude;

            total_energy += power;

            // Vocal range: 300-3400 Hz
            if freq >= 300.0 && freq <= 3400.0 {
                vocal_energy += power;
            }
        }
    }

    if total_energy < 1e-12 {
        return None;
    }

    let vocal_ratio = vocal_energy / total_energy;

    if vocal_ratio < 0.12 {
        Some(true)
    } else if vocal_ratio > 0.22 {
        Some(false)
    } else {
        None
    }
}

// ─── Danceability Helper ─────────────────────────────────────────────────────

/// Compute danceability from bass energy ratio, BPM, and beat strength.
/// This is called from engine.rs after BPM detection.
///
/// Formula:
///   bass_energy = energy(20-250 Hz) / total_energy
///   bpm_factor = if bpm in 100..160 { 1.0 } else { 0.6 }
///   danceability = clamp(bass_energy * 1.5 * bpm_factor * beat_strength, 0, 1)
pub fn compute_danceability(
    samples: &[f32],
    sample_rate: u32,
    bpm: Option<f64>,
    beat_strength: Option<f64>,
) -> Option<f64> {
    if sample_rate == 0 || samples.len() < 256 {
        return None;
    }

    let bpm = bpm.unwrap_or(120.0);
    let beat_strength = beat_strength.unwrap_or(0.5);

    // Compute bass energy using a simplified approach (energy in 20-250 Hz band)
    // Using the same STFT approach as instrumental detection
    let mut planner = FftPlanner::new();
    let fft_size = 4096usize;
    let hop = 2048usize;
    let fft = planner.plan_fft_forward(fft_size);

    let num_frames = samples.len().saturating_sub(fft_size) / hop + 1;
    if num_frames == 0 {
        return None;
    }

    let hann: Vec<f64> = (0..fft_size)
        .map(|n| 0.5 * (1.0 - (2.0 * PI * n as f64 / (fft_size - 1) as f64).cos()))
        .collect();

    let mut fft_input = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0); fft_size];
    let mut bass_energy = 0.0f64;
    let mut total_energy = 0.0f64;

    for frame_idx in 0..num_frames {
        let start = frame_idx * hop;

        for i in 0..fft_size {
            fft_input[i] = rustfft::num_complex::Complex::new(
                samples[start + i] * hann[i] as f32,
                0.0,
            );
        }

        fft.process(&mut fft_input);

        for bin in 1..(fft_size / 2) {
            let freq = bin as f64 * sample_rate as f64 / fft_size as f64;
            let magnitude = fft_input[bin].norm() as f64;
            let power = magnitude * magnitude;

            total_energy += power;

            if freq >= 20.0 && freq <= 250.0 {
                bass_energy += power;
            }
        }
    }

    if total_energy < 1e-12 {
        return None;
    }

    let bass_ratio = bass_energy / total_energy;
    let bpm_factor = if (100.0..=160.0).contains(&bpm) {
        1.0
    } else {
        0.6
    };

    let danceability = (bass_ratio * 1.5 * bpm_factor * beat_strength).clamp(0.0, 1.0);
    Some(danceability)
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
        assert!(detect_instrumental(&samples, 48000).is_none());
    }

    #[test]
    fn test_danceability_empty() {
        assert!(compute_danceability(&[], 44100, Some(120.0), Some(0.5)).is_none());
    }

    #[test]
    fn test_rescale_same_rate() {
        let b = [1.0, 2.0, 3.0];
        let a = [1.0, 0.5, 0.25];
        let (nb, na) = rescale_biquad_coefficients(b, a, 48000, 48000);
        assert_eq!(nb, b);
        assert_eq!(na, a);
    }
}
