//! Auto-rebuild orchestrator for the radio similarity index (`track_similarity`).
//!
//! `compute_track_similarity` populates the metadata-heuristic recall lane for
//! radio's Engine source (co-album / co-artist / co-listen / genre proximity).
//! It is a one-shot job with no trigger of its own, so if it is never run the
//! Engine lane silently contributes nothing to every radio queue (this is what
//! the 2026-05-14 radio_diagnostics review found). This module keeps the index
//! fresh without the user pressing the Settings button.
//!
//! Hook points (both wired in `main.rs`):
//!   1. A `LibrarySynced` listener — the library just changed, so the index is
//!      stale. This is the primary trigger.
//!   2. An hourly catch-up ticker — picks up changes that were debounced or
//!      deferred, and rebuilds a genuinely old index even on quiet installs.
//!
//! Three safety properties matter here:
//!   - **It must not freeze reads.** `compute_track_similarity` runs on an
//!     isolated connection (`Database::open_isolated`) inside `spawn_blocking`,
//!     so it holds neither the shared connection mutex nor an async worker —
//!     request-path reads keep flowing in WAL mode throughout, and see a
//!     consistent snapshot until the rebuild's single transaction commits.
//!   - **It must not starve writes.** The rebuild is a multi-minute SQLite
//!     write transaction, and WAL still allows only one writer. So the auto
//!     path runs only while the app is otherwise quiet (no playback, sync,
//!     enrichment, or analysis) — see the idle gate in `run_if_stale`. The
//!     manual Settings button is intentionally *not* gated: the user asked.
//!   - **It must not lose changes or thrash.** Concurrency is gated by the
//!     `radio_similarity_running` atomic; `LibrarySynced` rebuilds are
//!     debounced by `MIN_REBUILD_INTERVAL_SECS`; and a `LibrarySynced` that is
//!     debounced or idle-deferred sets a persistent `radio_similarity_dirty`
//!     flag so the hourly ticker rebuilds once the window clears, rather than
//!     the change waiting for the `MAX_STALE_AGE_SECS` sweep.

use crate::db::Database;
use crate::{AppEvent, SharedState};
use rusqlite::OptionalExtension;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// Debounce window for `LibrarySynced` rebuilds: a sync/enrichment burst won't
/// rebuild if the index was already rebuilt within this window.
const MIN_REBUILD_INTERVAL_SECS: i64 = 6 * 3600;
/// The hourly catch-up ticker rebuilds once the index crosses this age even
/// with no observed library change — a long-stop for quiet, drifting installs.
const MAX_STALE_AGE_SECS: i64 = 7 * 24 * 3600;
/// `server_config` key: "1" when a library change was observed but the rebuild
/// was debounced or deferred, so the ticker knows a rebuild is owed.
const DIRTY_FLAG_KEY: &str = "radio_similarity_dirty";

/// What asked for the rebuild — determines the freshness rule.
#[derive(Debug, Clone, Copy)]
pub enum RebuildTrigger {
    /// The library changed (sync, enrichment, dedup). The change itself is the
    /// staleness signal, so we rebuild once past the debounce window without
    /// guessing from row counts — a metadata- or genre-only change leaves the
    /// track count identical but still invalidates the similarity inputs.
    LibrarySynced,
    /// The hourly catch-up ticker. No specific change signal of its own, so it
    /// rebuilds only a genuinely old index or a pending (dirty) change.
    Periodic,
}

