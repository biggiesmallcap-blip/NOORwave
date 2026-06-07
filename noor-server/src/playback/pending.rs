//! Pending-row resolver lifecycle primitives.
//!
//! A "pending" queue row is one that was enqueued by an external producer
//! (Last.fm radio, automix-new, manual external add) before the TIDAL track
//! it points to was imported into the local library. The row carries
//! `pending_artist`, `pending_title`, optionally `tidal_id_hint`, and a NULL
//! `track_id`. Background resolvers (or a lazy fallback when the user reaches
//! the row) search TIDAL, import the result, then atomically promote the row
//! to point at the new library `track_id`.
//!
//! This module owns the SQL and the race-safety reasoning for that lifecycle:
//!
//! - [`try_claim`] sets `resolving_at = NOW()` iff `track_id` is still NULL and
//!   the row is unlocked or its lock is older than 30s. Only one fresh resolver
//!   can hold a row's lock at a time; competing resolvers see `claimed = false`
//!   and back off.
//! - [`release`] clears the lock when a resolver bails (no match, import
//!   failure, etc). The hourly GC also clears locks older than 30s in case a
//!   resolver crashed mid-claim, but playback does not have to wait for that GC
//!   pass before reclaiming a stale row.
//! - [`promote`] runs the atomic UPDATE `WHERE id = ? AND track_id IS NULL`
//!   that decides which resolver wins under concurrent attempts. Even if two
//!   resolvers both managed to import the same TIDAL track (race condition
//!   guarded separately by the `tracks.tidal_id` UNIQUE constraint), only one
//!   will win the queue-row promotion.
//! - [`read_identity`] / [`current_pending`] are SELECT helpers so callers
//!   don't need to know which columns spell "pending row".
//!
//! Callers (currently the resolver orchestrators in `server::routes`) reach
//! into this module instead of writing the SQL inline. That keeps the
//! schema-touching surface for `queue.pending_*` columns in one file.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::{models::AudioDjProfileKey, queries};

/// Try to acquire the resolution lock for a pending queue row. Returns
/// `Ok(true)` if this caller won the claim, `Ok(false)` if another resolver
/// already holds it (or the row has been resolved/deleted).
pub fn try_claim(conn: &Connection, queue_item_id: i64) -> Result<bool> {
    let updated = conn.execute(
        "UPDATE queue SET resolving_at = datetime('now')
         WHERE id = ?1
           AND track_id IS NULL
           AND (resolving_at IS NULL OR resolving_at < datetime('now', '-30 seconds'))",
        params![queue_item_id],
    )?;
    Ok(updated == 1)
}

/// Returns true when a pending queue row is still owned by a live resolver.
pub fn has_fresh_resolver_lock(conn: &Connection, queue_item_id: i64) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM queue
             WHERE id = ?1
               AND track_id IS NULL
               AND resolving_at IS NOT NULL
               AND resolving_at >= datetime('now', '-30 seconds')
         )",
        params![queue_item_id],
        |row| row.get(0),
    )?)
}

/// Best-effort lock release. Used by resolvers that bail before promoting
/// (no match, import error). Idempotent; if the row has been resolved in
/// the meantime, the UPDATE simply hits zero rows.
pub fn release(conn: &Connection, queue_item_id: i64) {
    let _ = conn.execute(
        "UPDATE queue SET resolving_at = NULL WHERE id = ?1",
        params![queue_item_id],
    );
}

