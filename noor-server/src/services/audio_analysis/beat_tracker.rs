//! Ellis 2007 dynamic-programming beat tracker.
//!
//! Given an onset envelope and a target period, find the beat sequence that
//! maximises:   Σ ODF[beat_i]  +  α · Σ -((log(period_i / target))²)
//! where period_i = beat_i - beat_{i-1}. α weights tempo-consistency vs.
//! onset-strength; α = 100 is Ellis's recommended value.

use crate::services::audio_analysis::onset::OnsetEnvelope;

pub const TIGHTNESS: f64 = 100.0;

/// Need at least one full 4/4 bar of beats to claim a beat track. Below this,
/// return None so callers fall back to tempogram-only confidence.
const MIN_BEATS: usize = 4;

#[derive(Debug)]
pub struct BeatTrack {
    /// Beat times in seconds from clip start. Asserted by tests; not yet
    /// consumed by production callers (kept for future beat-grid overlays).
    #[allow(dead_code)]
    pub beats: Vec<f64>,
    /// Average onset prominence at beat positions, in [0, 1]. Combined with
    /// tempogram strength in `bpm.rs` (geometric mean) to form the user-facing
    /// detector confidence.
    pub strength: f64,
}

pub fn track_beats(env: &OnsetEnvelope, target_bpm: f64) -> Option<BeatTrack> {
    let n = env.odf.len();
    if n < 32 || target_bpm <= 0.0 || env.hop_seconds <= 0.0 {
        return None;
    }

    let target_period = 60.0 / target_bpm / env.hop_seconds; // in ODF samples
    if target_period < 2.0 || target_period > n as f64 / 2.0 {
        return None;
    }

    // Score[i] = best cumulative score for a beat ending at frame i.
    // Backptr[i] = previous beat frame for frame i, or -1 if start.
    let mut score = vec![f64::NEG_INFINITY; n];
    let mut backptr = vec![-1i32; n];

    // Search window for the previous beat: [target_period * 0.5, target_period * 2.0].
    let lo = (target_period * 0.5).floor() as usize;
    let hi = (target_period * 2.0).ceil() as usize;

    for i in 0..n {
        // Allow the first beat anywhere in the first ~1.5 periods.
        if (i as f64) < target_period * 1.5 {
            score[i] = env.odf[i];
        }
        let start = i.saturating_sub(hi);
        let end = if i >= lo { i - lo } else { continue };
        for j in start..=end {
            if !score[j].is_finite() {
                continue;
            }
            let period = (i - j) as f64;
            let log_ratio = (period / target_period).ln();
            let penalty = -TIGHTNESS * log_ratio * log_ratio;
            let cand = score[j] + env.odf[i] + penalty;
            if cand > score[i] {
                score[i] = cand;
                backptr[i] = j as i32;
            }
        }
    }

    // Backtrace from the highest-scoring frame in the last ~target_period samples.
    let tail_start = n.saturating_sub(target_period.ceil() as usize);
    let (mut cur, _) = (tail_start..n)
        .map(|i| (i, score[i]))
        .filter(|&(_, s)| s.is_finite())
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;

    let mut frames = Vec::new();
    loop {
        frames.push(cur);
        if backptr[cur] < 0 {
            break;
        }
        cur = backptr[cur] as usize;
    }
    frames.reverse();

    if frames.len() < MIN_BEATS {
        return None;
    }

    let beats: Vec<f64> = frames.iter().map(|&f| f as f64 * env.hop_seconds).collect();
    let strength: f64 = frames.iter().map(|&f| env.odf[f]).sum::<f64>() / frames.len() as f64;

    Some(BeatTrack { beats, strength })
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
    fn tracks_120_bpm_clicks() {
        let env = compute_onset_envelope(&click_train(44100, 8.0, 0.5), 44100).unwrap();
        let track = track_beats(&env, 120.0).expect("should track 120 BPM clicks");
        // ~16 beats over 8 s.
        assert!(
            track.beats.len() >= 14 && track.beats.len() <= 18,
            "expected ~16 beats, got {}",
            track.beats.len()
        );
        // Inter-beat intervals should average ~0.5 s.
        let ibis: Vec<f64> = track.beats.windows(2).map(|w| w[1] - w[0]).collect();
        let mean_ibi = ibis.iter().sum::<f64>() / ibis.len() as f64;
        assert!(
            (mean_ibi - 0.5).abs() < 0.05,
            "mean IBI {} != 0.5",
            mean_ibi
        );
        assert!(track.strength > 0.3, "strength too low: {}", track.strength);
    }

    #[test]
    fn silence_produces_zero_strength() {
        let env = compute_onset_envelope(&vec![0.0f32; 44100 * 4], 44100).unwrap();
        let track = track_beats(&env, 120.0);
        if let Some(t) = track {
            assert!(
                t.strength < 0.05,
                "silence produced strength {}",
                t.strength
            );
        }
    }
}
