pub mod bpm;
pub mod engine;
pub mod features;
pub mod key;
pub mod scanner;
pub mod fingerprint;

use crate::AppEvent;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::info;

use crate::db::queries;

pub type AnalysisJob = (i64, Vec<f32>, u32); // (track_id, mono_samples, sample_rate)

/// Actor config: max samples to analyze per track, minimum interval between analyses
pub struct AnalysisConfig {
    pub max_seconds: u32,       // default 30
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

        while let Some((track_id, samples, sample_rate)) = rx.recv().await {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                info!("Analysis actor cancelled, stopping.");
                break;
            }

            // Respect config max_seconds — truncate samples
            let max_samples = (sample_rate * config.max_seconds) as usize;
            let samples = if samples.len() > max_samples {
                &samples[..max_samples]
            } else {
                &samples[..]
            };

            // Check if recently analyzed
            let already_analyzed = db
                .with_conn(|conn| queries::get_audio_dsp_features(conn, track_id))
                .ok()
                .flatten()
                .map(|f| f.analysis_version == "v1")
                .unwrap_or(false);

            if already_analyzed {
                continue;
            }

            // Analyze and save
            let result = engine::analyze_and_save(&db, samples, sample_rate, "passive", track_id);

            if result.is_some() {
                analyzed_count += 1;
                // Emit progress event
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
    camelot_number(a) == camelot_number(b)
        || camelot_number_diff(a, b) == 1
}

/// Check if two Camelot keys are adjacent (differ by 1 mod 12, or same number A<->B).
pub fn camelot_adjacent(a: &str, b: &str) -> bool {
    camelot_number_diff(a, b) == 1 || (camelot_number(a) == camelot_number(b) && camelot_letter(a) != camelot_letter(b))
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
    let diff = if na > nb { na - nb } else { nb - na };
    diff.min(12 - diff)
}
