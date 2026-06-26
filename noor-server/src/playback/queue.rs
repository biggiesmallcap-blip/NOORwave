use crate::db::models::{QueueItem, Track};
use crate::playback::shuffle::{
    WeightedShuffleProfile, artist_spread_shuffle_with_rng, genre_shuffle_with_rng, seeded_rng,
    true_shuffle_with_rng, weighted_shuffle_with_rng,
};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::collections::HashMap;

const TRACK_GENRE_CHUNK_SIZE: usize = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShuffleMode {
    Off,
    True,
    Weighted,
    Genre,
}

impl ShuffleMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::True => "true",
            Self::Weighted => "weighted",
            Self::Genre => "genre",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "true" => Self::True,
            "weighted" => Self::Weighted,
            "genre" => Self::Genre,
            _ => Self::Off,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ShuffleDebug {
    pub mode: String,
    pub seed: i64,
    pub scope: String,
    pub locked_count: usize,
    pub candidate_count: usize,
}

#[derive(Debug, Clone)]
pub struct ShuffleApplyResult {
    pub queue: Vec<QueueItem>,
    pub debug: Option<ShuffleDebug>,
}

pub fn load_queue(conn: &Connection) -> Result<Vec<QueueItem>> {
    let mut stmt = conn.prepare(
        // Phase 2c-ii-a: LEFT JOIN so pending rows (track_id IS NULL) appear.
        // COALESCE fills non-nullable Track fields from pending_* columns so the
        // row mapper doesn't need to know whether a row is pending or resolved.
        // Columns 0-3: queue metadata; 4-25: Track fields; 26: is_pending flag.
        // Ephemeral TIDAL rows (mix/album/playlist) are real rows with
        // track_id NULL whose source is in EPHEMERAL_TIDAL_SOURCES. They are NOT
        // pending (they stream directly), so is_pending excludes them and a
        // separate is_ephemeral flag tells the mapper to hydrate the synthetic
        // playable track from the ephemeral_* columns + tidal_id_hint instead of
        // the tracks join. The `lt` join recovers a local id + favourite state
        // when the TIDAL id already lives in the library.
        "SELECT q.id, q.position, q.source, q.reason,
                COALESCE(t.id, 0),
                COALESCE(t.title, q.pending_title, ''),
                COALESCE(t.artist_id, 0),
                COALESCE(a.name, q.pending_artist),
                t.album_id,
                al.title,
                t.disc_number,
                t.track_number,
                t.duration_ms,
                t.isrc,
                t.tidal_id,
                t.ytmusic_id,
                t.soundcloud_id,
                t.best_quality,
                t.best_source,
                COALESCE(t.fidelity_score, 0),
                COALESCE(t.is_favorite, 0),
                COALESCE(t.play_count, 0),
                t.last_played_at,
                t.date_added,
                COALESCE(t.source, 'tidal_stream'),
                al.artwork_url,
                (q.track_id IS NULL
                    AND q.source NOT IN ('tidal_mix','tidal_album','tidal_playlist')) AS is_pending,
                (q.track_id IS NULL
                    AND q.source IN ('tidal_mix','tidal_album','tidal_playlist')) AS is_ephemeral,
                q.tidal_id_hint,
                q.ephemeral_album_title,
                q.ephemeral_artwork_url,
                q.ephemeral_duration_ms,
                lt.id,
                lt.is_favorite
         FROM queue q
         LEFT JOIN tracks t ON q.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN tracks lt ON lt.tidal_id = q.tidal_id_hint
         ORDER BY q.position ASC, q.id ASC",
    )?;

    let items = stmt
        .query_map([], |row| {
            let is_ephemeral: bool = row.get(27)?;
            let track = if is_ephemeral {
                ephemeral_track_from_row(row)?
            } else {
                track_from_row_with_offset(row, 4)?
            };
            Ok(QueueItem {
                id: row.get(0)?,
                position: row.get(1)?,
                source: row.get(2)?,
                reason: row.get(3)?,
                track,
                is_pending: row.get(26)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(items)
}

pub fn queue_track_ids(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT track_id FROM queue WHERE track_id IS NOT NULL ORDER BY position ASC, id ASC",
    )?;
    let ids = stmt
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

pub fn append_tracks(conn: &Connection, tracks: &[Track], source: &str) -> Result<Vec<QueueItem>> {
    let with_reasons: Vec<(Track, Option<String>)> =
        tracks.iter().cloned().map(|track| (track, None)).collect();
    append_tracks_with_reasons(conn, &with_reasons, source)
}

/// Append tracks with per-row reason strings.
///
/// Radio and automix are the producers of reasons today; manual paths
/// pass `None` via [`append_tracks`]. The reason column stays queryable
/// but the frontend treats NULL as "no provenance recorded" and renders
/// no tooltip.
pub fn append_tracks_with_reasons(
    conn: &Connection,
    tracks: &[(Track, Option<String>)],
    source: &str,
) -> Result<Vec<QueueItem>> {
    if tracks.is_empty() {
        return load_queue(conn);
    }

    let start_pos: i32 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM queue",
        [],
        |row| row.get(0),
    )?;

    let tx = conn.unchecked_transaction()?;
    insert_tracks_with_reasons_at_position(&tx, tracks, source, start_pos)?;
    let queue = load_queue(&tx)?;
    tx.commit()?;
    Ok(queue)
}

fn insert_tracks_with_reasons_at_position(
    conn: &Connection,
    tracks: &[(Track, Option<String>)],
    source: &str,
    start_pos: i32,
) -> Result<()> {
    for (idx, (track, reason)) in tracks.iter().enumerate() {
        conn.execute(
            "INSERT INTO queue (track_id, position, source, reason) VALUES (?1, ?2, ?3, ?4)",
            params![track.id, start_pos + idx as i32, source, reason],
        )?;
    }
    Ok(())
}

/// A last.fm similar-track candidate that has not yet been resolved to a local Tidal track.
pub struct PendingCandidate {
    pub artist: String,
    pub title: String,
    pub reason: Option<String>,
}

/// Append non-library (pending) queue rows for last.fm candidates.
///
/// These rows have `track_id = NULL` and `pending_at` set. The background
/// resolver claims each row via `resolving_at`, performs a Tidal search, and
/// writes `track_id`, `resolved_at`, and `tidal_match_score` atomically.
/// At play time, if a row is still unresolved, the async caller performs a
/// lazy fallback search before calling `next_track()` again.
pub fn append_pending_tracks(
    conn: &Connection,
    candidates: &[PendingCandidate],
) -> Result<Vec<QueueItem>> {
    if candidates.is_empty() {
        return load_queue(conn);
    }

    let start_pos: i32 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM queue",
        [],
        |row| row.get(0),
    )?;

    let tx = conn.unchecked_transaction()?;
    insert_pending_tracks_at_position(&tx, candidates, start_pos)?;
    let queue = load_queue(&tx)?;
    tx.commit()?;
    Ok(queue)
}

fn insert_pending_tracks_at_position(
    conn: &Connection,
    candidates: &[PendingCandidate],
    start_pos: i32,
) -> Result<()> {
    for (idx, c) in candidates.iter().enumerate() {
        conn.execute(
            "INSERT INTO queue (track_id, position, source, reason, pending_artist, pending_title, pending_at)
             VALUES (NULL, ?1, 'radio_pending', ?2, ?3, ?4, datetime('now'))",
            params![start_pos + idx as i32, c.reason, c.artist, c.title],
        )?;
    }
    Ok(())
}

/// Description of a single track to insert into the queue from an external
/// source (search row, Last.fm radio candidate, Discover Space row). The two
/// "external" insert helpers below take this struct and dispatch to a library
/// row insert (when `local_track_id` is known) or a pending row insert
/// otherwise. `tidal_id_hint` is preserved on pending rows so the background
/// resolver can fetch by ID instead of searching by artist+title.
pub struct ExternalTrackInsert<'a> {
    pub artist: &'a str,
    pub title: &'a str,
    pub source: &'a str,
    pub reason: Option<&'a str>,
    pub tidal_id_hint: Option<i64>,
    pub local_track_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertResult {
    Library { queue_id: i64, track_id: i64 },
    Pending { queue_id: i64 },
}

impl InsertResult {
    #[cfg(test)]
    pub fn queue_id(self) -> i64 {
        match self {
            Self::Library { queue_id, .. } | Self::Pending { queue_id } => queue_id,
        }
    }
}

/// Insert an external track at the end of the queue.
///
/// Routes to a library-row insert when `local_track_id` is set, otherwise a
/// pending-row insert. Both paths keep position bookkeeping consistent — the
/// new row gets `MAX(position) + 1` so existing rows are never shifted.
pub fn append_external_track(
    conn: &Connection,
    insert: &ExternalTrackInsert<'_>,
) -> Result<InsertResult> {
    let position: i32 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM queue",
        [],
        |row| row.get(0),
    )?;
    insert_at_position(conn, insert, position)
}

/// Insert external tracks at the end of the queue with one position lookup.
pub fn append_external_tracks(
    conn: &Connection,
    inserts: &[ExternalTrackInsert<'_>],
) -> Result<Vec<InsertResult>> {
    if inserts.is_empty() {
        return Ok(Vec::new());
    }
    let start_position: i32 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM queue",
        [],
        |row| row.get(0),
    )?;
    let tx = conn.unchecked_transaction()?;
    let results = insert_many_at_position(&tx, inserts, start_position)?;
    tx.commit()?;
    Ok(results)
}

/// The lowest `position` currently in the queue, or `None` when the queue is
/// empty. Used by "Play next" during ephemeral-mix playback: the live track has
/// no queue row and the DB anchor is NULL, so "after current" has to mean "at
/// the front of the remaining continuation".
pub fn front_position(conn: &Connection) -> Result<Option<i32>> {
    Ok(
        conn.query_row("SELECT MIN(position) FROM queue", [], |row| {
            row.get::<_, Option<i32>>(0)
        })?,
    )
}

/// Insert an external track immediately after `after_position`. Existing rows
/// at that position or later are shifted by one. Used for "Play next" so the
/// new row lands at `current_position + 1`.
pub fn insert_external_track_after(
    conn: &Connection,
    insert: &ExternalTrackInsert<'_>,
    after_position: i32,
) -> Result<InsertResult> {
    let target = after_position + 1;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE queue SET position = position + 1 WHERE position >= ?1",
        params![target],
    )?;
    let result = insert_at_position(&tx, insert, target)?;
    tx.commit()?;
    Ok(result)
}

