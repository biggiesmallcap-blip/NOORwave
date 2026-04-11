/// Key detection using STFT chromagram + Krumhansl-Schmuckler key-finding algorithm.
///
/// Algorithm:
/// 1. STFT with FFT_SIZE=4096, HOP=2048, Hann window
/// 2. Accumulate bin magnitudes into 12 pitch-class profile (PCP)
/// 3. Pearson correlate rotated Krumhansl-Schmuckler profiles vs PCP
/// 4. Gate: best_corr > 0.6 AND margin over 2nd-best > 0.05
/// 5. Convert to Camelot notation

use rustfft::FftPlanner;
use std::f64::consts::PI;

const FFT_SIZE: usize = 4096;
const HOP: usize = 2048;
const C0_FREQ: f64 = 16.3516; // C0 in Hz

// Krumhansl-Schmuckler profiles
const MAJOR_PROFILE: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR_PROFILE: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

// Camelot lookup tables
const MAJOR_CAMELOT: [(&str, &str); 12] = [
    ("C", "8B"), ("C#", "9B"), ("D", "10B"), ("D#", "11B"),
    ("E", "12B"), ("F", "1B"), ("F#", "2B"), ("G", "3B"),
    ("G#", "4B"), ("A", "5B"), ("A#", "6B"), ("B", "7B"),
];
const MINOR_CAMELOT: [(&str, &str); 12] = [
    ("C", "8A"), ("C#", "9A"), ("D", "10A"), ("D#", "11A"),
    ("E", "12A"), ("F", "1A"), ("F#", "2A"), ("G", "3A"),
    ("G#", "4A"), ("A", "5A"), ("A#", "6A"), ("B", "7A"),
];

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Detect musical key from mono audio samples.
/// Returns `Some((key_signature, camelot_key))` or `None` if confidence too low.
pub fn detect_key(samples: &[f32], sample_rate: u32) -> Option<(String, String)> {
    if sample_rate == 0 || samples.len() < FFT_SIZE {
        return None;
    }

    // Build STFT and accumulate pitch-class profile
    let pcp = compute_pcp(samples, sample_rate);

    // Correlate with all 24 keys
    let mut correlations: Vec<(f64, String, String)> = Vec::with_capacity(24);

    for rotation in 0..12 {
        // Rotate profile by shifting
        let major_rotated = rotate_profile(&MAJOR_PROFILE, rotation);
        let minor_rotated = rotate_profile(&MINOR_PROFILE, rotation);

        let major_corr = pearson_correlation(&pcp, &major_rotated);
        let minor_corr = pearson_correlation(&pcp, &minor_rotated);

        let note = NOTE_NAMES[rotation];
        let major_key = format!("{note}maj");
        let minor_key = format!("{note}m");

        let major_camelot = MAJOR_CAMELOT[rotation].1.to_string();
        let minor_camelot = MINOR_CAMELOT[rotation].1.to_string();

        correlations.push((major_corr, major_key, major_camelot));
        correlations.push((minor_corr, minor_key, minor_camelot));
    }

    // Sort by correlation descending
    correlations.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let best_corr = correlations[0].0;
    let second_corr = correlations[1].0;
    let margin = best_corr - second_corr;

    // Gate
    if best_corr > 0.6 && margin > 0.05 {
        Some((correlations[0].1.clone(), correlations[0].2.clone()))
    } else {
        None
    }
}

/// Compute 12-element pitch-class profile from STFT.
fn compute_pcp(samples: &[f32], sample_rate: u32) -> [f64; 12] {
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);

    let mut pcp = [0.0f64; 12];
    let num_frames = samples.len().saturating_sub(FFT_SIZE) / HOP + 1;

    // Precompute Hann window
    let hann: Vec<f64> = (0..FFT_SIZE)
        .map(|n| 0.5 * (1.0 - (2.0 * PI * n as f64 / (FFT_SIZE - 1) as f64).cos()))
        .collect();

    let mut fft_input = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0); FFT_SIZE];

    for frame_idx in 0..num_frames {
        let start = frame_idx * HOP;

        // Apply Hann window
        for i in 0..FFT_SIZE {
            fft_input[i] = rustfft::num_complex::Complex::new(
                samples[start + i] * hann[i] as f32,
                0.0,
            );
        }

        fft.process(&mut fft_input);

        // Accumulate bin magnitudes into pitch classes
        // Only use bins for C2-C7 range (approx 65-2093 Hz)
        for bin in 1..(FFT_SIZE / 2) {
            let freq = bin as f64 * sample_rate as f64 / FFT_SIZE as f64;
            if freq < 65.0 || freq > 2100.0 {
                continue;
            }

            let magnitude = fft_input[bin].norm();
            let power = magnitude * magnitude;

            // Map frequency to pitch class
            let semitone = (12.0 * (freq / C0_FREQ).log2()).round() as i64;
            let pc = ((semitone % 12) + 12) % 12;
            pcp[pc as usize] += power as f64;
        }
    }

    // Normalise PCP
    let max_pcp = pcp.iter().cloned().fold(0.0f64, f64::max);
    if max_pcp > 1e-12 {
        for p in &mut pcp {
            *p /= max_pcp;
        }
    }

    pcp
}

