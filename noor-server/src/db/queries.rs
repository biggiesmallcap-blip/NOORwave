use super::models::*;
use anyhow::Result;
use crate::services::discovery::DiscoveryCandidateSeed;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

// ─── Server Config ────────────────────────────────────────

pub fn ensure_server_token(conn: &Connection) -> Result<String> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key='server_token'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    // Keep the existing token only if it matches the current 6-digit PIN format.
    // Legacy hex/word-phrase tokens are auto-upgraded on next startup.
    if let Some(token) = existing {
        if is_valid_pin(&token) {
            return Ok(token);
        }
    }

    let token = generate_readable_token();
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('server_token', ?1)",
        params![token],
    )?;
    Ok(token)
}

fn is_valid_pin(s: &str) -> bool {
    s.len() == 6 && s.chars().all(|c| c.is_ascii_digit())
}

pub fn regenerate_server_token(conn: &Connection) -> Result<String> {
    let token = generate_readable_token();
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('server_token', ?1)",
        params![token],
    )?;
    Ok(token)
}

fn generate_readable_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut bytes);
    let n = u32::from_le_bytes(bytes) % 1_000_000;
    format!("{:06}", n)
}


// ─── Tracks ───────────────────────────────────────────────

/// Optional DSP filters for get_tracks_with_dsp()
#[derive(Debug, Clone, Default)]
pub struct DspFilters {
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub energy_min: Option<f64>,
    pub energy_max: Option<f64>,
    pub key_signature: Option<String>,
    pub instrumental_only: bool,
}

pub fn get_tracks(
    conn: &Connection,
    sort_by: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
    favorite_only: bool,
) -> Result<Vec<Track>> {
    get_tracks_with_dsp(conn, sort_by, sort_dir, limit, offset, favorite_only, &DspFilters::default())
}