/// Insert external tracks immediately after `after_position`.
///
/// Existing rows are shifted once by the batch length, so large playlist
/// injections avoid repeatedly rewriting the same tail rows.
pub fn insert_external_tracks_after(
    conn: &Connection,
    inserts: &[ExternalTrackInsert<'_>],
    after_position: i32,
) -> Result<Vec<InsertResult>> {
    if inserts.is_empty() {
        return Ok(Vec::new());
    }
    let target = after_position + 1;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE queue SET position = position + ?1 WHERE position >= ?2",
        params![inserts.len() as i32, target],
    )?;
    let results = insert_many_at_position(&tx, inserts, target)?;
    tx.commit()?;
    Ok(results)
}

fn insert_many_at_position(
    conn: &Connection,
    inserts: &[ExternalTrackInsert<'_>],
    start_position: i32,
) -> Result<Vec<InsertResult>> {
    let mut results = Vec::with_capacity(inserts.len());
    for (idx, insert) in inserts.iter().enumerate() {
        results.push(insert_at_position(
            conn,
            insert,
            start_position + idx as i32,
        )?);
    }
    Ok(results)
}

fn insert_at_position(
    conn: &Connection,
    insert: &ExternalTrackInsert<'_>,
    position: i32,
) -> Result<InsertResult> {
    if let Some(track_id) = insert.local_track_id {
        conn.execute(
            "INSERT INTO queue (track_id, position, source, reason) VALUES (?1, ?2, ?3, ?4)",
            params![track_id, position, insert.source, insert.reason],
        )?;
        Ok(InsertResult::Library {
            queue_id: conn.last_insert_rowid(),
            track_id,
        })
    } else {
        conn.execute(
            "INSERT INTO queue (track_id, position, source, reason,
                                pending_artist, pending_title, pending_at, tidal_id_hint)
             VALUES (NULL, ?1, ?2, ?3, ?4, ?5, datetime('now'), ?6)",
            params![
                position,
                insert.source,
                insert.reason,
                insert.artist,
                insert.title,
                insert.tidal_id_hint,
            ],
        )?;
        Ok(InsertResult::Pending {
            queue_id: conn.last_insert_rowid(),
        })
    }
}

/// Queue `source` labels whose rows are ephemeral TIDAL collection tracks:
/// real, mutable queue rows that stream directly via `tidal_id` and are never
/// imported into the library. Distinct from Last.fm pending rows. Kept in sync
/// with the `IN (...)` literals in `load_queue` and the resolver/GC guards.
pub const EPHEMERAL_TIDAL_SOURCES: [&str; 3] = ["tidal_mix", "tidal_album", "tidal_playlist"];

/// A TIDAL track to enqueue as an ephemeral (never-imported) queue row.
pub struct EphemeralTidalInsert<'a> {
    pub tidal_id: i64,
    pub title: &'a str,
    pub artist: Option<&'a str>,
    pub album_title: Option<&'a str>,
    pub artwork_url: Option<&'a str>,
    pub duration_ms: Option<i64>,
}

/// Append ephemeral TIDAL rows at the end of the queue with one position lookup.
/// `source` must be one of [`EPHEMERAL_TIDAL_SOURCES`].
pub fn append_ephemeral_tidal_tracks(
    conn: &Connection,
    inserts: &[EphemeralTidalInsert<'_>],
    source: &str,
) -> Result<Vec<QueueItem>> {
    if inserts.is_empty() {
        return load_queue(conn);
    }
    let start_position: i32 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM queue",
        [],
        |row| row.get(0),
    )?;
    let tx = conn.unchecked_transaction()?;
    for (idx, insert) in inserts.iter().enumerate() {
        insert_ephemeral_at_position(&tx, insert, source, start_position + idx as i32)?;
    }
    let queue = load_queue(&tx)?;
    tx.commit()?;
    Ok(queue)
}

fn insert_ephemeral_at_position(
    conn: &Connection,
    insert: &EphemeralTidalInsert<'_>,
    source: &str,
    position: i32,
) -> Result<()> {
    // track_id NULL + no pending_at: these rows are never resolved or GC-swept.
    conn.execute(
        "INSERT INTO queue (track_id, position, source,
                            pending_artist, pending_title, tidal_id_hint,
                            ephemeral_album_title, ephemeral_artwork_url, ephemeral_duration_ms)
         VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            position,
            source,
            insert.artist,
            insert.title,
            insert.tidal_id,
            insert.album_title,
            insert.artwork_url,
            insert.duration_ms,
        ],
    )?;
    Ok(())
}

fn ephemeral_pending_from_row(
    row: &Row<'_>,
    id_offset: usize,
) -> rusqlite::Result<crate::PendingEphemeralTidalTrack> {
    Ok(crate::PendingEphemeralTidalTrack {
        tidal_track_id: row.get(id_offset)?,
        title: row
            .get::<_, Option<String>>(id_offset + 1)?
            .unwrap_or_default(),
        artist_name: row.get(id_offset + 2)?,
        album_title: row.get(id_offset + 3)?,
        artwork_url: row.get(id_offset + 4)?,
        duration_ms: row.get(id_offset + 5)?,
    })
}

const EPHEMERAL_TIDAL_ROW_FILTER: &str = "track_id IS NULL
       AND source IN ('tidal_mix','tidal_album','tidal_playlist')
       AND tidal_id_hint IS NOT NULL";

/// Read the next upcoming ephemeral TIDAL row (lowest position) without removing
/// it. Used by the DJ pre-buffer peek and transition-pair builder.
pub fn peek_next_ephemeral_tidal_track(
    conn: &Connection,
) -> Result<Option<crate::PendingEphemeralTidalTrack>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT tidal_id_hint, pending_title, pending_artist,
                        ephemeral_album_title, ephemeral_artwork_url, ephemeral_duration_ms
                 FROM queue WHERE {EPHEMERAL_TIDAL_ROW_FILTER}
                 ORDER BY position ASC, id ASC LIMIT 1"
            ),
            [],
            |row| ephemeral_pending_from_row(row, 0),
        )
        .optional()?)
}