/// Read pending identity (artist, title, tidal_id_hint) for a queue row that
/// is still unresolved. Returns `Ok(None)` if the row has been promoted or
/// doesn't exist.
pub fn read_identity(
    conn: &Connection,
    queue_item_id: i64,
) -> Result<Option<(String, String, Option<i64>)>> {
    Ok(conn
        .query_row(
            "SELECT pending_artist, pending_title, tidal_id_hint FROM queue
             WHERE id = ?1 AND track_id IS NULL AND pending_at IS NOT NULL",
            params![queue_item_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?)
}

/// Read the queue row that `playback_state.current_queue_item_id` points at,
/// but only if it's still pending. Returns `(queue_item_id, artist, title,
/// tidal_id_hint)` or `None` if there is no current row, or it has already
/// been resolved.
pub fn current_pending(conn: &Connection) -> Result<Option<(i64, String, String, Option<i64>)>> {
    Ok(conn
        .query_row(
            "SELECT q.id, q.pending_artist, q.pending_title, q.tidal_id_hint
             FROM playback_state ps
             JOIN queue q ON q.id = ps.current_queue_item_id
             WHERE ps.id = 1 AND q.track_id IS NULL AND q.pending_at IS NOT NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?)
}

/// Atomically promote a pending queue row to a resolved library row.
///
/// Returns `Ok(true)` iff this caller won the promotion race - the row's
/// `track_id` was NULL at the moment of the UPDATE. The `WHERE track_id IS
/// NULL` clause is the race-safety guarantee: even when two resolvers both
/// successfully import the same TIDAL track (independently caught by the
/// `tracks.tidal_id` UNIQUE constraint), only one of them will see this
/// UPDATE affect a row.
///
/// On success, also runs the external-candidate cleanup and promotes any
/// temporary queue-item DJ profile to the resolved TIDAL key inside the same
/// connection scope. Follow-up cleanup is best effort so it never masks a
/// successful queue-row promotion.
pub fn promote(
    conn: &Connection,
    queue_item_id: i64,
    local_track_id: i64,
    score_stored: i32,
) -> Result<bool> {
    // Read pending identity before the UPDATE. `pending_at IS NOT NULL` mirrors
    // the filter on read_identity / current_pending so the three SELECTs in this
    // module agree on what "still pending" means; an external write that left
    // pending_at NULL would skip the cleanup but the UPDATE still proceeds.
    let pending_identity: Option<(Option<i64>, String, String)> = conn
        .query_row(
            "SELECT tidal_id_hint, pending_title, pending_artist
             FROM queue
             WHERE id = ?1 AND track_id IS NULL AND pending_at IS NOT NULL",
            params![queue_item_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let promoted = conn.execute(
        "UPDATE queue
         SET track_id = ?1, resolved_at = datetime('now'),
             tidal_match_score = ?2, resolving_at = NULL
         WHERE id = ?3 AND track_id IS NULL",
        params![local_track_id, score_stored, queue_item_id],
    )? == 1;
    if promoted && let Some((tidal_id_hint, pending_title, pending_artist)) = pending_identity {
        // The row is already committed-promoted at this point. Any failure
        // reading the resolved tidal_id is best-effort: fall back to
        // tidal_id_hint so the external-candidate cleanup still runs, and
        // never let the followup read mask a successful promotion (the
        // caller's broadcast contract depends on promoted=true reaching it).
        let resolved_tidal_id = conn
            .query_row(
                "SELECT tidal_id FROM tracks WHERE id = ?1",
                params![local_track_id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .flatten()
            .or(tidal_id_hint);
        let _ = queries::mark_external_candidate_resolved(
            conn,
            resolved_tidal_id,
            &pending_title,
            &pending_artist,
            local_track_id,
        );
        if let Some(tidal_id) = resolved_tidal_id {
            let temporary_key = AudioDjProfileKey {
                media_ref_kind: "queue_item".to_string(),
                media_ref_id: queue_item_id.to_string(),
            };
            let stable_key = AudioDjProfileKey {
                media_ref_kind: "tidal_track".to_string(),
                media_ref_id: tidal_id.to_string(),
            };
            let _ = queries::promote_temporary_audio_dj_profile(
                conn,
                &temporary_key,
                &stable_key,
                Some(tidal_id),
            );
        }
    }
    Ok(promoted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{models::AudioDjProfileRow, schema};

    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("foreign keys");
        schema::run_migrations(&conn).expect("migrations");
        conn
    }

    fn seed_track(conn: &Connection, tidal_id: Option<i64>) -> i64 {
        conn.execute("INSERT INTO artists (name) VALUES ('Artist')", [])
            .expect("artist");
        let artist_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO tracks (title, artist_id, tidal_id) VALUES ('Track', ?1, ?2)",
            params![artist_id, tidal_id],
        )
        .expect("track");
        conn.last_insert_rowid()
    }

    fn seed_pending_queue_row(conn: &Connection, queue_item_id: i64) {
        conn.execute(
            "INSERT INTO queue (
                id, track_id, position, source, pending_artist, pending_title, pending_at
             )
             VALUES (?1, NULL, 0, 'radio_pending', 'Artist', 'Title', datetime('now'))",
            params![queue_item_id],
        )
        .expect("pending queue row");
    }

    fn set_resolving_at(conn: &Connection, queue_item_id: i64, value: &str) {
        conn.execute(
            "UPDATE queue SET resolving_at = datetime('now', ?1) WHERE id = ?2",
            params![value, queue_item_id],
        )
        .expect("set resolving_at");
    }

    fn profile_row(queue_item_id: i64) -> AudioDjProfileRow {
        AudioDjProfileRow {
            media_ref_kind: "queue_item".to_string(),
            media_ref_id: queue_item_id.to_string(),
            track_id: None,
            queue_item_id: Some(queue_item_id),
            tidal_id: None,
            profile_version: "dj_profile_v1".to_string(),
            beat_grid_blob: vec![1, 2, 3],
            downbeats_blob: vec![4],
            phrase_boundaries_blob: vec![5],
            mix_in_blob: vec![6],
            mix_out_blob: vec![7],
            intro_end_seconds: Some(16.0),
            outro_start_seconds: Some(180.0),
            breakdown_blob: vec![8],
            drop_blob: vec![9],
            safe_transition_windows_blob: vec![10],
            energy_contour_blob: vec![11],
            vocal_presence_blob: vec![12],
            vocal_density_blob: vec![13],
            waveform_peaks_blob: vec![14],
            lufs_loud_body: Some(-12.0),
            true_peak_dbtp: Some(-1.0),
            beat_confidence: Some(0.9),
            profile_confidence: 0.85,
            analysis_scope_ms: 90_000,
            is_temporary: true,
            source: "test".to_string(),
            computed_at: "2026-05-21T00:00:00Z".to_string(),
        }
    }

    fn key(kind: &str, id: &str) -> AudioDjProfileKey {
        AudioDjProfileKey {
            media_ref_kind: kind.to_string(),
            media_ref_id: id.to_string(),
        }
    }

    #[test]
    fn try_claim_rejects_fresh_resolver_lock() {
        let conn = setup_conn();
        seed_pending_queue_row(&conn, 40);
        set_resolving_at(&conn, 40, "-10 seconds");

        assert!(!try_claim(&conn, 40).expect("claim"));
    }

    #[test]
    fn try_claim_reclaims_stale_resolver_lock() {
        let conn = setup_conn();
        seed_pending_queue_row(&conn, 41);
        set_resolving_at(&conn, 41, "-31 seconds");

        assert!(try_claim(&conn, 41).expect("claim"));
        assert!(!try_claim(&conn, 41).expect("fresh second claim"));

        let is_fresh: bool = conn
            .query_row(
                "SELECT resolving_at >= datetime('now', '-30 seconds') FROM queue WHERE id = 41",
                [],
                |row| row.get(0),
            )
            .expect("fresh lock check");
        assert!(is_fresh);
    }

    #[test]
    fn try_claim_never_reclaims_resolved_row() {
        let conn = setup_conn();
        let track_id = seed_track(&conn, Some(541));
        seed_pending_queue_row(&conn, 42);
        conn.execute(
            "UPDATE queue
             SET track_id = ?1, resolving_at = datetime('now', '-31 seconds')
             WHERE id = 42",
            params![track_id],
        )
        .expect("resolve row");

        assert!(!try_claim(&conn, 42).expect("claim"));
    }

    #[test]
    fn has_fresh_resolver_lock_ignores_stale_and_resolved_rows() {
        let conn = setup_conn();
        let track_id = seed_track(&conn, Some(542));
        seed_pending_queue_row(&conn, 43);
        seed_pending_queue_row(&conn, 44);
        seed_pending_queue_row(&conn, 45);
        set_resolving_at(&conn, 43, "-10 seconds");
        set_resolving_at(&conn, 44, "-31 seconds");
        conn.execute(
            "UPDATE queue
             SET track_id = ?1, resolving_at = datetime('now', '-10 seconds')
             WHERE id = 45",
            params![track_id],
        )
        .expect("resolve row");

        assert!(has_fresh_resolver_lock(&conn, 43).expect("fresh lock"));
        assert!(!has_fresh_resolver_lock(&conn, 44).expect("stale lock"));
        assert!(!has_fresh_resolver_lock(&conn, 45).expect("resolved row"));
    }

    #[test]
    fn promote_copies_temporary_dj_profile_to_resolved_tidal_key() {
        let conn = setup_conn();
        let track_id = seed_track(&conn, Some(555));
        seed_pending_queue_row(&conn, 44);
        queries::upsert_audio_dj_profile(&conn, &profile_row(44)).expect("upsert temp profile");

        assert!(promote(&conn, 44, track_id, 100).expect("promote"));
        assert!(!promote(&conn, 44, track_id, 100).expect("second promote"));

        let stable = queries::get_audio_dj_profile(&conn, &key("tidal_track", "555"))
            .expect("stable lookup")
            .expect("stable profile");
        assert_eq!(stable.tidal_id, Some(555));
        assert!(!stable.is_temporary);
        assert_eq!(stable.profile_version, "dj_profile_v1");
    }

    #[test]
    fn promote_without_tidal_id_keeps_temporary_dj_profile_only() {
        let conn = setup_conn();
        let track_id = seed_track(&conn, None);
        seed_pending_queue_row(&conn, 45);
        queries::upsert_audio_dj_profile(&conn, &profile_row(45)).expect("upsert temp profile");

        assert!(promote(&conn, 45, track_id, 100).expect("promote"));

        let stable_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM audio_dj_profiles WHERE media_ref_kind <> 'queue_item'",
                [],
                |row| row.get(0),
            )
            .expect("stable count");
        assert_eq!(stable_count, 0);
    }
}
