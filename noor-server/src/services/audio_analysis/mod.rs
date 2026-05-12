pub mod beat_tracker;
pub mod bpm;
pub mod engine;
pub mod features;
pub mod fingerprint;
pub mod key;
pub mod onset;
pub mod scanner;
pub mod tempo;

pub const CURRENT_ANALYSIS_VERSION: &str = "v4";

/// Server-config key controlling whether the playback-driven actor analyses
/// audio at all. Defaults to enabled. Stored in the `server_config` k/v table
/// as "1" (enabled) or "0" (disabled). When disabled, the actor still runs
/// (consuming samples to drain the channel) but does not call analyse_and_save.
pub const PASSIVE_DSP_ENABLED_KEY: &str = "passive_dsp_enabled";
const PASSIVE_ANALYSIS_MAX_SAMPLE_RATE: u32 = 48_000;

use crate::AppEvent;
use rusqlite::Connection;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{broadcast, mpsc};
use tracing::info;

use crate::db::queries;

pub fn is_passive_enabled(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT value FROM server_config WHERE key = ?1",
        rusqlite::params![PASSIVE_DSP_ENABLED_KEY],
        |row| row.get::<_, String>(0),
    )
    .map(|v| v != "0")
    .unwrap_or(true)
}

pub fn set_passive_enabled(conn: &Connection, enabled: bool) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES (?1, ?2)",
        rusqlite::params![PASSIVE_DSP_ENABLED_KEY, if enabled { "1" } else { "0" }],
    )?;
    Ok(())
}

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
///
/// The actor lives for the entire process lifetime and stops when the mpsc
/// channel closes (i.e. when the last sender is dropped on server shutdown).
/// It does NOT honour the shared `audio_analysis_cancel` flag — that flag
/// belongs to the bulk preview scanner. When the user stops the bulk scan,
/// the actor must keep running so playback-driven analysis continues.
pub fn spawn_actor(
    db: crate::db::Database,
    event_tx: broadcast::Sender<AppEvent>,
    _cancel: Arc<AtomicBool>,
    config: AnalysisConfig,
) -> mpsc::UnboundedSender<AnalysisJob> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AnalysisJob>();

    tokio::spawn(async move {
        let mut analyzed_count: u32 = 0;

        while let Some((track_id, mut samples, mut sample_rate)) = rx.recv().await {
            // Bail early if the user has disabled passive DSP analysis. We
            // still consume the message so the channel drains; we just skip
            // the work.
            let passive_enabled = db
                .with_conn(|conn| Ok(is_passive_enabled(conn)))
                .unwrap_or(true);
            if !passive_enabled {
                continue;
            }

            let max_samples = (sample_rate * config.max_seconds) as usize;
            if samples.len() > max_samples {
                samples.truncate(max_samples);
            }
            (samples, sample_rate) = prepare_passive_analysis_samples(samples, sample_rate);

            // Skip tracks already on the current analysis version.
            let already_analyzed = db
                .with_conn(|conn| queries::get_audio_dsp_features(conn, track_id))
                .ok()
                .flatten()
                .map(|f| f.analysis_version == CURRENT_ANALYSIS_VERSION)
                .unwrap_or(false);

            if already_analyzed {
                continue;
            }

            // CPU-heavy DSP must run off the tokio worker (Issue A).
            let db_clone = db.clone();
            let result = tokio::task::spawn_blocking(move || {
                engine::analyze_and_save(&db_clone, &samples, sample_rate, "passive", track_id, 0)
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

fn prepare_passive_analysis_samples(samples: Vec<f32>, sample_rate: u32) -> (Vec<f32>, u32) {
    if sample_rate <= PASSIVE_ANALYSIS_MAX_SAMPLE_RATE || sample_rate == 0 || samples.is_empty() {
        return (samples, sample_rate);
    }

    let factor = sample_rate.div_ceil(PASSIVE_ANALYSIS_MAX_SAMPLE_RATE) as usize;
    if factor <= 1 {
        return (samples, sample_rate);
    }

    let downsampled = samples
        .chunks(factor)
        .map(|chunk| chunk.iter().copied().sum::<f32>() / chunk.len() as f32)
        .collect::<Vec<_>>();
    let downsampled_rate = (sample_rate / factor as u32).max(1);

    (downsampled, downsampled_rate)
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
    fn passive_analysis_downsamples_192khz_to_48khz() {
        let input: Vec<f32> = (0..16).map(|i| i as f32).collect();

        let (samples, sample_rate) = prepare_passive_analysis_samples(input, 192_000);

        assert_eq!(sample_rate, 48_000);
        assert_eq!(samples, vec![1.5, 5.5, 9.5, 13.5]);
    }

    #[test]
    fn passive_analysis_keeps_44khz_samples_native() {
        let input = vec![0.25, -0.25, 0.5, -0.5];

        let (samples, sample_rate) = prepare_passive_analysis_samples(input.clone(), 44_100);

        assert_eq!(sample_rate, 44_100);
        assert_eq!(samples, input);
    }

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