/// Read all upcoming ephemeral TIDAL rows in play order without removing them.
/// Used to pre-warm DJ transition profiles for the rest of the mix.
pub fn peek_ephemeral_tidal_tracks(
    conn: &Connection,
) -> Result<Vec<crate::PendingEphemeralTidalTrack>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT tidal_id_hint, pending_title, pending_artist,
                ephemeral_album_title, ephemeral_artwork_url, ephemeral_duration_ms
         FROM queue WHERE {EPHEMERAL_TIDAL_ROW_FILTER}
         ORDER BY position ASC, id ASC"
    ))?;
    let rows = stmt
        .query_map([], |row| ephemeral_pending_from_row(row, 0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Read-and-delete the next upcoming ephemeral TIDAL row (lowest position).
/// Mirrors the `VecDeque::pop_front` the old in-memory mix queue used.
pub fn pop_next_ephemeral_tidal_track(
    conn: &Connection,
) -> Result<Option<crate::PendingEphemeralTidalTrack>> {
    let tx = conn.unchecked_transaction()?;
    let row: Option<(i64, crate::PendingEphemeralTidalTrack)> = tx
        .query_row(
            &format!(
                "SELECT id, tidal_id_hint, pending_title, pending_artist,
                        ephemeral_album_title, ephemeral_artwork_url, ephemeral_duration_ms
                 FROM queue WHERE {EPHEMERAL_TIDAL_ROW_FILTER}
                 ORDER BY position ASC, id ASC LIMIT 1"
            ),
            [],
            |row| Ok((row.get(0)?, ephemeral_pending_from_row(row, 1)?)),
        )
        .optional()?;
    let Some((queue_id, track)) = row else {
        return Ok(None);
    };
    tx.execute("DELETE FROM queue WHERE id = ?1", params![queue_id])?;
    tx.commit()?;
    Ok(Some(track))
}

/// Find an upcoming ephemeral TIDAL row by its TIDAL id. Used to rebuild a
/// synthetic track for DJ profile lookups when a mix track is still queued.
pub fn find_ephemeral_tidal_track_by_tidal_id(
    conn: &Connection,
    tidal_id: i64,
) -> Result<Option<crate::PendingEphemeralTidalTrack>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT tidal_id_hint, pending_title, pending_artist,
                        ephemeral_album_title, ephemeral_artwork_url, ephemeral_duration_ms
                 FROM queue WHERE {EPHEMERAL_TIDAL_ROW_FILTER} AND tidal_id_hint = ?1
                 ORDER BY position ASC, id ASC LIMIT 1"
            ),
            params![tidal_id],
            |row| ephemeral_pending_from_row(row, 0),
        )
        .optional()?)
}

/// Delete every ephemeral TIDAL row. Used on stop and when the user starts a
/// track outside the current mix.
pub fn delete_all_ephemeral_tidal_rows(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM queue
         WHERE track_id IS NULL
           AND source IN ('tidal_mix','tidal_album','tidal_playlist')",
        [],
    )?)
}

/// "Jump to" trim: delete every ephemeral row up to and including the one whose
/// TIDAL id matches `tidal_id` (which the caller is about to start playing).
/// Returns true if that row was present; when absent (the user picked something
/// outside the mix) the caller clears all ephemeral rows instead.
pub fn trim_ephemeral_tidal_rows_through_tidal_id(
    conn: &Connection,
    tidal_id: i64,
) -> Result<bool> {
    let pos: Option<i32> = conn
        .query_row(
            &format!(
                "SELECT position FROM queue
                 WHERE {EPHEMERAL_TIDAL_ROW_FILTER} AND tidal_id_hint = ?1
                 ORDER BY position ASC, id ASC LIMIT 1"
            ),
            params![tidal_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(pos) = pos else {
        return Ok(false);
    };
    conn.execute(
        "DELETE FROM queue
         WHERE track_id IS NULL
           AND source IN ('tidal_mix','tidal_album','tidal_playlist')
           AND position <= ?1",
        params![pos],
    )?;
    Ok(true)
}

pub fn replace_queue(conn: &Connection, tracks: &[Track], source: &str) -> Result<Vec<QueueItem>> {
    let with_reasons: Vec<(Track, Option<String>)> =
        tracks.iter().cloned().map(|track| (track, None)).collect();
    replace_queue_with_reasons(conn, &with_reasons, source)
}

/// Wipe the queue and replace with tracks plus per-row reasons.
pub fn replace_queue_with_reasons(
    conn: &Connection,
    tracks: &[(Track, Option<String>)],
    source: &str,
) -> Result<Vec<QueueItem>> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM queue", [])?;
    insert_tracks_with_reasons_at_position(&tx, tracks, source, 0)?;
    let queue = load_queue(&tx)?;
    tx.commit()?;
    Ok(queue)
}

pub fn clear_queue(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM queue", [])?;
    Ok(())
}

pub fn remove_queue_item(conn: &Connection, item_id: i64) -> Result<()> {
    conn.execute("DELETE FROM queue WHERE id = ?1", params![item_id])?;
    normalize_positions(conn)?;
    Ok(())
}

/// Move the queue row identified by `item_id` to logical index `new_pos`
/// (0-based, after the move). Out-of-range targets are clamped to the
/// current queue length. After the move all positions are renormalised
/// so they are contiguous and 0-based.
pub fn move_queue_item(conn: &Connection, item_id: i64, new_pos: i32) -> Result<()> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM queue ORDER BY position ASC, id ASC")?;
        stmt.query_map([], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    if ids.is_empty() {
        return Ok(());
    }
    let from = match ids.iter().position(|&id| id == item_id) {
        Some(idx) => idx,
        None => return Ok(()),
    };
    let mut reordered = ids.clone();
    let id = reordered.remove(from);
    let target = (new_pos.max(0) as usize).min(reordered.len());
    reordered.insert(target, id);

    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare("UPDATE queue SET position = ?1 WHERE id = ?2")?;
        for (idx, qid) in reordered.iter().enumerate() {
            stmt.execute(params![idx as i32, qid])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn apply_shuffle_with_seed(
    conn: &Connection,
    mode: ShuffleMode,
    current_queue_item_id: Option<i64>,
    seed: i64,
    scope: &str,
) -> Result<ShuffleApplyResult> {
    let queue_items = load_queue(conn)?;
    let split_index = current_queue_item_id
        .and_then(|qid| queue_items.iter().position(|item| item.id == qid))
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let candidate_count = queue_items.len().saturating_sub(split_index);
    let debug = (mode != ShuffleMode::Off).then(|| ShuffleDebug {
        mode: mode.as_str().to_string(),
        seed,
        scope: scope.to_string(),
        locked_count: split_index,
        candidate_count,
    });

    if queue_items.len() <= 1 || mode == ShuffleMode::Off || candidate_count <= 1 {
        return Ok(ShuffleApplyResult {
            queue: queue_items,
            debug,
        });
    }

    let locked_qids: Vec<i64> = queue_items[..split_index].iter().map(|i| i.id).collect();
    let shuffled_qids =
        reorder_queue_item_ids_with_seed(conn, &queue_items[split_index..], mode, seed, scope)?;

    let final_qids: Vec<i64> = locked_qids.into_iter().chain(shuffled_qids).collect();

    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare("UPDATE queue SET position = ?1 WHERE id = ?2")?;
        for (idx, qid) in final_qids.iter().enumerate() {
            stmt.execute(params![idx as i32, qid])?;
        }
    }
    tx.commit()?;

    Ok(ShuffleApplyResult {
        queue: load_queue(conn)?,
        debug,
    })
}

pub(crate) fn reorder_tracks_with_seed(
    conn: &Connection,
    tracks: &[Track],
    mode: ShuffleMode,
    seed: i64,
    scope: &str,
) -> Result<Vec<Track>> {
    let mut rng = seeded_rng(seed, mode.as_str(), scope);
    match mode {
        ShuffleMode::Off => Ok(tracks.to_vec()),
        ShuffleMode::True => Ok(true_shuffle_with_rng(tracks, &mut rng)),
        ShuffleMode::Weighted => {
            let weighted =
                weighted_shuffle_with_rng(tracks, &WeightedShuffleProfile::default(), &mut rng);
            Ok(artist_spread_shuffle_with_rng(&weighted, &mut rng))
        }
        ShuffleMode::Genre => {
            let genre_map = get_track_genres(conn, tracks)?;
            Ok(genre_shuffle_with_rng(tracks, &genre_map, &mut rng))
        }
    }
}

fn reorder_queue_item_ids_with_seed(
    conn: &Connection,
    items: &[QueueItem],
    mode: ShuffleMode,
    seed: i64,
    scope: &str,
) -> Result<Vec<i64>> {
    let mut surrogate_tracks = Vec::with_capacity(items.len());
    for item in items {
        let mut track = item.track.clone();
        track.id = item.id;
        surrogate_tracks.push(track);
    }

    let mut genre_map = HashMap::new();
    if mode == ShuffleMode::Genre {
        let real_tracks = items
            .iter()
            .filter(|item| item.track.id > 0)
            .map(|item| item.track.clone())
            .collect::<Vec<_>>();
        let real_genres = get_track_genres(conn, &real_tracks)?;
        for item in items {
            if let Some(genres) = real_genres.get(&item.track.id) {
                genre_map.insert(item.id, genres.clone());
            }
        }
    }

    let mut rng = seeded_rng(seed, mode.as_str(), scope);
    let reordered = match mode {
        ShuffleMode::Off => surrogate_tracks,
        ShuffleMode::True => true_shuffle_with_rng(&surrogate_tracks, &mut rng),
        ShuffleMode::Weighted => {
            let weighted = weighted_shuffle_with_rng(
                &surrogate_tracks,
                &WeightedShuffleProfile::default(),
                &mut rng,
            );
            artist_spread_shuffle_with_rng(&weighted, &mut rng)
        }
        ShuffleMode::Genre => genre_shuffle_with_rng(&surrogate_tracks, &genre_map, &mut rng),
    };

    Ok(reordered.into_iter().map(|track| track.id).collect())
}

pub fn get_track_genres(conn: &Connection, tracks: &[Track]) -> Result<HashMap<i64, Vec<String>>> {
    let track_ids = tracks.iter().map(|track| track.id).collect::<Vec<_>>();
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }

    // Pre-compute genre paths once instead of recursive CTE per chunk.
    let genre_paths: HashMap<i64, String> = {
        let mut stmt = conn.prepare(
            "WITH RECURSIVE paths(id, parent_id, path) AS (
                SELECT id, parent_id, name FROM genres WHERE parent_id IS NULL
                UNION ALL
                SELECT g.id, g.parent_id, paths.path || ' > ' || g.name
                FROM genres g JOIN paths ON g.parent_id = paths.id
            )
            SELECT id, path FROM paths",
        )?;
        let mut map = HashMap::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            map.insert(id, path);
        }
        map
    };

    let mut by_track = HashMap::new();
    for chunk in track_ids.chunks(TRACK_GENRE_CHUNK_SIZE) {
        let placeholders = (1..=chunk.len())
            .map(|i| format!("?{}", i))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT track_id, genre_id FROM track_genres WHERE track_id IN ({})",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(chunk.iter());
        let mut rows = stmt.query(params)?;
        while let Some(row) = rows.next()? {
            let track_id: i64 = row.get(0)?;
            let genre_id: i64 = row.get(1)?;
            if let Some(path) = genre_paths.get(&genre_id) {
                by_track
                    .entry(track_id)
                    .or_insert_with(Vec::new)
                    .push(path.clone());
            }
        }
    }

    Ok(by_track)
}

