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
        "SELECT q.id, q.position, q.source,
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
                track: track_from_row_with_offset(row, 3)?,
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
    let start_pos: i32 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM queue",
        [],
        |row| row.get(0),
    )?;

    for (idx, track) in tracks.iter().enumerate() {
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (?1, ?2, ?3)",
            params![track.id, start_pos + idx as i32, source],
        )?;
    }

    load_queue(conn)
}

pub fn replace_queue(conn: &Connection, tracks: &[Track], source: &str) -> Result<Vec<QueueItem>> {
    conn.execute("DELETE FROM queue", [])?;
    append_tracks(conn, tracks, source)
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

    let mut by_track = HashMap::new();

    for chunk in track_ids.chunks(TRACK_GENRE_CHUNK_SIZE) {
        let mut query = String::from(
            "WITH RECURSIVE genre_paths(id, parent_id, path) AS (
                SELECT id, parent_id, name
                FROM genres
                WHERE parent_id IS NULL
                UNION ALL
                SELECT g.id, g.parent_id, genre_paths.path || ' > ' || g.name
                FROM genres g
                JOIN genre_paths ON g.parent_id = genre_paths.id
            )
            SELECT tg.track_id, genre_paths.path
            FROM track_genres tg
            JOIN genre_paths ON genre_paths.id = tg.genre_id
            WHERE tg.track_id IN (",
        );
        for idx in 0..chunk.len() {
            if idx > 0 {
                query.push_str(", ");
            }
            query.push('?');
            query.push_str(&(idx + 1).to_string());
        }
        query.push(')');
        query.push_str(" ORDER BY tg.track_id, genre_paths.path");

        let mut stmt = conn.prepare(&query)?;
        let params = rusqlite::params_from_iter(chunk.iter());
        let mut rows = stmt.query(params)?;
        while let Some(row) = rows.next()? {
            let track_id: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            by_track.entry(track_id).or_insert_with(Vec::new).push(path);
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
    let ids = {
        let mut stmt = conn.prepare("SELECT id FROM queue ORDER BY position ASC, id ASC")?;
        stmt.query_map([], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    for (position, id) in ids.into_iter().enumerate() {
        conn.execute(
            "UPDATE queue SET position = ?1 WHERE id = ?2",
            params![position as i32, id],
        )?;
    }
    Ok(())
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