pub fn get_tracks_with_dsp(
    conn: &Connection,
    sort_by: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
    favorite_only: bool,
    dsp: &DspFilters,
) -> Result<Vec<Track>> {
    let has_dsp = dsp.bpm_min.is_some() || dsp.bpm_max.is_some()
        || dsp.energy_min.is_some() || dsp.energy_max.is_some()
        || dsp.key_signature.is_some() || dsp.instrumental_only;

    let order_col = match sort_by {
        "title" => "t.title",
        "artist" => "a.name",
        "album" => "al.title",
        "year" => "al.year",
        "date_added" => "t.date_added",
        "duration" => "t.duration_ms",
        "play_count" => "t.play_count",
        "fidelity" => "t.fidelity_score",
        "bpm" => "COALESCE(a.bpm, 0)",
        "energy" => "COALESCE(a.energy, 0)",
        "danceability" => "COALESCE(a.danceability, 0)",
        _ => "t.date_added",
    };
    let dir = if sort_dir == "asc" { "ASC" } else { "DESC" };

    let mut conditions = Vec::new();
    if favorite_only {
        conditions.push(
            "(t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))"
                .to_string(),
        );
    }

    let join_clause = if has_dsp {
        " LEFT JOIN audio_dsp_features a ON t.id = a.track_id"
    } else {
        ""
    };

    if let Some(min) = dsp.bpm_min {
        conditions.push(format!("a.bpm >= {min}"));
    }
    if let Some(max) = dsp.bpm_max {
        conditions.push(format!("a.bpm <= {max}"));
    }
    if let Some(min) = dsp.energy_min {
        conditions.push(format!("a.energy >= {min}"));
    }
    if let Some(max) = dsp.energy_max {
        conditions.push(format!("a.energy <= {max}"));
    }
    if let Some(ref key) = dsp.key_signature {
        conditions.push(format!("a.key_signature = '{key}'"));
    }
    if dsp.instrumental_only {
        conditions.push("a.is_instrumental = 1".to_string());
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT t.id, t.title, t.artist_id, a_artists.name as artist_name,
                t.album_id, al.title as album_title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM tracks t
         LEFT JOIN artists a_artists ON t.artist_id = a_artists.id
         LEFT JOIN albums al ON t.album_id = al.id
         {join_clause}
         {where_clause}
         ORDER BY {order_col} {dir}
         LIMIT ?1 OFFSET ?2"
    );

    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(params![limit, offset], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

pub fn get_track_count(conn: &Connection, favorite_only: bool) -> Result<i64> {
    let filter = if favorite_only {
        " WHERE is_favorite = 1 OR album_id IN (SELECT id FROM albums WHERE is_favorite = 1)"
    } else {
        ""
    };
    Ok(
        conn.query_row(&format!("SELECT COUNT(*) FROM tracks{filter}"), [], |row| {
            row.get(0)
        })?,
    )
}

// ─── Albums ───────────────────────────────────────────────

pub fn get_albums(
    conn: &Connection,
    sort_by: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
    favorite_only: bool,
) -> Result<Vec<Album>> {
    let order_col = match sort_by {
        "title" => "al.title",
        "artist" => "a.name",
        "year" => "al.year",
        _ => "al.title",
    };
    let dir = if sort_dir == "asc" { "ASC" } else { "DESC" };

    let fav_filter = if favorite_only {
        " WHERE al.is_favorite = 1"
    } else {
        ""
    };

    let sql = format!(
        "SELECT al.id, al.tidal_id, al.ytmusic_id, al.title, al.artist_id,
                a.name as artist_name, al.year, al.artwork_url,
                al.release_type, al.label, al.track_count, al.is_favorite, al.source
         FROM albums al
         LEFT JOIN artists a ON al.artist_id = a.id
         {fav_filter}
         ORDER BY {order_col} {dir}
         LIMIT ?1 OFFSET ?2"
    );

    let mut stmt = conn.prepare(&sql)?;
    let albums = stmt
        .query_map(params![limit, offset], |row| {
            Ok(Album {
                id: row.get(0)?,
                tidal_id: row.get(1)?,
                ytmusic_id: row.get(2)?,
                title: row.get(3)?,
                artist_id: row.get(4)?,
                artist_name: row.get(5)?,
                year: row.get(6)?,
                artwork_url: row.get(7)?,
                release_type: row.get(8)?,
                label: row.get(9)?,
                track_count: row.get(10)?,
                is_favorite: row.get::<_, i32>(11)? != 0,
                source: row.get(12)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(albums)
}

pub fn get_album_count(conn: &Connection, favorite_only: bool) -> Result<i64> {
    let filter = if favorite_only {
        " WHERE is_favorite = 1"
    } else {
        ""
    };
    Ok(
        conn.query_row(&format!("SELECT COUNT(*) FROM albums{filter}"), [], |row| {
            row.get(0)
        })?,
    )
}

pub fn get_album_tracks(conn: &Connection, album_id: i64) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, t.artist_id, a.name as artist_name,
                t.album_id, al.title as album_title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.album_id = ?1
         ORDER BY
            COALESCE(t.disc_number, 1) ASC,
            COALESCE(t.track_number, 999999) ASC,
            t.title COLLATE NOCASE ASC",
    )?;

    let tracks = stmt
        .query_map(params![album_id], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

// ─── Artists ──────────────────────────────────────────────

pub fn get_artists(
    conn: &Connection,
    sort_by: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Artist>> {
    let order_col = match sort_by {
        "name" => "a.name",
        _ => "a.name",
    };
    let dir = if sort_dir == "asc" { "ASC" } else { "DESC" };

    let sql = format!(
        "SELECT a.id, a.tidal_id, a.ytmusic_id, a.soundcloud_id,
                a.name, a.name_sort, a.biography, a.photo_url
         FROM artists a
         ORDER BY {order_col} {dir}
         LIMIT ?1 OFFSET ?2"
    );

    let mut stmt = conn.prepare(&sql)?;
    let artists = stmt
        .query_map(params![limit, offset], |row| {
            Ok(Artist {
                id: row.get(0)?,
                tidal_id: row.get(1)?,
                ytmusic_id: row.get(2)?,
                soundcloud_id: row.get(3)?,
                name: row.get(4)?,
                name_sort: row.get(5)?,
                biography: row.get(6)?,
                photo_url: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(artists)
}

pub fn get_artist_tidal_id(conn: &Connection, artist_id: i64) -> Result<Option<i64>> {
    let mut stmt = conn.prepare("SELECT tidal_id FROM artists WHERE id = ?1")?;
    let tidal_id = stmt
        .query_row(params![artist_id], |row| row.get::<_, Option<i64>>(0))
        .optional()?
        .flatten();
    Ok(tidal_id)
}

pub fn get_known_album_tidal_ids(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, i64>> {
    if tidal_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = placeholders(tidal_ids.len());
    let sql = format!(
        "SELECT tidal_id, id FROM albums WHERE tidal_id IN ({placeholders})"
    );
    let params = params_from_iter(tidal_ids.iter().copied());
    let mut stmt = conn.prepare(&sql)?;
    let mut map = HashMap::new();
    let rows = stmt.query_map(params, |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (tidal_id, local_id) = row?;
        map.insert(tidal_id, local_id);
    }
    Ok(map)
}

pub fn get_artist_tracks(conn: &Connection, artist_id: i64) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, t.artist_id, a.name as artist_name,
                t.album_id, al.title as album_title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.artist_id = ?1
         ORDER BY
            al.year ASC,
            COALESCE(t.disc_number, 1) ASC,
            COALESCE(t.track_number, 999999) ASC,
            t.title COLLATE NOCASE ASC",
    )?;

    let tracks = stmt
        .query_map(params![artist_id], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

// ─── Playlists ────────────────────────────────────────────

pub fn get_playlists(conn: &Connection) -> Result<Vec<Playlist>> {
    let mut stmt = conn.prepare(
        "SELECT id, tidal_uuid, name, description, is_smart,
                smart_rules, is_synced, track_count
         FROM playlists
         ORDER BY name ASC",
    )?;

    let playlists = stmt
        .query_map([], |row| {
            Ok(Playlist {
                id: row.get(0)?,
                tidal_uuid: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                is_smart: row.get(4)?,
                smart_rules: row.get(5)?,
                is_synced: row.get(6)?,
                track_count: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(playlists)
}

pub fn get_playlist(conn: &Connection, playlist_id: i64) -> Result<Option<Playlist>> {
    let mut stmt = conn.prepare(
        "SELECT id, tidal_uuid, name, description, is_smart,
                smart_rules, is_synced, track_count
         FROM playlists
         WHERE id = ?1",
    )?;

    let mut rows = stmt.query(params![playlist_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(Playlist {
            id: row.get(0)?,
            tidal_uuid: row.get(1)?,
            name: row.get(2)?,
            description: row.get(3)?,
            is_smart: row.get(4)?,
            smart_rules: row.get(5)?,
            is_synced: row.get(6)?,
            track_count: row.get(7)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn get_playlist_tracks(conn: &Connection, playlist_id: i64) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, t.artist_id, a.name, t.album_id, al.title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM playlist_tracks pt
         JOIN tracks t ON pt.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE pt.playlist_id = ?1
         ORDER BY pt.position ASC",
    )?;

    let tracks = stmt
        .query_map(params![playlist_id], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

pub fn get_all_tracks(conn: &Connection) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, t.artist_id, a.name as artist_name,
                t.album_id, al.title as album_title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         ORDER BY t.date_added DESC, t.id DESC",
    )?;

    let tracks = stmt
        .query_map([], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

pub fn get_track_genre_paths(conn: &Connection) -> Result<HashMap<i64, Vec<String>>> {
    let mut stmt = conn.prepare(
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
        ORDER BY tg.track_id, genre_paths.path",
    )?;

    let mut rows = stmt.query([])?;
    let mut by_track: HashMap<i64, Vec<String>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let track_id: i64 = row.get(0)?;
        let path: String = row.get(1)?;
        by_track.entry(track_id).or_default().push(path);
    }

    Ok(by_track)
}

pub fn create_smart_playlist(
    conn: &Connection,
    name: &str,
    description: Option<&str>,
    rules_json: &str,
) -> Result<Playlist> {
    conn.execute(
        "INSERT INTO playlists (name, description, is_smart, smart_rules, is_synced, track_count)
         VALUES (?1, ?2, 1, ?3, 0, 0)",
        params![name, description, rules_json],
    )?;
    let id = conn.last_insert_rowid();
    get_playlist(conn, id)?.ok_or_else(|| anyhow::anyhow!("playlist not found after insert"))
}

pub fn update_smart_playlist(
    conn: &Connection,
    id: i64,
    name: &str,
    description: Option<&str>,
    rules_json: &str,
) -> Result<Playlist> {
    let rows = conn.execute(
        "UPDATE playlists
         SET name = ?1, description = ?2, smart_rules = ?3, updated_at = datetime('now')
         WHERE id = ?4 AND is_smart = 1",
        params![name, description, rules_json, id],
    )?;
    if rows == 0 {
        return Err(anyhow::anyhow!("smart playlist not found or not editable"));
    }
    get_playlist(conn, id)?.ok_or_else(|| anyhow::anyhow!("playlist not found after update"))
}

pub fn delete_smart_playlist(conn: &Connection, id: i64) -> Result<()> {
    let rows = conn.execute(
        "DELETE FROM playlists WHERE id = ?1 AND is_smart = 1",
        params![id],
    )?;
    if rows == 0 {
        return Err(anyhow::anyhow!("smart playlist not found"));
    }
    Ok(())
}

pub fn get_playlist_memberships(conn: &Connection) -> Result<HashMap<i64, HashSet<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT playlist_id, track_id
         FROM playlist_tracks
         ORDER BY playlist_id, position ASC",
    )?;

    let mut rows = stmt.query([])?;
    let mut memberships: HashMap<i64, HashSet<i64>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let playlist_id: i64 = row.get(0)?;
        let track_id: i64 = row.get(1)?;
        memberships.entry(playlist_id).or_default().insert(track_id);
    }

    Ok(memberships)
}

// ─── Genres ───────────────────────────────────────────────

pub fn get_genres(conn: &Connection) -> Result<Vec<Genre>> {
    let mut stmt = conn.prepare(
        "SELECT g.id, g.name, g.slug, g.parent_id, COUNT(tg.track_id) AS track_count
         FROM genres g
         LEFT JOIN track_genres tg ON tg.genre_id = g.id
         GROUP BY g.id, g.name, g.slug, g.parent_id
         ORDER BY COALESCE(g.parent_id, g.id), g.name ASC",
    )?;

    let genres = stmt
        .query_map([], genre_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(genres)
}

pub fn get_genre_tree(conn: &Connection) -> Result<Vec<Genre>> {
    let genres = get_genres(conn)?;
    Ok(build_genre_tree(genres))
}

pub fn get_genre_heat(conn: &Connection, days: i64) -> Result<Vec<GenreHeat>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE closure(ancestor_id, genre_id) AS (
            SELECT id, id
            FROM genres
            UNION ALL
            SELECT closure.ancestor_id, g.id
            FROM closure
            JOIN genres g ON g.parent_id = closure.genre_id
        )
        SELECT
            g.id,
            g.name,
            COUNT(lh.id) AS listen_count,
            COALESCE(SUM(lh.duration_listened_ms), 0) AS total_listened_ms
        FROM genres g
        LEFT JOIN closure ON closure.ancestor_id = g.id
        LEFT JOIN track_genres tg ON tg.genre_id = closure.genre_id
        LEFT JOIN listen_history lh
            ON lh.track_id = tg.track_id
           AND lh.started_at >= datetime('now', printf('-%d days', ?1))
        GROUP BY g.id, g.name
        ORDER BY COALESCE(g.parent_id, g.id), g.name ASC",
    )?;

    let heat = stmt
        .query_map(params![days.max(1)], |row| {
            Ok(GenreHeat {
                genre_id: row.get(0)?,
                genre_name: row.get(1)?,
                listen_count: row.get(2)?,
                total_listened_ms: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(heat)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenreSummary {
    pub genre_id: i64,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<i64>,
    pub direct_track_count: i64,
    pub total_track_count: i64,
    pub child_count: usize,
}

pub fn get_genre_summary(conn: &Connection, genre_id: i64) -> Result<Option<GenreSummary>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE selected_genres(id) AS (
            SELECT id FROM genres WHERE id = ?1
            UNION ALL
            SELECT g.id
            FROM genres g
            JOIN selected_genres sg ON g.parent_id = sg.id
        )
        SELECT
            g.id,
            g.name,
            g.slug,
            g.parent_id,
            COUNT(DISTINCT tg.track_id) AS direct_track_count,
            (
                SELECT COUNT(DISTINCT tg2.track_id)
                FROM selected_genres sg2
                JOIN track_genres tg2 ON tg2.genre_id = sg2.id
            ) AS total_track_count,
            (
                SELECT COUNT(*)
                FROM genres child
                WHERE child.parent_id = g.id
            ) AS child_count
        FROM genres g
        LEFT JOIN track_genres tg ON tg.genre_id = g.id
        WHERE g.id = ?1
        GROUP BY g.id, g.name, g.slug, g.parent_id",
    )?;

    let mut rows = stmt.query(params![genre_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(GenreSummary {
            genre_id: row.get(0)?,
            name: row.get(1)?,
            slug: row.get(2)?,
            parent_id: row.get(3)?,
            direct_track_count: row.get(4)?,
            total_track_count: row.get(5)?,
            child_count: row.get(6)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn get_genre_path(conn: &Connection, genre_id: i64) -> Result<Vec<Genre>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE ancestry(id, name, slug, parent_id, depth) AS (
            SELECT id, name, slug, parent_id, 0
            FROM genres
            WHERE id = ?1
            UNION ALL
            SELECT g.id, g.name, g.slug, g.parent_id, ancestry.depth + 1
            FROM genres g
            JOIN ancestry ON ancestry.parent_id = g.id
        )
        SELECT id, name, slug, parent_id, 0 AS child_count
        FROM ancestry
        ORDER BY depth DESC",
    )?;

    let path = stmt
        .query_map(params![genre_id], |row| {
            Ok(Genre {
                id: row.get(0)?,
                name: row.get(1)?,
                slug: row.get(2)?,
                parent_id: row.get(3)?,
                children: Vec::new(),
                track_count: Some(0),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(path)
}

pub fn get_tracks_by_genre(
    conn: &Connection,
    genre_id: i64,
    include_descendants: bool,
) -> Result<Vec<Track>> {
    if !genre_exists(conn, genre_id)? {
        return Ok(Vec::new());
    }

    // Filter rule: if a track has any Spotify tags, Spotify wins (only Spotify
    // decides genre membership for that track). If a track has no Spotify tags,
    // any source is accepted. This trusts Spotify over MusicBrainz on conflicts
    // while still showing tracks that only one source has data on.
    let sql = if include_descendants {
        "WITH RECURSIVE selected_genres(id) AS (
            SELECT id FROM genres WHERE id = ?1
            UNION ALL
            SELECT g.id
            FROM genres g
            JOIN selected_genres sg ON g.parent_id = sg.id
        )
        SELECT DISTINCT t.id, t.title, t.artist_id, a.name, t.album_id, al.title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM selected_genres sg
         JOIN track_genres tg ON tg.genre_id = sg.id
         JOIN tracks t ON tg.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE (
             NOT EXISTS (
                 SELECT 1 FROM track_genres tg_sp
                 WHERE tg_sp.track_id = t.id AND tg_sp.source = 'spotify'
             )
             OR EXISTS (
                 SELECT 1 FROM track_genres tg_sp
                 WHERE tg_sp.track_id = t.id
                   AND tg_sp.source = 'spotify'
                   AND tg_sp.genre_id IN (SELECT id FROM selected_genres)
             )
         )
         ORDER BY
            COALESCE(a.name, '') COLLATE NOCASE ASC,
            COALESCE(al.title, '') COLLATE NOCASE ASC,
            COALESCE(t.disc_number, 1) ASC,
            COALESCE(t.track_number, 999999) ASC,
            t.title COLLATE NOCASE ASC"
    } else {
        "SELECT DISTINCT t.id, t.title, t.artist_id, a.name, t.album_id, al.title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM track_genres tg
         JOIN tracks t ON tg.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE tg.genre_id = ?1
           AND (
               NOT EXISTS (
                   SELECT 1 FROM track_genres tg_sp
                   WHERE tg_sp.track_id = t.id AND tg_sp.source = 'spotify'
               )
               OR EXISTS (
                   SELECT 1 FROM track_genres tg_sp
                   WHERE tg_sp.track_id = t.id
                     AND tg_sp.source = 'spotify'
                     AND tg_sp.genre_id = ?1
               )
           )
         ORDER BY
            COALESCE(a.name, '') COLLATE NOCASE ASC,
            COALESCE(al.title, '') COLLATE NOCASE ASC,
            COALESCE(t.disc_number, 1) ASC,
            COALESCE(t.track_number, 999999) ASC,
            t.title COLLATE NOCASE ASC"
    };

    let mut stmt = conn.prepare(sql)?;
    let tracks = stmt
        .query_map(params![genre_id], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tracks)
}

pub fn genre_exists(conn: &Connection, genre_id: i64) -> Result<bool> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM genres WHERE id = ?1)",
        params![genre_id],
        |row| row.get(0),
    )?;
    Ok(exists)
}

pub fn count_genre_tracks(
    conn: &Connection,
    genre_id: i64,
    include_descendants: bool,
) -> Result<i64> {
    if include_descendants {
        conn.query_row(
            "WITH RECURSIVE selected_genres(id) AS (
                SELECT id FROM genres WHERE id = ?1
                UNION ALL
                SELECT g.id
                FROM genres g
                JOIN selected_genres sg ON g.parent_id = sg.id
            )
            SELECT COUNT(DISTINCT tg.track_id)
            FROM selected_genres sg
            JOIN track_genres tg ON tg.genre_id = sg.id",
            params![genre_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    } else {
        conn.query_row(
            "SELECT COUNT(DISTINCT track_id)
             FROM track_genres
             WHERE genre_id = ?1",
            params![genre_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }
}

pub fn assign_genre_to_tracks(
    conn: &Connection,
    genre_id: i64,
    track_ids: &[i64],
    source: &str,
) -> Result<usize> {
    if track_ids.is_empty() {
        return Ok(0);
    }

    let exists: Option<i64> = conn
        .query_row(
            "SELECT id FROM genres WHERE id = ?1",
            params![genre_id],
            |row| row.get(0),
        )
        .ok();
    if exists.is_none() {
        anyhow::bail!("genre not found");
    }

    let mut affected = 0;
    for track_id in track_ids {
        affected += conn.execute(
            "INSERT OR REPLACE INTO track_genres (track_id, genre_id, source, confidence)
             VALUES (?1, ?2, ?3, 1.0)",
            params![track_id, genre_id, source],
        )?;
    }

    Ok(affected)
}

pub fn replace_track_source_genres(
    conn: &Connection,
    track_id: i64,
    canonical_names: &[String],
    source: &str,
    confidence: f64,
) -> Result<usize> {
    conn.execute(
        "DELETE FROM track_genres WHERE track_id = ?1 AND source = ?2",
        params![track_id, source],
    )?;

    if canonical_names.is_empty() {
        return Ok(0);
    }

    let mut affected = 0usize;
    for canonical_name in canonical_names {
        let genre_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM genres WHERE name = ?1",
                params![canonical_name],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(genre_id) = genre_id {
            conn.execute(
                "INSERT OR REPLACE INTO track_genres (track_id, genre_id, source, confidence)
                 VALUES (?1, ?2, ?3, ?4)",
                params![track_id, genre_id, source, confidence],
            )?;
            affected += 1;
        }
    }

    Ok(affected)
}

pub fn get_track_tidal_ids(conn: &Connection, track_ids: &[i64]) -> Result<Vec<(i64, i64)>> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }

    let sql = format!(
        "SELECT id, tidal_id
         FROM tracks
         WHERE id IN ({})
           AND tidal_id IS NOT NULL",
        placeholders(track_ids.len())
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(track_ids.iter().copied()), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_album_tidal_ids(conn: &Connection, album_ids: &[i64]) -> Result<Vec<(i64, i64)>> {
    if album_ids.is_empty() {
        return Ok(Vec::new());
    }

    let sql = format!(
        "SELECT id, tidal_id
         FROM albums
         WHERE id IN ({})
           AND tidal_id IS NOT NULL",
        placeholders(album_ids.len())
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(album_ids.iter().copied()), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_analytics_overview(conn: &Connection) -> Result<AnalyticsOverview> {
    Ok(AnalyticsOverview {
        tracks: conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?,
        albums: conn.query_row("SELECT COUNT(*) FROM albums", [], |row| row.get(0))?,
        artists: conn.query_row("SELECT COUNT(*) FROM artists", [], |row| row.get(0))?,
        playlists: conn.query_row("SELECT COUNT(*) FROM playlists", [], |row| row.get(0))?,
        smart_playlists: conn.query_row(
            "SELECT COUNT(*) FROM playlists WHERE is_smart = 1",
            [],
            |row| row.get(0),
        )?,
        tagged_tracks: conn.query_row(
            "SELECT COUNT(DISTINCT track_id) FROM track_genres",
            [],
            |row| row.get(0),
        )?,
        total_listens: conn
            .query_row("SELECT COUNT(*) FROM listen_history", [], |row| row.get(0))?,
        favorite_tracks: conn.query_row(
            "SELECT COUNT(*) FROM tracks WHERE is_favorite = 1",
            [],
            |row| row.get(0),
        )?,
    })
}

pub fn record_listen_history(
    conn: &Connection,
    track_id: i64,
    started_at: &str,
    duration_listened_ms: i64,
    completed: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO listen_history (track_id, started_at, duration_listened_ms, completed)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            track_id,
            started_at,
            duration_listened_ms.max(0),
            completed as i32
        ],
    )?;
    Ok(())
}

pub fn increment_track_play_summary(
    conn: &Connection,
    track_id: i64,
    started_at: &str,
    completed: bool,
) -> Result<()> {
    if completed {
        conn.execute(
            "UPDATE tracks
             SET play_count = play_count + 1,
                 last_played_at = ?2
             WHERE id = ?1",
            params![track_id, started_at],
        )?;
    }
    Ok(())
}

pub fn get_recent_listens(conn: &Connection, limit: i64) -> Result<Vec<ListenHistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT lh.id, lh.track_id, t.title, a.name, al.title, al.artwork_url,
                lh.started_at, COALESCE(lh.duration_listened_ms, 0), lh.completed
         FROM listen_history lh
         JOIN tracks t ON lh.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         ORDER BY lh.started_at DESC, lh.id DESC
         LIMIT ?1",
    )?;

    let listens = stmt
        .query_map(params![limit], |row| {
            Ok(ListenHistoryEntry {
                id: row.get(0)?,
                track_id: row.get(1)?,
                track_title: row.get(2)?,
                artist_name: row.get(3)?,
                album_title: row.get(4)?,
                artwork_url: row.get(5)?,
                started_at: row.get(6)?,
                duration_listened_ms: row.get(7)?,
                completed: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(listens)
}

pub fn get_top_tracks_by_history(conn: &Connection, limit: i64) -> Result<Vec<AnalyticsTopTrack>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url,
                COUNT(lh.id) AS listens,
                COALESCE(SUM(CASE WHEN lh.completed = 1 THEN 1 ELSE 0 END), 0) AS completed_listens,
                COALESCE(SUM(lh.duration_listened_ms), 0) AS total_listened_ms
         FROM listen_history lh
         JOIN tracks t ON lh.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         GROUP BY t.id, t.title, a.name, al.title, al.artwork_url
         ORDER BY listens DESC, total_listened_ms DESC, t.title ASC
         LIMIT ?1",
    )?;

    let rows = stmt
        .query_map(params![limit.max(1)], |row| {
            Ok(AnalyticsTopTrack {
                track_id: row.get(0)?,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                album_title: row.get(3)?,
                artwork_url: row.get(4)?,
                listens: row.get(5)?,
                completed_listens: row.get(6)?,
                total_listened_ms: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

pub fn get_top_artists_by_history(
    conn: &Connection,
    limit: i64,
) -> Result<Vec<AnalyticsTopArtist>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.name,
                COUNT(lh.id) AS listens,
                COALESCE(SUM(CASE WHEN lh.completed = 1 THEN 1 ELSE 0 END), 0) AS completed_listens,
                COUNT(DISTINCT t.id) AS unique_tracks,
                COALESCE(SUM(lh.duration_listened_ms), 0) AS total_listened_ms
         FROM listen_history lh
         JOIN tracks t ON lh.track_id = t.id
         JOIN artists a ON t.artist_id = a.id
         GROUP BY a.id, a.name
         ORDER BY listens DESC, total_listened_ms DESC, a.name ASC
         LIMIT ?1",
    )?;

    let rows = stmt
        .query_map(params![limit.max(1)], |row| {
            Ok(AnalyticsTopArtist {
                artist_id: row.get(0)?,
                artist_name: row.get(1)?,
                listens: row.get(2)?,
                completed_listens: row.get(3)?,
                unique_tracks: row.get(4)?,
                total_listened_ms: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

pub fn get_top_genres_by_history(
    conn: &Connection,
    limit: i64,
) -> Result<Vec<AnalyticsGenreShare>> {
    let mut stmt = conn.prepare(
        "SELECT g.name,
                COUNT(lh.id) AS listens
         FROM listen_history lh
         JOIN track_genres tg ON lh.track_id = tg.track_id
         JOIN genres g ON tg.genre_id = g.id
         GROUP BY g.id, g.name
         ORDER BY listens DESC, g.name ASC
         LIMIT ?1",
    )?;

    let rows = stmt
        .query_map(params![limit.max(1)], |row| {
            Ok(AnalyticsGenreShare {
                genre_name: row.get(0)?,
                listens: row.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

pub fn get_listen_activity(conn: &Connection, days: i64) -> Result<Vec<AnalyticsActivityPoint>> {
    let mut stmt = conn.prepare(
        "SELECT DATE(started_at) AS day,
                COUNT(*) AS listens,
                COALESCE(SUM(CASE WHEN completed = 1 THEN 1 ELSE 0 END), 0) AS completed_listens,
                COALESCE(SUM(duration_listened_ms), 0) AS listened_ms
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY DATE(started_at)
         ORDER BY day ASC",
    )?;

    let rows = stmt
        .query_map(params![days.max(1)], |row| {
            Ok(AnalyticsActivityPoint {
                day: row.get(0)?,
                listens: row.get(1)?,
                completed_listens: row.get(2)?,
                listened_ms: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

pub fn get_behavior_metrics(conn: &Connection) -> Result<AnalyticsBehavior> {
    let (total_listened_ms, total_listens, completed_listens, unique_tracks, active_days): (
        Option<i64>,
        i64,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    ) = conn.query_row(
        "SELECT
            COALESCE(SUM(duration_listened_ms), 0),
            COUNT(*),
            COALESCE(SUM(CASE WHEN completed = 1 THEN 1 ELSE 0 END), 0),
            COUNT(DISTINCT track_id),
            COUNT(DISTINCT DATE(started_at))
         FROM listen_history",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let repeat_track_count: Option<i64> = conn.query_row(
        "SELECT COUNT(*)
         FROM (
            SELECT track_id
            FROM listen_history
            GROUP BY track_id
            HAVING COUNT(*) > 1
         )",
        [],
        |row| row.get(0),
    )?;

    let total_listened_ms = total_listened_ms.unwrap_or(0);
    let completed_listens = completed_listens.unwrap_or(0);
    let unique_tracks = unique_tracks.unwrap_or(0);
    let repeat_track_count = repeat_track_count.unwrap_or(0);
    let active_days = active_days.unwrap_or(0);
    let skipped_listens = total_listens.saturating_sub(completed_listens);
    let completion_rate = if total_listens == 0 {
        0.0
    } else {
        completed_listens as f64 / total_listens as f64
    };
    let average_listen_ms = if total_listens == 0 {
        0
    } else {
        total_listened_ms / total_listens
    };

    Ok(AnalyticsBehavior {
        total_listened_ms,
        total_listens,
        completed_listens,
        skipped_listens,
        completion_rate,
        average_listen_ms,
        unique_tracks,
        repeat_track_count,
        active_days,
    })
}

// ─── Sync Metadata ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncInfo {
    pub service: String,
    pub last_sync_at: String,
    pub auto_sync_daily: bool,
    pub last_sync_track_count: i64,
    pub last_sync_album_count: i64,
}

pub fn get_sync_info(conn: &Connection, service: &str) -> Result<Option<SyncInfo>> {
    let mut stmt = conn.prepare(
        "SELECT service, last_sync_at, auto_sync_daily, last_sync_track_count, last_sync_album_count
         FROM sync_metadata WHERE service = ?1",
    )?;
    let result = stmt.query_row([service], |row| {
        Ok(SyncInfo {
            service: row.get(0)?,
            last_sync_at: row.get(1)?,
            auto_sync_daily: row.get::<_, i64>(2)? != 0,
            last_sync_track_count: row.get(3)?,
            last_sync_album_count: row.get(4)?,
        })
    }).optional()?;
    Ok(result)
}

pub fn update_sync_timestamp(conn: &Connection, service: &str, track_count: i64, album_count: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_metadata (service, last_sync_at, auto_sync_daily, last_sync_track_count, last_sync_album_count)
         VALUES (?1, datetime('now'), 0, ?2, ?3)
         ON CONFLICT(service) DO UPDATE SET
             last_sync_at = datetime('now'),
             last_sync_track_count = ?2,
             last_sync_album_count = ?3",
        rusqlite::params![service, track_count, album_count],
    )?;
    Ok(())
}

pub fn set_auto_sync_daily(conn: &Connection, service: &str, enabled: bool) -> Result<()> {
    conn.execute(
        "UPDATE sync_metadata SET auto_sync_daily = ?1 WHERE service = ?2",
        rusqlite::params![if enabled { 1 } else { 0 }, service],
    )?;
    Ok(())
}

pub fn get_auto_sync_services(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT service FROM sync_metadata WHERE auto_sync_daily = 1")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

// ─── Genre Co-Occurrence (co-listening pairs) ────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreCoOccurrence {
    pub genre_a_id: i64,
    pub genre_a_name: String,
    pub genre_b_id: i64,
    pub genre_b_name: String,
    pub co_listen_count: i64,
    pub jaccard: f64,
}

/// Find genre-genre pairs that are co-listened within the same session window.
/// Two genres "co-occur" if a user listened to tracks from both genres within
/// `window_minutes` of each other (default 30 min). Returns pairs with at least
/// `min_count` co-occurrences, sorted by Jaccard similarity.
pub fn get_genre_co_occurrence(
    conn: &Connection,
    _days: i64,
    _window_minutes: i64,
    min_count: i64,
) -> Result<Vec<GenreCoOccurrence>> {
    // Find genre pairs that appear together on the same tracks
    // (co-tagged genres), then score by Jaccard similarity.
    let mut stmt = conn.prepare(
        "WITH track_genre_pairs AS (
            SELECT a.genre_id AS genre_a, b.genre_id AS genre_b
            FROM track_genres a
            JOIN track_genres b ON b.track_id = a.track_id AND b.genre_id > a.genre_id
        ),
        pair_counts AS (
            SELECT genre_a, genre_b, COUNT(*) AS co_count
            FROM track_genre_pairs
            GROUP BY genre_a, genre_b
            HAVING co_count >= ?1
        ),
        genre_totals AS (
            SELECT genre_id, COUNT(DISTINCT track_id) AS total_tracks
            FROM track_genres
            GROUP BY genre_id
        )
        SELECT
            ga.id, ga.name,
            gb.id, gb.name,
            pc.co_count,
            CAST(pc.co_count AS REAL) /
                MAX(1, gt_a.total_tracks + gt_b.total_tracks - pc.co_count) AS jaccard
        FROM pair_counts pc
        JOIN genres ga ON ga.id = pc.genre_a
        JOIN genres gb ON gb.id = pc.genre_b
        JOIN genre_totals gt_a ON gt_a.genre_id = pc.genre_a
        JOIN genre_totals gt_b ON gt_b.genre_id = pc.genre_b
        ORDER BY jaccard DESC, pc.co_count DESC",
    )?;

    let rows = stmt.query_map(
        params![min_count],
        |row| {
            Ok(GenreCoOccurrence {
                genre_a_id: row.get(0)?,
                genre_a_name: row.get(1)?,
                genre_b_id: row.get(2)?,
                genre_b_name: row.get(3)?,
                co_listen_count: row.get(4)?,
                jaccard: row.get(5)?,
            })
        },
    )?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

// ─── Genre Cohorts (personal clusters from time-based listening) ─────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreCohort {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub genre_ids: Vec<i64>,
    pub listen_count: i64,
    pub total_listened_ms: i64,
}

/// Derive personal listening cohorts by analyzing time-of-day and day-of-week
/// patterns. Groups genres into clusters like "Late Night", "Morning Commute",
/// "Weekend", "Deep Focus", etc.
pub fn get_genre_cohorts(conn: &Connection, days: i64) -> Result<Vec<GenreCohort>> {
    // We bucket listens into 4 time-of-day slots + weekend/weekday
    // Slot 0: 0-6 (Night), Slot 1: 6-12 (Morning), Slot 2: 12-18 (Afternoon), Slot 3: 18-24 (Evening)
    // Then find genres that dominate each slot.
    let mut stmt = conn.prepare(
        "WITH recent AS (
            SELECT
                lh.id AS listen_id,
                lh.track_id,
                lh.started_at,
                lh.duration_listened_ms,
                CAST(strftime('%H', lh.started_at) AS INTEGER) AS hour,
                CAST(strftime('%w', lh.started_at) AS INTEGER) AS dow
            FROM listen_history lh
            WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
        ),
        genre_buckets AS (
            SELECT
                tg.genre_id,
                g.name AS genre_name,
                CASE
                    WHEN r.hour < 6 THEN 'night'
                    WHEN r.hour < 12 THEN 'morning'
                    WHEN r.hour < 18 THEN 'afternoon'
                    ELSE 'evening'
                END AS time_slot,
                CASE
                    WHEN r.dow = 0 OR r.dow = 6 THEN 'weekend'
                    ELSE 'weekday'
                END AS day_type,
                COUNT(*) AS listens,
                COALESCE(SUM(r.duration_listened_ms), 0) AS listened_ms
            FROM recent r
            JOIN track_genres tg ON tg.track_id = r.track_id
            JOIN genres g ON g.id = tg.genre_id
            GROUP BY tg.genre_id, time_slot, day_type
        ),
        dominant AS (
            SELECT
                genre_id,
                genre_name,
                time_slot,
                day_type,
                listens,
                listened_ms,
                ROW_NUMBER() OVER (PARTITION BY genre_id ORDER BY listens DESC) AS rn
            FROM genre_buckets
        )
        SELECT genre_id, genre_name, time_slot, day_type, listens, listened_ms
        FROM dominant
        WHERE rn = 1
        ORDER BY listens DESC",
    )?;

    let rows = stmt.query_map(params![days], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
        ))
    })?;

    let entries: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;

    // Build cohorts from the dominant assignments
    let mut cohort_map: std::collections::HashMap<String, GenreCohort> =
        std::collections::HashMap::new();

    for (genre_id, _genre_name, time_slot, day_type, listens, listened_ms) in entries {
        let (id, label, icon) = match (time_slot.as_str(), day_type.as_str()) {
            ("night", _) => ("night_owl", "Night Owl", "🌙"),
            ("morning", "weekday") => ("morning_commute", "Morning Commute", "☀"),
            ("morning", "weekend") => ("lazy_morning", "Weekend Morning", "🌤"),
            ("afternoon", "weekday") => ("afternoon_drift", "Afternoon Drift", "☁"),
            ("afternoon", "weekend") => ("weekend_afternoon", "Weekend Afternoon", "🌿"),
            ("evening", "weekday") => ("evening_wind_down", "Evening Wind-Down", "🌆"),
            ("evening", "weekend") => ("weekend_evening", "Weekend Evening", "🎶"),
            _ => ("other", "Other", "✦"),
        };

        let cohort = cohort_map
            .entry(id.to_string())
            .or_insert_with(|| GenreCohort {
                id: id.to_string(),
                label: label.to_string(),
                icon: icon.to_string(),
                genre_ids: vec![],
                listen_count: 0,
                total_listened_ms: 0,
            });

        cohort.genre_ids.push(genre_id);
        cohort.listen_count += listens;
        cohort.total_listened_ms += listened_ms;
    }

    let mut cohorts: Vec<_> = cohort_map.into_values().collect();
    cohorts.sort_by(|a, b| b.listen_count.cmp(&a.listen_count));

    Ok(cohorts)
}

/// Map track IDs to their dominant cohort (id, label) using `get_genre_cohorts`.
/// Each genre belongs to at most one cohort (enforced by `get_genre_cohorts`).
/// For a track tagged with multiple genres mapped to *different* cohorts, the
/// helper picks the first matching genre row returned by SQLite (no `ORDER BY`),
/// which is effectively undefined order. Acceptable for now since cohorts are a
/// soft signal; revisit if cohort labels need to be deterministic per track.
pub fn get_track_cohort_assignments(
    conn: &Connection,
    track_ids: &[i64],
    days: i64,
) -> Result<std::collections::HashMap<i64, (String, String)>> {
    if track_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let cohorts = get_genre_cohorts(conn, days)?;
    if cohorts.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    // Build genre_id → (cohort_id, cohort_label), preferring earlier (higher-rank) cohorts.
    let mut genre_to_cohort: std::collections::HashMap<i64, (String, String)> =
        std::collections::HashMap::new();
    for cohort in &cohorts {
        for gid in &cohort.genre_ids {
            genre_to_cohort
                .entry(*gid)
                .or_insert((cohort.id.clone(), cohort.label.clone()));
        }
    }

    // Pull all (track_id, genre_id) pairs for the requested tracks.
    let ids_csv: String = track_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT track_id, genre_id FROM track_genres WHERE track_id IN ({ids_csv})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut assignments: std::collections::HashMap<i64, (String, String)> =
        std::collections::HashMap::new();
    for r in rows {
        let (track_id, genre_id) = r?;
        if assignments.contains_key(&track_id) {
            continue;
        }
        if let Some(pair) = genre_to_cohort.get(&genre_id) {
            assignments.insert(track_id, pair.clone());
        }
    }

    Ok(assignments)
}

// ─── Genre Evolution (time-sliced heat for temporal trails) ──────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreEvolutionPoint {
    pub genre_id: i64,
    pub genre_name: String,
    pub period_start: String,
    pub listen_count: i64,
    pub total_listened_ms: i64,
}

/// Return genre heat broken into weekly time slices over the past N days.
/// Each (genre_id, week_start) pair is one evolution point.
pub fn get_genre_evolution(conn: &Connection, days: i64) -> Result<Vec<GenreEvolutionPoint>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE closure(ancestor_id, genre_id) AS (
            SELECT id, id FROM genres
            UNION ALL
            SELECT closure.ancestor_id, g.id
            FROM closure JOIN genres g ON g.parent_id = closure.genre_id
        ),
        weekly AS (
            SELECT
                tg.genre_id,
                g.name AS genre_name,
                date(lh.started_at, 'weekday 0', '-6 days') AS period_start,
                COUNT(DISTINCT lh.id) AS listen_count,
                COALESCE(SUM(lh.duration_listened_ms), 0) AS total_listened_ms
            FROM listen_history lh
            JOIN track_genres tg ON tg.track_id = lh.track_id
            JOIN genres g ON g.id = tg.genre_id
            WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
            GROUP BY tg.genre_id, period_start
        )
        SELECT genre_id, genre_name, period_start, listen_count, total_listened_ms
        FROM weekly
        ORDER BY genre_id, period_start",
    )?;

    let rows = stmt.query_map(params![days], |row| {
        Ok(GenreEvolutionPoint {
            genre_id: row.get(0)?,
            genre_name: row.get(1)?,
            period_start: row.get(2)?,
            listen_count: row.get(3)?,
            total_listened_ms: row.get(4)?,
        })
    })?;

    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_discovery_candidate_tracks(conn: &Connection, limit: i64) -> Result<Vec<Track>> {
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
         ORDER BY t.is_favorite DESC, t.play_count DESC, t.date_added DESC, t.title ASC
         LIMIT ?1",
    )?;

    let tracks = stmt
        .query_map(params![limit.max(1)], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

pub fn get_tracks_excluding(conn: &Connection, excluded_track_ids: &[i64]) -> Result<Vec<Track>> {
    get_tracks_excluding_with_limit(conn, excluded_track_ids, 0)
}

/// Variant with an optional LIMIT for automix candidate selection.
/// When `max_candidates` > 0, only the top N tracks (by the default ordering) are returned,
/// dramatically reducing memory usage for automix which would otherwise load all 32k tracks.
pub fn get_tracks_excluding_with_limit(
    conn: &Connection,
    excluded_track_ids: &[i64],
    max_candidates: usize,
) -> Result<Vec<Track>> {
    let mut sql = String::from(
        "SELECT t.id, t.title, t.artist_id, a.name, t.album_id, al.title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id",
    );

    if !excluded_track_ids.is_empty() {
        sql.push_str(" WHERE t.id NOT IN (");
        sql.push_str(&placeholders(excluded_track_ids.len()));
        sql.push(')');
    }

    sql.push_str(" ORDER BY t.is_favorite DESC, t.play_count ASC, t.fidelity_score DESC, t.date_added DESC, t.title ASC");

    if max_candidates > 0 {
        sql.push_str(&format!(" LIMIT {}", max_candidates));
    }

    let mut stmt = conn.prepare(&sql)?;
    let params = params_from_iter(excluded_track_ids.iter().copied());
    let tracks = stmt
        .query_map(params, track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

pub fn get_existing_tidal_track_ids(conn: &Connection, tidal_ids: &[i64]) -> Result<HashSet<i64>> {
    if tidal_ids.is_empty() {
        return Ok(HashSet::new());
    }

    let placeholders = placeholders(tidal_ids.len());
    let query = format!(
        "SELECT tidal_id
         FROM tracks
         WHERE tidal_id IN ({placeholders})"
    );
    let params = params_from_iter(tidal_ids.iter().copied());
    let mut stmt = conn.prepare(&query)?;
    let ids = stmt
        .query_map(params, |row| row.get::<_, i64>(0))?
        .collect::<Result<HashSet<_>, _>>()?;

    Ok(ids)
}

pub fn list_discovery_presets(conn: &Connection) -> Result<Vec<DiscoveryPreset>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, words, mode, services, created_at
         FROM discovery_presets
         ORDER BY created_at DESC, id DESC",
    )?;

    let presets = stmt
        .query_map([], |row| {
            let services_raw: String = row.get(4)?;
            Ok(DiscoveryPreset {
                id: row.get(0)?,
                name: row.get(1)?,
                prompt: row.get(2)?,
                mode: row.get(3)?,
                services: parse_discovery_services(&services_raw),
                created_at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(presets)
}

pub fn create_discovery_preset(
    conn: &Connection,
    name: &str,
    prompt: &str,
    mode: &str,
    services_json: &str,
) -> Result<DiscoveryPreset> {
    conn.execute(
        "INSERT INTO discovery_presets (name, words, mode, services)
         VALUES (?1, ?2, ?3, ?4)",
        params![name, prompt, mode, services_json],
    )?;

    let id = conn.last_insert_rowid();
    let created_at: String = conn.query_row(
        "SELECT created_at FROM discovery_presets WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;

    Ok(DiscoveryPreset {
        id,
        name: name.to_string(),
        prompt: prompt.to_string(),
        mode: mode.to_string(),
        services: parse_discovery_services(services_json),
        created_at,
    })
}

pub fn cache_discovery_results(
    conn: &Connection,
    preset_id: Option<i64>,
    results: &[DiscoveryPreviewResult],
) -> Result<()> {
    for result in results {
        conn.execute(
            "INSERT INTO discovery_results (
                preset_id, track_title, artist_name, service, service_track_id,
                relevance_score, preview_url
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                preset_id,
                result.title,
                result.artist_name,
                result.service,
                result.service_track_id,
                result.score as f64 / 100.0,
                result.artwork_url,
            ],
        )?;
    }

    Ok(())
}

fn parse_discovery_services(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw)
        .ok()
        .filter(|values| !values.is_empty())
        .or_else(|| {
            serde_json::from_str::<Value>(raw)
                .ok()
                .and_then(|value| match value {
                    Value::String(single) => Some(vec![single]),
                    _ => None,
                })
        })
        .unwrap_or_else(|| vec!["tidal".to_string()])
}

// ─── Search (FTS5) ────────────────────────────────────────

pub fn search(conn: &Connection, query: &str, limit: i64) -> Result<SearchResults> {
    let normalized = query.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(SearchResults {
            tracks: Vec::new(),
            albums: Vec::new(),
            artists: Vec::new(),
        });
    }

    let contains_pattern = format!("%{normalized}%");
    let prefix_pattern = format!("{normalized}%");
    let limit = limit.max(1);

    // Search tracks across track title, artist name, and album title.
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
         WHERE LOWER(t.title) LIKE ?1
            OR LOWER(COALESCE(a.name, '')) LIKE ?1
            OR LOWER(COALESCE(al.title, '')) LIKE ?1
         ORDER BY
            CASE
                WHEN LOWER(COALESCE(a.name, '')) = ?2 THEN 0
                WHEN LOWER(t.title) = ?2 THEN 1
                WHEN LOWER(COALESCE(al.title, '')) = ?2 THEN 2
                WHEN LOWER(COALESCE(a.name, '')) LIKE ?3 THEN 3
                WHEN LOWER(t.title) LIKE ?3 THEN 4
                WHEN LOWER(COALESCE(al.title, '')) LIKE ?3 THEN 5
                ELSE 6
            END,
            t.is_favorite DESC,
            t.play_count DESC,
            t.fidelity_score DESC,
            t.title ASC
         LIMIT ?4",
    )?;

    let tracks = stmt
        .query_map(
            params![contains_pattern, normalized, prefix_pattern, limit],
            track_from_row,
        )?
        .collect::<Result<Vec<_>, _>>()?;

    // Search artists.
    let mut stmt = conn.prepare(
        "SELECT a.id, a.tidal_id, a.ytmusic_id, a.soundcloud_id,
                a.name, a.name_sort, a.biography, a.photo_url
         FROM artists a
         WHERE LOWER(a.name) LIKE ?1
         ORDER BY
            CASE
                WHEN LOWER(a.name) = ?2 THEN 0
                WHEN LOWER(a.name) LIKE ?3 THEN 1
                ELSE 2
            END,
            a.name ASC
         LIMIT ?4",
    )?;

    let artists = stmt
        .query_map(
            params![contains_pattern, normalized, prefix_pattern, limit],
            |row| {
                Ok(Artist {
                    id: row.get(0)?,
                    tidal_id: row.get(1)?,
                    ytmusic_id: row.get(2)?,
                    soundcloud_id: row.get(3)?,
                    name: row.get(4)?,
                    name_sort: row.get(5)?,
                    biography: row.get(6)?,
                    photo_url: row.get(7)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;

    // Search albums across album title and artist name.
    let mut stmt = conn.prepare(
        "SELECT al.id, al.tidal_id, al.ytmusic_id, al.title, al.artist_id,
                a.name, al.year, al.artwork_url,
                al.release_type, al.label, al.track_count, al.is_favorite, al.source
         FROM albums al
         LEFT JOIN artists a ON al.artist_id = a.id
         WHERE LOWER(al.title) LIKE ?1
            OR LOWER(COALESCE(a.name, '')) LIKE ?1
         ORDER BY
            CASE
                WHEN LOWER(COALESCE(a.name, '')) = ?2 THEN 0
                WHEN LOWER(al.title) = ?2 THEN 1
                WHEN LOWER(COALESCE(a.name, '')) LIKE ?3 THEN 2
                WHEN LOWER(al.title) LIKE ?3 THEN 3
                ELSE 4
            END,
            al.is_favorite DESC,
            al.year DESC,
            al.title ASC
         LIMIT ?4",
    )?;

    let albums = stmt
        .query_map(
            params![contains_pattern, normalized, prefix_pattern, limit],
            |row| {
                Ok(Album {
                    id: row.get(0)?,
                    tidal_id: row.get(1)?,
                    ytmusic_id: row.get(2)?,
                    title: row.get(3)?,
                    artist_id: row.get(4)?,
                    artist_name: row.get(5)?,
                    year: row.get(6)?,
                    artwork_url: row.get(7)?,
                    release_type: row.get(8)?,
                    label: row.get(9)?,
                    track_count: row.get(10)?,
                    is_favorite: row.get::<_, i32>(11)? != 0,
                    source: row.get(12)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(SearchResults {
        tracks,
        albums,
        artists,
    })
}

fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        title: row.get(1)?,
        artist_id: row.get(2)?,
        artist_name: row.get(3)?,
        album_id: row.get(4)?,
        album_title: row.get(5)?,
        disc_number: row.get(6)?,
        track_number: row.get(7)?,
        duration_ms: row.get(8)?,
        isrc: row.get(9)?,
        tidal_id: row.get(10)?,
        ytmusic_id: row.get(11)?,
        soundcloud_id: row.get(12)?,
        best_quality: row.get(13)?,
        best_source: row.get(14)?,
        fidelity_score: row.get(15)?,
        is_favorite: row.get(16)?,
        play_count: row.get(17)?,
        last_played_at: row.get(18)?,
        date_added: row.get(19)?,
        source: row.get(20)?,
        artwork_url: row.get(21)?,
    })
}

fn genre_from_row(row: &Row<'_>) -> rusqlite::Result<Genre> {
    Ok(Genre {
        id: row.get(0)?,
        name: row.get(1)?,
        slug: row.get(2)?,
        parent_id: row.get(3)?,
        children: Vec::new(),
        track_count: Some(row.get(4)?),
    })
}

fn build_genre_tree(genres: Vec<Genre>) -> Vec<Genre> {
    let mut children_by_parent: HashMap<Option<i64>, Vec<Genre>> = HashMap::new();
    for genre in genres {
        children_by_parent
            .entry(genre.parent_id)
            .or_default()
            .push(genre);
    }

    fn attach_children(
        parent_id: Option<i64>,
        children_by_parent: &mut HashMap<Option<i64>, Vec<Genre>>,
    ) -> Vec<Genre> {
        let mut children = children_by_parent.remove(&parent_id).unwrap_or_default();
        children.sort_by(|left, right| left.name.cmp(&right.name));

        for child in &mut children {
            child.children = attach_children(Some(child.id), children_by_parent);
            child.track_count = Some(aggregate_track_count(child));
        }

        children
    }

    attach_children(None, &mut children_by_parent)
}

fn aggregate_track_count(node: &Genre) -> i64 {
    node.track_count.unwrap_or(0) + node.children.iter().map(aggregate_track_count).sum::<i64>()
}

fn placeholders(count: usize) -> String {
    std::iter::repeat("?")
        .take(count)
        .collect::<Vec<_>>()
        .join(",")
}

// ─── Track Similarity (Similar Radio) ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackSimilarityResult {
    pub track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub best_quality: Option<String>,
    pub similarity_score: f64,
    pub co_listen_score: f64,
    pub co_album_score: f64,
    pub co_artist_score: f64,
    pub genre_proximity: f64,
}

/// Compute similarity scores for all track pairs in the library.
/// Build pre-computed similarity pairs for the radio feature.
/// Fixes: Stage 1 now enumerates ALL pairs per album/artist (not just MIN/MAX).
///        Stage 2 uses indexed temp tables so scores merge correctly.
pub fn compute_track_similarity(conn: &Connection) -> Result<usize> {
    // Run all stages inside a single transaction so a kill or error mid-way
    // can't leave the table in a half-populated state (rows inserted but
    // component scores never updated). Dropping `tx` without commit rolls back.
    let tx = conn.unchecked_transaction()?;

    tx.execute("DELETE FROM track_similarity", [])?;

    // ── Stage 1: candidate pairs ─────────────────────────────────────────────
    // Each INSERT must satisfy CHECK (track_a < track_b), ensured by `b.id > a.id`.

    tx.execute_batch("
        -- 1a: Same-album pairs (all combinations, not just min/max)
        INSERT OR IGNORE INTO track_similarity (track_a, track_b)
        SELECT a.id, b.id
        FROM tracks a
        JOIN tracks b ON b.album_id = a.album_id AND b.id > a.id
        WHERE a.album_id IS NOT NULL;

        -- 1b: Same-artist pairs (cap at artists with <=100 tracks)
        INSERT OR IGNORE INTO track_similarity (track_a, track_b)
        SELECT a.id, b.id
        FROM tracks a
        JOIN tracks b ON b.artist_id = a.artist_id AND b.id > a.id
        WHERE a.artist_id IN (
            SELECT artist_id FROM tracks GROUP BY artist_id HAVING COUNT(*) <= 100
        );

        -- 1c: Shared-genre pairs (deduplicated by GROUP BY, limited to avoid explosion)
        INSERT OR IGNORE INTO track_similarity (track_a, track_b)
        SELECT a.track_id, b.track_id
        FROM track_genres a
        JOIN track_genres b ON b.genre_id = a.genre_id AND b.track_id > a.track_id
        GROUP BY a.track_id, b.track_id
        LIMIT 300000;
    ")?;

    // ── Stage 2: aggregate signals into indexed temp tables ──────────────────

    tx.execute_batch("
        DROP TABLE IF EXISTS _co_listen;
        CREATE TEMP TABLE _co_listen AS
        SELECT
            MIN(a.track_id, b.track_id) AS ta,
            MAX(a.track_id, b.track_id) AS tb,
            CAST(COUNT(*) AS REAL) AS n
        FROM listen_history a
        JOIN listen_history b
            ON b.track_id != a.track_id
            AND b.started_at BETWEEN a.started_at AND datetime(a.started_at, '+30 minutes')
        WHERE a.started_at >= datetime('now', '-90 days')
        GROUP BY ta, tb
        HAVING COUNT(*) >= 2;
        CREATE INDEX _co_listen_idx ON _co_listen(ta, tb);

        DROP TABLE IF EXISTS _genre_shared;
        CREATE TEMP TABLE _genre_shared AS
        SELECT a.track_id AS ta, b.track_id AS tb, COUNT(DISTINCT a.genre_id) AS shared
        FROM track_genres a
        JOIN track_genres b ON b.genre_id = a.genre_id AND b.track_id > a.track_id
        GROUP BY a.track_id, b.track_id;
        CREATE INDEX _genre_shared_idx ON _genre_shared(ta, tb);

        -- Track → release year, for era_proximity. Albums table holds the year.
        DROP TABLE IF EXISTS _track_year;
        CREATE TEMP TABLE _track_year AS
        SELECT t.id AS track_id, al.year AS year
        FROM tracks t
        JOIN albums al ON al.id = t.album_id
        WHERE al.year IS NOT NULL;
        CREATE INDEX _track_year_idx ON _track_year(track_id);
    ")?;

    // ── Stage 3: score each component ────────────────────────────────────────

    // co_album: 1.0 if same album
    tx.execute("
        UPDATE track_similarity SET co_album_score = 1.0
        WHERE EXISTS (
            SELECT 1 FROM tracks a, tracks b
            WHERE a.id = track_similarity.track_a
              AND b.id = track_similarity.track_b
              AND a.album_id IS NOT NULL
              AND a.album_id = b.album_id
        )
    ", [])?;

    // co_artist: 1.0 if same artist
    tx.execute("
        UPDATE track_similarity SET co_artist_score = 1.0
        WHERE EXISTS (
            SELECT 1 FROM tracks a, tracks b
            WHERE a.id = track_similarity.track_a
              AND b.id = track_similarity.track_b
              AND a.artist_id IS NOT NULL
              AND a.artist_id = b.artist_id
        )
    ", [])?;

    // genre_proximity: shared genres / max genres on any single track
    tx.execute("
        UPDATE track_similarity SET genre_proximity = COALESCE((
            SELECT CAST(gs.shared AS REAL) / NULLIF(
                (SELECT MAX(c) FROM (SELECT COUNT(DISTINCT genre_id) AS c FROM track_genres GROUP BY track_id)),
                0)
            FROM _genre_shared gs
            WHERE gs.ta = track_similarity.track_a AND gs.tb = track_similarity.track_b
        ), 0)
    ", [])?;

    // duration_proximity: 1 - |dur_a - dur_b| / 180s, clamped 0-1
    tx.execute("
        UPDATE track_similarity SET duration_proximity = COALESCE((
            SELECT 1.0 - MIN(CAST(ABS(a.duration_ms - b.duration_ms) AS REAL) / 180000.0, 1.0)
            FROM tracks a, tracks b
            WHERE a.id = track_similarity.track_a AND b.id = track_similarity.track_b
              AND a.duration_ms IS NOT NULL AND b.duration_ms IS NOT NULL
        ), 0)
    ", [])?;

    // co_listen: normalized co-occurrence count
    tx.execute("
        UPDATE track_similarity SET co_listen_score = COALESCE((
            SELECT cl.n / NULLIF((SELECT MAX(n) FROM _co_listen), 0)
            FROM _co_listen cl
            WHERE cl.ta = track_similarity.track_a AND cl.tb = track_similarity.track_b
        ), 0)
    ", [])?;

    // era_proximity: 1 - |year_a - year_b| / 25, clamped 0-1. Zero when either year is unknown.
    tx.execute("
        UPDATE track_similarity SET era_proximity = COALESCE((
            SELECT 1.0 - MIN(CAST(ABS(ya.year - yb.year) AS REAL) / 25.0, 1.0)
            FROM _track_year ya, _track_year yb
            WHERE ya.track_id = track_similarity.track_a
              AND yb.track_id = track_similarity.track_b
        ), 0)
    ", [])?;

    // Final weighted score. era_proximity replaces some duration_proximity weight
    // because era is a stronger taste signal than song length.
    tx.execute("
        UPDATE track_similarity SET similarity_score =
            co_listen_score    * 0.30 +
            co_album_score     * 0.20 +
            co_artist_score    * 0.20 +
            genre_proximity    * 0.15 +
            era_proximity      * 0.10 +
            duration_proximity * 0.05
    ", [])?;

    tx.execute_batch("
        DROP TABLE IF EXISTS _co_listen;
        DROP TABLE IF EXISTS _genre_shared;
        DROP TABLE IF EXISTS _track_year;
    ")?;

    let count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM track_similarity", [], |row| row.get(0)
    )?;

    tx.commit()?;
    Ok(count as usize)
}

/// Get similar tracks to a given track, ordered by similarity.
/// Returns up to `limit` tracks with similarity scores.
pub fn get_similar_tracks(
    conn: &Connection,
    track_id: i64,
    limit: i64,
    exclude_ids: &[i64],
) -> Result<Vec<TrackSimilarityResult>> {
    // For simplicity, handle exclude via post-filtering (limit is small, ~20-50)
    let sql = "SELECT t.id, t.title, a.name, al.title, al.artwork_url,
                      t.duration_ms, t.best_quality,
                      ts.similarity_score, ts.co_listen_score, ts.co_album_score,
                      ts.co_artist_score, ts.genre_proximity
               FROM track_similarity ts
               JOIN tracks t ON t.id = CASE
                   WHEN ts.track_a = ?1 THEN ts.track_b
                   ELSE ts.track_a
               END
               LEFT JOIN artists a ON a.id = t.artist_id
               LEFT JOIN albums al ON al.id = t.album_id
               WHERE (ts.track_a = ?1 OR ts.track_b = ?1)
                 AND t.id != ?1
               ORDER BY ts.similarity_score DESC
               LIMIT ?2";

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![track_id, limit], |row| {
        Ok(TrackSimilarityResult {
            track_id: row.get(0)?,
            title: row.get(1)?,
            artist_name: row.get(2)?,
            album_title: row.get(3)?,
            artwork_url: row.get(4)?,
            duration_ms: row.get(5)?,
            best_quality: row.get(6)?,
            similarity_score: row.get(7)?,
            co_listen_score: row.get(8)?,
            co_album_score: row.get(9)?,
            co_artist_score: row.get(10)?,
            genre_proximity: row.get(11)?,
        })
    })?;

    let mut results: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;

    // Post-filter excluded IDs
    if !exclude_ids.is_empty() {
        let exclude_set: HashSet<i64> = exclude_ids.iter().copied().collect();
        results.retain(|r| !exclude_set.contains(&r.track_id));
    }

    Ok(results)
}

/// Get similarity computation status
pub fn get_similarity_computed_at(conn: &Connection) -> Result<Option<String>> {
    Ok(conn.query_row(
        "SELECT MAX(computed_at) FROM track_similarity",
        [],
        |row| row.get(0),
    ).optional()?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingTrackRow {
    pub track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub duration_ms: Option<i64>,
    pub best_quality: Option<String>,
    pub source: String,
    pub play_count: i32,
    pub is_favorite: bool,
    pub playlist_memberships: i64,
    pub genre_paths: Vec<String>,
    // DSP features (None if not yet analyzed)
    pub bpm: Option<f64>,
    pub energy: Option<f64>,
    pub camelot_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingNeighborRow {
    pub track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub best_quality: Option<String>,
    pub score: f64,
    pub behavioral_score: f64,
    pub audio_score: f64,
    pub metadata_score: f64,
    pub reason_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEmbeddingRow {
    pub track_id: i64,
    pub vector_blob: Vec<u8>,
    pub l2_norm: f64,
}

pub fn upsert_embedding_model(
    conn: &Connection,
    model_key: &str,
    family: &str,
    dimension: i32,
    status: &str,
    config_json: Option<&str>,
) -> Result<EmbeddingModel> {
    conn.execute(
        "INSERT INTO embedding_models (model_key, family, dimension, status, config_json)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(model_key) DO UPDATE SET
             family = excluded.family,
             dimension = excluded.dimension,
             status = excluded.status,
             config_json = excluded.config_json",
        params![model_key, family, dimension, status, config_json],
    )?;

    conn.query_row(
        "SELECT id, model_key, family, dimension, status, is_active, trained_at, config_json, metrics_json, created_at
         FROM embedding_models WHERE model_key = ?1",
        params![model_key],
        |row| {
            Ok(EmbeddingModel {
                id: row.get(0)?,
                model_key: row.get(1)?,
                family: row.get(2)?,
                dimension: row.get(3)?,
                status: row.get(4)?,
                is_active: row.get(5)?,
                trained_at: row.get(6)?,
                config_json: row.get(7)?,
                metrics_json: row.get(8)?,
                created_at: row.get(9)?,
            })
        },
    )
    .map_err(Into::into)
}

pub fn update_embedding_model_metrics(
    conn: &Connection,
    model_id: i64,
    status: &str,
    metrics_json: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE embedding_models
         SET status = ?2, metrics_json = ?3, trained_at = datetime('now')
         WHERE id = ?1",
        params![model_id, status, metrics_json],
    )?;
    Ok(())
}

pub fn deactivate_embedding_models(conn: &Connection) -> Result<()> {
    conn.execute("UPDATE embedding_models SET is_active = 0", [])?;
    Ok(())
}

pub fn activate_embedding_model(conn: &Connection, model_id: i64) -> Result<()> {
    deactivate_embedding_models(conn)?;
    conn.execute(
        "UPDATE embedding_models SET is_active = 1, status = 'ready', trained_at = datetime('now')
         WHERE id = ?1",
        params![model_id],
    )?;
    Ok(())
}

pub fn get_active_embedding_model(conn: &Connection) -> Result<Option<EmbeddingModel>> {
    conn.query_row(
        "SELECT id, model_key, family, dimension, status, is_active, trained_at, config_json, metrics_json, created_at
         FROM embedding_models
         WHERE is_active = 1
         ORDER BY trained_at DESC, id DESC
         LIMIT 1",
        [],
        |row| {
            Ok(EmbeddingModel {
                id: row.get(0)?,
                model_key: row.get(1)?,
                family: row.get(2)?,
                dimension: row.get(3)?,
                status: row.get(4)?,
                is_active: row.get(5)?,
                trained_at: row.get(6)?,
                config_json: row.get(7)?,
                metrics_json: row.get(8)?,
                created_at: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn create_training_run(
    conn: &Connection,
    model_id: Option<i64>,
    stage: &str,
    status: &str,
) -> Result<DiscoveryTrainingRun> {
    conn.execute(
        "INSERT INTO training_runs (model_id, stage, status)
         VALUES (?1, ?2, ?3)",
        params![model_id, stage, status],
    )?;
    let id = conn.last_insert_rowid();
    get_training_run(conn, id)?.ok_or_else(|| anyhow::anyhow!("training run missing after insert"))
}

pub fn get_training_run(conn: &Connection, run_id: i64) -> Result<Option<DiscoveryTrainingRun>> {
    conn.query_row(
        "SELECT id, model_id, stage, status, progress, items_total, items_done, started_at, finished_at, error_text
         FROM training_runs WHERE id = ?1",
        params![run_id],
        |row| {
            Ok(DiscoveryTrainingRun {
                id: row.get(0)?,
                model_id: row.get(1)?,
                stage: row.get(2)?,
                status: row.get(3)?,
                progress: row.get(4)?,
                items_total: row.get(5)?,
                items_done: row.get(6)?,
                started_at: row.get(7)?,
                finished_at: row.get(8)?,
                error_text: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_latest_training_run(conn: &Connection) -> Result<Option<DiscoveryTrainingRun>> {
    conn.query_row(
        "SELECT id, model_id, stage, status, progress, items_total, items_done, started_at, finished_at, error_text
         FROM training_runs
         ORDER BY started_at DESC, id DESC
         LIMIT 1",
        [],
        |row| {
            Ok(DiscoveryTrainingRun {
                id: row.get(0)?,
                model_id: row.get(1)?,
                stage: row.get(2)?,
                status: row.get(3)?,
                progress: row.get(4)?,
                items_total: row.get(5)?,
                items_done: row.get(6)?,
                started_at: row.get(7)?,
                finished_at: row.get(8)?,
                error_text: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn update_training_run_progress(
    conn: &Connection,
    run_id: i64,
    stage: &str,
    status: &str,
    progress: f64,
    items_total: Option<i64>,
    items_done: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE training_runs
         SET stage = ?2, status = ?3, progress = ?4, items_total = ?5, items_done = ?6
         WHERE id = ?1",
        params![run_id, stage, status, progress, items_total, items_done],
    )?;
    Ok(())
}

pub fn finish_training_run(conn: &Connection, run_id: i64, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE training_runs
         SET status = ?2, progress = 1.0, finished_at = datetime('now')
         WHERE id = ?1",
        params![run_id, status],
    )?;
    Ok(())
}

pub fn fail_training_run(conn: &Connection, run_id: i64, error_text: &str) -> Result<()> {
    conn.execute(
        "UPDATE training_runs
         SET status = 'failed', error_text = ?2, finished_at = datetime('now')
         WHERE id = ?1",
        params![run_id, error_text],
    )?;
    Ok(())
}

pub fn replace_track_embeddings(
    conn: &Connection,
    model_id: i64,
    embeddings: &[(i64, Vec<u8>, f64)],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM track_embeddings WHERE model_id = ?1",
        params![model_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO track_embeddings (track_id, model_id, vector_blob, l2_norm)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (track_id, blob, norm) in embeddings {
            stmt.execute(params![track_id, model_id, blob, norm])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn replace_track_audio_features(
    conn: &Connection,
    features: &[(i64, String, Vec<u8>, i64, i64)],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO track_audio_features (track_id, feature_version, vector_blob, clip_start_ms, clip_duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(track_id) DO UPDATE SET
                 feature_version = excluded.feature_version,
                 vector_blob = excluded.vector_blob,
                 clip_start_ms = excluded.clip_start_ms,
                 clip_duration_ms = excluded.clip_duration_ms,
                 computed_at = datetime('now')",
        )?;
        for (track_id, version, blob, start_ms, duration_ms) in features {
            stmt.execute(params![track_id, version, blob, start_ms, duration_ms])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn replace_track_neighbors(
    conn: &Connection,
    model_id: i64,
    neighbors: &[(i64, i64, i32, f64, f64, f64, f64, Option<String>)],
) -> Result<()> {
    // Single transaction: ~2M+ INSERTs auto-committing one-by-one is what makes
    // training appear to hang. Batching also makes the DELETE+INSERT atomic so a
    // killed process can't leave the table half-populated.
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM track_neighbors WHERE model_id = ?1",
        params![model_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO track_neighbors
             (track_id, neighbor_track_id, model_id, rank, score, behavioral_score, audio_score, metadata_score, reason_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        for (track_id, neighbor_track_id, rank, score, behavioral_score, audio_score, metadata_score, reason_json) in neighbors {
            stmt.execute(params![
                track_id,
                neighbor_track_id,
                model_id,
                rank,
                score,
                behavioral_score,
                audio_score,
                metadata_score,
                reason_json
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn get_track_neighbors(
    conn: &Connection,
    model_id: i64,
    track_id: i64,
    limit: i64,
    exclude_ids: &[i64],
) -> Result<Vec<EmbeddingNeighborRow>> {
    let sql = "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, t.best_quality,
                      n.score, n.behavioral_score, n.audio_score, n.metadata_score, n.reason_json
               FROM track_neighbors n
               JOIN tracks t ON t.id = n.neighbor_track_id
               LEFT JOIN artists a ON a.id = t.artist_id
               LEFT JOIN albums al ON al.id = t.album_id
               WHERE n.model_id = ?1 AND n.track_id = ?2
               ORDER BY n.rank ASC
               LIMIT ?3";
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt
        .query_map(params![model_id, track_id, limit.max(1)], |row| {
            Ok(EmbeddingNeighborRow {
                track_id: row.get(0)?,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                album_title: row.get(3)?,
                artwork_url: row.get(4)?,
                duration_ms: row.get(5)?,
                best_quality: row.get(6)?,
                score: row.get(7)?,
                behavioral_score: row.get(8)?,
                audio_score: row.get(9)?,
                metadata_score: row.get(10)?,
                reason_json: row.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !exclude_ids.is_empty() {
        let exclude = exclude_ids.iter().copied().collect::<HashSet<_>>();
        rows.retain(|row| !exclude.contains(&row.track_id));
    }
    Ok(rows)
}

pub fn get_model_embeddings(conn: &Connection, model_id: i64) -> Result<Vec<ModelEmbeddingRow>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, vector_blob, l2_norm
         FROM track_embeddings
         WHERE model_id = ?1",
    )?;
    stmt.query_map(params![model_id], |row| {
        Ok(ModelEmbeddingRow {
            track_id: row.get(0)?,
            vector_blob: row.get(1)?,
            l2_norm: row.get(2)?,
        })
    })?
    .collect::<rusqlite::Result<Vec<_>>>()
    .map_err(Into::into)
}

pub fn get_embedding_track_rows(conn: &Connection) -> Result<Vec<EmbeddingTrackRow>> {
    let genre_paths = get_track_genre_paths(conn)?;
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, a.name, al.title, t.duration_ms, t.best_quality, t.source,
                t.play_count, t.is_favorite,
                (SELECT COUNT(*) FROM playlist_tracks pt WHERE pt.track_id = t.id) AS playlist_memberships,
                d.bpm, d.energy, d.camelot_key
         FROM tracks t
         LEFT JOIN artists a ON a.id = t.artist_id
         LEFT JOIN albums al ON al.id = t.album_id
         LEFT JOIN audio_dsp_features d ON d.track_id = t.id
         WHERE t.tidal_id IS NOT NULL OR t.file_path IS NOT NULL OR t.ytmusic_id IS NOT NULL OR t.soundcloud_id IS NOT NULL",
    )?;
    let mut rows = stmt
        .query_map([], |row| {
            let track_id = row.get::<_, i64>(0)?;
            Ok(EmbeddingTrackRow {
                track_id,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                album_title: row.get(3)?,
                duration_ms: row.get(4)?,
                best_quality: row.get(5)?,
                source: row.get(6)?,
                play_count: row.get(7)?,
                is_favorite: row.get(8)?,
                playlist_memberships: row.get(9)?,
                genre_paths: genre_paths.get(&track_id).cloned().unwrap_or_default(),
                bpm: row.get(10)?,
                energy: row.get(11)?,
                camelot_key: row.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.sort_by_key(|row| row.track_id);
    Ok(rows)
}

pub fn get_discovery_status(conn: &Connection) -> Result<DiscoveryStatus> {
    let active_model = get_active_embedding_model(conn)?;
    let latest_run = get_latest_training_run(conn)?;
    let playable_tracks: i64 = conn.query_row(
        "SELECT COUNT(*)
         FROM tracks
         WHERE tidal_id IS NOT NULL OR file_path IS NOT NULL OR ytmusic_id IS NOT NULL OR soundcloud_id IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    let embedded_tracks: i64 = match active_model.as_ref() {
        Some(model) => conn.query_row(
            "SELECT COUNT(*) FROM track_embeddings WHERE model_id = ?1",
            params![model.id],
            |row| row.get(0),
        )?,
        None => 0,
    };
    let neighbor_tracks: i64 = match active_model.as_ref() {
        Some(model) => conn.query_row(
            "SELECT COUNT(DISTINCT track_id) FROM track_neighbors WHERE model_id = ?1",
            params![model.id],
            |row| row.get(0),
        )?,
        None => 0,
    };
    let clip_cache_tracks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM track_audio_features",
        [],
        |row| row.get(0),
    )?;
    let coverage_ratio = if playable_tracks == 0 {
        0.0
    } else {
        neighbor_tracks as f64 / playable_tracks as f64
    };

    Ok(DiscoveryStatus {
        fallback_active: active_model.is_none(),
        active_model,
        latest_run,
        coverage_ratio,
        playable_tracks,
        embedded_tracks,
        neighbor_tracks,
        clip_cache_tracks,
    })
}

pub fn record_playback_transition(
    conn: &Connection,
    from_track_id: i64,
    to_track_id: i64,
    transition_source: &str,
    completed_prev: bool,
    gap_ms: i64,
) -> Result<()> {
    if from_track_id <= 0 || to_track_id <= 0 || from_track_id == to_track_id {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO playback_transitions
         (from_track_id, to_track_id, transition_source, completed_prev, gap_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![from_track_id, to_track_id, transition_source, completed_prev, gap_ms],
    )?;
    Ok(())
}

pub fn record_discovery_feedback(
    conn: &Connection,
    seed_track_id: i64,
    candidate_track_id: i64,
    action: &str,
    surface: &str,
    context_json: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO discovery_feedback
         (seed_track_id, candidate_track_id, action, surface, context_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![seed_track_id, candidate_track_id, action, surface, context_json],
    )?;
    Ok(())
}

pub fn get_playback_transition_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT from_track_id, to_track_id
         FROM playback_transitions
         ORDER BY created_at ASC, id ASC",
    )?;
    let pairs = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(pairs.into_iter().map(|(a, b)| vec![a, b]).collect())
}

pub fn get_listen_history_sequences(conn: &Connection, session_window_minutes: i64) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, started_at
         FROM listen_history
         ORDER BY started_at ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut sequences = Vec::new();
    let mut current = Vec::new();
    let mut previous_at: Option<chrono::DateTime<chrono::Utc>> = None;
    for (track_id, started_at) in rows {
        let parsed = chrono::DateTime::parse_from_rfc3339(&format!(
            "{}{}",
            started_at,
            if started_at.ends_with('Z') { "" } else { "Z" }
        ))
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&started_at, "%Y-%m-%d %H:%M:%S").map(|dt| dt.and_utc()))
        .ok();
        if let Some(prev) = previous_at {
            if let Some(next) = parsed {
                if (next - prev).num_minutes() > session_window_minutes {
                    if current.len() > 1 {
                        sequences.push(current.clone());
                    }
                    current.clear();
                }
                previous_at = Some(next);
            }
        } else if let Some(next) = parsed {
            previous_at = Some(next);
        }
        current.push(track_id);
    }
    if current.len() > 1 {
        sequences.push(current);
    }
    Ok(sequences)
}

pub fn get_playlist_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT playlist_id, track_id
         FROM playlist_tracks
         ORDER BY playlist_id ASC, position ASC",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for (playlist_id, track_id) in rows {
        grouped.entry(playlist_id).or_default().push(track_id);
    }
    Ok(grouped.into_values().filter(|seq| seq.len() > 1).collect())
}

pub fn get_album_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT album_id, id
         FROM tracks
         WHERE album_id IS NOT NULL
         ORDER BY album_id ASC, disc_number ASC, track_number ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for (album_id, track_id) in rows {
        grouped.entry(album_id).or_default().push(track_id);
    }
    Ok(grouped.into_values().filter(|seq| seq.len() > 1).collect())
}

pub fn get_artist_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT artist_id, id
         FROM tracks
         WHERE artist_id IS NOT NULL
         ORDER BY artist_id ASC, play_count DESC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for (artist_id, track_id) in rows {
        grouped.entry(artist_id).or_default().push(track_id);
    }
    // Truncate large artists rather than dropping them — an artist with 200 tracks
    // should still contribute co-occurrence signal for the tracks it includes.
    let mut seqs: Vec<Vec<i64>> = grouped.into_values().filter(|seq| seq.len() > 1).collect();
    for seq in &mut seqs {
        if seq.len() > 80 {
            seq.truncate(80); // keep top-80 by play_count (already sorted DESC)
        }
    }
    Ok(seqs)
}

pub fn get_genre_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT tg.genre_id, tg.track_id
         FROM track_genres tg
         JOIN tracks t ON t.id = tg.track_id
         ORDER BY tg.genre_id ASC, t.play_count DESC, t.id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for (genre_id, track_id) in rows {
        grouped.entry(genre_id).or_default().push(track_id);
    }
    let mut seqs: Vec<Vec<i64>> = grouped.into_values().filter(|seq| seq.len() > 1).collect();
    for seq in &mut seqs {
        if seq.len() > 80 {
            seq.truncate(80);
        }
    }
    Ok(seqs)
}

pub fn get_favorite_track_ids(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM tracks WHERE is_favorite = 1 ORDER BY play_count DESC, id ASC",
    )?;
    stmt.query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

// ─── Audio DSP Features ─────────────────────────────────────────────────────

pub fn upsert_audio_dsp_features(conn: &Connection, f: &AudioDspFeatures) -> Result<()> {
    conn.execute(
        "INSERT INTO audio_dsp_features
         (track_id, bpm, key_signature, camelot_key, loudness_lufs, energy, danceability,
          beat_strength, spectral_centroid, stereo_width, is_instrumental,
          analysis_source, analysis_offset_ms, samples_analyzed, analyzed_at, analysis_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(track_id) DO UPDATE SET
             bpm = excluded.bpm,
             key_signature = excluded.key_signature,
             camelot_key = excluded.camelot_key,
             loudness_lufs = excluded.loudness_lufs,
             energy = excluded.energy,
             danceability = excluded.danceability,
             beat_strength = excluded.beat_strength,
             spectral_centroid = excluded.spectral_centroid,
             stereo_width = excluded.stereo_width,
             is_instrumental = excluded.is_instrumental,
             analysis_source = excluded.analysis_source,
             analysis_offset_ms = excluded.analysis_offset_ms,
             samples_analyzed = excluded.samples_analyzed,
             analyzed_at = excluded.analyzed_at,
             analysis_version = excluded.analysis_version",
        params![
            f.track_id,
            f.bpm,
            f.key_signature,
            f.camelot_key,
            f.loudness_lufs,
            f.energy,
            f.danceability,
            f.beat_strength,
            f.spectral_centroid,
            f.stereo_width,
            f.is_instrumental as i32,
            f.analysis_source,
            f.analysis_offset_ms,
            f.samples_analyzed,
            f.analyzed_at,
            f.analysis_version,
        ],
    )?;
    Ok(())
}

pub fn get_audio_dsp_features(conn: &Connection, track_id: i64) -> Result<Option<AudioDspFeatures>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, bpm, key_signature, camelot_key, loudness_lufs,
                energy, danceability, beat_strength, spectral_centroid, stereo_width,
                is_instrumental, analysis_source, analysis_offset_ms, samples_analyzed,
                analyzed_at, analysis_version
         FROM audio_dsp_features
         WHERE track_id = ?1",
    )?;
    let result = stmt.query_row(params![track_id], |row| {
        Ok(AudioDspFeatures {
            track_id: row.get(0)?,
            bpm: row.get(1)?,
            key_signature: row.get(2)?,
            camelot_key: row.get(3)?,
            loudness_lufs: row.get(4)?,
            energy: row.get(5)?,
            danceability: row.get(6)?,
            beat_strength: row.get(7)?,
            spectral_centroid: row.get(8)?,
            stereo_width: row.get(9)?,
            is_instrumental: row.get::<_, i32>(10)? != 0,
            analysis_source: row.get(11)?,
            analysis_offset_ms: row.get(12)?,
            samples_analyzed: row.get(13)?,
            analyzed_at: row.get(14)?,
            analysis_version: row.get(15)?,
        })
    }).optional()?;
    Ok(result)
}

pub fn get_tracks_missing_dsp_features(conn: &Connection, limit: i64) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, t.artist_id, a.name as artist_name,
                t.album_id, al.title as album_title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN audio_dsp_features dsp ON t.id = dsp.track_id
         WHERE dsp.track_id IS NULL
         LIMIT ?1",
    )?;
    let tracks = stmt
        .query_map(params![limit], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tracks)
}

pub fn get_audio_features_stats(conn: &Connection) -> Result<AudioFeaturesStats> {
    let (total_analyzed, avg_bpm, avg_energy): (i64, Option<f64>, Option<f64>) =
        conn.query_row(
            "SELECT COUNT(*), AVG(bpm), AVG(energy) FROM audio_dsp_features",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

    // Top key (most common)
    let top_key: Option<String> = conn.query_row(
        "SELECT key_signature
         FROM audio_dsp_features
         WHERE key_signature IS NOT NULL
         GROUP BY key_signature
         ORDER BY COUNT(*) DESC
         LIMIT 1",
        [],
        |row| row.get(0),
    ).optional()?;

    // Key distribution
    let mut stmt = conn.prepare(
        "SELECT key_signature, COUNT(*)
         FROM audio_dsp_features
         WHERE key_signature IS NOT NULL
         GROUP BY key_signature
         ORDER BY COUNT(*) DESC",
    )?;
    let key_rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let key_distribution: HashMap<String, i64> = key_rows.collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .collect();

    Ok(AudioFeaturesStats {
        total_analyzed,
        avg_bpm,
        top_key,
        avg_energy,
        key_distribution,
    })
}

/// Bulk-load DSP features for every analyzed track. Used by smart playlist evaluation
/// so a single scan populates the evaluation context for all rules at once.
pub fn get_all_audio_dsp_features(
    conn: &Connection,
) -> Result<Vec<(i64, Option<f64>, Option<String>, Option<String>, Option<f64>, Option<f64>, bool)>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, bpm, key_signature, camelot_key, energy, danceability, is_instrumental
         FROM audio_dsp_features",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<f64>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, Option<i32>>(6)?.unwrap_or(0) != 0,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Track IDs that have at least one ACRCloud sample match.
pub fn get_track_ids_with_acrcloud_match(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT DISTINCT track_id FROM acrcloud_results")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Track IDs that have a stored audio fingerprint.
pub fn get_track_ids_with_fingerprint(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT track_id FROM audio_fingerprints")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn count_audio_dsp_features(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM audio_dsp_features", [], |row| row.get(0))
        .map_err(Into::into)
}

pub fn delete_all_audio_dsp_features(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM audio_dsp_features", [])?;
    Ok(())
}

pub fn get_genre_audio_metrics(conn: &Connection) -> Result<Vec<GenreAudioMetrics>> {
    let mut stmt = conn.prepare(
        "SELECT g.id, g.name,
                AVG(a.bpm) AS avg_bpm,
                AVG(a.energy) AS avg_energy,
                AVG(a.danceability) AS avg_danceability,
                COUNT(DISTINCT a.track_id) AS analyzed_count
         FROM genres g
         JOIN track_genres tg ON tg.genre_id = g.id
         JOIN audio_dsp_features a ON a.track_id = tg.track_id
         GROUP BY g.id, g.name
         ORDER BY analyzed_count DESC, g.name ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(GenreAudioMetrics {
            genre_id: row.get(0)?,
            genre_name: row.get(1)?,
            avg_bpm: row.get(2)?,
            avg_energy: row.get(3)?,
            avg_danceability: row.get(4)?,
            analyzed_count: row.get(5)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn upsert_fingerprint(conn: &Connection, track_id: i64, fp: &AudioFingerprint) -> Result<()> {
    conn.execute(
        "INSERT INTO audio_fingerprints (track_id, hashes_blob, peak_count)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(track_id) DO UPDATE SET
             hashes_blob = excluded.hashes_blob,
             peak_count = excluded.peak_count",
        params![track_id, fp.hashes_blob, fp.peak_count],
    )?;
    Ok(())
}

pub fn insert_fingerprint_hashes(conn: &Connection, track_id: i64, hashes: &[(u32, u32)]) -> Result<()> {
    if hashes.is_empty() {
        return Ok(());
    }

    // Wrap the whole payload in an explicit transaction; chunk inserts so a
    // very large hash list doesn't hold a single statement open for too long.
    const CHUNK: usize = 1000;
    conn.execute_batch("BEGIN;")?;
    let insert_result: Result<()> = (|| {
        let mut stmt = conn.prepare(
            "INSERT OR IGNORE INTO fingerprint_hashes (hash, track_id, time_offset)
             VALUES (?1, ?2, ?3)",
        )?;
        for chunk in hashes.chunks(CHUNK) {
            for (hash, time_offset) in chunk {
                stmt.execute(params![*hash as i64, track_id, *time_offset])?;
            }
        }
        Ok(())
    })();

    match insert_result {
        Ok(()) => {
            conn.execute_batch("COMMIT;")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

/// Run `PRAGMA optimize; ANALYZE fingerprint_hashes;` after a bulk fingerprint scan.
/// Failures are logged but not fatal.
pub fn optimize_fingerprint_hashes(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA optimize; ANALYZE fingerprint_hashes;")?;
    Ok(())
}

// ── Duplicate group helpers (fingerprint-driven dedup) ───────────────────────

/// Find an existing duplicate_group that already contains BOTH `a` and `b` as members.
pub fn find_duplicate_group_for_tracks(
    conn: &Connection,
    a: i64,
    b: i64,
) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT ma.group_id
         FROM duplicate_members ma
         JOIN duplicate_members mb ON mb.group_id = ma.group_id
         WHERE ma.track_id = ?1 AND mb.track_id = ?2
         LIMIT 1",
        params![a, b],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Create a new empty duplicate_group and return its id.
pub fn create_duplicate_group(conn: &Connection) -> Result<i64> {
    conn.execute(
        "INSERT INTO duplicate_groups (status) VALUES ('pending')",
        [],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insert a member into a duplicate_group. Idempotent (ON CONFLICT IGNORE).
pub fn add_duplicate_member(
    conn: &Connection,
    gid: i64,
    tid: i64,
    preferred: bool,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO duplicate_members (group_id, track_id, is_preferred)
         VALUES (?1, ?2, ?3)",
        params![gid, tid, if preferred { 1 } else { 0 }],
    )?;
    Ok(())
}

/// Tag a duplicate_group with its source (e.g. 'fingerprint') and a confidence value.
pub fn set_duplicate_group_source(
    conn: &Connection,
    gid: i64,
    source: &str,
    confidence: f64,
) -> Result<()> {
    conn.execute(
        "UPDATE duplicate_groups SET source = ?2, confidence = ?3 WHERE id = ?1",
        params![gid, source, confidence],
    )?;
    Ok(())
}

// ── Analysis quality & stale detection ───────────────────────────────────────

/// Snapshot of DSP-analysis coverage across the library.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AudioFeaturesQuality {
    pub total_tracks: i64,
    pub analyzed: i64,
    pub analysis_v1: i64,
    pub analysis_stale: i64,
    pub low_confidence_bpm: i64,
    pub low_confidence_key: i64,
    pub no_preview_url: i64,
    pub fingerprinted: i64,
}

pub fn get_audio_features_quality(conn: &Connection) -> Result<AudioFeaturesQuality> {
    let total_tracks: i64 = conn
        .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
        .unwrap_or(0);
    let analyzed: i64 = conn
        .query_row("SELECT COUNT(*) FROM audio_dsp_features", [], |r| r.get(0))
        .unwrap_or(0);
    let analysis_v1: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audio_dsp_features WHERE analysis_version = 'v1'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let analysis_stale: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audio_dsp_features WHERE analysis_version != 'v1'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let low_confidence_bpm: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audio_dsp_features WHERE bpm IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let low_confidence_key: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audio_dsp_features WHERE key_signature IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    // "No preview URL" = tracks we can't currently pull preview audio for.
    // We treat tracks lacking a tidal_id AND file_path as having no preview source.
    let no_preview_url: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tracks
             WHERE tidal_id IS NULL
               AND (file_path IS NULL OR file_path = '')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let fingerprinted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audio_fingerprints",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    Ok(AudioFeaturesQuality {
        total_tracks,
        analyzed,
        analysis_v1,
        analysis_stale,
        low_confidence_bpm,
        low_confidence_key,
        no_preview_url,
        fingerprinted,
    })
}

/// Return the ids of all tracks whose stored analysis_version is not 'v1'
/// (i.e. need to be re-analysed after an analysis-version bump).
pub fn get_stale_analysis_track_ids(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT track_id FROM audio_dsp_features WHERE analysis_version != 'v1'",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn find_tracks_by_hash(conn: &Connection, hashes: &[u32]) -> Result<Vec<(i64, u32)>> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders: Vec<String> = hashes.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT track_id, hash
         FROM fingerprint_hashes
         WHERE hash IN ({})
         ORDER BY hash",
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let hash_params: Vec<i64> = hashes.iter().map(|h| *h as i64).collect();
    let rows = stmt.query_map(params_from_iter(hash_params.iter()), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? as u32))
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use rusqlite::Connection;

    #[test]
    fn discovery_presets_round_trip_mode() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        let created = create_discovery_preset(
            &conn,
            "After Hours",
            "glassy synths",
            "reference",
            r#"["tidal","soundcloud"]"#,
        )
        .expect("preset created");

        assert_eq!(created.mode, "reference");

        let presets = list_discovery_presets(&conn).expect("preset list");
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].mode, "reference");
        assert_eq!(presets[0].services, vec!["tidal", "soundcloud"]);
    }

    #[test]
    fn search_matches_artist_names_for_tracks_and_albums() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'The Cure')", [])
            .expect("artist inserted");
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, is_favorite, source) VALUES (1, 'Disintegration', 1, 1, 'tidal')",
            [],
        )
        .expect("album inserted");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source
            ) VALUES (1, 'Pictures of You', 1, 1, 420000, 101, 'LOSSLESS', 'tidal', 10, 1, 'tidal')",
            [],
        )
        .expect("track inserted");

        let results = search(&conn, "the cure", 10).expect("search results");

        assert_eq!(results.artists.len(), 1);
        assert_eq!(results.artists[0].name, "The Cure");
        assert_eq!(results.albums.len(), 1);
        assert_eq!(results.albums[0].title, "Disintegration");
        assert_eq!(results.tracks.len(), 1);
        assert_eq!(results.tracks[0].title, "Pictures of You");
    }

    #[test]
    fn genre_heat_rolls_descendant_listens_up_to_ancestors() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute(
            "INSERT INTO genres (id, name, slug, parent_id) VALUES
                (1, 'Electronic', 'electronic', NULL),
                (2, 'Drum and Bass', 'drum-and-bass', 1)",
            [],
        )
        .expect("genres inserted");
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Rufige Kru')",
            [],
        )
        .expect("artist inserted");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, duration_ms, tidal_id, best_quality, best_source, fidelity_score, is_favorite, source
            ) VALUES (1, 'Terminator', 1, 360000, 101, 'LOSSLESS', 'tidal', 10, 1, 'tidal')",
            [],
        )
        .expect("track inserted");
        conn.execute(
            "INSERT INTO track_genres (track_id, genre_id, source, confidence)
             VALUES (1, 2, 'musicbrainz', 1.0)",
            [],
        )
        .expect("track genre inserted");
        conn.execute(
            "INSERT INTO listen_history (track_id, started_at, duration_listened_ms, completed)
             VALUES (1, datetime('now', '-10 days'), 120000, 1)",
            [],
        )
        .expect("listen inserted");

        let heat = get_genre_heat(&conn, 90).expect("genre heat");
        let electronic = heat
            .iter()
            .find(|entry| entry.genre_id == 1)
            .expect("electronic heat");
        let dnb = heat
            .iter()
            .find(|entry| entry.genre_id == 2)
            .expect("dnb heat");

        assert_eq!(electronic.listen_count, 1);
        assert_eq!(electronic.total_listened_ms, 120000);
        assert_eq!(dnb.listen_count, 1);
        assert_eq!(dnb.total_listened_ms, 120000);
    }

    #[test]
    fn genre_heat_returns_zero_rows_for_cold_genres() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute(
            "INSERT INTO genres (id, name, slug, parent_id) VALUES
                (1, 'Electronic', 'electronic', NULL),
                (2, 'Ambient', 'ambient', 1)",
            [],
        )
        .expect("genres inserted");

        let heat = get_genre_heat(&conn, 90).expect("genre heat");
        assert_eq!(heat.len(), 2);
        assert!(heat.iter().all(|entry| entry.listen_count == 0));
        assert!(heat.iter().all(|entry| entry.total_listened_ms == 0));
    }
}

/// Load enough metadata about a library track to seed external Tidal discovery.
/// Returns None if the track id isn't found.
///
/// `provider_track_id` is set from `tracks.tidal_id` if available; otherwise the
/// library `id` is used as a string. `normalized_genres` is the top 5 genres
/// for the track ordered by descending confidence.
pub fn load_external_seed_from_track(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<DiscoveryCandidateSeed>> {
    let row = conn.query_row(
        "SELECT t.id, t.tidal_id, t.title, ar.name, al.title
         FROM tracks t
         LEFT JOIN artists ar ON t.artist_id = ar.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.id = ?1",
        params![track_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        },
    );

    let (id, tidal_id, title, artist_name, album_title) = match row {
        Ok(r) => r,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let mut stmt = conn.prepare(
        "SELECT g.name
         FROM track_genres tg
         JOIN genres g ON g.id = tg.genre_id
         WHERE tg.track_id = ?1
         ORDER BY COALESCE(tg.confidence, 0) DESC
         LIMIT 5",
    )?;
    let genres: Vec<String> = stmt
        .query_map(params![track_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Some(DiscoveryCandidateSeed {
        provider_track_id: tidal_id
            .map(|t| t.to_string())
            .unwrap_or_else(|| id.to_string()),
        title,
        artist_name,
        album_title,
        normalized_genres: genres,
    }))
}