pub fn get_track_by_id(conn: &Connection, track_id: i64) -> Result<Option<Track>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, t.artist_id, a.name, t.album_id, al.title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.id = ?1",
    )?;

    let track = stmt
        .query_row(params![track_id], |row| track_from_row_with_offset(row, 0))
        .optional()?;
    Ok(track)
}

fn normalize_positions(conn: &Connection) -> Result<()> {
    // Single batched UPDATE using a temporary table to avoid N individual statements.
    // We rebuild positions sequentially: each row gets the next available position
    // ordered by the current position/id.
    conn.execute_batch("
        CREATE TEMPORARY TABLE IF NOT EXISTS _queue_reorder (id INTEGER PRIMARY KEY, new_pos INTEGER);
        DELETE FROM _queue_reorder;
    ")?;

    let ids: Vec<(i64, i32)> = {
        let mut stmt = conn.prepare("SELECT id FROM queue ORDER BY position ASC, id ASC")?;
        stmt.query_map([], |row| row.get::<_, i64>(0))?
            .enumerate()
            .map(|(i, id_r)| Ok((id_r?, i as i32)))
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    if ids.is_empty() {
        return Ok(());
    }

    // Insert all new positions in one transaction.
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare("INSERT INTO _queue_reorder (id, new_pos) VALUES (?1, ?2)")?;
        for &(id, pos) in &ids {
            stmt.execute(params![id, pos])?;
        }
    }
    tx.execute("UPDATE queue SET position = (SELECT new_pos FROM _queue_reorder WHERE _queue_reorder.id = queue.id) WHERE id IN (SELECT id FROM _queue_reorder)", [])?;
    tx.execute("DELETE FROM _queue_reorder", [])?;
    tx.commit()?;
    Ok(())
}

/// Fetch multiple tracks by their IDs in a single query.
/// Returns only the tracks that were found, in the order of the input IDs.
pub fn get_tracks_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<Track>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = (1..=ids.len())
        .map(|i| format!("?{}", i))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT t.id, t.title, t.artist_id, a.name, t.album_id, al.title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.id IN ({})",
        placeholders
    );
    let mut stmt = conn.prepare(&sql)?;
    let params = rusqlite::params_from_iter(ids.iter());
    let tracks = stmt
        .query_map(params, |row| track_from_row_with_offset(row, 0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(tracks)
}

fn track_from_row_with_offset(row: &Row<'_>, offset: usize) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(offset)?,
        title: row.get(offset + 1)?,
        artist_id: row.get(offset + 2)?,
        artist_name: row.get(offset + 3)?,
        album_id: row.get(offset + 4)?,
        album_title: row.get(offset + 5)?,
        disc_number: row.get(offset + 6)?,
        track_number: row.get(offset + 7)?,
        duration_ms: row.get(offset + 8)?,
        isrc: row.get(offset + 9)?,
        tidal_id: row.get(offset + 10)?,
        ytmusic_id: row.get(offset + 11)?,
        soundcloud_id: row.get(offset + 12)?,
        best_quality: row.get(offset + 13)?,
        best_source: row.get(offset + 14)?,
        fidelity_score: row.get(offset + 15)?,
        is_favorite: row.get(offset + 16)?,
        play_count: row.get(offset + 17)?,
        last_played_at: row.get(offset + 18)?,
        date_added: row.get(offset + 19)?,
        source: row.get(offset + 20)?,
        artwork_url: row.get(offset + 21)?,
    })
}

/// Hydrate a synthetic playable `Track` for an ephemeral TIDAL queue row.
///
/// Mirrors the shape the old in-memory overlay built: negative `id` derived from
/// the TIDAL id (or the local id when the track already lives in the library),
/// `source = tidal_ephemeral`, metadata pulled from the queue row's pending_* /
/// ephemeral_* columns. Column indices match the extended `load_queue` SELECT.
fn ephemeral_track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
    let tidal_id: i64 = row.get(28)?;
    let local_id: Option<i64> = row.get(32)?;
    let library_favorite: Option<bool> = row.get(33)?;
    Ok(Track {
        id: local_id.unwrap_or(-tidal_id),
        title: row.get(5)?, // COALESCE(t.title, q.pending_title, '')
        artist_id: 0,
        artist_name: row.get(7)?, // COALESCE(a.name, q.pending_artist)
        album_id: None,
        album_title: row.get(29)?,
        disc_number: None,
        track_number: None,
        duration_ms: row.get(31)?,
        isrc: None,
        tidal_id: Some(tidal_id),
        ytmusic_id: None,
        soundcloud_id: None,
        best_quality: Some("LOSSLESS".to_string()),
        best_source: Some("tidal".to_string()),
        fidelity_score: 0,
        is_favorite: library_favorite.unwrap_or(false),
        play_count: 0,
        last_played_at: None,
        date_added: None,
        source: "tidal_ephemeral".to_string(),
        artwork_url: row.get(30)?,
    })
}

