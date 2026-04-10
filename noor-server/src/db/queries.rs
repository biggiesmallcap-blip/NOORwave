use super::models::*;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

// ─── Tracks ───────────────────────────────────────────────

pub fn get_tracks(
    conn: &Connection,
    sort_by: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
    favorite_only: bool,
) -> Result<Vec<Track>> {
    let order_col = match sort_by {
        "title" => "t.title",
        "artist" => "a.name",
        "album" => "al.title",
        "year" => "al.year",
        "date_added" => "t.date_added",
        "duration" => "t.duration_ms",
        "play_count" => "t.play_count",
        "fidelity" => "t.fidelity_score",
        _ => "t.date_added",
    };
    let dir = if sort_dir == "asc" { "ASC" } else { "DESC" };

    let fav_filter = if favorite_only {
        " WHERE t.is_favorite = 1"
    } else {
        ""
    };

    let sql = format!(
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
         {fav_filter}
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
        " WHERE is_favorite = 1"
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
    days: i64,
    window_minutes: i64,
    min_count: i64,
) -> Result<Vec<GenreCoOccurrence>> {
    let window_seconds = window_minutes * 60;
    let mut stmt = conn.prepare(
        "WITH recent_listens AS (
            SELECT lh.track_id, lh.started_at, lh.id AS listen_id
            FROM listen_history lh
            WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
        ),
        genre_listens AS (
            SELECT tg.genre_id, rl.started_at, rl.listen_id
            FROM recent_listens rl
            JOIN track_genres tg ON tg.track_id = rl.track_id
        ),
        pairs AS (
            SELECT
                MIN(a.genre_id) AS genre_a,
                MAX(a.genre_id) AS genre_b,
                COUNT(*) AS raw_count
            FROM genre_listens a
            JOIN genre_listens b
                ON b.listen_id >= a.listen_id
               AND b.listen_id <= a.listen_id + ?2
               AND b.genre_id > a.genre_id
            GROUP BY genre_a, genre_b
            HAVING raw_count >= ?3
        ),
        genre_totals AS (
            SELECT genre_id, COUNT(DISTINCT listen_id) AS total_listens
            FROM genre_listens
            GROUP BY genre_id
        )
        SELECT
            ga.id, ga.name,
            gb.id, gb.name,
            p.raw_count,
            CAST(p.raw_count AS REAL) /
                (gt_a.total_listens + gt_b.total_listens - p.raw_count) AS jaccard
        FROM pairs p
        JOIN genres ga ON ga.id = p.genre_a
        JOIN genres gb ON gb.id = p.genre_b
        JOIN genre_totals gt_a ON gt_a.genre_id = p.genre_a
        JOIN genre_totals gt_b ON gt_b.genre_id = p.genre_b
        ORDER BY jaccard DESC, p.raw_count DESC",
    )?;

    let rows = stmt.query_map(
        params![days, window_seconds, min_count],
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
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
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
