//! Auto-enrichment orchestrator.
//!
//! Triggers Last.fm + MusicBrainz tag enrichment in the background without
//! blocking the calling task. Designed to be called from two places in
//! `main.rs`:
//!
//! 1. A `LibrarySynced` event listener — runs whenever a sync/import handler
//!    emits the broadcast event, so newly-added tracks get enriched without
//!    the user pressing the Settings buttons.
//! 2. A daily `tokio::interval` catch-up loop — sweeps any tracks the listener
//!    missed (e.g. tracks added by code paths that don't currently emit
//!    `LibrarySynced`) and any rows that failed enrichment on a previous pass.
//!
//! Both entry points call `run_if_idle`. Concurrency is gated by the
//! per-runner `*_enrich_running` atomics on `SharedState`, so duplicate
//! invocations short-circuit cheaply — there is no lock or queue.

use crate::AppEvent;
use crate::SharedState;
use std::sync::atomic::Ordering;
use tracing::{debug, info, warn};

/// Spawn Last.fm + MusicBrainz enrichment if neither is already running and
/// each has work to do. Returns immediately after spawning the background
/// tasks; never blocks the caller.
pub async fn run_if_idle(state: SharedState) {
    spawn_lastfm_if_idle(state.clone()).await;
    spawn_musicbrainz_if_idle(state).await;
}

async fn spawn_lastfm_if_idle(state: SharedState) {
    use crate::services::lastfm;
    use crate::services::lastfm::enrichment::EnrichmentMode;

    let (
        http,
        event_tx,
        running,
        cancel,
        total_atom,
        processed_atom,
        prefetch_total_atom,
        prefetch_done_atom,
        started_at_atom,
    ) = {
        let s = state.read().await;
        (
            s.http_client.clone(),
            s.event_tx.clone(),
            s.lastfm_enrich_running.clone(),
            s.lastfm_enrich_cancel.clone(),
            s.lastfm_enrich_total.clone(),
            s.lastfm_enrich_processed.clone(),
            s.lastfm_prefetch_total.clone(),
            s.lastfm_prefetch_done.clone(),
            s.lastfm_enrich_started_at.clone(),
        )
    };

    if running.load(Ordering::SeqCst) {
        debug!(target: "noor.auto_enrich", service = "lastfm", "already running, skipping");
        return;
    }

    // Skip silently when no creds are configured — we don't want this auto-path
    // to log warnings on every fire for users who haven't connected Last.fm.
    let creds = {
        let s = state.read().await;
        s.db.with_conn(|conn| Ok(lastfm::auth::load_credentials(conn).ok().flatten()))
            .unwrap_or(None)
    };
    let Some(creds) = creds else {
        debug!(target: "noor.auto_enrich", service = "lastfm", "no credentials, skipping");
        return;
    };

    let total: usize = {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            lastfm::enrichment::count_tracks_to_enrich(conn, EnrichmentMode::Pending)
        })
        .unwrap_or(0)
    };

    if total == 0 {
        debug!(target: "noor.auto_enrich", service = "lastfm", "nothing to enrich");
        return;
    }

    info!(target: "noor.auto_enrich", service = "lastfm", total, "starting auto-enrichment");

    cancel.store(false, Ordering::SeqCst);
    running.store(true, Ordering::SeqCst);
    total_atom.store(total, Ordering::SeqCst);
    processed_atom.store(0, Ordering::SeqCst);
    prefetch_total_atom.store(0, Ordering::SeqCst);
    prefetch_done_atom.store(0, Ordering::SeqCst);
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    started_at_atom.store(now_secs, Ordering::SeqCst);

    let api_key = creds.api_key.clone();
    let started_at_atom_cleanup = started_at_atom.clone();
    tokio::spawn(async move {
        let progress_tx = event_tx.clone();
        let artist_tx = event_tx.clone();
        let total_atom_cb = total_atom.clone();
        let processed_atom_cb = processed_atom.clone();
        let prefetch_total_cb = prefetch_total_atom.clone();
        let prefetch_done_cb = prefetch_done_atom.clone();
        let result = lastfm::enrichment::run_enrichment(
            state,
            http,
            api_key,
            EnrichmentMode::Pending,
            cancel,
            move |done, artist_total| {
                prefetch_total_cb.store(artist_total, Ordering::SeqCst);
                prefetch_done_cb.store(done, Ordering::SeqCst);
                let _ = artist_tx.send(AppEvent::SyncProgress {
                    service: "lastfm".to_string(),
                    progress: done as f32 / artist_total.max(1) as f32,
                });
            },
            move |current, total| {
                processed_atom_cb.store(current, Ordering::SeqCst);
                if total > 0 {
                    total_atom_cb.store(total, Ordering::SeqCst);
                }
                let _ = progress_tx.send(AppEvent::SyncProgress {
                    service: "lastfm".to_string(),
                    progress: current as f32 / total.max(1) as f32,
                });
            },
        )
        .await;
        running.store(false, Ordering::SeqCst);
        started_at_atom_cleanup.store(0, Ordering::SeqCst);
        match result {
            Ok(_) => {
                info!(target: "noor.auto_enrich", service = "lastfm", "auto-enrichment complete");
                // Galaxy listens for MusicBrainzEnriched on the WS to schedule
                // a refresh. Reusing it (rather than adding a LastFmEnriched
                // variant) matches the existing manual-trigger handler at
                // routes.rs:13600.
                let _ = event_tx.send(AppEvent::MusicBrainzEnriched);
            }
            Err(err) => {
                warn!(target: "noor.auto_enrich", service = "lastfm", error = %err, "auto-enrichment failed");
            }
        }
    });
}

async fn spawn_musicbrainz_if_idle(state: SharedState) {
    use crate::services::musicbrainz;

    let (http_client, event_tx, running) = {
        let s = state.read().await;
        (
            s.http_client.clone(),
            s.event_tx.clone(),
            s.musicbrainz_enrich_running.clone(),
        )
    };

    if running.load(Ordering::SeqCst) {
        debug!(target: "noor.auto_enrich", service = "musicbrainz", "already running, skipping");
        return;
    }

    let total: usize = {
        let s = state.read().await;
        s.db.with_conn(musicbrainz::count_unenriched_tracks)
            .unwrap_or(0)
    };

    if total == 0 {
        debug!(target: "noor.auto_enrich", service = "musicbrainz", "nothing to enrich");
        return;
    }

    info!(target: "noor.auto_enrich", service = "musicbrainz", total, "starting auto-enrichment");
    running.store(true, Ordering::SeqCst);

    tokio::spawn(async move {
        let progress_tx = event_tx.clone();
        let result = musicbrainz::run_enrichment(
            state,
            http_client,
            move |progress| {
                let _ = progress_tx.send(AppEvent::SyncProgress {
                    service: "musicbrainz".to_string(),
                    progress: progress.processed as f32 / progress.total.max(1) as f32,
                });
            },
            1,
        )
        .await;
        running.store(false, Ordering::SeqCst);
        match result {
            Ok(_) => {
                info!(target: "noor.auto_enrich", service = "musicbrainz", "auto-enrichment complete");
                let _ = event_tx.send(AppEvent::MusicBrainzEnriched);
                // Deliberately NOT re-emitting LibrarySynced from the auto path
                // to avoid a feedback loop where MB completion would re-trigger
                // another auto-enrich cycle via the LibrarySynced listener.
            }
            Err(err) => {
                warn!(target: "noor.auto_enrich", service = "musicbrainz", error = %err, "auto-enrichment failed");
            }
        }
    });
}
