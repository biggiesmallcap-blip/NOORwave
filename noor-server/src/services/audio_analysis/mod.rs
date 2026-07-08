pub mod bpm;
pub mod dj_profile;
pub mod engine;
pub mod features;
pub mod fingerprint;
pub mod key;
pub mod onset;
pub mod queue_prescanner;
pub mod scanner;
pub mod tempo;

// v11: energy is now a perceptual map of integrated LUFS (-30 -> 0.0,
// -6 -> 1.0) instead of RMS/0.7, which pinned ~97% of a real library below
// 0.5 and crushed the Sonic Field chart into the left third of its axis.
// Bumping the version re-runs analysis once on every existing row (playback
// actor, queue prescanner, and the Settings preview scan all key off it) so
// stored energy values migrate to the new scale in the background.
// v10: passive analysis skipped the track intro before key detection.
pub const CURRENT_ANALYSIS_VERSION: &str = "v11";

/// Server-config key controlling whether the playback-driven actor analyses
/// audio at all. Defaults to enabled. Stored in the `server_config` k/v table
/// as "1" (enabled) or "0" (disabled). When disabled, the actor still runs
/// (consuming samples to drain the channel) but does not call analyse_and_save.
pub const PASSIVE_DSP_ENABLED_KEY: &str = "passive_dsp_enabled";
const PASSIVE_ANALYSIS_MAX_SAMPLE_RATE: u32 = 48_000;
/// Seconds of intro dropped before the passive analysis window. An electronic
/// track's first ~15s is frequently a quiet/atonal open (pads, filtered
/// sweeps, single-instrument builds) whose flat pitch-class profile fails key
/// detection even though the body of the track has a clear key. Mirrors the
/// preview scanner's PREVIEW_OFFSET_SEC, tuned longer for club intros. Only
/// applied when the full analysis window still remains after the skip.
const PASSIVE_INTRO_SKIP_SEC: u32 = 15;

use crate::AppEvent;
use rusqlite::Connection;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{broadcast, mpsc};
use tracing::info;

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

pub(crate) fn should_defer_background_analysis_for_active_playback(
    is_playing: bool,
    runtime_present: bool,
) -> bool {
    is_playing && runtime_present
}

pub type AnalysisJob = (i64, Vec<f32>, u32); // (track_id, mono_samples, sample_rate)

/// Actor config: max samples to analyze per track.
pub struct AnalysisConfig {
    pub max_seconds: u32, // default 30
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self { max_seconds: 30 }
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

        while let Some((track_id, samples, sample_rate)) = rx.recv().await {
            // Bail early if the user has disabled passive DSP analysis. We
            // still consume the message so the channel drains; we just skip
            // the work.
            let passive_enabled = db
                .with_conn(|conn| Ok(is_passive_enabled(conn)))
                .unwrap_or(true);
            if !passive_enabled {
                continue;
            }

            // Skip tracks already on the current analysis version OR with a
            // manual BPM override (the user has spoken; don't clobber it).
            let already_analyzed = db
                .with_conn(|conn| -> anyhow::Result<bool> {
                    use rusqlite::OptionalExtension;
                    let row: Option<(String, i64)> = conn
                        .query_row(
                            "SELECT analysis_version, manual_override FROM audio_dsp_features WHERE track_id = ?1",
                            rusqlite::params![track_id],
                            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                        )
                        .optional()?;
                    Ok(match row {
                        Some((v, override_flag)) => {
                            override_flag != 0 || v == CURRENT_ANALYSIS_VERSION
                        }
                        None => false,
                    })
                })
                .unwrap_or(false);

            let Some((samples, sample_rate, offset_ms)) = prepare_passive_analysis_job(
                samples,
                sample_rate,
                config.max_seconds,
                already_analyzed,
            ) else {
                continue;
            };

            // CPU-heavy DSP must run off the tokio worker (Issue A).
            let db_clone = db.clone();
            let result = tokio::task::spawn_blocking(move || {
                engine::analyze_and_save(
                    &db_clone,
                    &samples,
                    sample_rate,
                    "passive",
                    track_id,
                    offset_ms,
                )
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
                let _ = event_tx.send(AppEvent::TrackAnalyzed { track_id });
            }
        }

        info!(
            "Analysis actor shut down. Analyzed {} tracks this session.",
            analyzed_count
        );
    });

    tx
}

