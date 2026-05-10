use crate::db::models::{QueueItem, Track};
use crate::playback::shuffle::{
    WeightedShuffleProfile, artist_spread_shuffle, genre_shuffle, true_shuffle, weighted_shuffle,
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

pub fn load_queue(conn: &Connection) -> Result<Vec<QueueItem>> {
    let mut stmt = conn.prepare(
        // Phase 2c-ii-a: LEFT JOIN so pending rows (track_id IS NULL) appear.
        // COALESCE fills non-nullable Track fields from pending_* columns so the
        // row mapper doesn't need to know whether a row is pending or resolved.
        // Columns 0-3: queue metadata; 4-25: Track fields; 26: is_pending flag.
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
                (q.track_id IS NULL) AS is_pending
         FROM queue q
         LEFT JOIN tracks t ON q.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         ORDER BY q.position ASC, q.id ASC",
    )?;

    let items = stmt
        .query_map([], |row| {
            Ok(QueueItem {
                id: row.get(0)?,
                position: row.get(1)?,
                source: row.get(2)?,
                reason: row.get(3)?,
                track: track_from_row_with_offset(row, 4)?,
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
/// Radio is the producer of reasons today; automix and manual paths
/// pass `None` via [`append_tracks`]. The reason column stays
/// queryable but the frontend treats NULL as "no provenance recorded"
/// and renders no tooltip.
pub fn append_tracks_with_reasons(
    conn: &Connection,
    tracks: &[(Track, Option<String>)],
    source: &str,
) -> Result<Vec<QueueItem>> {
    let start_pos: i32 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM queue",
        [],
        |row| row.get(0),
    )?;

    for (idx, (track, reason)) in tracks.iter().enumerate() {
        conn.execute(
            "INSERT INTO queue (track_id, position, source, reason) VALUES (?1, ?2, ?3, ?4)",
            params![track.id, start_pos + idx as i32, source, reason],
        )?;
    }

    load_queue(conn)
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
    let start_pos: i32 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM queue",
        [],
        |row| row.get(0),
    )?;

    for (idx, c) in candidates.iter().enumerate() {
        conn.execute(
            "INSERT INTO queue (track_id, position, source, reason, pending_artist, pending_title, pending_at)
             VALUES (NULL, ?1, 'radio_pending', ?2, ?3, ?4, datetime('now'))",
            params![start_pos + idx as i32, c.reason, c.artist, c.title],
        )?;
    }

    load_queue(conn)
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

pub fn replace_queue(conn: &Connection, tracks: &[Track], source: &str) -> Result<Vec<QueueItem>> {
    conn.execute("DELETE FROM queue", [])?;
    append_tracks(conn, tracks, source)
}

/// Wipe the queue and replace with tracks plus per-row reasons.
pub fn replace_queue_with_reasons(
    conn: &Connection,
    tracks: &[(Track, Option<String>)],
    source: &str,
) -> Result<Vec<QueueItem>> {
    conn.execute("DELETE FROM queue", [])?;
    append_tracks_with_reasons(conn, tracks, source)
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

pub fn apply_shuffle(
    conn: &Connection,
    mode: ShuffleMode,
    current_queue_item_id: Option<i64>,
) -> Result<Vec<QueueItem>> {
    let queue_items = load_queue(conn)?;
    if queue_items.len() <= 1 || mode == ShuffleMode::Off {
        return Ok(queue_items);
    }

    let split_index = current_queue_item_id
        .and_then(|qid| queue_items.iter().position(|item| item.id == qid))
        .map(|idx| idx + 1)
        .unwrap_or(0);

    let locked_qids: Vec<i64> = queue_items[..split_index].iter().map(|i| i.id).collect();
    let candidate_tracks: Vec<Track> = queue_items[split_index..]
        .iter()
        .map(|i| i.track.clone())
        .collect();

    let reordered_tracks = reorder_tracks(conn, &candidate_tracks, mode)?;

    // Map shuffled tracks back to queue item IDs. Pending rows all share
    // `track.id == 0`, so we route by track.id with a per-id FIFO of qids:
    // the i-th pending row in the shuffled output gets the i-th pending
    // qid from the candidate region. Library rows use the same machinery
    // and handle the rare duplicate-track-id case as a side-effect.
    use std::collections::VecDeque;
    let mut qid_buckets: HashMap<i64, VecDeque<i64>> = HashMap::new();
    for item in &queue_items[split_index..] {
        qid_buckets
            .entry(item.track.id)
            .or_default()
            .push_back(item.id);
    }
    let mut shuffled_qids: Vec<i64> = Vec::with_capacity(reordered_tracks.len());
    for t in &reordered_tracks {
        if let Some(bucket) = qid_buckets.get_mut(&t.id)
            && let Some(qid) = bucket.pop_front() {
                shuffled_qids.push(qid);
            }
    }
    // Defensive: if reorder_tracks dropped any rows (it shouldn't), append
    // remaining qids in their original order so the queue stays intact.
    for (_id, bucket) in qid_buckets.iter_mut() {
        while let Some(qid) = bucket.pop_front() {
            shuffled_qids.push(qid);
        }
    }

    let final_qids: Vec<i64> = locked_qids
        .into_iter()
        .chain(shuffled_qids)
        .collect();

    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare("UPDATE queue SET position = ?1 WHERE id = ?2")?;
        for (idx, qid) in final_qids.iter().enumerate() {
            stmt.execute(params![idx as i32, qid])?;
        }
    }
    tx.commit()?;

    load_queue(conn)
}

fn reorder_tracks(conn: &Connection, tracks: &[Track], mode: ShuffleMode) -> Result<Vec<Track>> {
    // Off must preserve the caller's order; an artist post-pass would silently
    // rearrange tracks the user didn't ask to shuffle. Genre mode already runs
    // artist-spread + genre-stabilize internally — running artist-spread again
    // here re-clusters genres and undoes that work.
    match mode {
        ShuffleMode::Off => Ok(tracks.to_vec()),
        ShuffleMode::True => Ok(artist_spread_shuffle(&true_shuffle(tracks))),
        ShuffleMode::Weighted => {
            let weighted = weighted_shuffle(tracks, &WeightedShuffleProfile::default());
            Ok(artist_spread_shuffle(&weighted))
        }
        ShuffleMode::Genre => {
            let genre_map = get_track_genres(conn, tracks)?;
            Ok(genre_shuffle(tracks, &genre_map))
        }
    }
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
                tidal_id_hint    INTEGER
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
        for _ in 0..10 {
            apply_shuffle(&conn, ShuffleMode::True, Some(current_qid)).unwrap();
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
    fn reorder_off_preserves_input_order() {
        let conn = conn();
        let tracks: Vec<Track> = (1..=4)
            .map(|id| get_track_by_id(&conn, id).unwrap().unwrap())
            .collect();

        // Off must be a pure identity. Run repeatedly so any thread_rng-driven
        // post-pass would surface as an occasional reorder.
        for _ in 0..20 {
            let reordered = reorder_tracks(&conn, &tracks, ShuffleMode::Off).unwrap();
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

        // Two genres × two tracks each → alternation is always achievable.
        // If the unconditional artist post-pass returns, this fails on most seeds.
        for _ in 0..20 {
            let reordered = reorder_tracks(&conn, &tracks, ShuffleMode::Genre).unwrap();
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
}
