//! Self-healing TIDAL metadata repair.
//!
//! Some tracks land in the library with only id/title/artist and no real
//! duration or album link. The common shape: a Spotify-resolved mix/playlist
//! track that was streamed by TIDAL id and persisted (via
//! `import_track_from_metadata`) before its full TIDAL metadata was ever
//! fetched, so the row ends up with `duration_ms = 0` and `album_id IS NULL`.
//!
//! Those two gaps drive a cluster of player symptoms: the transport renders
//! `-:--`, seeking is disabled (no length to scrub within), the smooth position
//! ticker bails because `duration_ms` is falsy so position only advances on the
//! coarse server poll, and there is no artwork (no album row to hang it on).
//!
//! This background pass finds such rows and backfills them in place from TIDAL,
//! so shipped installs heal themselves - no manual migration or per-user
//! backfill required. It is wired next to `auto_enrich::run_if_idle` in
//! `main.rs`: it fires on the `LibrarySynced` broadcast and on the daily
//! catch-up interval. A per-process atomic (`tidal_repair_running`) gates it so
//! overlapping triggers short-circuit cheaply.
//!
//! The sweep selects TIDAL-backed rows missing a real duration or an album
//! link. Rows TIDAL itself cannot improve (a record with no album, a dead or
//! region-locked id) are remembered for the lifetime of the process and
//! skipped on later triggers, so nothing is refetched in a loop and a block
//! of permanently-stuck rows cannot starve fixable ones out of the batch.

use crate::SharedState;
use crate::services::tidal::client::TidalClient;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::Duration;
use tracing::{debug, info, warn};

/// Hard cap on rows repaired per run so a large backlog is chipped away across
/// several triggers instead of hammering TIDAL in one burst.
const REPAIR_BATCH_CAP: usize = 300;

/// Spacing between TIDAL `get_track` calls - polite pacing, well under the rate
/// at which the app already issues stream-resolve calls during playback.
const REPAIR_CALL_SPACING: Duration = Duration::from_millis(120);

/// Shared WHERE fragment for "TIDAL-backed track persisted without full
/// metadata": no real duration, or no album link (and therefore no artwork).
/// One definition keeps the count and batch queries in lockstep.
const NEEDS_REPAIR_WHERE: &str = "tidal_id IS NOT NULL AND tidal_id > 0
            AND source LIKE 'tidal%'
            AND (duration_ms IS NULL OR duration_ms = 0 OR album_id IS NULL)";

/// Local track ids the sweep already attempted this process run. A row that
/// repairs fully leaves the SQL predicate on its own; one that cannot be
/// improved (TIDAL has no album for it, dead id) would otherwise be refetched
/// on every trigger and, ordered newest-first, permanently crowd fixable rows
/// out of the batch. Remembering attempts for the process lifetime bounds the
/// cost to one TIDAL call per row per app run.
fn attempted_ids() -> &'static StdMutex<HashSet<i64>> {
    static ATTEMPTED: OnceLock<StdMutex<HashSet<i64>>> = OnceLock::new();
    ATTEMPTED.get_or_init(|| StdMutex::new(HashSet::new()))
}

fn mark_attempted(local_id: i64) {
    attempted_ids()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(local_id);
}

/// Message-sniff for an expired/invalid TIDAL session. Mirrors
/// `server::routes::error_looks_like_auth`, which is not visible from the
/// services layer.
fn looks_like_auth_error(err: &anyhow::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("401") || message.contains("unauthorized")
}

/// Count TIDAL-backed tracks that were persisted without full metadata.
pub fn count_tracks_needing_repair(conn: &rusqlite::Connection) -> rusqlite::Result<usize> {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM tracks WHERE {NEEDS_REPAIR_WHERE}"),
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n as usize)
}

/// Pull every `(local_id, tidal_id)` pair matching the repair predicate,
/// newest first so a user's most recent (most likely on screen) additions heal
/// first. The caller filters out already-attempted rows and applies
/// [`REPAIR_BATCH_CAP`]; fetching the full set here is what lets fixable rows
/// surface even when hundreds of newer, unfixable rows match the predicate.
fn fetch_repair_candidates(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, tidal_id FROM tracks WHERE {NEEDS_REPAIR_WHERE} ORDER BY id DESC"
    ))?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Spawn a repair pass if one isn't already running, TIDAL is connected, and
