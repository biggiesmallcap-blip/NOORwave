//! Fixtures for On Air tests: a migrated in-memory database and helpers that
//! write the rows the other Atlas and Air workers' builds would produce.

use rusqlite::{Connection, params};

use crate::services::video_sets::VideoCandidate;

pub fn conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::run_migrations(&conn).unwrap();
    conn
}

pub fn add_catalog(conn: &Connection, id: i64, artist: i64, title: &str, duration_s: i64) {
    let video = VideoCandidate {
        tidal_id: id,
        title: title.to_string(),
        duration_s: Some(duration_s),
        artist_id: Some(artist),
        artist_name: Some(format!("Artist {artist}")),
        artwork_url: Some(format!(
            "https://resources.tidal.com/images/{id}/640x640.jpg"
        )),
        ..Default::default()
    };
    conn.execute(
        "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            id,
            artist,
            video.artist_name,
            serde_json::to_string(&video).unwrap()
        ],
    )
    .unwrap();
}

pub fn add_facet(conn: &Connection, id: i64, artist: i64, cut: &str, song_key: &str, quality: f64) {
    let duration: Option<i64> = conn
        .query_row(
            "SELECT json_extract(item_json, '$.duration_s') FROM video_catalog WHERE tidal_video_id = ?1",
            [id],
            |row| row.get(0),
        )
        .unwrap_or(Some(200));
    conn.execute(
        "INSERT INTO video_facets
             (tidal_video_id, artist_tidal_id, cut, programmable, song_key, duration_s, quality)
         VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6)",
        params![id, artist, cut, song_key, duration, quality],
    )
    .unwrap();
}

/// A catalog row plus its facet in one go.
pub fn add_video(conn: &Connection, id: i64, artist: i64, cut: &str, song: &str, duration_s: i64) {
    add_catalog(conn, id, artist, song, duration_s);
    add_facet(conn, id, artist, cut, &format!("id:{artist}:{song}"), 0.5);
}

pub fn add_song(conn: &Connection, key: &str, artist: i64, best: i64, year: Option<i64>) {
    conn.execute(
        "INSERT INTO video_songs (song_key, artist_tidal_id, title, best_video_id, cut_count, programmable, song_year)
         VALUES (?1, ?2, ?1, ?3, 1, 1, ?4)",
        params![key, artist, best, year],
    )
    .unwrap();
}

pub fn add_scene(conn: &Connection, id: i64, name: &str, channel_no: Option<i64>) {
    conn.execute(
        "INSERT INTO video_scenes (scene_id, name, hue, channel_no, exemplars_json)
         VALUES (?1, ?2, ?3, ?4, '[\"Exemplar One\",\"Exemplar Two\"]')",
        params![id, name, (id * 37 % 360) as f64, channel_no],
    )
    .unwrap();
}

pub fn add_artist_scene(conn: &Connection, artist: i64, scene: i64) {
    conn.execute(
        "INSERT INTO video_artist_facets (artist_tidal_id, name, scene_id, charted, playable)
         VALUES (?1, ?2, ?3, 1, 1)
         ON CONFLICT(artist_tidal_id) DO UPDATE SET scene_id = excluded.scene_id",
        params![artist, format!("Artist {artist}"), scene],
    )
    .unwrap();
}

pub fn add_concert(conn: &Connection, id: i64, artist: i64, label: &str, videos: &[i64]) {
    conn.execute(
        "INSERT INTO video_concerts
             (concert_id, artist_tidal_id, label, video_count, total_s, first_video_id, video_ids_json)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)",
        params![
            id,
            artist,
            label,
            videos.len() as i64,
            videos.first().copied().unwrap_or(0),
            serde_json::to_string(videos).unwrap()
        ],
    )
    .unwrap();
    for video in videos {
        conn.execute(
            "UPDATE video_facets SET concert_id = ?1 WHERE tidal_video_id = ?2",
            params![id, video],
        )
        .unwrap();
    }
}

/// A library artist (local id = tidal id) with `liked` favorite tracks and
/// `plays` manual listens.
pub fn add_library_artist(conn: &Connection, tidal_id: i64, name: &str, liked: i64, plays: i64) {
    conn.execute(
        "INSERT INTO artists (id, tidal_id, name) VALUES (?1, ?1, ?2)",
        params![tidal_id, name],
    )
    .unwrap();
    for n in 0..liked.max(1) {
        let track = tidal_id * 1000 + n;
        conn.execute(
            "INSERT INTO tracks (id, artist_id, title, is_favorite, is_library) VALUES (?1, ?2, 'T', ?3, 1)",
            params![track, tidal_id, i64::from(n < liked)],
        )
        .unwrap();
    }
    for n in 0..plays {
        conn.execute(
            "INSERT INTO listen_history (track_id, started_at, source) VALUES (?1, datetime('now', ?2), 'manual')",
            params![tidal_id * 1000, format!("-{} hours", n + 1)],
        )
        .unwrap();
    }
}

pub fn add_edge(conn: &Connection, from: i64, to: i64, weight: f64) {
    conn.execute(
        "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, weight)
         VALUES (?1, ?2, '', 'tidal', ?3)",
        params![from, to, weight],
    )
    .unwrap();
}

/// A watch `hours_ago`, watched for `watched_s` of `duration_s`.
pub fn add_watch(
    conn: &Connection,
    video: i64,
    artist: i64,
    watched_s: i64,
    duration_s: i64,
    hours_ago: i64,
) {
    conn.execute(
        "INSERT INTO video_history
             (tidal_video_id, artist_tidal_id, artist_name, started_at, duration_watched_ms, video_duration_ms, completed)
         VALUES (?1, ?2, ?3, datetime('now', ?4), ?5, ?6, ?7)",
        params![
            video,
            artist,
            format!("Artist {artist}"),
            format!("-{hours_ago} hours"),
            watched_s * 1000,
            duration_s * 1000,
            i64::from(watched_s >= duration_s)
        ],
    )
    .unwrap();
}
