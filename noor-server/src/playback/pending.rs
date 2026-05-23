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
//! - [`try_claim`] sets `resolving_at = NOW()` iff it's currently NULL and
//!   `track_id` is still NULL. Only one resolver can hold a row's lock at a
//!   time; competing resolvers see `claimed = false` and back off.
//! - [`release`] clears the lock when a resolver bails (no match, import
//!   failure, etc). The hourly GC also clears locks older than 30s in case a
//!   resolver crashed mid-claim.
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

use crate::db::queries;

/// Try to acquire the resolution lock for a pending queue row. Returns
/// `Ok(true)` if this caller won the claim, `Ok(false)` if another resolver
/// already holds it (or the row has been resolved/deleted).
pub fn try_claim(conn: &Connection, queue_item_id: i64) -> Result<bool> {
    let updated = conn.execute(
        "UPDATE queue SET resolving_at = datetime('now')
         WHERE id = ?1 AND resolving_at IS NULL AND track_id IS NULL",
        params![queue_item_id],
    )?;
    Ok(updated == 1)
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
pub fn current_pending(
    conn: &Connection,
) -> Result<Option<(i64, String, String, Option<i64>)>> {
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
/// On success, also runs the external-candidate cleanup
/// (`mark_external_candidate_resolved`) inside the same connection scope so
/// the prior pending identity is correctly attributed to the new library
/// track. The cleanup uses the resolved library track's `tidal_id` when it
/// has one, falling back to whatever `tidal_id_hint` the pending row carried.
pub fn promote(
    conn: &Connection,
    queue_item_id: i64,
    local_track_id: i64,
    score_stored: i32,
) -> Result<bool> {
    let pending_identity: Option<(Option<i64>, String, String)> = conn
        .query_row(
            "SELECT tidal_id_hint, pending_title, pending_artist
             FROM queue
             WHERE id = ?1 AND track_id IS NULL",
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
    if promoted
        && let Some((tidal_id_hint, pending_title, pending_artist)) = pending_identity
    {
        let resolved_tidal_id = conn
            .query_row(
                "SELECT tidal_id FROM tracks WHERE id = ?1",
                params![local_track_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten()
            .or(tidal_id_hint);
        let _ = queries::mark_external_candidate_resolved(
            conn,
            resolved_tidal_id,
            &pending_title,
            &pending_artist,
            local_track_id,
        );
    }
    Ok(promoted)
}
