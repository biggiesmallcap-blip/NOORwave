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
//! ## Freshness model
//!
//! Two timestamps in `server_config` drive everything; nothing is derived from
//! `track_similarity`'s own rows (a valid library can legitimately produce zero
//! similarity pairs, which must not read as "never built"):
//!   - `radio_similarity_built_at` — the *start* time of the last successful
//!     rebuild. Start, not finish, so a `LibrarySynced` that lands during a
//!     rebuild stays "newer" and re-triggers instead of being absorbed.
//!   - `radio_similarity_library_changed_at` — stamped on *every* `LibrarySynced`,
//!     unconditionally and before any other check, so a change observed while a
//!     rebuild is running, during the debounce window, or while the app is busy
//!     is never lost.
//! `dirty` is simply `library_changed_at > built_at` — no flag to clear, no
//! clear/set race.
//!
//! ## Safety
//!   - **Reads never freeze.** The rebuild runs on an isolated connection
//!     (`Database::open_isolated`) inside `spawn_blocking`, holding neither the
//!     shared connection mutex nor an async worker.
//!   - **Writes are not starved.** The rebuild is a multi-minute SQLite write
//!     transaction, and WAL allows only one writer. Both the auto path *and*
//!     the manual Settings route gate on `busy_reason`: a rebuild only starts
//!     when no other writer (playback, sync, enrichment, analysis) is active.
//!   - **No thrash.** Single-flight via the `radio_similarity_running` atomic;
//!     `LibrarySynced` rebuilds debounced by `MIN_REBUILD_INTERVAL_SECS`.

use crate::db::Database;
use crate::{AppEvent, SharedState};
use rusqlite::{Connection, OptionalExtension};
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
/// `server_config` key: start timestamp of the last successful rebuild.
const BUILT_AT_KEY: &str = "radio_similarity_built_at";
/// `server_config` key: timestamp the library last changed (any `LibrarySynced`).
const LIBRARY_CHANGED_AT_KEY: &str = "radio_similarity_library_changed_at";

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

/// Index freshness, read from `server_config` and independent of how many rows
/// `track_similarity` happens to hold.
#[derive(Debug)]
struct Freshness {
    /// Seconds since the last successful rebuild started; `None` = never built.
    built_age_secs: Option<i64>,
    /// True when the library changed after the last rebuild started.
    dirty: bool,
}

/// Rebuild the radio similarity index in the background if it is stale for the
/// given trigger, the app is idle, and no rebuild is already running. Returns
/// immediately; never blocks the caller.
pub async fn run_if_stale(state: SharedState, trigger: RebuildTrigger) {
    let (db, event_tx, running, busy) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.event_tx.clone(),
            s.radio_similarity_running.clone(),
            busy_reason(&s),
        )
    };

    // Record the library change first — before the running, freshness, and
    // idle checks — so a change observed mid-rebuild, mid-debounce, or while
    // the app is busy is never lost. It is only ever resolved by a later
    // rebuild stamping `built_at` past it.
    if matches!(trigger, RebuildTrigger::LibrarySynced) {
        mark_library_changed(&db);
    }

    if running.load(Ordering::SeqCst) {
        debug!(target: "noor.radio_similarity", "rebuild already running, skipping");
        return;
    }

    let freshness = match db.with_conn(read_freshness) {
        Ok(f) => f,
        Err(err) => {
            warn!(target: "noor.radio_similarity", error = %err, "freshness check failed");
            return;
        }
    };

    if !should_rebuild(freshness.built_age_secs, freshness.dirty, trigger) {
        debug!(
            target: "noor.radio_similarity",
            ?trigger,
            built_age_secs = ?freshness.built_age_secs,
            dirty = freshness.dirty,
            "index fresh for this trigger, skipping rebuild"
        );
        return;
    }

    // A rebuild is warranted, but it owns SQLite's single writer slot for
    // minutes. Run it only while the app is otherwise quiet; otherwise defer.
    // The change is already recorded in `library_changed_at`, so the hourly
    // ticker will retry once things settle.
    if let Some(reason) = busy {
        debug!(target: "noor.radio_similarity", ?trigger, reason, "app busy, deferring rebuild");
        return;
    }

    if try_spawn_rebuild(db, event_tx, running) {
        info!(
            target: "noor.radio_similarity",
            ?trigger,
            built_age_secs = ?freshness.built_age_secs,
            "radio similarity rebuild started"
        );
    }
}