/// Rebuild the radio similarity index in the background if it is stale for the
/// given trigger, the app is idle, and no rebuild is already running. Returns
/// immediately; never blocks the caller.
pub async fn run_if_stale(state: SharedState, trigger: RebuildTrigger) {
    let (
        db,
        event_tx,
        running,
        audio_active,
        tidal_sync_running,
        lastfm_enrich_running,
        musicbrainz_enrich_running,
        spotify_enrich_running,
        audio_analysis_running,
        acrcloud_scan_running,
    ) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.event_tx.clone(),
            s.radio_similarity_running.clone(),
            s.audio_active.clone(),
            s.tidal_sync_running.clone(),
            s.lastfm_enrich_running.clone(),
            s.musicbrainz_enrich_running.clone(),
            s.spotify_enrich_running.clone(),
            s.audio_analysis_running.clone(),
            s.acrcloud_scan_running.clone(),
        )
    };

    if running.load(Ordering::SeqCst) {
        debug!(target: "noor.radio_similarity", "rebuild already running, skipping");
        return;
    }

    // Freshness inputs in one connection grab: index age (None = never built)
    // and the pending-rebuild flag.
    let snapshot = db.with_conn(|conn| {
        let age_secs: Option<i64> = conn.query_row(
            "SELECT CAST((julianday('now') - julianday(MAX(computed_at))) * 86400 AS INTEGER)
             FROM track_similarity",
            [],
            |r| r.get(0),
        )?;
        let dirty: bool = conn
            .query_row(
                "SELECT value FROM server_config WHERE key = ?1",
                rusqlite::params![DIRTY_FLAG_KEY],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|v| v == "1")
            .unwrap_or(false);
        Ok((age_secs, dirty))
    });
    let (age_secs, dirty) = match snapshot {
        Ok(v) => v,
        Err(err) => {
            warn!(target: "noor.radio_similarity", error = %err, "freshness check failed");
            return;
        }
    };

    if !should_rebuild(age_secs, dirty, trigger) {
        // A `LibrarySynced` inside the debounce window still changed the
        // library — record it so the hourly ticker rebuilds once the window
        // clears, instead of the change waiting for the stale-age sweep.
        if matches!(trigger, RebuildTrigger::LibrarySynced) {
            mark_dirty(&db);
        }
        debug!(
            target: "noor.radio_similarity",
            ?trigger, age_secs, dirty, "index fresh for this trigger, skipping rebuild"
        );
        return;
    }

    // A rebuild is warranted — but it is a multi-minute SQLite write
    // transaction, and WAL allows only one writer. Run it only while the app
    // is otherwise quiet, so foreground writes (listen history, queue/runtime
    // state, sync metadata) don't fail on the busy timeout. If something is
    // active, defer: a `LibrarySynced` trigger leaves the dirty flag set so the
    // ticker retries; a `Periodic` trigger simply re-evaluates next tick.
    let busy_reason = if audio_active.load(Ordering::SeqCst) {
        Some("audio playback")
    } else if tidal_sync_running.load(Ordering::SeqCst) {
        Some("tidal sync")
    } else if lastfm_enrich_running.load(Ordering::SeqCst) {
        Some("last.fm enrichment")
    } else if musicbrainz_enrich_running.load(Ordering::SeqCst) {
        Some("musicbrainz enrichment")
    } else if spotify_enrich_running.load(Ordering::SeqCst) {
        Some("spotify enrichment")
    } else if audio_analysis_running.load(Ordering::SeqCst) {
        Some("audio analysis")
    } else if acrcloud_scan_running.load(Ordering::SeqCst) {
        Some("acrcloud scan")
    } else {
        None
    };
    if let Some(reason) = busy_reason {
        if matches!(trigger, RebuildTrigger::LibrarySynced) {
            mark_dirty(&db);
        }
        debug!(
            target: "noor.radio_similarity",
            ?trigger, reason, "app busy, deferring rebuild"
        );
        return;
    }

    if try_spawn_rebuild(db, event_tx, running) {
        info!(
            target: "noor.radio_similarity",
            ?trigger, age_secs, "radio similarity rebuild started"
        );
    }
}

/// Pure rebuild decision, split out so the freshness logic is unit-testable.
/// Considers only index age, the dirty flag, and the trigger — not app
/// activity (the idle gate) or the single-flight slot.
fn should_rebuild(age_secs: Option<i64>, dirty: bool, trigger: RebuildTrigger) -> bool {
    match (age_secs, trigger) {
        // Never built — always worth building, whatever asked.
        (None, _) => true,
        // A library change: rebuild once past the debounce window. Inside the
        // window the caller records the change via the dirty flag instead.
        (Some(age), RebuildTrigger::LibrarySynced) => age >= MIN_REBUILD_INTERVAL_SECS,
        // Periodic catch-up: a genuinely old index, or a debounced/deferred
        // change that is now past the debounce window.
        (Some(age), RebuildTrigger::Periodic) => {
            age > MAX_STALE_AGE_SECS || (dirty && age >= MIN_REBUILD_INTERVAL_SECS)
        }
    }
}

