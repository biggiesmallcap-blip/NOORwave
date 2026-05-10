//! Spectral flux onset detection function.
//!
//! Pipeline:
//!   1. STFT with Hann window (size 1024, hop 256 → ~5.8 ms @ 44.1 kHz)
//!   2. Log-magnitude compression: log(1 + γ·|X|), γ = 1000
//!   3. Half-wave rectified bin-wise difference, summed across bins
//!   4. Normalised to [0, 1] by dividing by the running 99th percentile
//!
//! Output: one ODF sample per STFT hop. The hop length in seconds is returned
//! so downstream tempo estimation can convert lags to BPM.

use std::f64::consts::PI;

pub const ODF_FFT_SIZE: usize = 1024;
pub const ODF_HOP: usize = 256;
const LOG_COMPRESS_GAMMA: f64 = 1000.0;

#[derive(Debug)]
pub struct OnsetEnvelope {
    /// One sample per hop. Values in [0, 1].
    pub odf: Vec<f64>,
    /// Seconds per ODF sample.
    pub hop_seconds: f64,
}

pub fn compute_onset_envelope(samples: &[f32], sample_rate: u32) -> Option<OnsetEnvelope> {
    if sample_rate == 0 || samples.len() < ODF_FFT_SIZE + ODF_HOP {
        return None;
    }

    let hann: Vec<f64> = (0..ODF_FFT_SIZE)
        .map(|n| 0.5 * (1.0 - (2.0 * PI * n as f64 / (ODF_FFT_SIZE - 1) as f64).cos()))
        .collect();

    let num_frames = (samples.len() - ODF_FFT_SIZE) / ODF_HOP + 1;
    let bins = ODF_FFT_SIZE / 2;

    let mut planner = rustfft::FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(ODF_FFT_SIZE);
    let mut buf = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0); ODF_FFT_SIZE];

    let mut prev_log_mag = vec![0.0f64; bins];
    let mut flux = Vec::with_capacity(num_frames.saturating_sub(1));

    for f in 0..num_frames {
        let start = f * ODF_HOP;
        for i in 0..ODF_FFT_SIZE {
            buf[i].re = samples[start + i] * hann[i] as f32;
            buf[i].im = 0.0;
        }
        fft.process(&mut buf);

        let mut sf = 0.0f64;
        for k in 1..bins {
            let mag = buf[k].norm() as f64;
            let log_mag = (1.0 + LOG_COMPRESS_GAMMA * mag).ln();
            if f > 0 {
                let d = log_mag - prev_log_mag[k];
                if d > 0.0 {
                    sf += d;
                }
            }
            prev_log_mag[k] = log_mag;
        }
        if f > 0 {
            flux.push(sf);
        }
    }

    if flux.is_empty() {
        return None;
    }

    // Adaptive normalisation to [0, 1] using the 99th percentile so a single
    // huge transient does not flatten the rest of the envelope.
    let mut sorted = flux.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((sorted.len() as f64 * 0.99).floor() as usize).min(sorted.len() - 1);
    let p99 = sorted[idx];
    let denom = if p99 > 1e-9 { p99 } else { 1.0 };
    for v in flux.iter_mut() {
        *v = (*v / denom).min(1.0);
    }

    Some(OnsetEnvelope {
        odf: flux,
        hop_seconds: ODF_HOP as f64 / sample_rate as f64,
    })
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
    fn rejects_empty_input() {
        assert!(compute_onset_envelope(&[], 44100).is_none());
    }

    #[test]
    fn rejects_zero_sample_rate() {
        let s = vec![0.0f32; 44100];
        assert!(compute_onset_envelope(&s, 0).is_none());
    }

    #[test]
    fn silence_has_low_energy() {
        let s = vec![0.0f32; 44100 * 4];
        let env = compute_onset_envelope(&s, 44100).unwrap();
        let max = env.odf.iter().cloned().fold(0.0f64, f64::max);
        assert_eq!(
            max, 0.0,
            "silence must produce a zero ODF, got peak {}",
            max
        );
    }

    #[test]
    fn click_train_produces_sharp_peaks() {
        // 120 BPM clicks (every 0.5 s)
        let s = click_train(44100, 8.0, 0.5);
        let env = compute_onset_envelope(&s, 44100).unwrap();
        // Most ODF samples should be near zero; a few should be near 1.
        let above_half: usize = env.odf.iter().filter(|&&v| v > 0.5).count();
        let total = env.odf.len();
        assert!(
            above_half * 30 < total,
            "fewer than ~3% of frames should exceed 0.5 (got {}/{} = {:.1}%)",
            above_half,
            total,
            100.0 * above_half as f64 / total as f64,
        );
        assert!(
            above_half >= 8,
            "expected at least 8 strong onsets, got {}",
            above_half
        );

        // Peak periodicity: the strong-onset frames must fall on a near-uniform grid
        // matching the 0.5 s click period (the whole reason the ODF exists).
        let peak_frames: Vec<usize> = env
            .odf
            .iter()
            .enumerate()
            .filter(|&(_, &v)| v > 0.5)
            .map(|(i, _)| i)
            .collect();
        let spacings: Vec<f64> = peak_frames
            .windows(2)
            .map(|w| (w[1] - w[0]) as f64 * env.hop_seconds)
            .collect();
        let mut sorted_spacings = spacings.clone();
        sorted_spacings.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = sorted_spacings[sorted_spacings.len() / 2];
        assert!(
            (median - 0.5).abs() < 0.02,
            "median peak spacing {:.4} s != 0.5 s (clicks should drive a 120 BPM ODF)",
            median,
        );
    }

    #[test]
    fn hop_seconds_matches_definition() {
        let s = vec![0.0f32; 44100];
        let env = compute_onset_envelope(&s, 44100).unwrap();
        let expected = ODF_HOP as f64 / 44100.0;
        assert!(
            (env.hop_seconds - expected).abs() < 1e-12,
            "hop_seconds {} != {}",
            env.hop_seconds,
            expected,
        );
    }
}
