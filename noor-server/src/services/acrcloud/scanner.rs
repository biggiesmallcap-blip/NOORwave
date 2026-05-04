use crate::db::queries;
use crate::services::acrcloud::identify::{IdentifyResult, identify_track};
use crate::services::audio_analysis::fingerprint::extract_fingerprint;
use crate::{AppEvent, SharedState};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;
use tracing::{debug, info, warn};

/// Minimum number of constellation peaks required before an ACRCloud query is
/// worth spending. Anything sparser is almost certainly silence, a tone, or
/// unanalysable — skip locally rather than consume a daily request.
const MIN_PEAKS_FOR_QUERY: u32 = 50;

/// Seconds to back off after a 429 response.
const RATE_LIMIT_BACKOFF_SECS: u64 = 60;

/// Shared backoff gate: unix-timestamp (seconds) after which ACRCloud requests
/// are allowed again. `0` means "no backoff active".
pub type BackoffGate = Arc<AtomicU64>;

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// If a backoff is active, sleep until it elapses. Returns early if the scan
/// is cancelled during the wait.
async fn wait_for_backoff(backoff_until: &BackoffGate, cancel: &Arc<AtomicBool>) {
    loop {
        let resume_at = backoff_until.load(Ordering::Relaxed);
        let now = now_unix();
        if resume_at <= now {
            return;
        }
        let remaining = resume_at - now;
        debug!("ACRCloud backoff active: {}s remaining", remaining);
        // Sleep in short slices so cancellation stays responsive.
        let slice = remaining.min(2);
        sleep(Duration::from_secs(slice)).await;
        if cancel.load(Ordering::Relaxed) {
            return;
        }
    }
}

/// Run ACRCloud scan on library tracks
pub async fn run_acrcloud_scan(state: SharedState, cancel: Arc<AtomicBool>) {
    info!("Starting ACRCloud library scan.");

    let client = {
        let s = state.read().await;
        s.acrcloud_client.clone()
    };

    if client.is_none() {
        warn!("ACRCloud client not configured, skipping scan.");
        return;
    }

    let client = client.unwrap();

    // Shared backoff gate — populated by 429 responses.
    let backoff_until: BackoffGate = Arc::new(AtomicU64::new(0));

    // Get tracks missing ACRCloud results
    let tracks = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            // Query tracks that don't have acrcloud_results yet
            // For now, use get_tracks_missing_dsp_features as proxy
            queries::get_tracks_missing_dsp_features(conn, 500)
        })
        .unwrap_or_default();

    let total = tracks.len() as u32;
    let mut scanned = 0u32;
    let mut matches_found = 0u32;

    for track in tracks {
        if cancel.load(Ordering::Relaxed) {
            info!("ACRCloud scan cancelled at {}/{}", scanned, total);
            break;
        }

        // Respect any active rate-limit backoff before doing *anything* network-y.
        wait_for_backoff(&backoff_until, &cancel).await;
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        // --- Sparse-signal guard ---------------------------------------
        // We don't have raw PCM samples in this placeholder path yet, but the
        // guard belongs on the fingerprint result regardless — so whenever we
        // *do* have samples we check before calling the API. For now the
        // placeholder skips the API entirely; once samples are wired through,
        // the block below gates the call.
        let samples: Vec<f32> = Vec::new();
        let sample_rate: u32 = 44_100;

        if !samples.is_empty() {
            let (_hashes, peak_count) = extract_fingerprint(&samples, sample_rate);
            if peak_count < MIN_PEAKS_FOR_QUERY {
                debug!(
                    "ACRCloud: skipping track {} — only {} peaks (< {} required)",
                    track.id, peak_count, MIN_PEAKS_FOR_QUERY
                );
                scanned += 1;
                let _ = state
                    .read()
                    .await
                    .event_tx
                    .send(AppEvent::AcrCloudScanProgress {
                        scanned,
                        total,
                        matches_found,
                    });
                continue;
            }

            match identify_track(&client, &samples, sample_rate).await {
                IdentifyResult::Match(_hit) => {
                    matches_found += 1;
                }
                IdentifyResult::NoMatch => {
                    // Normal miss or recoverable network error — move on.
                }
                IdentifyResult::RateLimited => {
                    let resume = now_unix() + RATE_LIMIT_BACKOFF_SECS;
                    backoff_until.store(resume, Ordering::Relaxed);
                    warn!(
                        "ACRCloud: rate-limited, pausing scan for {}s",
                        RATE_LIMIT_BACKOFF_SECS
                    );
                    // Don't count this track as scanned — retry on next loop
                    // after the backoff elapses. Wait here so we don't spin.
                    wait_for_backoff(&backoff_until, &cancel).await;
                    continue;
                }
            }
        }

        scanned += 1;

        let _ = state
            .read()
            .await
            .event_tx
            .send(AppEvent::AcrCloudScanProgress {
                scanned,
                total,
                matches_found,
            });

        sleep(Duration::from_secs(3)).await; // Rate limit
    }

    let _ = state
        .read()
        .await
        .event_tx
        .send(AppEvent::AcrCloudScanComplete {
            scanned,
            matches_found,
        });
    info!("ACRCloud scan complete. {} matches found.", matches_found);
}
