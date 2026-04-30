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
        "SELECT q.id, q.position, q.source, q.reason,
                t.id, t.title, t.artist_id, a.name, t.album_id, al.title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM queue q
         JOIN tracks t ON q.track_id = t.id
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
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(items)
}

pub fn queue_track_ids(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT track_id FROM queue ORDER BY position ASC, id ASC")?;
    let ids = stmt
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

pub fn append_tracks(conn: &Connection, tracks: &[Track], source: &str) -> Result<Vec<QueueItem>> {
    let with_reasons: Vec<(Track, Option<String>)> = tracks
        .iter()
        .cloned()
        .map(|track| (track, None))
        .collect();
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
    current_track_id: Option<i64>,
) -> Result<Vec<QueueItem>> {
    let queue_items = load_queue(conn)?;
    if queue_items.len() <= 1 || mode == ShuffleMode::Off {
        return Ok(queue_items);
    }

    let mut locked_prefix = Vec::new();
    let mut candidates = Vec::new();
    let split_index = current_track_id.and_then(|track_id| {
        queue_items
            .iter()
            .position(|item| item.track.id == track_id)
            .map(|idx| idx + 1)
    });

    for (idx, item) in queue_items.iter().cloned().enumerate() {
        if split_index.is_some_and(|split| idx < split) {
            locked_prefix.push(item.track);
        } else {
            candidates.push(item.track);
        }
    }

    let reordered = reorder_tracks(conn, &candidates, mode)?;
    let final_tracks = locked_prefix
        .into_iter()
        .chain(reordered)
        .collect::<Vec<_>>();

    replace_queue(conn, &final_tracks, "playback")
}

fn reorder_tracks(conn: &Connection, tracks: &[Track], mode: ShuffleMode) -> Result<Vec<Track>> {
    let reordered = match mode {
        ShuffleMode::Off => tracks.to_vec(),
        ShuffleMode::True => true_shuffle(tracks),
        ShuffleMode::Weighted => weighted_shuffle(tracks, &WeightedShuffleProfile::default()),
        ShuffleMode::Genre => {
            let genre_map = get_track_genres(conn, tracks)?;
            genre_shuffle(tracks, &genre_map)
        }
    };

    let spread = artist_spread_shuffle(&reordered);
    Ok(spread)
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
                id INTEGER PRIMARY KEY,
                track_id INTEGER NOT NULL,
                position INTEGER NOT NULL,
                source TEXT DEFAULT 'user'
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
}