/// Rotate a profile by shifting (circular shift left by `rotation`).
fn rotate_profile(profile: &[f64; 12], rotation: usize) -> [f64; 12] {
    let mut rotated = [0.0f64; 12];
    for i in 0..12 {
        rotated[i] = profile[(i + rotation) % 12];
    }
    rotated
}

/// Pearson correlation coefficient between two vectors.
fn pearson_correlation(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len() as f64;
    let sum_x: f64 = x.iter().sum();
    let sum_y: f64 = y.iter().sum();
    let sum_xy: f64 = x.iter().zip(y.iter()).map(|(a, b)| a * b).sum();
    let sum_x2: f64 = x.iter().map(|v| v * v).sum();
    let sum_y2: f64 = y.iter().map(|v| v * v).sum();

    let numerator = n * sum_xy - sum_x * sum_y;
    let denom_x = n * sum_x2 - sum_x * sum_x;
    let denom_y = n * sum_y2 - sum_y * sum_y;

    let denominator = (denom_x * denom_y).sqrt();
    if denominator < 1e-12 {
        return 0.0;
    }

    numerator / denominator
}

/// Check if two Camelot keys are compatible for harmonic mixing.
/// Compatible if: same number, or differ by 1 (mod 12), or same number A<->B.
pub fn camelot_compatible(key_a: &str, key_b: &str) -> bool {
    if key_a == key_b {
        return true;
    }

    let (num_a, type_a) = parse_camelot(key_a);
    let (num_b, type_b) = parse_camelot(key_b);

    // Same number, A<->B switch
    if num_a == num_b && type_a != type_b {
        return true;
    }

    // Differ by 1 (mod 12)
    let diff = ((num_a as i32 - num_b as i32).abs()) % 12;
    if diff == 1 {
        return true;
    }

    false
}

/// Check if two Camelot keys are adjacent (differ by 1 mod 12, but NOT same number).
pub fn camelot_adjacent(key_a: &str, key_b: &str) -> bool {
    let (num_a, _) = parse_camelot(key_a);
    let (num_b, _) = parse_camelot(key_b);

    let diff = ((num_a as i32 - num_b as i32).abs()) % 12;
    diff == 1 && num_a != num_b
}

/// Parse a Camelot key like "8A" or "12B" into (number, type_char).
fn parse_camelot(key: &str) -> (u32, char) {
    let chars: Vec<char> = key.chars().collect();
    if chars.is_empty() {
        return (0, 'A');
    }
    let type_char = chars.last().copied().unwrap_or('A');
    let num_str: String = chars[..chars.len() - 1].iter().collect();
    let num = num_str.parse::<u32>().unwrap_or(0);
    (num, type_char)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_samples() {
        assert!(detect_key(&[], 44100).is_none());
    }

    #[test]
    fn test_too_short() {
        let samples = vec![0.0f32; 100];
        assert!(detect_key(&samples, 44100).is_none());
    }

    #[test]
    fn test_camelot_compatible_same() {
        assert!(camelot_compatible("8A", "8A"));
        assert!(camelot_compatible("8A", "8B"));
        assert!(camelot_compatible("8B", "8A"));
    }

    #[test]
    fn test_camelot_compatible_adjacent() {
        assert!(camelot_compatible("8A", "9A"));
        assert!(camelot_compatible("12B", "1B")); // wrap-around
    }

    #[test]
    fn test_camelot_compatible_incompatible() {
        assert!(!camelot_compatible("8A", "10A"));
        assert!(!camelot_compatible("1A", "5A"));
    }

    #[test]
    fn test_camelot_adjacent_yes() {
        assert!(camelot_adjacent("8A", "9A"));
        assert!(camelot_adjacent("12B", "1B"));
    }

    #[test]
    fn test_camelot_adjacent_same_number() {
        // Same number is NOT "adjacent" per definition
        assert!(!camelot_adjacent("8A", "8B"));
    }

    #[test]
    fn test_parse_camelot() {
        assert_eq!(parse_camelot("8A"), (8, 'A'));
        assert_eq!(parse_camelot("12B"), (12, 'B'));
        assert_eq!(parse_camelot("1A"), (1, 'A'));
    }
}