/// Record that a library change is owed a rebuild. Cleared by `try_spawn_rebuild`
/// once a rebuild completes.
fn mark_dirty(db: &Database) {
    let result = db.with_conn(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO server_config (key, value) VALUES (?1, '1')",
            rusqlite::params![DIRTY_FLAG_KEY],
        )?;
        Ok(())
    });
    if let Err(err) = result {
        warn!(target: "noor.radio_similarity", error = %err, "failed to set dirty flag");
    }
}

/// Claim the single-flight slot and spawn the rebuild on a dedicated
/// connection. Returns `false` (without spawning) if a rebuild is already
/// running. Shared by the auto-rebuild triggers and the manual Settings route.
///
/// The `swap` is the real single-flight guard: callers may race past the
/// cheaper `load` check in `run_if_stale`, but only one wins the swap.
pub fn try_spawn_rebuild(
    db: Database,
    event_tx: broadcast::Sender<AppEvent>,
    running: Arc<AtomicBool>,
) -> bool {
    if running.swap(true, Ordering::SeqCst) {
        return false;
    }
    // spawn_blocking, not spawn: compute_track_similarity is a synchronous,
    // minutes-long call — it must not occupy an async worker thread.
    tokio::task::spawn_blocking(move || {
        // Releases the single-flight flag on drop, including on panic — a
        // wedged flag would silently block every future rebuild.
        let _guard = RunningGuard(running);
        let result = db.open_isolated().and_then(|conn| {
            let pairs = crate::db::queries::compute_track_similarity(&conn)?;
            // The index now reflects current library state — clear the pending
            // flag so the ticker doesn't rebuild again needlessly.
            if let Err(err) = conn.execute(
                "INSERT OR REPLACE INTO server_config (key, value) VALUES (?1, '0')",
                rusqlite::params![DIRTY_FLAG_KEY],
            ) {
                warn!(target: "noor.radio_similarity", error = %err, "failed to clear dirty flag");
            }
            Ok(pairs)
        });
        match result {
            Ok(pairs) => {
                info!(
                    target: "noor.radio_similarity",
                    pairs, "radio similarity rebuild complete"
                );
                let _ = event_tx.send(AppEvent::RadioSimilarityComputed {
                    pairs: pairs as i64,
                });
            }
            Err(err) => {
                warn!(target: "noor.radio_similarity", error = %err, "radio similarity rebuild failed");
            }
        }
    });
    true
}

/// Releases the `radio_similarity_running` single-flight flag on drop, so a
/// panic mid-rebuild can't leave it stuck at `true` and block every later run.
struct RunningGuard(Arc<AtomicBool>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_built_always_rebuilds() {
        assert!(should_rebuild(None, false, RebuildTrigger::LibrarySynced));
        assert!(should_rebuild(None, false, RebuildTrigger::Periodic));
    }

    #[test]
    fn library_synced_debounces_recent_rebuilds() {
        // Inside the debounce window — skip (the caller marks it dirty).
        assert!(!should_rebuild(
            Some(60),
            false,
            RebuildTrigger::LibrarySynced
        ));
        // Past the debounce window — rebuild.
        assert!(should_rebuild(
            Some(MIN_REBUILD_INTERVAL_SECS),
            false,
            RebuildTrigger::LibrarySynced
        ));
    }

    #[test]
    fn periodic_honors_dirty_flag_after_debounce() {
        // Fresh index, nothing pending — nothing to do.
        assert!(!should_rebuild(Some(3600), false, RebuildTrigger::Periodic));
        // A change is pending but still inside the debounce window — wait.
        assert!(!should_rebuild(Some(3600), true, RebuildTrigger::Periodic));
        // Debounce window passed and a change is pending — rebuild now, without
        // waiting for the stale-age sweep.
        assert!(should_rebuild(
            Some(MIN_REBUILD_INTERVAL_SECS),
            true,
            RebuildTrigger::Periodic
        ));
    }

    #[test]
    fn periodic_rebuilds_a_very_stale_index() {
        assert!(should_rebuild(
            Some(MAX_STALE_AGE_SECS + 1),
            false,
            RebuildTrigger::Periodic
        ));
    }
}