/// Remove stale pending rows and clear orphaned resolver locks.
///
/// Two sweeps:
///   1. Delete rows where `track_id IS NULL AND pending_at < datetime('now', '-6 hours')`.
///      These are unresolvable: either Tidal has no match or the row was abandoned.
///   2. Clear `resolving_at` on rows where `resolving_at < datetime('now', '-30 seconds')
///      AND track_id IS NULL` — the resolver crashed or timed out; returning to NULL lets the
///      lazy path reclaim the row.
///
/// Returns `(expired_deleted, locks_cleared)`.
pub fn gc_pending_queue(conn: &Connection) -> Result<(usize, usize)> {
    let expired = conn.execute(
        "DELETE FROM queue WHERE track_id IS NULL AND pending_at < datetime('now', '-6 hours')",
        [],
    )?;
    let locks = conn.execute(
        "UPDATE queue SET resolving_at = NULL
         WHERE track_id IS NULL AND resolving_at < datetime('now', '-30 seconds')",
        [],
    )?;
    Ok((expired, locks))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT);
            CREATE TABLE albums (id INTEGER PRIMARY KEY, title TEXT, artwork_url TEXT);
            CREATE TABLE tracks (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                artist_id INTEGER NOT NULL,
                album_id INTEGER,
                disc_number INTEGER,
                track_number INTEGER,
                duration_ms INTEGER,
                isrc TEXT,
                tidal_id INTEGER,
                ytmusic_id TEXT,
                soundcloud_id INTEGER,
                best_quality TEXT,
                best_source TEXT,
                fidelity_score INTEGER DEFAULT 0,
                is_favorite INTEGER DEFAULT 0,
                play_count INTEGER DEFAULT 0,
                last_played_at TEXT,
                date_added TEXT,
                source TEXT DEFAULT 'tidal'
            );
            CREATE TABLE queue (
                id               INTEGER PRIMARY KEY,
                track_id         INTEGER,
                position         INTEGER NOT NULL,
                source           TEXT    DEFAULT 'user',
                reason           TEXT,
                pending_artist   TEXT,
                pending_title    TEXT,
                pending_at       TIMESTAMP,
                resolving_at     TIMESTAMP,
                resolved_at      TIMESTAMP,
                tidal_match_score REAL,
                tidal_id_hint    INTEGER,
                ephemeral_album_title TEXT,
                ephemeral_artwork_url TEXT,
                ephemeral_duration_ms INTEGER
            );
            CREATE TABLE genres (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                parent_id INTEGER
            );
            CREATE TABLE track_genres (
                track_id INTEGER NOT NULL,
                genre_id INTEGER NOT NULL,
                source TEXT,
                confidence REAL DEFAULT 1.0
            );
            ",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'A'), (2, 'B'), (3, 'C')",
            [],
        )
        .unwrap();
        for id in 1..=4 {
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, album_id, disc_number, track_number, duration_ms, isrc,
                    tidal_id, ytmusic_id, soundcloud_id, best_quality, best_source, fidelity_score,
                    is_favorite, play_count, last_played_at, date_added, source
                ) VALUES (?1, ?2, ?3, NULL, 1, ?1, 1000, NULL, ?1, NULL, NULL, 'LOSSLESS', 'tidal', 10, 0, 0, NULL, '2025-01-01', 'tidal')",
                params![id, format!("Track {id}"), ((id - 1) % 3) + 1],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn replace_and_load_queue_round_trip() {
        let conn = conn();
        let tracks = vec![
            get_track_by_id(&conn, 1).unwrap().unwrap(),
            get_track_by_id(&conn, 2).unwrap().unwrap(),
        ];

        replace_queue(&conn, &tracks, "test").unwrap();
        let queue = load_queue(&conn).unwrap();

        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].track.id, 1);
        assert_eq!(queue[1].track.id, 2);
    }

    #[test]
    fn front_position_and_play_next_front_insert_during_mix() {
        let conn = conn();
        // Simulate a TIDAL mix continuation: ephemeral rows, no library/anchor.
        for (pos, tidal) in [(0, 101_i64), (1, 102), (2, 103)] {
            conn.execute(
                "INSERT INTO queue (track_id, position, source, tidal_id_hint)
                 VALUES (NULL, ?1, 'tidal_mix', ?2)",
                params![pos, tidal],
            )
            .unwrap();
        }
        assert_eq!(front_position(&conn).unwrap(), Some(0));

        // "Play next" during a mix inserts at front_position - 1, so the new row
        // becomes the very next track instead of the bottom of the continuation.
        let insert = ExternalTrackInsert {
            artist: "New Artist",
            title: "Play Next Pick",
            source: "user_play_next",
            reason: None,
            tidal_id_hint: Some(999),
            local_track_id: None,
        };
        let front = front_position(&conn).unwrap().unwrap();
        insert_external_track_after(&conn, &insert, front - 1).unwrap();

        let new_pos: i32 = conn
            .query_row(
                "SELECT position FROM queue WHERE tidal_id_hint = 999",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(new_pos, 0, "play-next row should land at the front");
        let first_mix_pos: i32 = conn
            .query_row(
                "SELECT position FROM queue WHERE tidal_id_hint = 101",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(first_mix_pos, 1, "mix rows shift down by one");
        assert_eq!(load_queue(&conn).unwrap().len(), 4);
    }

    #[test]
    fn front_position_is_none_for_empty_queue() {
        let conn = conn();
        assert_eq!(front_position(&conn).unwrap(), None);
    }

    #[test]
    fn replace_queue_handles_large_playlist_in_order() {
        let conn = conn();
        for id in 5..=1000 {
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, album_id, disc_number, track_number, duration_ms, isrc,
                    tidal_id, ytmusic_id, soundcloud_id, best_quality, best_source, fidelity_score,
                    is_favorite, play_count, last_played_at, date_added, source
                ) VALUES (?1, ?2, 1, NULL, 1, ?1, 1000, NULL, ?1, NULL, NULL, 'LOSSLESS', 'tidal', 10, 0, 0, NULL, '2025-01-01', 'tidal')",
                params![id, format!("Track {id}")],
            )
            .unwrap();
        }
        let tracks = (1..=1000)
            .map(|id| get_track_by_id(&conn, id).unwrap().unwrap())
            .collect::<Vec<_>>();

        let queue = replace_queue(&conn, &tracks, "test").unwrap();

        assert_eq!(queue.len(), 1000);
        assert_eq!(queue.first().map(|item| item.track.id), Some(1));
        assert_eq!(queue.last().map(|item| item.track.id), Some(1000));
        for (idx, item) in queue.iter().enumerate() {
            assert_eq!(item.position, idx as i32);
            assert_eq!(item.track.id, (idx + 1) as i64);
        }
    }

    #[test]
    fn move_queue_item_reorders_within_queue() {
        let conn = conn();
        let tracks = vec![
            get_track_by_id(&conn, 1).unwrap().unwrap(),
            get_track_by_id(&conn, 2).unwrap().unwrap(),
            get_track_by_id(&conn, 3).unwrap().unwrap(),
            get_track_by_id(&conn, 4).unwrap().unwrap(),
        ];

        replace_queue(&conn, &tracks, "test").unwrap();
        let queue = load_queue(&conn).unwrap();

        // Move the third item (track 3) to position 0.
        let third = queue[2].id;
        move_queue_item(&conn, third, 0).unwrap();
        let queue = load_queue(&conn).unwrap();
        assert_eq!(queue[0].track.id, 3);
        assert_eq!(queue[1].track.id, 1);
        assert_eq!(queue[2].track.id, 2);
        assert_eq!(queue[3].track.id, 4);

        // Out-of-range new_pos clamps to the end.
        move_queue_item(&conn, queue[0].id, 999).unwrap();
        let queue = load_queue(&conn).unwrap();
        assert_eq!(queue[3].track.id, 3);
        // Positions stay contiguous starting at 0.
        for (idx, item) in queue.iter().enumerate() {
            assert_eq!(item.position, idx as i32);
        }
    }

    #[test]
    fn remove_queue_item_reorders_positions() {
        let conn = conn();
        let tracks = vec![
            get_track_by_id(&conn, 1).unwrap().unwrap(),
            get_track_by_id(&conn, 2).unwrap().unwrap(),
            get_track_by_id(&conn, 3).unwrap().unwrap(),
        ];

        replace_queue(&conn, &tracks, "test").unwrap();
        let queue = load_queue(&conn).unwrap();
        remove_queue_item(&conn, queue[1].id).unwrap();
        let queue = load_queue(&conn).unwrap();

        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].position, 0);
        assert_eq!(queue[1].position, 1);
    }

    #[test]
    fn get_track_genres_handles_large_track_sets() {
        let conn = conn();
        conn.execute(
            "INSERT INTO genres (id, name, slug, parent_id) VALUES (1, 'Electronic', 'electronic', NULL)",
            [],
        )
        .unwrap();

        for id in 5..=1_050 {
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, album_id, disc_number, track_number, duration_ms, isrc,
                    tidal_id, ytmusic_id, soundcloud_id, best_quality, best_source, fidelity_score,
                    is_favorite, play_count, last_played_at, date_added, source
                ) VALUES (?1, ?2, 1, NULL, 1, ?1, 1000, NULL, ?1, NULL, NULL, 'LOSSLESS', 'tidal', 10, 0, 0, NULL, '2025-01-01', 'tidal')",
                params![id, format!("Track {id}")],
            )
            .unwrap();
        }

        let mut tracks = Vec::new();
        for id in 1..=1_050 {
            conn.execute(
                "INSERT INTO track_genres (track_id, genre_id, source, confidence) VALUES (?1, 1, 'musicbrainz', 1.0)",
                params![id],
            )
            .unwrap();
            tracks.push(get_track_by_id(&conn, id).unwrap().unwrap());
        }

        let genres = get_track_genres(&conn, &tracks).unwrap();

        assert_eq!(genres.len(), 1_050);
        assert_eq!(
            genres
                .get(&1)
                .and_then(|paths| paths.first())
                .map(String::as_str),
            Some("Electronic")
        );
        assert_eq!(
            genres
                .get(&1_050)
                .and_then(|paths| paths.first())
                .map(String::as_str),
            Some("Electronic")
        );
    }

    #[test]
    fn append_external_track_pending_creates_resolvable_row() {
        let conn = conn();
        let result = append_external_track(
            &conn,
            &ExternalTrackInsert {
                artist: "Aphex Twin",
                title: "Xtal",
                source: "user_queue",
                reason: None,
                tidal_id_hint: Some(123),
                local_track_id: None,
            },
        )
        .unwrap();
        assert!(matches!(result, InsertResult::Pending { .. }));

        let rows = load_queue(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].track.title, "Xtal");
        assert_eq!(rows[0].track.artist_name.as_deref(), Some("Aphex Twin"));
        assert!(rows[0].is_pending);
        assert_eq!(rows[0].source, "user_queue");

        let hint: Option<i64> = conn
            .query_row(
                "SELECT tidal_id_hint FROM queue WHERE id = ?1",
                params![result.queue_id()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hint, Some(123));
    }

    #[test]
    fn append_external_track_library_creates_normal_row() {
        let conn = conn();
        let result = append_external_track(
            &conn,
            &ExternalTrackInsert {
                artist: "ignored",
                title: "ignored",
                source: "user_queue",
                reason: Some("seeded"),
                tidal_id_hint: None,
                local_track_id: Some(1),
            },
        )
        .unwrap();
        assert_eq!(
            result,
            InsertResult::Library {
                queue_id: result.queue_id(),
                track_id: 1,
            }
        );

        let rows = load_queue(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].track.id, 1);
        assert_eq!(rows[0].track.title, "Track 1");
        assert!(!rows[0].is_pending);
        assert_eq!(rows[0].reason.as_deref(), Some("seeded"));
    }

    #[test]
    fn append_external_tracks_preserves_batch_order() {
        let conn = conn();
        let inserts = vec![
            ExternalTrackInsert {
                artist: "ignored",
                title: "ignored",
                source: "user_queue",
                reason: None,
                tidal_id_hint: None,
                local_track_id: Some(1),
            },
            ExternalTrackInsert {
                artist: "Aphex Twin",
                title: "Xtal",
                source: "user_queue",
                reason: Some("external"),
                tidal_id_hint: Some(123),
                local_track_id: None,
            },
            ExternalTrackInsert {
                artist: "ignored",
                title: "ignored",
                source: "user_queue",
                reason: None,
                tidal_id_hint: None,
                local_track_id: Some(2),
            },
        ];

        let results = append_external_tracks(&conn, &inserts).unwrap();
        assert_eq!(results.len(), 3);

        let rows = load_queue(&conn).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].track.id, 1);
        assert_eq!(rows[1].track.title, "Xtal");
        assert!(rows[1].is_pending);
        assert_eq!(rows[2].track.id, 2);
        for (idx, item) in rows.iter().enumerate() {
            assert_eq!(item.position, idx as i32);
        }
    }

    #[test]
    fn apply_shuffle_preserves_pending_artist_title() {
        let conn = conn();

        // Seed: track 1 as current (locked prefix), then three pending rows.
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'test')",
            [],
        )
        .unwrap();
        for (idx, (artist, title)) in [
            ("Aphex Twin", "Xtal"),
            ("Boards of Canada", "Roygbiv"),
            ("Plaid", "Itsu"),
        ]
        .iter()
        .enumerate()
        {
            conn.execute(
                "INSERT INTO queue (track_id, position, source, pending_artist, pending_title, pending_at)
                 VALUES (NULL, ?1, 'radio_pending', ?2, ?3, datetime('now'))",
                params![(idx as i32) + 1, artist, title],
            )
            .unwrap();
        }
        let q_before = load_queue(&conn).unwrap();
        let current_qid = q_before[0].id;

        // True shuffle the candidates (pending rows). Several runs because
        // the shuffle may permute to the same order by chance — we assert
        // the metadata invariant on every run.
        for seed in 7_654_321..7_654_331 {
            apply_shuffle_with_seed(&conn, ShuffleMode::True, Some(current_qid), seed, "test")
                .unwrap();
            let q_after = load_queue(&conn).unwrap();

            assert_eq!(q_after.len(), 4);
            // Locked prefix (current) is unchanged.
            assert_eq!(q_after[0].id, current_qid);
            assert_eq!(q_after[0].track.id, 1);

            // Every pending row still carries its artist + title — the bug
            // before was that they all blanked out after shuffle.
            let pending_titles: Vec<String> = q_after[1..]
                .iter()
                .map(|item| item.track.title.clone())
                .collect();
            for title in ["Xtal", "Roygbiv", "Itsu"] {
                assert!(
                    pending_titles.iter().any(|t| t == title),
                    "pending title {title:?} disappeared from queue after shuffle: {pending_titles:?}",
                );
            }
            for item in &q_after[1..] {
                assert!(item.is_pending);
                assert!(
                    item.track
                        .artist_name
                        .as_ref()
                        .is_some_and(|a| !a.is_empty())
                );
            }
            // Positions stay contiguous.
            for (idx, item) in q_after.iter().enumerate() {
                assert_eq!(item.position, idx as i32);
            }
        }
    }

    #[test]
    fn insert_external_track_after_shifts_later_rows() {
        let conn = conn();
        let tracks = vec![
            get_track_by_id(&conn, 1).unwrap().unwrap(),
            get_track_by_id(&conn, 2).unwrap().unwrap(),
            get_track_by_id(&conn, 3).unwrap().unwrap(),
        ];
        replace_queue(&conn, &tracks, "test").unwrap();

        // Insert track 4 immediately after position 0 (i.e. between t1 and t2).
        insert_external_track_after(
            &conn,
            &ExternalTrackInsert {
                artist: "ignored",
                title: "ignored",
                source: "user_play_next",
                reason: None,
                tidal_id_hint: None,
                local_track_id: Some(4),
            },
            0,
        )
        .unwrap();

        let queue = load_queue(&conn).unwrap();
        assert_eq!(queue.len(), 4);
        assert_eq!(queue[0].track.id, 1);
        assert_eq!(queue[1].track.id, 4);
        assert_eq!(queue[2].track.id, 2);
        assert_eq!(queue[3].track.id, 3);
        // Positions are contiguous after the shift.
        for (idx, item) in queue.iter().enumerate() {
            assert_eq!(item.position, idx as i32);
        }
    }

    #[test]
    fn insert_external_tracks_after_shifts_tail_once_and_preserves_order() {
        let conn = conn();
        let tracks = vec![
            get_track_by_id(&conn, 1).unwrap().unwrap(),
            get_track_by_id(&conn, 2).unwrap().unwrap(),
            get_track_by_id(&conn, 3).unwrap().unwrap(),
        ];
        replace_queue(&conn, &tracks, "test").unwrap();

        let inserts = vec![
            ExternalTrackInsert {
                artist: "A",
                title: "First external",
                source: "user_play_next",
                reason: None,
                tidal_id_hint: Some(101),
                local_track_id: None,
            },
            ExternalTrackInsert {
                artist: "B",
                title: "Second external",
                source: "user_play_next",
                reason: None,
                tidal_id_hint: Some(102),
                local_track_id: None,
            },
        ];

        insert_external_tracks_after(&conn, &inserts, 0).unwrap();

        let queue = load_queue(&conn).unwrap();
        assert_eq!(queue.len(), 5);
        assert_eq!(queue[0].track.id, 1);
        assert_eq!(queue[1].track.title, "First external");
        assert_eq!(queue[2].track.title, "Second external");
        assert_eq!(queue[3].track.id, 2);
        assert_eq!(queue[4].track.id, 3);
        for (idx, item) in queue.iter().enumerate() {
            assert_eq!(item.position, idx as i32);
        }
    }

    #[test]
    fn reorder_off_preserves_input_order() {
        let conn = conn();
        let tracks: Vec<Track> = (1..=4)
            .map(|id| get_track_by_id(&conn, id).unwrap().unwrap())
            .collect();

        // Off must be a pure identity. Run repeatedly so any thread_rng-driven
        // post-pass would surface as an occasional reorder.
        for _ in 0..20 {
            let reordered =
                reorder_tracks_with_seed(&conn, &tracks, ShuffleMode::Off, 7_654_321, "test")
                    .unwrap();
            let ids: Vec<i64> = reordered.iter().map(|t| t.id).collect();
            assert_eq!(ids, vec![1, 2, 3, 4]);
        }
    }

    #[test]
    fn reorder_genre_alternates_genres_on_balanced_library() {
        let conn = conn();
        conn.execute_batch(
            "INSERT INTO genres (id, name, slug, parent_id) VALUES
                (1, 'House', 'house', NULL),
                (2, 'Ambient', 'ambient', NULL);
             INSERT INTO track_genres (track_id, genre_id) VALUES
                (1, 1), (2, 1), (3, 2), (4, 2);",
        )
        .unwrap();

        let tracks: Vec<Track> = (1..=4)
            .map(|id| get_track_by_id(&conn, id).unwrap().unwrap())
            .collect();
        let genre_of = |id: i64| if id <= 2 { "House" } else { "Ambient" };

        // Two genres with two tracks each can alternate every adjacent pair.
        // If the unconditional artist post-pass returns, this fails on most seeds.
        for attempt in 0..20 {
            let reordered = reorder_tracks_with_seed(
                &conn,
                &tracks,
                ShuffleMode::Genre,
                7_654_321 + i64::from(attempt),
                "test",
            )
            .unwrap();
            let ids: Vec<i64> = reordered.iter().map(|t| t.id).collect();
            assert_eq!(ids.len(), 4);
            for pair in ids.windows(2) {
                assert_ne!(
                    genre_of(pair[0]),
                    genre_of(pair[1]),
                    "adjacent same-genre tracks {} and {} in {:?}",
                    pair[0],
                    pair[1],
                    ids
                );
            }
        }
    }

    #[test]
    fn apply_shuffle_with_seed_is_repeatable() {
        let conn = conn();
        let tracks: Vec<Track> = (1..=4)
            .map(|id| get_track_by_id(&conn, id).unwrap().unwrap())
            .collect();
        replace_queue(&conn, &tracks, "test").unwrap();

        let first =
            apply_shuffle_with_seed(&conn, ShuffleMode::True, None, 7_654_321, "test").unwrap();
        let first_ids: Vec<i64> = first.queue.iter().map(|item| item.track.id).collect();
        let first_debug = first.debug.expect("shuffle debug");

        replace_queue(&conn, &tracks, "test").unwrap();
        let second =
            apply_shuffle_with_seed(&conn, ShuffleMode::True, None, 7_654_321, "test").unwrap();
        let second_ids: Vec<i64> = second.queue.iter().map(|item| item.track.id).collect();

        assert_eq!(first_ids, second_ids);
        assert_eq!(first_debug.mode, "true");
        assert_eq!(first_debug.seed, 7_654_321);
        assert_eq!(first_debug.scope, "test");
        assert_eq!(first_debug.locked_count, 0);
        assert_eq!(first_debug.candidate_count, 4);
    }

    #[test]
    fn seeded_shuffle_moves_pending_queue_item_ids() {
        let conn = conn();
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'test')",
            [],
        )
        .unwrap();
        for (idx, (artist, title)) in [
            ("Aphex Twin", "Xtal"),
            ("Boards of Canada", "Roygbiv"),
            ("Plaid", "Itsu"),
            ("Autechre", "Bike"),
        ]
        .iter()
        .enumerate()
        {
            conn.execute(
                "INSERT INTO queue (track_id, position, source, pending_artist, pending_title, pending_at)
                 VALUES (NULL, ?1, 'radio_pending', ?2, ?3, datetime('now'))",
                params![(idx as i32) + 1, artist, title],
            )
            .unwrap();
        }
        let before = load_queue(&conn).unwrap();
        let current_qid = before[0].id;
        let before_pending_qids = before[1..].iter().map(|item| item.id).collect::<Vec<_>>();

        let shuffled = apply_shuffle_with_seed(
            &conn,
            ShuffleMode::True,
            Some(current_qid),
            7_654_321,
            "test",
        )
        .unwrap();
        let after_pending_qids = shuffled.queue[1..]
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>();

        assert_eq!(shuffled.queue[0].id, current_qid);
        assert_ne!(after_pending_qids, before_pending_qids);
        let mut sorted_before = before_pending_qids;
        let mut sorted_after = after_pending_qids;
        sorted_before.sort_unstable();
        sorted_after.sort_unstable();
        assert_eq!(sorted_after, sorted_before);
    }

    #[test]
    fn seeded_true_mode_matches_plain_fisher_yates() {
        let conn = conn();
        let tracks: Vec<Track> = (1..=4)
            .map(|id| get_track_by_id(&conn, id).unwrap().unwrap())
            .collect();
        let seed = 7_654_321;
        let scope = "flat_true";

        let actual = reorder_tracks_with_seed(&conn, &tracks, ShuffleMode::True, seed, scope)
            .unwrap()
            .into_iter()
            .map(|track| track.id)
            .collect::<Vec<_>>();
        let mut rng = crate::playback::shuffle::seeded_rng(seed, ShuffleMode::True.as_str(), scope);
        let expected = crate::playback::shuffle::true_shuffle_with_rng(&tracks, &mut rng)
            .into_iter()
            .map(|track| track.id)
            .collect::<Vec<_>>();

        assert_eq!(actual, expected);
    }

    #[test]
    fn seeded_shuffle_reorders_duplicate_track_queue_item_ids() {
        let conn = conn();
        let repeated = get_track_by_id(&conn, 1).unwrap().unwrap();
        replace_queue(
            &conn,
            &[
                repeated.clone(),
                repeated.clone(),
                repeated.clone(),
                repeated.clone(),
            ],
            "test",
        )
        .unwrap();
        let before_qids: Vec<i64> = load_queue(&conn)
            .unwrap()
            .iter()
            .map(|item| item.id)
            .collect();

        let shuffled =
            apply_shuffle_with_seed(&conn, ShuffleMode::True, None, 7_654_321, "test").unwrap();
        let after_qids: Vec<i64> = shuffled.queue.iter().map(|item| item.id).collect();

        assert_ne!(after_qids, before_qids);
        let mut sorted_before = before_qids.clone();
        let mut sorted_after = after_qids;
        sorted_before.sort_unstable();
        sorted_after.sort_unstable();
        assert_eq!(sorted_after, sorted_before);
    }

    #[test]
    fn gc_deletes_stale_pending_rows() {
        let conn = conn();
        conn.execute(
            "INSERT INTO queue (track_id, position, source, pending_at)
             VALUES (NULL, 0, 'test', datetime('now', '-7 hours'))",
            [],
        )
        .unwrap();

        let (deleted, unlocked) = super::gc_pending_queue(&conn).unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(unlocked, 0);

        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM queue", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn gc_keeps_fresh_pending_rows() {
        let conn = conn();
        conn.execute(
            "INSERT INTO queue (track_id, position, source, pending_at)
             VALUES (NULL, 0, 'test', datetime('now', '-1 hour'))",
            [],
        )
        .unwrap();

        let (deleted, unlocked) = super::gc_pending_queue(&conn).unwrap();
        assert_eq!(deleted, 0);
        assert_eq!(unlocked, 0);

        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM queue", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn gc_clears_stale_resolving_lock() {
        let conn = conn();
        conn.execute(
            "INSERT INTO queue (track_id, position, source, pending_at, resolving_at)
             VALUES (NULL, 0, 'test', datetime('now', '-1 hour'), datetime('now', '-1 minute'))",
            [],
        )
        .unwrap();
        let row_id: i32 = conn
            .query_row("SELECT id FROM queue", [], |row| row.get(0))
            .unwrap();

        let (deleted, unlocked) = super::gc_pending_queue(&conn).unwrap();
        assert_eq!(deleted, 0);
        assert_eq!(unlocked, 1);

        let resolving_at: Option<String> = conn
            .query_row(
                "SELECT resolving_at FROM queue WHERE id = ?",
                params![row_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(resolving_at, None::<String>);
    }

    #[test]
    fn gc_keeps_fresh_resolving_lock() {
        let conn = conn();
        conn.execute(
            "INSERT INTO queue (track_id, position, source, pending_at, resolving_at)
             VALUES (NULL, 0, 'test', datetime('now', '-1 hour'), datetime('now', '-10 seconds'))",
            [],
        )
        .unwrap();
        let row_id: i32 = conn
            .query_row("SELECT id FROM queue", [], |row| row.get(0))
            .unwrap();

        let (deleted, unlocked) = super::gc_pending_queue(&conn).unwrap();
        assert_eq!(deleted, 0);
        assert_eq!(unlocked, 0);

        let resolving_at: Option<String> = conn
            .query_row(
                "SELECT resolving_at FROM queue WHERE id = ?",
                params![row_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(resolving_at.is_some());
    }

    #[test]
    fn gc_does_not_touch_resolved_rows() {
        let conn = conn();
        // track_id is set (resolved), but pending_at is old and resolving_at is stale.
        conn.execute(
            "INSERT INTO queue (track_id, position, source, pending_at, resolving_at)
             VALUES (1, 0, 'test', datetime('now', '-7 hours'), datetime('now', '-1 minute'))",
            [],
        )
        .unwrap();
        let row_id: i32 = conn
            .query_row("SELECT id FROM queue", [], |row| row.get(0))
            .unwrap();

        let (deleted, unlocked) = super::gc_pending_queue(&conn).unwrap();
        assert_eq!(deleted, 0);
        assert_eq!(unlocked, 0);

        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM queue", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let resolving_at: Option<String> = conn
            .query_row(
                "SELECT resolving_at FROM queue WHERE id = ?",
                params![row_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(resolving_at.is_some());
    }

    fn ephemeral(tidal_id: i64, title: &str) -> EphemeralTidalInsert<'_> {
        EphemeralTidalInsert {
            tidal_id,
            title,
            artist: Some("Artist"),
            album_title: Some("Album"),
            artwork_url: Some("https://resources.tidal.com/x.jpg"),
            duration_ms: Some(180_000),
        }
    }

    #[test]
    fn ephemeral_rows_load_as_playable_not_pending() {
        let conn = conn();
        append_ephemeral_tidal_tracks(
            &conn,
            &[ephemeral(501, "First"), ephemeral(502, "Second")],
            "tidal_mix",
        )
        .unwrap();

        let items = load_queue(&conn).unwrap();
        assert_eq!(items.len(), 2);
        for item in &items {
            assert!(!item.is_pending, "ephemeral rows are directly playable");
            assert_eq!(item.source, "tidal_mix");
            assert_eq!(item.track.source, "tidal_ephemeral");
            assert_eq!(item.track.album_title.as_deref(), Some("Album"));
        }
        // Synthetic negative track id derived from the TIDAL id, tidal_id set.
        assert_eq!(items[0].track.id, -501);
        assert_eq!(items[0].track.tidal_id, Some(501));
        assert_eq!(items[0].track.title, "First");
    }

    #[test]
    fn ephemeral_row_in_library_uses_local_id_and_favorite() {
        let conn = conn();
        // Track 1 already lives in the library with tidal_id 501.
        conn.execute(
            "UPDATE tracks SET tidal_id = 501, is_favorite = 1 WHERE id = 1",
            [],
        )
        .unwrap();
        append_ephemeral_tidal_tracks(&conn, &[ephemeral(501, "First")], "tidal_album").unwrap();

        let items = load_queue(&conn).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].track.id, 1, "library local id recovered");
        assert!(items[0].track.is_favorite);
        assert!(!items[0].is_pending);
    }

    #[test]
    fn pop_next_ephemeral_consumes_in_order() {
        let conn = conn();
        append_ephemeral_tidal_tracks(
            &conn,
            &[
                ephemeral(601, "A"),
                ephemeral(602, "B"),
                ephemeral(603, "C"),
            ],
            "tidal_mix",
        )
        .unwrap();

        let first = pop_next_ephemeral_tidal_track(&conn).unwrap().unwrap();
        assert_eq!(first.tidal_track_id, 601);
        let second = peek_next_ephemeral_tidal_track(&conn).unwrap().unwrap();
        assert_eq!(second.tidal_track_id, 602, "peek does not consume");
        assert_eq!(load_queue(&conn).unwrap().len(), 2);
    }

    #[test]
    fn trim_through_tidal_id_drops_rows_up_to_target() {
        let conn = conn();
        append_ephemeral_tidal_tracks(
            &conn,
            &[
                ephemeral(701, "A"),
                ephemeral(702, "B"),
                ephemeral(703, "C"),
            ],
            "tidal_mix",
        )
        .unwrap();

        let found = trim_ephemeral_tidal_rows_through_tidal_id(&conn, 702).unwrap();
        assert!(found);
        let remaining: Vec<i64> = peek_ephemeral_tidal_tracks(&conn)
            .unwrap()
            .into_iter()
            .map(|t| t.tidal_track_id)
            .collect();
        assert_eq!(remaining, vec![703], "rows up to and including 702 removed");

        // A tidal id outside the mix reports not-found and leaves rows intact.
        assert!(!trim_ephemeral_tidal_rows_through_tidal_id(&conn, 999).unwrap());
        assert_eq!(peek_ephemeral_tidal_tracks(&conn).unwrap().len(), 1);
    }

    #[test]
    fn delete_all_ephemeral_leaves_library_rows() {
        let conn = conn();
        // A real library queue row plus ephemeral rows.
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
            [],
        )
        .unwrap();
        append_ephemeral_tidal_tracks(&conn, &[ephemeral(801, "A")], "tidal_mix").unwrap();

        let removed = delete_all_ephemeral_tidal_rows(&conn).unwrap();
        assert_eq!(removed, 1);
        let items = load_queue(&conn).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].track.id, 1, "library row survives");
    }
}