/// The active foreground job that should hold off a rebuild, or `None` if the
/// app is idle enough. A rebuild owns SQLite's single writer slot for minutes,
/// so neither the auto path nor the manual Settings route may run one while
/// another writer is active.
pub fn busy_reason(state: &crate::AppState) -> Option<&'static str> {
    use std::sync::atomic::Ordering::SeqCst;
    if state.audio_active.load(SeqCst) {
        Some("audio playback")
    } else if state.tidal_sync_running.load(SeqCst) {
        Some("a TIDAL sync")
    } else if state.lastfm_enrich_running.load(SeqCst) {
        Some("Last.fm enrichment")
    } else if state.musicbrainz_enrich_running.load(SeqCst) {
        Some("MusicBrainz enrichment")
    } else if state.spotify_enrich_running.load(SeqCst) {
        Some("Spotify enrichment")
    } else if state.audio_analysis_running.load(SeqCst) {
        Some("audio analysis")
    } else if state.acrcloud_scan_running.load(SeqCst) {
        Some("an ACRCloud scan")
    } else {
        None
    }
}

/// Pure rebuild decision, split out so the freshness logic is unit-testable.
/// Considers only index age, the dirty flag, and the trigger — not app
/// activity (the idle gate) or the single-flight slot.
fn should_rebuild(built_age: Option<i64>, dirty: bool, trigger: RebuildTrigger) -> bool {
    match (built_age, trigger) {
        // Never built — always worth building, whatever asked.
        (None, _) => true,
        // A library change: rebuild once past the debounce window. Inside the
        // window the change stays recorded in `library_changed_at`.
        (Some(age), RebuildTrigger::LibrarySynced) => age >= MIN_REBUILD_INTERVAL_SECS,
        // Periodic catch-up: a genuinely old index, or a debounced/deferred
        // change that is now past the debounce window.
        (Some(age), RebuildTrigger::Periodic) => {
            age > MAX_STALE_AGE_SECS || (dirty && age >= MIN_REBUILD_INTERVAL_SECS)
        }
    }
}

fn read_config(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM server_config WHERE key = ?1",
        rusqlite::params![key],
        |r| r.get(0),
    )
    .optional()
}

/// Read index freshness from `server_config`. Deliberately does not touch
/// `track_similarity` — see the module's freshness-model note.
fn read_freshness(conn: &Connection) -> anyhow::Result<Freshness> {
    let built_at = read_config(conn, BUILT_AT_KEY)?;
    let library_changed_at = read_config(conn, LIBRARY_CHANGED_AT_KEY)?;

    let built_age_secs: Option<i64> = if built_at.is_some() {
        conn.query_row(
            "SELECT CAST((julianday('now') - julianday(value)) * 86400 AS INTEGER)
             FROM server_config WHERE key = ?1",
            rusqlite::params![BUILT_AT_KEY],
            |r| r.get(0),
        )
        .optional()?
    } else {
        None
    };

    // ISO-8601-ish timestamps from datetime('now') sort correctly as text.
    let dirty = match (&library_changed_at, &built_at) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(changed), Some(built)) => changed.as_str() > built.as_str(),
    };

    Ok(Freshness {
        built_age_secs,
        dirty,
    })
}

/// Stamp `library_changed_at = now`. Called for every `LibrarySynced` before
/// any other check, so the change survives a busy app or an in-flight rebuild.
fn mark_library_changed(db: &Database) {
    let result = db.with_conn(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO server_config (key, value) VALUES (?1, datetime('now'))",
            rusqlite::params![LIBRARY_CHANGED_AT_KEY],
        )?;
        Ok(())
    });
    if let Err(err) = result {
        warn!(target: "noor.radio_similarity", error = %err, "failed to record library change");
    }
}