/// there is work to do. Returns immediately after spawning; never blocks the
/// caller.
pub async fn run_if_idle(state: SharedState) {
    let (db, running, tokens, tidal_http, event_tx) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.tidal_repair_running.clone(),
            s.tidal_tokens.clone(),
            s.tidal_http_client.clone(),
            s.event_tx.clone(),
        )
    };

    if running.load(Ordering::SeqCst) {
        debug!(target: "noor.tidal_repair", "already running, skipping");
        return;
    }

    // No TIDAL session -> nothing we can fetch. Skip quietly; the next trigger
    // after the user connects will pick the work back up.
    let Some(tokens) = tokens else {
        debug!(target: "noor.tidal_repair", "TIDAL not connected, skipping");
        return;
    };

    let total = db
        .with_conn(|conn| Ok(count_tracks_needing_repair(conn)?))
        .unwrap_or(0);
    if total == 0 {
        debug!(target: "noor.tidal_repair", "nothing to repair");
        return;
    }

    info!(target: "noor.tidal_repair", total, "starting TIDAL metadata repair");
    running.store(true, Ordering::SeqCst);

    tokio::spawn(async move {
        let client = TidalClient::with_http(
            tidal_http,
            tokens.access_token.clone(),
            tokens.country_code.clone(),
        );

        let candidates = db
            .with_conn(|conn| Ok(fetch_repair_candidates(conn)?))
            .unwrap_or_default();
        let batch: Vec<(i64, i64)> = {
            let attempted = attempted_ids().lock().unwrap_or_else(|e| e.into_inner());
            candidates
                .into_iter()
                .filter(|(local_id, _)| !attempted.contains(local_id))
                .take(REPAIR_BATCH_CAP)
                .collect()
        };
        if batch.is_empty() {
            running.store(false, Ordering::SeqCst);
            debug!(target: "noor.tidal_repair", "all matching rows already attempted this run");
            return;
        }

        let mut repaired = 0usize;
        let mut failed = 0usize;
        for (local_id, tidal_id) in batch {
            match client.get_track(tidal_id).await {
                Ok(track) => {
                    mark_attempted(local_id);
                    let res = db.with_conn(move |conn| {
                        Ok(crate::services::tidal::import::repair_track_metadata_tx(
                            conn, local_id, &track,
                        )?)
                    });
                    match res {
                        Ok(true) => repaired += 1,
                        Ok(false) => {}
                        Err(e) => {
                            failed += 1;
                            warn!(target: "noor.tidal_repair", local_id, tidal_id, error = %e, "repair write failed");
                        }
                    }
                }
                Err(e) if looks_like_auth_error(&e) => {
                    // Expired session: every remaining call in this batch would
                    // fail the same way, so stop burning quota now. The row is
                    // NOT marked attempted - it failed for token reasons, not
                    // row reasons - and the whole set retries on the next
                    // trigger once playback (or the resolver) refreshes the
                    // session.
                    warn!(target: "noor.tidal_repair", local_id, tidal_id, error = %e, "TIDAL session expired; aborting sweep until next trigger");
                    break;
                }
                Err(e) => {
                    // Dead/region-locked ids and transient network blips land
                    // here. Marked attempted, so this process won't refetch
                    // them; the next app run gives them one more chance.
                    mark_attempted(local_id);
                    failed += 1;
                    debug!(target: "noor.tidal_repair", local_id, tidal_id, error = %e, "get_track failed");
                }
            }
            tokio::time::sleep(REPAIR_CALL_SPACING).await;
        }

        running.store(false, Ordering::SeqCst);
        info!(target: "noor.tidal_repair", repaired, failed, "TIDAL metadata repair complete");

        // Nudge connected clients to re-pull so freshly-filled durations and
        // artwork appear without a manual refresh. Safe against a feedback loop:
        // a re-entrant repair run finds the shrunken set and short-circuits once
        // it hits zero (or drains the remaining backlog above the batch cap).
        if repaired > 0 {
            let _ = event_tx.send(crate::AppEvent::LibrarySynced);
        }
    });
}