fn prepare_passive_analysis_job(
    mut samples: Vec<f32>,
    mut sample_rate: u32,
    max_seconds: u32,
    already_analyzed: bool,
) -> Option<(Vec<f32>, u32, i64)> {
    if already_analyzed {
        return None;
    }

    let window_samples = (sample_rate as usize).saturating_mul(max_seconds as usize);
    let skip_samples = (sample_rate as usize).saturating_mul(PASSIVE_INTRO_SKIP_SEC as usize);
    // Skip the intro only when the whole analysis window still remains after
    // it; short tracks (or an early-flushed decode) keep the from-start window
    // so they are never left unanalysed.
    let offset_ms = if samples.len() >= skip_samples.saturating_add(window_samples) {
        samples.drain(..skip_samples);
        i64::from(PASSIVE_INTRO_SKIP_SEC) * 1000
    } else {
        0
    };

    if samples.len() > window_samples {
        samples.truncate(window_samples);
    }
    (samples, sample_rate) = prepare_passive_analysis_samples(samples, sample_rate);

    Some((samples, sample_rate, offset_ms))
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

/// How two Camelot keys relate harmonically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CamelotRelation {
    /// Same key, relative major/minor, or one step around the wheel.
    Compatible,
    /// Adjacent on the wheel but not compatible.
    Adjacent,
    /// Neither — a harmonic clash.
    Clash,
}

/// Classify the harmonic relationship between two Camelot keys. Single source
/// of truth shared by `compute_harmonic_multiplier` and automix's reason
/// signals, so the score multiplier and the user-facing "Why" can't drift.
pub fn camelot_relation(a: &str, b: &str) -> CamelotRelation {
    if camelot_compatible(a, b) {
        CamelotRelation::Compatible
    } else if camelot_adjacent(a, b) {
        CamelotRelation::Adjacent
    } else {
        CamelotRelation::Clash
    }
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
    fn passive_analysis_plan_skips_current_version_before_preparing_samples() {
        let input: Vec<f32> = (0..16).map(|i| i as f32).collect();

        let planned = prepare_passive_analysis_job(input, u32::MAX, 30, true);

        assert!(planned.is_none());
    }

    #[test]
    fn passive_analysis_plan_skips_intro_when_enough_audio() {
        // 60s at 10 Hz = 600 samples; a 15s intro skip (150) plus a 30s window
        // (300) fits, so the window starts at the 15s mark.
        let input: Vec<f32> = (0..600).map(|i| i as f32).collect();

        let (samples, sample_rate, offset_ms) =
            prepare_passive_analysis_job(input, 10, 30, false).expect("planned");

        assert_eq!(sample_rate, 10);
        assert_eq!(offset_ms, 15_000);
        assert_eq!(samples.len(), 300);
        assert_eq!(samples[0], 150.0);
        assert_eq!(samples[299], 449.0);
    }

    #[test]
    fn passive_analysis_plan_keeps_start_when_too_short_to_skip() {
        // 30s at 10 Hz = 300 samples: not enough to drop a 15s intro and still
        // keep a 30s window, so analyse from the start rather than skip.
        let input: Vec<f32> = (0..300).map(|i| i as f32).collect();

        let (samples, sample_rate, offset_ms) =
            prepare_passive_analysis_job(input, 10, 30, false).expect("planned");

        assert_eq!(sample_rate, 10);
        assert_eq!(offset_ms, 0);
        assert_eq!(samples.len(), 300);
        assert_eq!(samples[0], 0.0);
    }

    #[test]
    fn background_analysis_defers_while_foreground_playback_is_active() {
        assert!(!should_defer_background_analysis_for_active_playback(
            false, true,
        ));
        assert!(!should_defer_background_analysis_for_active_playback(
            true, false,
        ));
        assert!(should_defer_background_analysis_for_active_playback(
            true, true,
        ));
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
        mult *= match camelot_relation(a, b) {
            CamelotRelation::Compatible => 2.2,
            CamelotRelation::Adjacent => 1.4,
            CamelotRelation::Clash => 0.6,
        };
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