/// Claim the single-flight slot and spawn the rebuild on a dedicated
/// connection. Returns `false` (without spawning) if a rebuild is already
/// running. Shared by the auto-rebuild triggers and the manual Settings route.
///
/// Callers are responsible for the idle gate (`busy_reason`); this only owns
/// the single-flight `swap` — callers may race past the cheaper `load` check,
/// but only one wins the swap.
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
        let result = (|| -> anyhow::Result<usize> {
            let conn = db.open_isolated()?;
            // Capture the start time before the rebuild reads any data: a
            // LibrarySynced that lands during the rebuild stamps a later
            // `library_changed_at`, which stays newer than this `built_at` and
            // so re-triggers a rebuild instead of being silently absorbed.
            let started_at: String =
                conn.query_row("SELECT datetime('now')", [], |r| r.get(0))?;
            let pairs = crate::db::queries::compute_track_similarity(&conn)?;
            // Record completion independently of row count — a valid library
            // can legitimately produce zero pairs, which must not read as
            // "never built".
            conn.execute(
                "INSERT OR REPLACE INTO server_config (key, value) VALUES (?1, ?2)",
                rusqlite::params![BUILT_AT_KEY, started_at],
            )?;
            Ok(pairs)
        })();
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
    use crate::db::Database;

    #[test]
    fn never_built_always_rebuilds() {
        assert!(should_rebuild(None, false, RebuildTrigger::LibrarySynced));
        assert!(should_rebuild(None, false, RebuildTrigger::Periodic));
    }

    #[test]
    fn library_synced_debounces_recent_rebuilds() {
        // Inside the debounce window — skip (the change stays recorded).
        assert!(!should_rebuild(Some(60), false, RebuildTrigger::LibrarySynced));
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

    fn set_config(db: &Database, key: &str, value: &str) {
        db.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO server_config (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn freshness_never_built_never_changed() {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        let f = db.with_conn(read_freshness).unwrap();
        assert_eq!(f.built_age_secs, None);
        assert!(!f.dirty);
    }

    #[test]
    fn freshness_zero_row_index_is_not_never_built() {
        // Regression for the Codex finding: a successful rebuild that produced
        // zero similarity rows still stamps built_at, so freshness must read it
        // as built (Some age), not None — `read_freshness` never looks at
        // `track_similarity` rows at all.
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        set_config(&db, BUILT_AT_KEY, "2026-05-14 00:00:00");
        let f = db.with_conn(read_freshness).unwrap();
        assert!(
            f.built_age_secs.is_some(),
            "built_at must drive freshness, not row count"
        );
        assert!(!f.dirty);
    }

    #[test]
    fn freshness_dirty_when_library_changed_after_build() {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        set_config(&db, BUILT_AT_KEY, "2026-05-14 00:00:00");
        set_config(&db, LIBRARY_CHANGED_AT_KEY, "2026-05-14 01:00:00");
        let f = db.with_conn(read_freshness).unwrap();
        assert!(f.dirty);
    }

    #[test]
    fn freshness_not_dirty_when_build_is_newer() {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        set_config(&db, LIBRARY_CHANGED_AT_KEY, "2026-05-14 00:00:00");
        set_config(&db, BUILT_AT_KEY, "2026-05-14 02:00:00");
        let f = db.with_conn(read_freshness).unwrap();
        assert!(!f.dirty);
    }

    #[test]
    fn freshness_dirty_when_changed_but_never_built() {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        set_config(&db, LIBRARY_CHANGED_AT_KEY, "2026-05-14 00:00:00");
        let f = db.with_conn(read_freshness).unwrap();
        assert_eq!(f.built_age_secs, None);
        assert!(f.dirty);
    }
}
