use crate::{AppEvent, SharedState};
use crate::services::acrcloud::identify::identify_track;
use crate::db::queries;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

/// Run ACRCloud scan on library tracks
pub async fn run_acrcloud_scan(
    state: SharedState,
    cancel: Arc<AtomicBool>,
) {
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

    // Get tracks missing ACRCloud results
    let tracks = state.read().await.db.with_conn(|conn| {
        // Query tracks that don't have acrcloud_results yet
        // For now, use get_tracks_missing_dsp_features as proxy
        queries::get_tracks_missing_dsp_features(conn, 500)
    }).unwrap_or_default();

    let total = tracks.len() as u32;
    let mut scanned = 0u32;
    let mut matches_found = 0u32;

    for track in tracks {
        if cancel.load(Ordering::Relaxed) {
            info!("ACRCloud scan cancelled at {}/{}", scanned, total);
            break;
        }

        // We'd need audio samples to send to ACRCloud
        // Placeholder: skip for now
        scanned += 1;

        let _ = state.read().await.event_tx.send(AppEvent::AcrCloudScanProgress {
            scanned,
            total,
            matches_found,
        });

        sleep(Duration::from_secs(3)).await; // Rate limit
    }

    let _ = state.read().await.event_tx.send(AppEvent::AcrCloudScanComplete {
        scanned,
        matches_found,
    });
    info!("ACRCloud scan complete. {} matches found.", matches_found);
}
