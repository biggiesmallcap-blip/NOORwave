pub mod bpm;
pub mod engine;
pub mod features;
pub mod fingerprint;
pub mod key;
pub mod onset;
pub mod scanner;

pub const CURRENT_ANALYSIS_VERSION: &str = "v3";

use crate::AppEvent;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{broadcast, mpsc};
use tracing::info;

use crate::db::queries;

pub type AnalysisJob = (i64, Vec<f32>, u32); // (track_id, mono_samples, sample_rate)

/// Actor config: max samples to analyze per track, minimum interval between analyses
pub struct AnalysisConfig {
    pub max_seconds: u32, // default 30
    #[allow(dead_code)]
    pub min_interval_hours: u32, // default 7
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            max_seconds: 30,
            min_interval_hours: 7,
        }
    }
}

/// Spawn the analysis actor. Returns the sender for jobs.
/// The actor runs on its own tokio task and processes jobs sequentially.
pub fn spawn_actor(
    db: crate::db::Database,
    event_tx: broadcast::Sender<AppEvent>,
    cancel: Arc<AtomicBool>,
    config: AnalysisConfig,
) -> mpsc::UnboundedSender<AnalysisJob> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AnalysisJob>();

    tokio::spawn(async move {
        let mut analyzed_count: u32 = 0;

        while let Some((track_id, mut samples, sample_rate)) = rx.recv().await {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                info!("Analysis actor cancelled, stopping.");
                break;
            }

            let max_samples = (sample_rate * config.max_seconds) as usize;
            if samples.len() > max_samples {
                samples.truncate(max_samples);
            }

            // Skip tracks already on the current analysis version.
            let already_analyzed = db
                .with_conn(|conn| queries::get_audio_dsp_features(conn, track_id))
                .ok()
                .flatten()
                .map(|f| f.analysis_version == "v2")
                .unwrap_or(false);

            if already_analyzed {
                continue;
            }

            // CPU-heavy DSP must run off the tokio worker (Issue A).
            let db_clone = db.clone();
            let result = tokio::task::spawn_blocking(move || {
                engine::analyze_and_save(&db_clone, &samples, sample_rate, "passive", track_id)
            })
            .await
            .ok()
            .flatten();

            if result.is_some() {
                analyzed_count += 1;
                let _ = event_tx.send(AppEvent::AudioAnalysisProgress {
                    analyzed: analyzed_count,
                    total: 0, // unknown total in passive mode
                    mode: "passive".to_string(),
                });
            }
        }

        info!(
            "Analysis actor shut down. Analyzed {} tracks this session.",
            analyzed_count
        );
    });

    tx
}

/// Camelot compatibility helpers (reused by automix scoring + radio).

/// Check if two Camelot keys are compatible (same number, or differ by 1 mod 12).
pub fn camelot_compatible(a: &str, b: &str) -> bool {
    camelot_number(a) == camelot_number(b) || camelot_number_diff(a, b) == 1
}

/// Check if two Camelot keys are adjacent (differ by 1 mod 12, or same number A<->B).
pub fn camelot_adjacent(a: &str, b: &str) -> bool {
    camelot_number_diff(a, b) == 1
        || (camelot_number(a) == camelot_number(b) && camelot_letter(a) != camelot_letter(b))
}

fn camelot_number(k: &str) -> u32 {
    k.trim_end_matches(['A', 'B']).parse::<u32>().unwrap_or(0)
}

fn camelot_letter(k: &str) -> char {
    k.chars().last().unwrap_or('A')
}

fn camelot_number_diff(a: &str, b: &str) -> u32 {
    let na = camelot_number(a);
    let nb = camelot_number(b);
    let diff = na.abs_diff(nb);
    diff.min(12 - diff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camelot_compatible_accepts_same_relative_and_adjacent_keys() {
        assert!(camelot_compatible("8A", "8A"));
        assert!(camelot_compatible("8A", "8B"));
        assert!(camelot_compatible("8B", "8A"));
        assert!(camelot_compatible("8A", "9A"));
        assert!(camelot_compatible("12B", "1B"));
    }

    #[test]
    fn camelot_compatible_rejects_distant_keys() {
        assert!(!camelot_compatible("8A", "10A"));
        assert!(!camelot_compatible("1A", "5A"));
    }

    #[test]
    fn camelot_adjacent_includes_relative_and_neighbor_keys() {
        assert!(camelot_adjacent("8A", "8B"));
        assert!(camelot_adjacent("8A", "9A"));
        assert!(camelot_adjacent("12B", "1B"));
    }
}

/// Compute a shared harmonic/BPM multiplier used by both automix (`player.rs`)
/// and radio post-scoring (`server/routes.rs`).
///
/// Returns 1.0 when either side is unanalyzed so we never penalise tracks we
/// simply don't know anything about.
///
/// Camelot: compatible → *2.2, adjacent → *1.4, clash → *0.6
/// BPM: diff <5 → *1.8, <10 → *1.3, <20 → *0.9, else *0.65
pub fn compute_harmonic_multiplier(
    seed_camelot: Option<&str>,
    cand_camelot: Option<&str>,
    seed_bpm: Option<f64>,
    cand_bpm: Option<f64>,
) -> f64 {
    let mut mult = 1.0_f64;

    if let (Some(a), Some(b)) = (seed_camelot, cand_camelot) {
        if camelot_compatible(a, b) {
            mult *= 2.2;
        } else if camelot_adjacent(a, b) {
            mult *= 1.4;
        } else {
            mult *= 0.6;
        }
    }

    if let (Some(a), Some(b)) = (seed_bpm, cand_bpm) {
        let diff = (a - b).abs();
        if diff < 5.0 {
            mult *= 1.8;
        } else if diff < 10.0 {
            mult *= 1.3;
        } else if diff < 20.0 {
            mult *= 0.9;
        } else {
            mult *= 0.65;
        }
    }

    mult
}
