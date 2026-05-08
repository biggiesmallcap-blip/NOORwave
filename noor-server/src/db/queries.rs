use super::models::*;
use crate::services::discovery::DiscoveryCandidateSeed;
use anyhow::Result;
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
    if let Some(token) = existing
        && is_valid_pin(&token) {
            return Ok(token);
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

/// First-run onboarding flag. When the row is missing, treat an existing
/// `service_auth` TIDAL row as implicit completion and persist the flag —
/// this keeps users upgrading from earlier versions out of the onboarding
/// flow they've effectively already done.
pub fn get_onboarding_complete(conn: &Connection) -> Result<bool> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key='onboarding_complete'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(value) = stored {
        return Ok(value == "1");
    }

    let has_tidal: bool = conn
        .query_row(
            "SELECT 1 FROM service_auth WHERE service='tidal' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();

    if has_tidal {
        set_onboarding_complete(conn)?;
        return Ok(true);
    }

    Ok(false)
}

pub fn set_onboarding_complete(conn: &Connection) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('onboarding_complete', '1')",
        [],
    )?;
    Ok(())
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

// Single source of truth for the favorite/liked WHERE predicate.
// Used by both get_tracks_with_dsp and get_track_count so they cannot drift.
//
// `favorite_only` is legacy naming: it currently means "library tracks" =
// tracks where tracks.is_favorite=1 OR the parent album has albums.is_favorite=1.
// `liked_only` is the strict "user explicitly liked this track" filter.
// liked_only takes precedence over favorite_only.
//
// All callers must alias the tracks table as `t` for this predicate to apply.
fn favorite_predicate(favorite_only: bool, liked_only: bool) -> Option<&'static str> {
    if liked_only {
        Some("t.is_favorite = 1")
    } else if favorite_only {
        Some("(t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))")
    } else {
        None
    }
}

pub fn get_tracks(
    conn: &Connection,
    sort_by: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
    favorite_only: bool,
    liked_only: bool,
) -> Result<Vec<Track>> {
    get_tracks_with_dsp(
        conn,
        sort_by,
        sort_dir,
        limit,
        offset,
        favorite_only,
        liked_only,
        &DspFilters::default(),
    )
}

pub fn get_tracks_with_dsp(
    conn: &Connection,
    sort_by: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
    favorite_only: bool,
    liked_only: bool,
    dsp: &DspFilters,
) -> Result<Vec<Track>> {
    let has_dsp = dsp.bpm_min.is_some()
        || dsp.bpm_max.is_some()
        || dsp.energy_min.is_some()
        || dsp.energy_max.is_some()
        || dsp.key_signature.is_some()
        || dsp.instrumental_only;

    let order_col = match sort_by {
        "title" => "t.title",
        "artist" => "a_artists.name",
        "album" => "al.title",
        "year" => "al.year",
        "date_added" => "t.date_added",
        "duration" => "t.duration_ms",
        "play_count" => "t.play_count",
        "fidelity" => "t.fidelity_score",
        "bpm" => "COALESCE(a.bpm, 0)",
        "energy" => "COALESCE(a.energy, 0)",
        "danceability" => "COALESCE(a.danceability, 0)",
        "last_played_at" => "COALESCE(t.last_played_at, '')",
        _ => "t.date_added",
    };
    let dir = if sort_dir == "asc" { "ASC" } else { "DESC" };

    let mut conditions = Vec::new();
    if let Some(pred) = favorite_predicate(favorite_only, liked_only) {
        conditions.push(pred.to_string());
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

pub fn get_track_count(conn: &Connection, favorite_only: bool, liked_only: bool) -> Result<i64> {
    // FROM tracks t alias is required so favorite_predicate's "t."-prefixed SQL applies.
    let filter = match favorite_predicate(favorite_only, liked_only) {
        Some(pred) => format!(" WHERE {pred}"),
        None => String::new(),
    };
    Ok(conn.query_row(
        &format!("SELECT COUNT(*) FROM tracks t{filter}"),
        [],
        |row| row.get(0),
    )?)
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

/// Single artist row plus library-side counts (tracks belonging to this
/// artist, distinct albums those tracks span). Counts reflect the local
/// library only — TIDAL-side totals come from the discography handler.
pub fn get_artist_with_counts(
    conn: &Connection,
    artist_id: i64,
) -> Result<Option<(Artist, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.tidal_id, a.ytmusic_id, a.soundcloud_id,
                a.name, a.name_sort, a.biography, a.photo_url
         FROM artists a
         WHERE a.id = ?1",
    )?;
    let artist = stmt
        .query_row(params![artist_id], |row| {
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
        })
        .optional()?;

    let Some(artist) = artist else {
        return Ok(None);
    };

    let track_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE artist_id = ?1",
        params![artist_id],
        |row| row.get(0),
    )?;
    let album_count: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT album_id) FROM tracks
         WHERE artist_id = ?1 AND album_id IS NOT NULL",
        params![artist_id],
        |row| row.get(0),
    )?;

    Ok(Some((artist, track_count, album_count)))
}

pub fn get_artist_tidal_id(conn: &Connection, artist_id: i64) -> Result<Option<i64>> {
    let mut stmt = conn.prepare("SELECT tidal_id FROM artists WHERE id = ?1")?;
    let tidal_id = stmt
        .query_row(params![artist_id], |row| row.get::<_, Option<i64>>(0))
        .optional()?
        .flatten();
    Ok(tidal_id)
}

pub fn get_known_artist_tidal_ids(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, i64>> {
    if tidal_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = placeholders(tidal_ids.len());
    let sql = format!("SELECT tidal_id, id FROM artists WHERE tidal_id IN ({placeholders})");
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

pub fn get_known_album_tidal_ids(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, i64>> {
    if tidal_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = placeholders(tidal_ids.len());
    let sql = format!("SELECT tidal_id, id FROM albums WHERE tidal_id IN ({placeholders})");
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
                smart_rules, is_synced, track_count, is_favorite,
                created_at, updated_at
         FROM playlists
         ORDER BY is_favorite DESC, name ASC",
    )?;

    let playlists = stmt
        .query_map([], |row| {
            Ok(Playlist {
                id: row.get(0)?,
                tidal_uuid: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                is_smart: row.get::<_, i32>(4)? != 0,
                smart_rules: row.get(5)?,
                is_synced: row.get::<_, i32>(6)? != 0,
                track_count: row.get(7)?,
                is_favorite: row.get::<_, i32>(8)? != 0,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(playlists)
}

pub fn get_playlist(conn: &Connection, playlist_id: i64) -> Result<Option<Playlist>> {
    let mut stmt = conn.prepare(
        "SELECT id, tidal_uuid, name, description, is_smart,
                smart_rules, is_synced, track_count, is_favorite,
                created_at, updated_at
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
            is_smart: row.get::<_, i32>(4)? != 0,
            smart_rules: row.get(5)?,
            is_synced: row.get::<_, i32>(6)? != 0,
            track_count: row.get(7)?,
            is_favorite: row.get::<_, i32>(8)? != 0,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn toggle_playlist_favorite(conn: &Connection, playlist_id: i64) -> Result<Playlist> {
    conn.execute(
        "UPDATE playlists SET is_favorite = NOT is_favorite WHERE id = ?1",
        params![playlist_id],
    )?;
    get_playlist(conn, playlist_id)?.ok_or_else(|| anyhow::anyhow!("playlist not found"))
}

/// Bulk-insert tracks into a playlist, skipping any already present.
/// Returns the number of tracks actually inserted.
pub fn add_tracks_to_playlist(
    conn: &Connection,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<usize> {
    if track_ids.is_empty() {
        return Ok(0);
    }

    // Find which tracks are already in the playlist
    let existing: std::collections::HashSet<i64> = {
        let mut stmt =
            conn.prepare("SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1")?;
        stmt.query_map(params![playlist_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?
    };

    let to_insert: Vec<i64> = {
        let mut seen = std::collections::HashSet::new();
        track_ids
            .iter()
            .copied()
            .filter(|id| !existing.contains(id) && seen.insert(*id))
            .collect()
    };

    if to_insert.is_empty() {
        return Ok(0);
    }

    // Get the current max position
    let max_pos: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) FROM playlist_tracks WHERE playlist_id = ?1",
        params![playlist_id],
        |row| row.get(0),
    )?;

    let mut stmt = conn.prepare(
        "INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
    )?;
    for (i, &track_id) in to_insert.iter().enumerate() {
        stmt.execute(params![playlist_id, track_id, max_pos + 1 + i as i64])?;
    }

    // Keep track_count in sync and bump updated_at so "Recently updated"
    // sorts reflect content changes, not just smart-rule edits.
    conn.execute(
        "UPDATE playlists SET track_count = (
            SELECT COUNT(*) FROM playlist_tracks WHERE playlist_id = ?1
         ),
         updated_at = datetime('now')
         WHERE id = ?1",
        params![playlist_id],
    )?;

    Ok(to_insert.len())
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

/// Provenance of a [`ResolvedGenre`] — distinguishes ground-truth track-level
/// data from album/artist fallback rescues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenreSource {
    /// Direct row from `track_genres`.
    Track,
    /// Aggregated from sibling tracks on the same single-artist album.
    AlbumFallback,
    /// Aggregated from other tracks by the same artist.
    ArtistFallback,
}

impl GenreSource {
    fn from_sql_source(value: &str) -> Self {
        match value {
            "album_fallback" => GenreSource::AlbumFallback,
            "artist_fallback" => GenreSource::ArtistFallback,
            _ => GenreSource::Track,
        }
    }
}

/// One genre path string for a track, with provenance. Path is the same
/// `"Parent > Leaf"` shape `get_genres_for_tracks` returns.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ResolvedGenre {
    pub path: String,
    pub source: GenreSource,
}

impl ResolvedGenre {
    /// Adapter for callers that consume only the path strings (e.g.
    /// `weighted_genre_set` in the Phase-2b Jaccard scorer).
    pub fn paths_only(rows: &[ResolvedGenre]) -> Vec<String> {
        rows.iter().map(|r| r.path.clone()).collect()
    }
}

/// Variant of [`get_genres_for_tracks`] that fills empty tracks via
/// album-then-artist fallback. Tracks with at least one row in
/// `track_genres` are returned untouched at [`GenreSource::Track`]. Tracks
/// with no rows get rescued from siblings on the same single-artist album
/// ([`GenreSource::AlbumFallback`]) or, failing that, from other tracks by
/// the same artist ([`GenreSource::ArtistFallback`]). Multi-artist albums
/// (compilations) are skipped at the album tier to avoid cross-artist
/// contamination.
///
/// Top-[`crate::genre::filter::FALLBACK_ROWS_PER_TRACK`] fallback rows per
/// tier per track. Sibling rows are taken from `track_genres` directly
/// (the inner rule is `GalaxyFilterRule::All`) — Path A consumers (radio
/// Jaccard, JSON exports) want the full sibling material.
///
/// See the parent module docs and the `filter_subquery_with_fallback`
/// implementation for cascade semantics.
pub fn get_genres_for_tracks_with_fallback(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<HashMap<i64, Vec<ResolvedGenre>>> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }

    // Use the per-track-narrowed cascade builder. Without it, every call would
    // enumerate all 35k tracks in `needs_fallback` and scan the whole
    // track_genres table for primary rows. The narrow form inlines an
    // IN(?,?,...) filter so SQLite touches only the requested track ids.
    let cascade = crate::genre::filter::filter_subquery_with_fallback_for_tracks(
        crate::genre::filter::GalaxyFilterRule::All,
        track_ids.len(),
    );
    let sql = format!(
        "WITH RECURSIVE genre_paths(id, parent_id, path) AS (
            SELECT id, parent_id, name
            FROM genres
            WHERE parent_id IS NULL
            UNION ALL
            SELECT g.id, g.parent_id, genre_paths.path || ' > ' || g.name
            FROM genres g
            JOIN genre_paths ON g.parent_id = genre_paths.id
        )
        SELECT cr.track_id, genre_paths.path, cr.source
        FROM ({cascade}) cr
        JOIN genre_paths ON genre_paths.id = cr.genre_id
        ORDER BY cr.track_id, genre_paths.path"
    );
    let mut stmt = conn.prepare(&sql)?;
    let params_iter = rusqlite::params_from_iter(track_ids.iter().copied());
    let mut rows = stmt.query(params_iter)?;

    let mut by_track: HashMap<i64, Vec<ResolvedGenre>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let track_id: i64 = row.get(0)?;
        let path: String = row.get(1)?;
        let src: String = row.get(2)?;
        by_track.entry(track_id).or_default().push(ResolvedGenre {
            path,
            source: GenreSource::from_sql_source(&src),
        });
    }

    Ok(by_track)
}

/// Whole-library variant of [`get_genres_for_tracks_with_fallback`]. Returns
/// the same `(track_id → Vec<ResolvedGenre>)` map for every track in the
/// library, including tracks rescued via album/artist fallback. Used by the
/// galaxy/discovery JSON-export endpoints.
///
/// More expensive than the per-track form — the cascade processes the
/// whole library. Profile via `EXPLAIN QUERY PLAN` if perf becomes a
/// concern.
pub fn get_track_genre_paths_with_fallback(
    conn: &Connection,
) -> Result<HashMap<i64, Vec<ResolvedGenre>>> {
    let cascade =
        crate::genre::filter::filter_subquery_with_fallback(crate::genre::filter::GalaxyFilterRule::All);
    let sql = format!(
        "WITH RECURSIVE genre_paths(id, parent_id, path) AS (
            SELECT id, parent_id, name
            FROM genres
            WHERE parent_id IS NULL
            UNION ALL
            SELECT g.id, g.parent_id, genre_paths.path || ' > ' || g.name
            FROM genres g
            JOIN genre_paths ON g.parent_id = genre_paths.id
        )
        SELECT cr.track_id, genre_paths.path, cr.source
        FROM ({cascade}) cr
        JOIN genre_paths ON genre_paths.id = cr.genre_id
        ORDER BY cr.track_id, genre_paths.path"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;

    let mut by_track: HashMap<i64, Vec<ResolvedGenre>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let track_id: i64 = row.get(0)?;
        let path: String = row.get(1)?;
        let src: String = row.get(2)?;
        by_track.entry(track_id).or_default().push(ResolvedGenre {
            path,
            source: GenreSource::from_sql_source(&src),
        });
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

pub fn get_genres_filtered(
    conn: &Connection,
    filter: crate::genre::filter::GalaxyFilterRule,
) -> Result<Vec<Genre>> {
    let sub = crate::genre::filter::filter_subquery(filter);
    let sql = format!(
        "SELECT g.id, g.name, g.slug, g.parent_id, COUNT(tg.track_id) AS track_count
         FROM genres g
         LEFT JOIN ({sub}) tg ON tg.genre_id = g.id
         GROUP BY g.id, g.name, g.slug, g.parent_id
         ORDER BY COALESCE(g.parent_id, g.id), g.name ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let genres = stmt
        .query_map([], genre_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(genres)
}

pub fn get_genre_tree_filtered(
    conn: &Connection,
    filter: crate::genre::filter::GalaxyFilterRule,
) -> Result<Vec<Genre>> {
    let genres = get_genres_filtered(conn, filter)?;
    Ok(build_genre_tree(genres))
}

pub fn get_genre_heat_filtered(
    conn: &Connection,
    days: i64,
    filter: crate::genre::filter::GalaxyFilterRule,
) -> Result<Vec<GenreHeat>> {
    let sub = crate::genre::filter::filter_subquery(filter);
    let sql = format!(
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
        LEFT JOIN ({sub}) tg ON tg.genre_id = closure.genre_id
        LEFT JOIN listen_history lh
            ON lh.track_id = tg.track_id
           AND lh.started_at >= datetime('now', printf('-%d days', ?1))
        GROUP BY g.id, g.name
        ORDER BY COALESCE(g.parent_id, g.id), g.name ASC"
    );
    let mut stmt = conn.prepare(&sql)?;

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
#[allow(dead_code)]
pub struct GenreSummary {
    pub genre_id: i64,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<i64>,
    pub direct_track_count: i64,
    pub total_track_count: i64,
    pub child_count: usize,
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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

pub fn get_tracks_by_genre_filtered(
    conn: &Connection,
    genre_id: i64,
    include_descendants: bool,
    filter: crate::genre::filter::GalaxyFilterRule,
) -> Result<Vec<Track>> {
    if !genre_exists(conn, genre_id)? {
        return Ok(Vec::new());
    }

    let sub = crate::genre::filter::filter_subquery(filter);
    // The Spotify-dominance EXISTS check still queries raw `track_genres` —
    // it's a "did Spotify ever tag this track at all" predicate, independent
    // of the confidence filter that decides which clusters the track is
    // visible in. The MAIN membership join uses the filtered rowset.
    let sql = if include_descendants {
        format!(
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
             JOIN ({sub}) tg ON tg.genre_id = sg.id
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
        )
    } else {
        format!(
            "SELECT DISTINCT t.id, t.title, t.artist_id, a.name, t.album_id, al.title,
                    t.disc_number, t.track_number, t.duration_ms, t.isrc,
                    t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                    t.best_quality, t.best_source, t.fidelity_score,
                    t.is_favorite, t.play_count, t.last_played_at,
                    t.date_added, t.source, al.artwork_url
             FROM ({sub}) tg
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
        )
    };

    let mut stmt = conn.prepare(&sql)?;
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

#[allow(dead_code)]
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
    session_id: Option<&str>,
    source: Option<ListenSource>,
    position_in_session: Option<i32>,
    transition_from_track_id: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO listen_history
            (track_id, started_at, duration_listened_ms, completed,
             session_id, source, position_in_session, transition_from_track_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            track_id,
            started_at,
            duration_listened_ms.max(0),
            completed as i32,
            session_id,
            source.map(|s| s.as_str()),
            position_in_session,
            transition_from_track_id,
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
                lh.started_at, COALESCE(lh.duration_listened_ms, 0), lh.completed,
                lh.session_id, lh.source, lh.position_in_session, lh.transition_from_track_id
         FROM listen_history lh
         JOIN tracks t ON lh.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         ORDER BY lh.started_at DESC, lh.id DESC
         LIMIT ?1",
    )?;

    let listens = stmt
        .query_map(params![limit], |row| {
            let source_raw: Option<String> = row.get(10)?;
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
                session_id: row.get(9)?,
                source: source_raw.as_deref().and_then(ListenSource::parse),
                position_in_session: row.get(11)?,
                transition_from_track_id: row.get(12)?,
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
        "SELECT DATE(started_at, 'localtime') AS day,
                COUNT(*) AS listens,
                COALESCE(SUM(CASE WHEN completed = 1 THEN 1 ELSE 0 END), 0) AS completed_listens,
                COALESCE(SUM(duration_listened_ms), 0) AS listened_ms
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY DATE(started_at, 'localtime')
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

// ─── Analytics Signals ──────────────────────────────────────────────────────
// Backend for GET /api/analytics/signals — the visual-overhaul analytics page.
// Spec: noor-server/tests/fixtures/signals-schema.json
// Spot-check: noor-server/tests/manual/analytics-spot-check.sql

pub const BPM_MIN: i32 = 60;
pub const BPM_MAX: i32 = 200;
pub const BPM_STEP: i32 = 4;
/// Dense bucket count = (max - min) / step. Buckets cover [60, 64, ..., 196] — 35 entries.
/// The TempoRow.buckets array always has exactly this length.
pub const BPM_BUCKET_COUNT: usize = ((BPM_MAX - BPM_MIN) / BPM_STEP) as usize;

// Night = 22:00–04:00 inclusive (7 hours). Morning = 05:00–09:00 inclusive (5 hours).
// The hour lists are inlined into the SQL where they're consumed (see get_signals_hero_stats);
// the labels live there as the canonical source of truth.

pub const SONIC_FIELD_LIMIT: i64 = 1500;
const COHORT_NEW_DAYS: i64 = 30;
const COHORT_DEEP_DAYS: i64 = 180;
const COHORT_DEEP_LIFETIME_LISTENS: i64 = 5;
const MONTH_ROW_CAP: usize = 24;
const RIDGELINE_DAY_CAP: i64 = 365;

/// Granularity selection — locked fallback rule.
///
///   1..=7   → Day (always)
///   8..=30  → Day by default; fall back to Week when ridges would be mostly empty:
///             distinct_days < 15 OR median listens-per-day < 5
///   31..=90 → Week
///   _       → Month (capped at 24 rows downstream)
pub fn select_granularity(conn: &Connection, days: i64) -> Result<Granularity> {
    let base = match days {
        1..=7 => Granularity::Day,
        8..=30 => Granularity::Day,
        31..=90 => Granularity::Week,
        _ => Granularity::Month,
    };
    if !(8..=30).contains(&days) {
        return Ok(base);
    }
    let (distinct_days, median_per_day) = compute_30d_density(conn, days)?;
    if distinct_days < 15 || median_per_day < 5.0 {
        Ok(Granularity::Week)
    } else {
        Ok(base)
    }
}

fn compute_30d_density(conn: &Connection, days: i64) -> Result<(i64, f64)> {
    let distinct_days: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT DATE(started_at))
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))",
        params![days],
        |row| row.get(0),
    )?;
    if distinct_days == 0 {
        return Ok((0, 0.0));
    }
    let mut stmt = conn.prepare(
        "SELECT COUNT(*) FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY DATE(started_at)
         ORDER BY COUNT(*) ASC",
    )?;
    let counts: Vec<i64> = stmt
        .query_map(params![days], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<_>>()?;
    let median = median_of_sorted(&counts).unwrap_or(0.0);
    Ok((distinct_days, median))
}

fn median_of_sorted(sorted: &[i64]) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let n = sorted.len();
    if n % 2 == 1 {
        Some(sorted[n / 2] as f64)
    } else {
        let mid = n / 2;
        Some((sorted[mid - 1] + sorted[mid]) as f64 / 2.0)
    }
}

/// Rhythm CV formula. Returns None if fewer than 5 days have any listens.
///
/// For each day d in window:
///   σ_d = stddev(listens per hour across 24 hours of day d)
/// mean_σ = average of σ_d over days with listens
/// mean_listens = mean hourly listens across window (per hour-slot, total/days/24)
/// cv = mean_σ / mean_listens
/// rhythm = round(100 * clamp(1 - cv, 0, 1))
///
/// CV form so a quiet week and a busy week with the same routine score identically.
pub fn compute_rhythm(per_day_per_hour: &[[i64; 24]]) -> Option<i32> {
    let active: Vec<&[i64; 24]> = per_day_per_hour
        .iter()
        .filter(|hours| hours.iter().any(|&h| h > 0))
        .collect();
    if active.len() < 5 {
        return None;
    }
    let mut total_listens: f64 = 0.0;
    let mut sigma_sum: f64 = 0.0;
    for hours in &active {
        let mean = hours.iter().map(|&h| h as f64).sum::<f64>() / 24.0;
        let var = hours
            .iter()
            .map(|&h| {
                let diff = h as f64 - mean;
                diff * diff
            })
            .sum::<f64>()
            / 24.0;
        sigma_sum += var.sqrt();
        total_listens += mean * 24.0;
    }
    let mean_sigma = sigma_sum / active.len() as f64;
    let mean_listens_per_hour = total_listens / (active.len() as f64 * 24.0);
    if mean_listens_per_hour == 0.0 {
        return Some(0);
    }
    let cv = mean_sigma / mean_listens_per_hour;
    let rhythm = (100.0 * (1.0 - cv).clamp(0.0, 1.0)).round() as i32;
    Some(rhythm)
}

/// Listen-weighted median over (bpm, listens) pairs. Mathematically identical to
/// expanding to a per-listen vector and taking its median, without the memory cost.
fn weighted_median_bpm(weighted: &[(f64, i64)]) -> Option<f64> {
    if weighted.is_empty() {
        return None;
    }
    let mut pairs: Vec<(f64, i64)> = weighted.to_vec();
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let total: i64 = pairs.iter().map(|(_, w)| *w).sum();
    if total == 0 {
        return None;
    }
    let half = total as f64 / 2.0;
    let mut cum: f64 = 0.0;
    for (i, (bpm, w)) in pairs.iter().enumerate() {
        let next = cum + *w as f64;
        if next >= half {
            // For even totals where we land exactly on the boundary, average with the next.
            if (next - half).abs() < f64::EPSILON && i + 1 < pairs.len() {
                return Some((bpm + pairs[i + 1].0) / 2.0);
            }
            return Some(*bpm);
        }
        cum = next;
    }
    pairs.last().map(|(b, _)| *b)
}

/// Listen-weighted stddev over (bpm, listens) pairs.
fn weighted_stddev_bpm(weighted: &[(f64, i64)]) -> Option<f64> {
    let total: i64 = weighted.iter().map(|(_, w)| *w).sum();
    if total < 2 {
        return None;
    }
    let total_f = total as f64;
    let mean: f64 = weighted.iter().map(|(b, w)| b * *w as f64).sum::<f64>() / total_f;
    let variance: f64 = weighted
        .iter()
        .map(|(b, w)| {
            let diff = b - mean;
            diff * diff * (*w as f64)
        })
        .sum::<f64>()
        / total_f;
    Some(variance.sqrt())
}

/// Window bookkeeping — cur/prev start timestamps formatted to match SQLite's `datetime('now', '-Xd')`.
pub fn build_signals_window(days: i64) -> SignalsWindow {
    let started_at_sql = format!("datetime('now', '-{} days')", days);
    let prev_started_at_sql = format!("datetime('now', '-{} days')", days * 2);
    SignalsWindow {
        days,
        started_at: started_at_sql,
        previous_started_at: prev_started_at_sql,
    }
}

// ─── KPI window: listened_ms / sessions / completion / skip_rate (cur+prev) ──

pub fn get_signals_kpis(conn: &Connection, days: i64) -> Result<SignalsKpis> {
    let cur_offset = days;
    let prev_offset = days * 2;

    let (cur_listens, cur_completed, cur_ms, prev_listens, prev_completed, prev_ms): (
        i64, i64, i64, i64, i64, i64,
    ) = conn.query_row(
        "SELECT
            COUNT(*) FILTER (WHERE started_at >= datetime('now', printf('-%d days', ?1))),
            COUNT(*) FILTER (WHERE started_at >= datetime('now', printf('-%d days', ?1)) AND completed = 1),
            COALESCE(SUM(duration_listened_ms) FILTER (WHERE started_at >= datetime('now', printf('-%d days', ?1))), 0),
            COUNT(*) FILTER (WHERE started_at >= datetime('now', printf('-%d days', ?2)) AND started_at < datetime('now', printf('-%d days', ?1))),
            COUNT(*) FILTER (WHERE started_at >= datetime('now', printf('-%d days', ?2)) AND started_at < datetime('now', printf('-%d days', ?1)) AND completed = 1),
            COALESCE(SUM(duration_listened_ms) FILTER (WHERE started_at >= datetime('now', printf('-%d days', ?2)) AND started_at < datetime('now', printf('-%d days', ?1))), 0)
         FROM listen_history",
        params![cur_offset, prev_offset],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
    )?;

    // Sessions (post-MIGRATION_023 only — session_id IS NULL on older history).
    let (cur_sessions, prev_sessions): (i64, i64) = conn.query_row(
        "SELECT
            COUNT(DISTINCT CASE WHEN started_at >= datetime('now', printf('-%d days', ?1)) AND session_id IS NOT NULL THEN session_id END),
            COUNT(DISTINCT CASE WHEN started_at >= datetime('now', printf('-%d days', ?2)) AND started_at < datetime('now', printf('-%d days', ?1)) AND session_id IS NOT NULL THEN session_id END)
         FROM listen_history",
        params![cur_offset, prev_offset],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let (sessions_tracked, sessions_untracked): (i64, i64) = conn.query_row(
        "SELECT
            COUNT(*) FILTER (WHERE session_id IS NOT NULL),
            COUNT(*) FILTER (WHERE session_id IS NULL)
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))",
        params![cur_offset],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    // Daily series for the MiniSilhouette curves.
    let mut daily_stmt = conn.prepare(
        "SELECT
            DATE(started_at, 'localtime') AS day,
            COUNT(*) AS listens,
            COALESCE(SUM(duration_listened_ms), 0) AS listened_ms,
            COALESCE(SUM(CASE WHEN completed = 1 THEN 1 ELSE 0 END), 0) AS completed
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY DATE(started_at, 'localtime')
         ORDER BY day ASC",
    )?;
    let daily: Vec<DailyKpi> = daily_stmt
        .query_map(params![cur_offset], |row| {
            Ok(DailyKpi {
                day: row.get(0)?,
                listens: row.get(1)?,
                listened_ms: row.get(2)?,
                completed: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    let completion = ratio_or_none(cur_completed, cur_listens);
    let prev_completion = ratio_or_none(prev_completed, prev_listens);

    let kpis = SignalsKpis {
        listened_ms: KpiPairInt { current: cur_ms, previous: prev_ms },
        sessions: KpiPairInt { current: cur_sessions, previous: prev_sessions },
        completion: KpiPairFloat { current: completion, previous: prev_completion },
        skip_rate: KpiPairFloat {
            current: completion.map(|c| 1.0 - c),
            previous: prev_completion.map(|c| 1.0 - c),
        },
        daily,
        hero_stats: get_signals_hero_stats(conn, days)?,
        sessions_coverage: SessionsCoverage {
            tracked: sessions_tracked,
            untracked: sessions_untracked,
        },
    };

    Ok(kpis)
}

fn ratio_or_none(num: i64, denom: i64) -> Option<f64> {
    if denom == 0 {
        None
    } else {
        Some(num as f64 / denom as f64)
    }
}

// ─── Hero stats ─────────────────────────────────────────────────────────────

fn get_signals_hero_stats(conn: &Connection, days: i64) -> Result<HeroStats> {
    // Peak hour: hour-of-day with max total listens, tie-break earliest.
    let peak_hour: Option<i32> = conn
        .query_row(
            "SELECT CAST(strftime('%H', started_at, 'localtime') AS INTEGER) AS h
             FROM listen_history
             WHERE started_at >= datetime('now', printf('-%d days', ?1))
             GROUP BY h
             ORDER BY COUNT(*) DESC, h ASC
             LIMIT 1",
            params![days],
            |row| row.get::<_, i32>(0),
        )
        .optional()?;

    // Per-day per-hour matrix (zero-filled) for Rhythm.
    let mut hour_stmt = conn.prepare(
        "SELECT DATE(started_at, 'localtime') AS day, CAST(strftime('%H', started_at, 'localtime') AS INTEGER) AS h, COUNT(*) AS c
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY day, h
         ORDER BY day, h",
    )?;
    let mut day_map: HashMap<String, [i64; 24]> = HashMap::new();
    let rows = hour_stmt.query_map(params![days], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i32>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for r in rows {
        let (day, h, c) = r?;
        let entry = day_map.entry(day).or_insert([0i64; 24]);
        if (0..24).contains(&h) {
            entry[h as usize] = c;
        }
    }
    let per_day: Vec<[i64; 24]> = day_map.values().copied().collect();
    let rhythm = compute_rhythm(&per_day);

    // Night / Morning shares — None when there are no listens in window.
    let (total, night, morning): (i64, i64, i64) = conn.query_row(
        "SELECT
            COUNT(*),
            COUNT(*) FILTER (WHERE CAST(strftime('%H', started_at, 'localtime') AS INTEGER) IN (22, 23, 0, 1, 2, 3, 4)),
            COUNT(*) FILTER (WHERE CAST(strftime('%H', started_at, 'localtime') AS INTEGER) IN (5, 6, 7, 8, 9))
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))",
        params![days],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let (night_share, morning_share) = if total == 0 {
        (None, None)
    } else {
        (
            Some(night as f64 / total as f64),
            Some(morning as f64 / total as f64),
        )
    };

    // Single-day mode (days <= 1) populates the two extra spine stats.
    let (longest_session_ms, distinct_tracks) = if days <= 1 {
        let longest: Option<i64> = conn
            .query_row(
                "SELECT MAX(session_total) FROM (
                     SELECT session_id, SUM(duration_listened_ms) AS session_total
                     FROM listen_history
                     WHERE started_at >= datetime('now', printf('-%d days', ?1))
                       AND session_id IS NOT NULL
                     GROUP BY session_id
                 )",
                params![days],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten();
        let distinct: Option<i64> = conn
            .query_row(
                "SELECT COUNT(DISTINCT track_id)
                 FROM listen_history
                 WHERE started_at >= datetime('now', printf('-%d days', ?1))",
                params![days],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        (longest, distinct)
    } else {
        (None, None)
    };

    Ok(HeroStats {
        peak_hour: if total == 0 { None } else { peak_hour },
        rhythm,
        night_share,
        morning_share,
        longest_session_ms,
        distinct_tracks,
    })
}

// ─── Tempo ──────────────────────────────────────────────────────────────────

pub fn get_signals_tempo(
    conn: &Connection,
    days: i64,
    granularity: Granularity,
) -> Result<TempoView> {
    // Per-row × per-bucket aggregation (label, bucket, listens) over the window.
    let label_expr = match granularity {
        Granularity::Day => "DATE(lh.started_at, 'localtime')",
        Granularity::Week => "strftime('%Y-%U', lh.started_at, 'localtime')", // %U = Sunday-start (NOT %W)
        Granularity::Month => "strftime('%Y-%m', lh.started_at, 'localtime')",
    };
    let sql = format!(
        "SELECT
            {label_expr} AS label,
            (CAST(adf.bpm AS INTEGER) / {step}) * {step} AS bucket,
            COUNT(*) AS listens
         FROM listen_history lh
         JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
           AND adf.bpm >= {min} AND adf.bpm < {max}
         GROUP BY label, bucket
         ORDER BY label, bucket",
        label_expr = label_expr,
        step = BPM_STEP,
        min = BPM_MIN,
        max = BPM_MAX,
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(String, i32, i64)> = stmt
        .query_map(params![days], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?, row.get::<_, i64>(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    // Group by label, dense-fill buckets.
    let mut per_label: Vec<(String, Vec<BpmBucket>)> = Vec::new();
    let mut current: Option<(String, Vec<BpmBucket>)> = None;
    for (label, bucket, listens) in &rows {
        if current.as_ref().map(|(l, _)| l) != Some(label) {
            if let Some(prev) = current.take() {
                per_label.push(prev);
            }
            current = Some((label.clone(), dense_buckets()));
        }
        if let Some((_, buckets)) = current.as_mut()
            && let Some(bb) = buckets.iter_mut().find(|b| b.bucket == *bucket)
        {
            bb.listens = *listens;
        }
    }
    if let Some(prev) = current.take() {
        per_label.push(prev);
    }

    // Cap month rows at the most recent 24; cap day rows at 365.
    let cap = match granularity {
        Granularity::Day => RIDGELINE_DAY_CAP as usize,
        Granularity::Week => usize::MAX,
        Granularity::Month => MONTH_ROW_CAP,
    };
    if per_label.len() > cap {
        let skip = per_label.len() - cap;
        per_label = per_label.into_iter().skip(skip).collect();
    }

    let tempo_rows: Vec<TempoRow> = per_label
        .into_iter()
        .map(|(label, buckets)| TempoRow { label, granularity, buckets })
        .collect();

    // Per-listen weighted stats. Same query, but the (bpm, listens) pairs aggregate
    // across the whole window so popular tracks dominate the median/mode/sigma.
    let mut weighted_stmt = conn.prepare(&format!(
        "SELECT adf.bpm, COUNT(*) AS listens
         FROM listen_history lh
         JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
           AND adf.bpm >= {min} AND adf.bpm < {max}
         GROUP BY adf.bpm",
        min = BPM_MIN, max = BPM_MAX
    ))?;
    let weighted: Vec<(f64, i64)> = weighted_stmt
        .query_map(params![days], |row| Ok((row.get::<_, f64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<_>>()?;

    let median = weighted_median_bpm(&weighted);
    let sigma = weighted_stddev_bpm(&weighted);
    // Mode = bucket centre (lower-edge + step/2) of the listens-argmax bucket.
    let mode = mode_bucket_centre(&tempo_rows);

    let stats = TempoStats { median, mode, sigma };

    // Coverage: analysed tracks / total listened tracks within the window.
    let total_listened: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT track_id)
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))",
        params![days],
        |row| row.get(0),
    )?;
    let analyzed: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT lh.track_id)
         FROM listen_history lh
         JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
           AND adf.bpm IS NOT NULL",
        params![days],
        |row| row.get(0),
    )?;

    // ridge_amp_max = P95 across all per-row per-bucket density values.
    let mut all_listens: Vec<i64> = tempo_rows
        .iter()
        .flat_map(|r| r.buckets.iter().map(|b| b.listens))
        .collect();
    all_listens.sort_unstable();
    let ridge_amp_max = percentile_i64(&all_listens, 95.0).unwrap_or(0.0);

    Ok(TempoView {
        bucket_axis: BucketAxis { min: BPM_MIN, max: BPM_MAX, step: BPM_STEP },
        rows: tempo_rows,
        stats,
        coverage: Coverage { analyzed, total_listened },
        ridge_amp_max,
    })
}

fn dense_buckets() -> Vec<BpmBucket> {
    (0..BPM_BUCKET_COUNT)
        .map(|i| BpmBucket {
            bucket: BPM_MIN + (i as i32) * BPM_STEP,
            listens: 0,
        })
        .collect()
}

fn mode_bucket_centre(rows: &[TempoRow]) -> Option<f64> {
    let mut totals: HashMap<i32, i64> = HashMap::new();
    for r in rows {
        for b in &r.buckets {
            *totals.entry(b.bucket).or_insert(0) += b.listens;
        }
    }
    let (best_bucket, best_listens) = totals.into_iter().max_by_key(|(_, l)| *l)?;
    if best_listens == 0 {
        return None;
    }
    Some(best_bucket as f64 + (BPM_STEP as f64) / 2.0)
}

fn percentile_i64(sorted_asc: &[i64], pct: f64) -> Option<f64> {
    if sorted_asc.is_empty() {
        return None;
    }
    let n = sorted_asc.len();
    let rank = (pct / 100.0) * (n as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        return Some(sorted_asc[lo] as f64);
    }
    let frac = rank - lo as f64;
    Some(sorted_asc[lo] as f64 * (1.0 - frac) + sorted_asc[hi] as f64 * frac)
}

// ─── Sonic field ────────────────────────────────────────────────────────────

pub fn get_signals_sonic_field(conn: &Connection, days: i64) -> Result<SonicView> {
    let mut stmt = conn.prepare(
        "SELECT
            lh.track_id,
            t.title,
            ar.name AS artist_name,
            al.title AS album,
            al.artwork_url AS artwork_path,
            t.file_path,
            adf.energy,
            adf.danceability,
            adf.bpm,
            COUNT(*) AS listens
         FROM listen_history lh
         JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
         JOIN tracks t ON t.id = lh.track_id
         LEFT JOIN artists ar ON ar.id = t.artist_id
         LEFT JOIN albums al ON al.id = t.album_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
           AND adf.energy IS NOT NULL
           AND adf.danceability IS NOT NULL
           AND adf.bpm IS NOT NULL
           AND adf.bpm >= ?2 AND adf.bpm < ?3
         GROUP BY lh.track_id, t.title, ar.name, al.title, al.artwork_url, t.file_path, adf.energy, adf.danceability, adf.bpm
         ORDER BY listens DESC, t.title ASC
         LIMIT ?4",
    )?;
    let tracks: Vec<SonicTrack> = stmt
        .query_map(
            params![days, BPM_MIN as f64, BPM_MAX as f64, SONIC_FIELD_LIMIT],
            |row| {
                Ok(SonicTrack {
                    track_id: row.get(0)?,
                    title: row.get(1)?,
                    artist_name: row.get(2)?,
                    album: row.get(3)?,
                    artwork_path: row.get(4)?,
                    file_path: row.get(5)?,
                    e: row.get(6)?,
                    d: row.get(7)?,
                    bpm: row.get(8)?,
                    listens: row.get(9)?,
                })
            },
        )?
        .collect::<rusqlite::Result<_>>()?;

    let total_listened: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT track_id)
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))",
        params![days],
        |row| row.get(0),
    )?;
    let analyzed: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT lh.track_id)
         FROM listen_history lh
         JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
           AND adf.energy IS NOT NULL AND adf.danceability IS NOT NULL AND adf.bpm IS NOT NULL",
        params![days],
        |row| row.get(0),
    )?;

    let total = tracks.len() as i64;
    Ok(SonicView {
        tracks,
        total,
        coverage: Coverage { analyzed, total_listened },
    })
}

// ─── Ridgeline (hero) ───────────────────────────────────────────────────────

pub fn get_signals_ridgeline(conn: &Connection, days: i64) -> Result<Vec<RidgeRow>> {
    // Cap at 365 days to keep the SVG sane; longer windows render the most-recent year.
    let effective = days.min(RIDGELINE_DAY_CAP);

    // Pull the per-day per-hour listens, then zero-fill the date axis so every day in the
    // window renders even if it has no listens. Per the plan: "one ridge per day in the
    // chosen window" — a flat row IS the ridge for an empty day.
    let mut stmt = conn.prepare(
        "SELECT
            DATE(started_at, 'localtime') AS day,
            CAST(strftime('%H', started_at, 'localtime') AS INTEGER) AS hour,
            COUNT(*) AS listens
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY day, hour
         ORDER BY day, hour",
    )?;
    let mut by_day_map: std::collections::BTreeMap<String, [i64; 24]> = std::collections::BTreeMap::new();
    let rows = stmt.query_map(params![effective], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i32>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for r in rows {
        let (day, hour, listens) = r?;
        let entry = by_day_map.entry(day).or_insert([0i64; 24]);
        if (0..24).contains(&hour) {
            entry[hour as usize] = listens;
        }
    }

    // Build the canonical date axis (oldest → newest, inclusive of today).
    let axis_dates: Vec<String> = conn
        .prepare(
            "WITH RECURSIVE axis(d) AS (
                SELECT DATE(datetime('now', 'localtime', printf('-%d days', ?1 - 1)))
                UNION ALL
                SELECT DATE(d, '+1 day') FROM axis WHERE d < DATE('now', 'localtime')
            )
            SELECT d FROM axis",
        )?
        .query_map(params![effective], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;

    Ok(axis_dates
        .into_iter()
        .map(|date| {
            let hourly = by_day_map
                .get(&date)
                .copied()
                .unwrap_or([0i64; 24])
                .to_vec();
            RidgeRow { date, hourly }
        })
        .collect())
}

// ─── Windowed top tracks / artists / genres ─────────────────────────────────

pub fn get_top_tracks_windowed(
    conn: &Connection,
    days: i64,
    limit: i64,
) -> Result<Vec<AnalyticsTopTrack>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url,
                COUNT(lh.id) AS listens,
                COALESCE(SUM(CASE WHEN lh.completed = 1 THEN 1 ELSE 0 END), 0) AS completed_listens,
                COALESCE(SUM(lh.duration_listened_ms), 0) AS total_listened_ms
         FROM listen_history lh
         JOIN tracks t ON lh.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY t.id, t.title, a.name, al.title, al.artwork_url
         ORDER BY listens DESC, total_listened_ms DESC, t.title ASC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![days, limit], |row| {
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
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_top_artists_windowed(
    conn: &Connection,
    days: i64,
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
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY a.id, a.name
         ORDER BY listens DESC, total_listened_ms DESC, a.name ASC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![days, limit], |row| {
            Ok(AnalyticsTopArtist {
                artist_id: row.get(0)?,
                artist_name: row.get(1)?,
                listens: row.get(2)?,
                completed_listens: row.get(3)?,
                unique_tracks: row.get(4)?,
                total_listened_ms: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_top_genres_windowed(
    conn: &Connection,
    days: i64,
    limit: i64,
) -> Result<Vec<AnalyticsGenreShare>> {
    let mut stmt = conn.prepare(
        "SELECT g.name, COUNT(lh.id) AS listens
         FROM listen_history lh
         JOIN track_genres tg ON lh.track_id = tg.track_id
         JOIN genres g ON tg.genre_id = g.id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
         GROUP BY g.id, g.name
         ORDER BY listens DESC, g.name ASC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![days, limit], |row| {
            Ok(AnalyticsGenreShare {
                genre_name: row.get(0)?,
                listens: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ─── Cohorts ────────────────────────────────────────────────────────────────

pub fn get_signals_cohorts(conn: &Connection, days: i64) -> Result<Vec<Cohort>> {
    // Per-track first_at + lifetime_listens via the new idx_listen_history_track_started index.
    let sql = "
        WITH first_listens AS (
            SELECT track_id,
                   MIN(started_at) AS first_at,
                   COUNT(*) AS lifetime_listens
            FROM listen_history
            GROUP BY track_id
        ),
        windowed AS (
            SELECT lh.id, lh.track_id, lh.duration_listened_ms, lh.completed, lh.session_id
            FROM listen_history lh
            WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))
        ),
        joined AS (
            SELECT
                w.id, w.track_id, w.duration_listened_ms, w.completed, w.session_id,
                fl.first_at, fl.lifetime_listens,
                t.artist_id,
                CASE
                    WHEN fl.first_at >= datetime('now', printf('-%d days', ?1)) THEN 'new_this_month'
                    WHEN fl.first_at < datetime('now', printf('-%d days', ?2))
                         AND fl.lifetime_listens >= ?3 THEN 'deep_cuts'
                    ELSE 'established'
                END AS cohort_key
            FROM windowed w
            JOIN first_listens fl ON fl.track_id = w.track_id
            JOIN tracks t ON t.id = w.track_id
        )
        SELECT
            cohort_key,
            COUNT(DISTINCT track_id) AS tracks,
            COALESCE(SUM(duration_listened_ms), 0) AS listened_ms,
            COUNT(DISTINCT CASE WHEN session_id IS NOT NULL THEN session_id END) AS sessions,
            COUNT(*) AS listens,
            COALESCE(SUM(CASE WHEN completed = 1 THEN 1 ELSE 0 END), 0) AS completed_listens,
            COUNT(DISTINCT CASE WHEN first_at >= datetime('now', printf('-%d days', ?1)) THEN artist_id END) AS new_artists
        FROM joined
        GROUP BY cohort_key
    ";
    let mut stmt = conn.prepare(sql)?;
    let rows: HashMap<String, (i64, i64, i64, i64, i64, i64)> = stmt
        .query_map(
            params![days, COHORT_DEEP_DAYS, COHORT_DEEP_LIFETIME_LISTENS],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (
                        row.get::<_, i64>(1)?, // tracks
                        row.get::<_, i64>(2)?, // listened_ms
                        row.get::<_, i64>(3)?, // sessions
                        row.get::<_, i64>(4)?, // listens
                        row.get::<_, i64>(5)?, // completed_listens
                        row.get::<_, i64>(6)?, // new_artists
                    ),
                ))
            },
        )?
        .collect::<rusqlite::Result<_>>()?;

    let _ = COHORT_NEW_DAYS; // current cohort window matches `days`; reserved for future split.

    let labels = [
        ("new_this_month", "New this month"),
        ("established", "Established"),
        ("deep_cuts", "Deep cuts"),
    ];
    let mut out: Vec<Cohort> = Vec::with_capacity(3);
    for (key, label) in labels {
        let (tracks, listened_ms, sessions, listens, completed, new_artists) = rows
            .get(key)
            .copied()
            .unwrap_or((0, 0, 0, 0, 0, 0));
        let completion = ratio_or_none(completed, listens);
        let skip_rate = completion.map(|c| 1.0 - c);
        let repeat_rate = if tracks == 0 {
            None
        } else {
            Some(listens as f64 / tracks as f64)
        };
        out.push(Cohort {
            key: key.to_string(),
            label: label.to_string(),
            tracks,
            listened_ms,
            sessions,
            completion,
            skip_rate,
            new_artists,
            repeat_rate,
        });
    }
    Ok(out)
}

// ─── Audio profile ──────────────────────────────────────────────────────────

pub fn get_signals_audio_profile(conn: &Connection, days: i64) -> Result<AudioProfile> {
    // Listen-weighted loudness vector + spectral centroid mean.
    let mut stmt = conn.prepare(
        "SELECT adf.loudness_lufs, adf.spectral_centroid
         FROM listen_history lh
         JOIN audio_dsp_features adf ON adf.track_id = lh.track_id
         WHERE lh.started_at >= datetime('now', printf('-%d days', ?1))",
    )?;
    let pairs: Vec<(Option<f64>, Option<f64>)> = stmt
        .query_map(params![days], |row| {
            Ok((row.get::<_, Option<f64>>(0)?, row.get::<_, Option<f64>>(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let loudness_vals: Vec<f64> = pairs.iter().filter_map(|(l, _)| *l).collect();
    let centroid_vals: Vec<f64> = pairs.iter().filter_map(|(_, c)| *c).collect();
    let analyzed = loudness_vals.len().max(centroid_vals.len()) as i64;

    let total_listened: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT track_id)
         FROM listen_history
         WHERE started_at >= datetime('now', printf('-%d days', ?1))",
        params![days],
        |row| row.get(0),
    )?;

    let loudness_lufs = if loudness_vals.is_empty() {
        None
    } else {
        Some(loudness_vals.iter().sum::<f64>() / loudness_vals.len() as f64)
    };

    let dynamic_range_dr = if loudness_vals.len() < 5 {
        None
    } else {
        let mut sorted = loudness_vals.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p5 = percentile_f64(&sorted, 5.0);
        let p95 = percentile_f64(&sorted, 95.0);
        match (p5, p95) {
            (Some(lo), Some(hi)) => Some((hi - lo).max(0.0)),
            _ => None,
        }
    };

    let bass_tilt = if centroid_vals.is_empty() {
        None
    } else {
        let mean = centroid_vals.iter().sum::<f64>() / centroid_vals.len() as f64;
        if mean <= 0.0 {
            None
        } else {
            // bass_tilt = clamp(20 * log10(2000 / mean_centroid), -6, +6)
            Some((20.0 * (2000.0_f64 / mean).log10()).clamp(-6.0, 6.0))
        }
    };
    let treble_tilt = bass_tilt.map(|b| -b);

    Ok(AudioProfile {
        dynamic_range_dr,
        loudness_lufs,
        bass_tilt,
        treble_tilt,
        coverage: Coverage { analyzed, total_listened },
    })
}

fn percentile_f64(sorted_asc: &[f64], pct: f64) -> Option<f64> {
    if sorted_asc.is_empty() {
        return None;
    }
    let n = sorted_asc.len();
    let rank = (pct / 100.0) * (n as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        return Some(sorted_asc[lo]);
    }
    let frac = rank - lo as f64;
    Some(sorted_asc[lo] * (1.0 - frac) + sorted_asc[hi] * frac)
}

// ─── Top-level signals fetcher ──────────────────────────────────────────────

pub fn get_analytics_signals(conn: &Connection, days: i64) -> Result<AnalyticsSignals> {
    let granularity = select_granularity(conn, days)?;
    let kpis = get_signals_kpis(conn, days)?;
    let tempo = get_signals_tempo(conn, days, granularity)?;
    let sonic_field = get_signals_sonic_field(conn, days)?;
    let ridgeline = get_signals_ridgeline(conn, days)?;
    let top_tracks = get_top_tracks_windowed(conn, days, 5)?;
    let top_artists = get_top_artists_windowed(conn, days, 5)?;
    let top_genres = get_top_genres_windowed(conn, days, 6)?;
    let cohorts = get_signals_cohorts(conn, days)?;
    let audio_profile = get_signals_audio_profile(conn, days)?;
    let window = build_signals_window(days);

    Ok(AnalyticsSignals {
        window,
        kpis,
        tempo,
        sonic_field,
        ridgeline,
        top_tracks,
        top_artists,
        top_genres,
        cohorts,
        audio_profile,
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
    let result = stmt
        .query_row([service], |row| {
            Ok(SyncInfo {
                service: row.get(0)?,
                last_sync_at: row.get(1)?,
                auto_sync_daily: row.get::<_, i64>(2)? != 0,
                last_sync_track_count: row.get(3)?,
                last_sync_album_count: row.get(4)?,
            })
        })
        .optional()?;
    Ok(result)
}

pub fn update_sync_timestamp(
    conn: &Connection,
    service: &str,
    track_count: i64,
    album_count: i64,
) -> Result<()> {
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

/// True if `service` has a recorded `last_sync_at` within `window_secs`.
/// Used by the boot-time auto-sync to honour the "daily" promise instead of
/// running on every server start.
pub fn sync_within_window(conn: &Connection, service: &str, window_secs: i64) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sync_metadata
         WHERE service = ?1
           AND last_sync_at IS NOT NULL
           AND last_sync_at != ''
           AND (strftime('%s','now') - strftime('%s', last_sync_at)) < ?2",
        rusqlite::params![service, window_secs],
        |row| row.get(0),
    )?;
    Ok(count > 0)
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
pub fn get_genre_co_occurrence_filtered(
    conn: &Connection,
    _days: i64,
    _window_minutes: i64,
    min_count: i64,
    filter: crate::genre::filter::GalaxyFilterRule,
) -> Result<Vec<GenreCoOccurrence>> {
    let sub = crate::genre::filter::filter_subquery(filter);
    // Same query as before but built against the filtered rowset. The
    // subquery is inlined twice rather than CTE'd because SQLite doesn't
    // share materialization between CTE references reliably.
    let sql = format!(
        "WITH track_genre_pairs AS (
            SELECT a.genre_id AS genre_a, b.genre_id AS genre_b
            FROM ({sub}) a
            JOIN ({sub}) b ON b.track_id = a.track_id AND b.genre_id > a.genre_id
        ),
        pair_counts AS (
            SELECT genre_a, genre_b, COUNT(*) AS co_count
            FROM track_genre_pairs
            GROUP BY genre_a, genre_b
            HAVING co_count >= ?1
        ),
        genre_totals AS (
            SELECT genre_id, COUNT(DISTINCT track_id) AS total_tracks
            FROM ({sub})
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
        ORDER BY jaccard DESC, pc.co_count DESC"
    );
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map(params![min_count], |row| {
        Ok(GenreCoOccurrence {
            genre_a_id: row.get(0)?,
            genre_a_name: row.get(1)?,
            genre_b_id: row.get(2)?,
            genre_b_name: row.get(3)?,
            co_listen_count: row.get(4)?,
            jaccard: row.get(5)?,
        })
    })?;

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
pub fn get_genre_cohorts_filtered(
    conn: &Connection,
    days: i64,
    filter: crate::genre::filter::GalaxyFilterRule,
    with_fallback: bool,
) -> Result<Vec<GenreCohort>> {
    let _ = days; // bound via ?1 below
    let sub = if with_fallback {
        crate::genre::filter::filter_subquery_with_fallback(filter)
    } else {
        crate::genre::filter::filter_subquery(filter)
    };
    // We bucket listens into 4 time-of-day slots + weekend/weekday
    // Slot 0: 0-6 (Night), Slot 1: 6-12 (Morning), Slot 2: 12-18 (Afternoon), Slot 3: 18-24 (Evening)
    // Then find genres that dominate each slot.
    let sql = format!(
        "WITH recent AS (
            SELECT
                lh.id AS listen_id,
                lh.track_id,
                lh.started_at,
                lh.duration_listened_ms,
                CAST(strftime('%H', lh.started_at, 'localtime') AS INTEGER) AS hour,
                CAST(strftime('%w', lh.started_at, 'localtime') AS INTEGER) AS dow
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
            JOIN ({sub}) tg ON tg.track_id = r.track_id
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
        ORDER BY listens DESC"
    );
    let mut stmt = conn.prepare(&sql)?;

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

    let cohorts = get_genre_cohorts_filtered(
        conn,
        days,
        crate::genre::filter::GalaxyFilterRule::default_rule(),
        true, // with_fallback: cohort labels need to cover empty-genre tracks
    )?;
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
    let ids_csv: String = track_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT track_id, genre_id FROM track_genres WHERE track_id IN ({ids_csv})");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;

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

pub fn get_tidal_track_local_ids(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, i64>> {
    if tidal_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = placeholders(tidal_ids.len());
    let sql = format!("SELECT tidal_id, id FROM tracks WHERE tidal_id IN ({placeholders})");
    let params = params_from_iter(tidal_ids.iter().copied());
    let mut stmt = conn.prepare(&sql)?;
    let mut map = HashMap::new();
    let rows = stmt.query_map(params, |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (tid, lid) = row?;
        map.insert(tid, lid);
    }
    Ok(map)
}

pub fn get_artist_photos_by_tidal_ids(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, String>> {
    if tidal_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = placeholders(tidal_ids.len());
    let sql = format!(
        "SELECT tidal_id, photo_url FROM artists WHERE tidal_id IN ({placeholders}) AND photo_url IS NOT NULL"
    );
    let params = params_from_iter(tidal_ids.iter().copied());
    let mut stmt = conn.prepare(&sql)?;
    let mut map = HashMap::new();
    let rows = stmt.query_map(params, |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (tid, photo) = row?;
        map.insert(tid, photo);
    }
    Ok(map)
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

// ─── Search (FTS5 + LIKE fallback) ────────────────────────────────────────

/// Strip FTS5 special chars and append `*` to each token for prefix matching.
fn to_fts_query(input: &str) -> String {
    let clean: String = input
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect();
    clean
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| format!("{}*", t))
        .collect::<Vec<_>>()
        .join(" ")
}

fn search_tracks_fts(conn: &Connection, fts_query: &str, limit: i64) -> Result<Vec<Track>> {
    // Positional ORDER BY (17/18/16/2) instead of named columns: SQLite rejects
    // bare column names in compound-SELECT (UNION) ORDER BY when the SELECTs
    // contain JOINs, with "1st ORDER BY term does not match any column in the
    // result set". Positional indices sidestep the resolver entirely. Mapping:
    //   2  = t.title
    //   16 = t.fidelity_score
    //   17 = t.is_favorite
    //   18 = t.play_count
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
         JOIN tracks_fts ON tracks_fts.rowid = t.id
         WHERE tracks_fts MATCH ?1
         UNION
         SELECT t.id, t.title, t.artist_id, a.name, t.album_id, al.title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         JOIN artists_fts ON artists_fts.rowid = t.artist_id
         WHERE artists_fts MATCH ?1
         ORDER BY 17 DESC, 18 DESC, 16 DESC, 2 ASC
         LIMIT ?2",
    )?;
    stmt.query_map(params![fts_query, limit], track_from_row)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn search_tracks_like(conn: &Connection, normalized: &str, limit: i64) -> Result<Vec<Track>> {
    let contains_pattern = format!("%{normalized}%");
    let prefix_pattern = format!("{normalized}%");
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
    stmt.query_map(
        params![contains_pattern, normalized, prefix_pattern, limit],
        track_from_row,
    )?
    .collect::<Result<Vec<_>, _>>()
    .map_err(Into::into)
}

fn search_artists_fts(conn: &Connection, fts_query: &str, limit: i64) -> Result<Vec<Artist>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.tidal_id, a.ytmusic_id, a.soundcloud_id,
                a.name, a.name_sort, a.biography, a.photo_url
         FROM artists a
         JOIN artists_fts ON artists_fts.rowid = a.id
         WHERE artists_fts MATCH ?1
         ORDER BY a.name ASC
         LIMIT ?2",
    )?;
    stmt.query_map(params![fts_query, limit], |row| {
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
    .collect::<Result<Vec<_>, _>>()
    .map_err(Into::into)
}

fn search_artists_like(conn: &Connection, normalized: &str, limit: i64) -> Result<Vec<Artist>> {
    let contains_pattern = format!("%{normalized}%");
    let prefix_pattern = format!("{normalized}%");
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
    stmt.query_map(
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
    .collect::<Result<Vec<_>, _>>()
    .map_err(Into::into)
}

fn search_albums_fts(conn: &Connection, fts_query: &str, limit: i64) -> Result<Vec<Album>> {
    let mut stmt = conn.prepare(
        "SELECT al.id, al.tidal_id, al.ytmusic_id, al.title, al.artist_id,
                a.name, al.year, al.artwork_url,
                al.release_type, al.label, al.track_count, al.is_favorite, al.source
         FROM albums al
         LEFT JOIN artists a ON a.id = al.artist_id
         JOIN albums_fts ON albums_fts.rowid = al.id
         WHERE albums_fts MATCH ?1
         UNION
         SELECT al.id, al.tidal_id, al.ytmusic_id, al.title, al.artist_id,
                a.name, al.year, al.artwork_url,
                al.release_type, al.label, al.track_count, al.is_favorite, al.source
         FROM albums al
         LEFT JOIN artists a ON a.id = al.artist_id
         JOIN artists_fts ON artists_fts.rowid = al.artist_id
         WHERE artists_fts MATCH ?1
         ORDER BY title ASC
         LIMIT ?2",
    )?;
    stmt.query_map(params![fts_query, limit], |row| {
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
    .collect::<Result<Vec<_>, _>>()
    .map_err(Into::into)
}

fn search_albums_like(conn: &Connection, normalized: &str, limit: i64) -> Result<Vec<Album>> {
    let contains_pattern = format!("%{normalized}%");
    let prefix_pattern = format!("{normalized}%");
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
    stmt.query_map(
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
    .collect::<Result<Vec<_>, _>>()
    .map_err(Into::into)
}

pub fn search(conn: &Connection, query: &str, limit: i64) -> Result<SearchResults> {
    let normalized = query.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(SearchResults {
            tracks: Vec::new(),
            albums: Vec::new(),
            artists: Vec::new(),
        });
    }

    let limit = limit.max(1);
    let fts_query = to_fts_query(&normalized);

    // Try FTS first; fall back to LIKE on any error.
    let tracks = search_tracks_fts(conn, &fts_query, limit)
        .unwrap_or_else(|_| search_tracks_like(conn, &normalized, limit).unwrap_or_default());
    let artists = search_artists_fts(conn, &fts_query, limit)
        .unwrap_or_else(|_| search_artists_like(conn, &normalized, limit).unwrap_or_default());
    let albums = search_albums_fts(conn, &fts_query, limit)
        .unwrap_or_else(|_| search_albums_like(conn, &normalized, limit).unwrap_or_default());

    Ok(SearchResults {
        tracks,
        artists,
        albums,
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

    tx.execute_batch(
        "
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
    ",
    )?;

    // ── Stage 2: aggregate signals into indexed temp tables ──────────────────

    tx.execute_batch(
        "
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
    ",
    )?;

    // ── Stage 3: score each component ────────────────────────────────────────

    // co_album: 1.0 if same album
    tx.execute(
        "
        UPDATE track_similarity SET co_album_score = 1.0
        WHERE EXISTS (
            SELECT 1 FROM tracks a, tracks b
            WHERE a.id = track_similarity.track_a
              AND b.id = track_similarity.track_b
              AND a.album_id IS NOT NULL
              AND a.album_id = b.album_id
        )
    ",
        [],
    )?;

    // co_artist: 1.0 if same artist
    tx.execute(
        "
        UPDATE track_similarity SET co_artist_score = 1.0
        WHERE EXISTS (
            SELECT 1 FROM tracks a, tracks b
            WHERE a.id = track_similarity.track_a
              AND b.id = track_similarity.track_b
              AND a.artist_id IS NOT NULL
              AND a.artist_id = b.artist_id
        )
    ",
        [],
    )?;

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
    tx.execute(
        "
        UPDATE track_similarity SET duration_proximity = COALESCE((
            SELECT 1.0 - MIN(CAST(ABS(a.duration_ms - b.duration_ms) AS REAL) / 180000.0, 1.0)
            FROM tracks a, tracks b
            WHERE a.id = track_similarity.track_a AND b.id = track_similarity.track_b
              AND a.duration_ms IS NOT NULL AND b.duration_ms IS NOT NULL
        ), 0)
    ",
        [],
    )?;

    // co_listen: normalized co-occurrence count
    tx.execute(
        "
        UPDATE track_similarity SET co_listen_score = COALESCE((
            SELECT cl.n / NULLIF((SELECT MAX(n) FROM _co_listen), 0)
            FROM _co_listen cl
            WHERE cl.ta = track_similarity.track_a AND cl.tb = track_similarity.track_b
        ), 0)
    ",
        [],
    )?;

    // era_proximity: 1 - |year_a - year_b| / 25, clamped 0-1. Zero when either year is unknown.
    tx.execute(
        "
        UPDATE track_similarity SET era_proximity = COALESCE((
            SELECT 1.0 - MIN(CAST(ABS(ya.year - yb.year) AS REAL) / 25.0, 1.0)
            FROM _track_year ya, _track_year yb
            WHERE ya.track_id = track_similarity.track_a
              AND yb.track_id = track_similarity.track_b
        ), 0)
    ",
        [],
    )?;

    // Final weighted score. era_proximity replaces some duration_proximity weight
    // because era is a stronger taste signal than song length.
    tx.execute(
        "
        UPDATE track_similarity SET similarity_score =
            co_listen_score    * 0.30 +
            co_album_score     * 0.20 +
            co_artist_score    * 0.20 +
            genre_proximity    * 0.15 +
            era_proximity      * 0.10 +
            duration_proximity * 0.05
    ",
        [],
    )?;

    tx.execute_batch(
        "
        DROP TABLE IF EXISTS _co_listen;
        DROP TABLE IF EXISTS _genre_shared;
        DROP TABLE IF EXISTS _track_year;
    ",
    )?;

    let count: i64 = tx.query_row("SELECT COUNT(*) FROM track_similarity", [], |row| {
        row.get(0)
    })?;

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
    Ok(conn
        .query_row("SELECT MAX(computed_at) FROM track_similarity", [], |row| {
            row.get(0)
        })
        .optional()?)
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
    pub confidence: f64,
    pub support_count: i64,
    pub candidate_in_degree: i64,
    pub candidate_in_degree_percentile: f64,
    pub play_count_seed: i64,
    pub play_count_candidate: i64,
    pub primary_reason: Option<String>,
}

// Trainer write payload. Replaces the 9-tuple that replace_track_neighbors used
// to take — at 16 fields, named struct fields are necessary for any chance at
// not mixing up arguments.
#[derive(Debug, Clone)]
pub struct NeighborWriteRow {
    pub track_id: i64,
    pub neighbor_track_id: i64,
    pub rank: i32,
    pub score: f64,
    pub behavioral_score: f64,
    pub audio_score: f64,
    pub metadata_score: f64,
    pub reason_json: Option<String>,
    pub primary_reason: Option<String>,
    pub confidence: f64,
    pub support_count: i64,
    pub candidate_in_degree: i64,
    pub candidate_in_degree_percentile: f64,
    pub play_count_seed: i64,
    pub play_count_candidate: i64,
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

#[allow(dead_code)]
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
    // Refuse to wipe an already-populated model with an empty payload. The
    // trainer is supposed to bail before reaching here when it produces no
    // output (cancel checks at every stage), but a logic bug or panic-recovery
    // path could still get us here with an empty slice — and a silent wipe
    // turns a recoverable issue into a "discovery engine just died" bug for
    // the user. Leave the prior rows in place; tracing makes the skip visible.
    if embeddings.is_empty() {
        let existing: i64 = conn.query_row(
            "SELECT COUNT(*) FROM track_embeddings WHERE model_id = ?1",
            params![model_id],
            |row| row.get(0),
        )?;
        if existing > 0 {
            tracing::warn!(
                target: "noor.discovery.training",
                model_id,
                existing_rows = existing,
                "skipping embedding wipe: trainer returned 0 vectors but model has prior data"
            );
            return Ok(());
        }
    }
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

/// Cached row from `track_audio_features`, used by Incremental Refresh to
/// avoid recomputing the audio-proxy stage when the prior run's features are
/// still valid. Caller is responsible for filtering rows whose unpacked
/// vector dimension doesn't match the current intensity tier.
pub struct CachedAudioFeatureRow {
    pub track_id: i64,
    pub feature_version: String,
    pub vector_blob: Vec<u8>,
    pub clip_start_ms: i64,
    pub clip_duration_ms: i64,
}

pub fn get_cached_audio_features(conn: &Connection) -> Result<Vec<CachedAudioFeatureRow>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, feature_version, vector_blob, clip_start_ms, clip_duration_ms
         FROM track_audio_features",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CachedAudioFeatureRow {
                track_id: row.get(0)?,
                feature_version: row.get(1)?,
                vector_blob: row.get(2)?,
                clip_start_ms: row.get(3)?,
                clip_duration_ms: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
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

// Per-reason held-out hit-rate row, mirroring the structure emitted by the
// trainer. Kept as a separate struct on the queries side so the trainer module
// doesn't need to depend on rusqlite param plumbing.
pub struct ReasonHitRateRow {
    pub primary_reason: String,
    pub impressions: i64,
    pub hits: i64,
    pub hit_rate: f64,
    pub mean_rank: Option<f64>,
    pub mrr_contribution: f64,
    pub insufficient_data: bool,
}

// Replaces all per-reason hit-rate rows for a model. Wrapped in a transaction
// so a partial replacement can't leave stale rows from a prior training run.
pub fn replace_discovery_diagnostics(
    conn: &Connection,
    model_id: i64,
    rates: &[ReasonHitRateRow],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM discovery_diagnostics WHERE model_id = ?1",
        params![model_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO discovery_diagnostics
             (model_id, primary_reason, impressions, hits, hit_rate,
              mean_rank, mrr_contribution, insufficient_data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for r in rates {
            stmt.execute(params![
                model_id,
                r.primary_reason,
                r.impressions,
                r.hits,
                r.hit_rate,
                r.mean_rank,
                r.mrr_contribution,
                r.insufficient_data as i32,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[allow(dead_code)]
pub fn get_per_reason_hit_rates(conn: &Connection, model_id: i64) -> Result<Vec<ReasonHitRateRow>> {
    let mut stmt = conn.prepare(
        "SELECT primary_reason, impressions, hits, hit_rate,
                mean_rank, mrr_contribution, insufficient_data
         FROM discovery_diagnostics
         WHERE model_id = ?1
         ORDER BY impressions DESC",
    )?;
    let rows = stmt
        .query_map(params![model_id], |row| {
            Ok(ReasonHitRateRow {
                primary_reason: row.get(0)?,
                impressions: row.get(1)?,
                hits: row.get(2)?,
                hit_rate: row.get(3)?,
                mean_rank: row.get(4)?,
                mrr_contribution: row.get(5)?,
                insufficient_data: row.get::<_, i32>(6)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn replace_track_neighbors(
    conn: &Connection,
    model_id: i64,
    neighbors: &[NeighborWriteRow],
) -> Result<()> {
    // Single transaction: ~2M+ INSERTs auto-committing one-by-one is what makes
    // training appear to hang. Batching also makes the DELETE+INSERT atomic so a
    // killed process can't leave the table half-populated.
    //
    // Same defensive skip as `replace_track_embeddings`: an empty slice on a
    // populated model leaves the prior graph intact rather than wiping it.
    if neighbors.is_empty() {
        let existing: i64 = conn.query_row(
            "SELECT COUNT(*) FROM track_neighbors WHERE model_id = ?1",
            params![model_id],
            |row| row.get(0),
        )?;
        if existing > 0 {
            tracing::warn!(
                target: "noor.discovery.training",
                model_id,
                existing_rows = existing,
                "skipping neighbor wipe: trainer returned 0 edges but model has prior data"
            );
            return Ok(());
        }
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM track_neighbors WHERE model_id = ?1",
        params![model_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO track_neighbors
             (track_id, neighbor_track_id, model_id, rank, score,
              behavioral_score, audio_score, metadata_score, reason_json, primary_reason,
              confidence, support_count, candidate_in_degree, candidate_in_degree_percentile,
              play_count_seed, play_count_candidate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        )?;
        for n in neighbors {
            stmt.execute(params![
                n.track_id,
                n.neighbor_track_id,
                model_id,
                n.rank,
                n.score,
                n.behavioral_score,
                n.audio_score,
                n.metadata_score,
                n.reason_json,
                n.primary_reason,
                n.confidence,
                n.support_count,
                n.candidate_in_degree,
                n.candidate_in_degree_percentile,
                n.play_count_seed,
                n.play_count_candidate,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Replace neighbor rows for a single seed track only — used by the background
/// per-seed refresh so it doesn't wipe every other track's neighbors.
pub fn replace_seed_neighbors(
    conn: &Connection,
    model_id: i64,
    seed_id: i64,
    rows: &[NeighborWriteRow],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM track_neighbors WHERE model_id = ?1 AND track_id = ?2",
        params![model_id, seed_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO track_neighbors
             (track_id, neighbor_track_id, model_id, rank, score,
              behavioral_score, audio_score, metadata_score, reason_json, primary_reason,
              confidence, support_count, candidate_in_degree, candidate_in_degree_percentile,
              play_count_seed, play_count_candidate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        )?;
        for n in rows {
            stmt.execute(params![
                n.track_id,
                n.neighbor_track_id,
                model_id,
                n.rank,
                n.score,
                n.behavioral_score,
                n.audio_score,
                n.metadata_score,
                n.reason_json,
                n.primary_reason,
                n.confidence,
                n.support_count,
                n.candidate_in_degree,
                n.candidate_in_degree_percentile,
                n.play_count_seed,
                n.play_count_candidate,
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
    let sql =
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, t.best_quality,
                      n.score, n.behavioral_score, n.audio_score, n.metadata_score, n.reason_json,
                      n.confidence, n.support_count, n.candidate_in_degree,
                      n.candidate_in_degree_percentile, n.play_count_seed, n.play_count_candidate,
                      n.primary_reason
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
                confidence: row.get(12)?,
                support_count: row.get(13)?,
                candidate_in_degree: row.get(14)?,
                candidate_in_degree_percentile: row.get(15)?,
                play_count_seed: row.get(16)?,
                play_count_candidate: row.get(17)?,
                primary_reason: row.get(18)?,
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
    // Discovery embedding training pulls in album/artist fallback genres so
    // niche tracks (no track-level enrichment yet) cluster near coherent
    // peers in the embedding instead of isolating. Cost ~2s on training start;
    // training is periodic, not per-request, so the whole-library scan is fine.
    let genre_paths_with_provenance = get_track_genre_paths_with_fallback(conn)?;
    let genre_paths: HashMap<i64, Vec<String>> = genre_paths_with_provenance
        .into_iter()
        .map(|(id, rows)| (id, ResolvedGenre::paths_only(&rows)))
        .collect();
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
    let clip_cache_tracks: i64 =
        conn.query_row("SELECT COUNT(*) FROM track_audio_features", [], |row| {
            row.get(0)
        })?;
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
        params![
            from_track_id,
            to_track_id,
            transition_source,
            completed_prev,
            gap_ms
        ],
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
    session_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO discovery_feedback
         (seed_track_id, candidate_track_id, action, surface, context_json, session_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            seed_track_id,
            candidate_track_id,
            action,
            surface,
            context_json,
            session_id
        ],
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

pub fn get_listen_history_sequences(
    conn: &Connection,
    session_window_minutes: i64,
) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, started_at
         FROM listen_history
         ORDER BY started_at ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
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
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(&started_at, "%Y-%m-%d %H:%M:%S")
                .map(|dt| dt.and_utc())
        })
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
    let mut stmt = conn
        .prepare("SELECT id FROM tracks WHERE is_favorite = 1 ORDER BY play_count DESC, id ASC")?;
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

pub fn get_audio_dsp_features(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<AudioDspFeatures>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, bpm, key_signature, camelot_key, loudness_lufs,
                energy, danceability, beat_strength, spectral_centroid, stereo_width,
                is_instrumental, analysis_source, analysis_offset_ms, samples_analyzed,
                analyzed_at, analysis_version
         FROM audio_dsp_features
         WHERE track_id = ?1",
    )?;
    let result = stmt
        .query_row(params![track_id], |row| {
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
        })
        .optional()?;
    Ok(result)
}

pub fn get_tracks_missing_dsp_features(conn: &Connection, limit: i64) -> Result<Vec<Track>> {
    // CURRENT_ANALYSIS_VERSION is a compile-time constant — safe to interpolate.
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
         LEFT JOIN audio_dsp_features dsp ON t.id = dsp.track_id
         WHERE dsp.track_id IS NULL OR dsp.analysis_version != '{}'
         LIMIT ?1",
        crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
    );
    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(params![limit], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tracks)
}

pub fn get_audio_features_stats(conn: &Connection) -> Result<AudioFeaturesStats> {
    let (total_analyzed, avg_bpm, avg_energy): (i64, Option<f64>, Option<f64>) = conn.query_row(
        "SELECT COUNT(*), AVG(bpm), AVG(energy) FROM audio_dsp_features",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;

    // Top key (most common)
    let top_key: Option<String> = conn
        .query_row(
            "SELECT key_signature
         FROM audio_dsp_features
         WHERE key_signature IS NOT NULL
         GROUP BY key_signature
         ORDER BY COUNT(*) DESC
         LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;

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
    let key_distribution: HashMap<String, i64> = key_rows
        .collect::<Result<Vec<_>, _>>()?
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
) -> Result<
    Vec<(
        i64,
        Option<f64>,
        Option<String>,
        Option<String>,
        Option<f64>,
        Option<f64>,
        bool,
    )>,
> {
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
    conn.query_row("SELECT COUNT(*) FROM audio_dsp_features", [], |row| {
        row.get(0)
    })
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

#[allow(dead_code)]
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

#[allow(dead_code)]
pub fn insert_fingerprint_hashes(
    conn: &Connection,
    track_id: i64,
    hashes: &[(u32, u32)],
) -> Result<()> {
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
pub fn find_duplicate_group_for_tracks(conn: &Connection, a: i64, b: i64) -> Result<Option<i64>> {
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
pub fn add_duplicate_member(conn: &Connection, gid: i64, tid: i64, preferred: bool) -> Result<()> {
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
    pub analysis_current: i64,
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
    // CURRENT_ANALYSIS_VERSION is a compile-time constant — safe to interpolate.
    let analyzed_current_sql = format!(
        "SELECT COUNT(*) FROM audio_dsp_features WHERE analysis_version = '{}'",
        crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
    );
    let analysis_current: i64 = conn
        .query_row(&analyzed_current_sql, [], |r| r.get(0))
        .unwrap_or(0);
    // CURRENT_ANALYSIS_VERSION is a compile-time constant — safe to interpolate.
    let analysis_stale_sql = format!(
        "SELECT COUNT(*) FROM audio_dsp_features WHERE analysis_version != '{}'",
        crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
    );
    let analysis_stale: i64 = conn
        .query_row(&analysis_stale_sql, [], |r| r.get(0))
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
        .query_row("SELECT COUNT(*) FROM audio_fingerprints", [], |r| r.get(0))
        .unwrap_or(0);

    Ok(AudioFeaturesQuality {
        total_tracks,
        analyzed,
        analysis_current,
        analysis_stale,
        low_confidence_bpm,
        low_confidence_key,
        no_preview_url,
        fingerprinted,
    })
}

/// Return the ids of all tracks whose stored analysis_version is not the current
/// `CURRENT_ANALYSIS_VERSION`. Used by the re-analyze admin endpoint.
pub fn get_stale_analysis_track_ids(conn: &Connection) -> Result<Vec<i64>> {
    // CURRENT_ANALYSIS_VERSION is a compile-time constant — safe to interpolate.
    let sql = format!(
        "SELECT track_id FROM audio_dsp_features WHERE analysis_version != '{}'",
        crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn find_tracks_by_hash(
    conn: &Connection,
    hashes: &[u32],
) -> Result<Vec<(i64, u32, u32)>> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders: Vec<String> = hashes.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT track_id, hash, time_offset
         FROM fingerprint_hashes
         WHERE hash IN ({})
         ORDER BY hash",
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let hash_params: Vec<i64> = hashes.iter().map(|h| *h as i64).collect();
    let rows = stmt.query_map(params_from_iter(hash_params.iter()), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)? as u32,
            row.get::<_, i64>(2)? as u32,
        ))
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use rusqlite::Connection;

    fn read_onboarding_value(conn: &Connection) -> Option<String> {
        conn.query_row(
            "SELECT value FROM server_config WHERE key='onboarding_complete'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .expect("query server_config")
    }

    #[test]
    fn onboarding_unset_no_tidal_returns_false() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        assert!(!get_onboarding_complete(&conn).expect("read flag"));
        assert!(
            read_onboarding_value(&conn).is_none(),
            "must not write a row when nothing implies completion"
        );
    }

    #[test]
    fn onboarding_unset_with_tidal_writes_flag_and_returns_true() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute(
            "INSERT INTO service_auth (service, user_id) VALUES ('tidal', 'u-123')",
            [],
        )
        .expect("seed tidal auth");

        assert!(get_onboarding_complete(&conn).expect("read flag"));
        assert_eq!(read_onboarding_value(&conn).as_deref(), Some("1"));
    }

    #[test]
    fn onboarding_flag_present_returns_true_without_tidal() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        set_onboarding_complete(&conn).expect("set flag");

        assert!(get_onboarding_complete(&conn).expect("read flag"));
        assert_eq!(read_onboarding_value(&conn).as_deref(), Some("1"));
    }

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

        let heat = get_genre_heat_filtered(
            &conn,
            90,
            crate::genre::filter::GalaxyFilterRule::All,
        )
        .expect("genre heat");
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

        let heat = get_genre_heat_filtered(
            &conn,
            90,
            crate::genre::filter::GalaxyFilterRule::All,
        )
        .expect("genre heat");
        assert_eq!(heat.len(), 2);
        assert!(heat.iter().all(|entry| entry.listen_count == 0));
        assert!(heat.iter().all(|entry| entry.total_listened_ms == 0));
    }

    #[test]
    fn test_add_tracks_to_playlist_deduplicates() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute_batch(
            r#"
            INSERT INTO artists (id, name) VALUES (1, 'Test Artist');
            INSERT INTO albums (id, title, artist_id) VALUES (1, 'Test Album', 1);
            INSERT INTO tracks (id, title, artist_id, album_id) VALUES (1, 'Track A', 1, 1);
            INSERT INTO tracks (id, title, artist_id, album_id) VALUES (2, 'Track B', 1, 1);
            INSERT INTO tracks (id, title, artist_id, album_id) VALUES (3, 'Track C', 1, 1);
            INSERT INTO playlists (id, name, is_smart, is_synced) VALUES (1, 'My Playlist', 0, 1);
        "#,
        )
        .unwrap();

        // First call adds both tracks
        let added = add_tracks_to_playlist(&conn, 1, &[1, 2]).unwrap();
        assert_eq!(added, 2);

        // Second call with same tracks returns 0 (already present)
        let added_again = add_tracks_to_playlist(&conn, 1, &[1, 2]).unwrap();
        assert_eq!(added_again, 0);

        // Duplicate IDs within a single call: [1, 1] — track 1 already present, so 0 added
        let added_dup = add_tracks_to_playlist(&conn, 1, &[1, 1]).unwrap();
        assert_eq!(added_dup, 0);

        // Mixed: [1, 3] — track 1 already present, track 3 is new → 1 added
        let added_mixed = add_tracks_to_playlist(&conn, 1, &[1, 3]).unwrap();
        assert_eq!(added_mixed, 1);

        // [3, 3] — track 3 now present, duplicate in input → 0 added
        let added_dup_present = add_tracks_to_playlist(&conn, 1, &[3, 3]).unwrap();
        assert_eq!(added_dup_present, 0);
    }

    // ─── liked_only vs favorite_only regression ───────────────────────────
    //
    // Bug being guarded: favorite_only=true used to silently mean "library tracks"
    // (liked tracks ∪ tracks from favorited albums), so saved-album tracks leaked
    // into what the UI presented as "liked". liked_only must be strict.
    fn seed_album_with_one_liked_track(conn: &Connection) {
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Brooks & Dunn')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, is_favorite, source)
             VALUES (1, '#1s ... and then some', 1, 1, 'tidal')",
            [],
        )
        .unwrap();
        // Three tracks in the favorited album; only "Neon Blue" has tracks.is_favorite = 1.
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                                  best_quality, best_source, fidelity_score, is_favorite, source)
             VALUES (1, 'Neon Blue', 1, 1, 200000, 101, 'LOSSLESS', 'tidal', 10, 1, 'tidal')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                                  best_quality, best_source, fidelity_score, is_favorite, source)
             VALUES (2, 'Brand New Man', 1, 1, 180000, 102, 'LOSSLESS', 'tidal', 10, 0, 'tidal')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, duration_ms, tidal_id,
                                  best_quality, best_source, fidelity_score, is_favorite, source)
             VALUES (3, 'Boot Scootin Boogie', 1, 1, 198000, 103, 'LOSSLESS', 'tidal', 10, 0, 'tidal')",
            [],
        ).unwrap();
    }

    #[test]
    fn liked_only_excludes_album_favorited_tracks() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_album_with_one_liked_track(&conn);

        let tracks = get_tracks(&conn, "title", "asc", 100, 0, false, true).expect("liked tracks");
        assert_eq!(
            tracks.len(),
            1,
            "liked_only must return only truly-liked tracks"
        );
        assert_eq!(tracks[0].title, "Neon Blue");

        let count = get_track_count(&conn, false, true).expect("liked count");
        assert_eq!(count, 1, "count must match liked-only data query");
    }

    #[test]
    fn favorite_only_preserves_legacy_union_behavior() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_album_with_one_liked_track(&conn);

        let tracks =
            get_tracks(&conn, "title", "asc", 100, 0, true, false).expect("library tracks");
        assert_eq!(
            tracks.len(),
            3,
            "favorite_only must keep returning all tracks from favorited albums"
        );

        let count = get_track_count(&conn, true, false).expect("library count");
        assert_eq!(count, 3, "count must match favorite_only data query");
    }

    #[test]
    fn liked_only_takes_precedence_over_favorite_only() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_album_with_one_liked_track(&conn);

        let tracks = get_tracks(&conn, "title", "asc", 100, 0, true, true).expect("strict tracks");
        assert_eq!(tracks.len(), 1, "liked_only must override favorite_only");
        assert_eq!(tracks[0].title, "Neon Blue");

        let count = get_track_count(&conn, true, true).expect("strict count");
        assert_eq!(count, 1);
    }

    #[test]
    fn no_filter_returns_everything() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        seed_album_with_one_liked_track(&conn);

        let tracks = get_tracks(&conn, "title", "asc", 100, 0, false, false).expect("all tracks");
        assert_eq!(tracks.len(), 3);

        let count = get_track_count(&conn, false, false).expect("all count");
        assert_eq!(count, 3);
    }

    // ─── FTS-first library search tests ──────────────────────────────────────

    #[test]
    fn library_search_multi_token_and_within_column_non_contiguous() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        // FTS5 AND-prefix semantics: tokens must all appear in the same indexed
        // column, but NOT necessarily contiguously and NOT necessarily in order.
        //   1001 — title "The Long Strokes": both tokens present, non-contiguous.
        //          (Today's LIKE on "the strokes" would MISS this — substring fail.)
        //   1002 — title "The Anthem": only "the". Missing "strokes". Should NOT match.
        //   1003 — title "Strokes": only "strokes". Missing "the". Should NOT match.
        conn.execute("INSERT INTO artists (id, name) VALUES (1001, 'Test')", []).expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (1001, 'Plain', 1001, 'tidal')", []).expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES
                (1001, 'The Long Strokes', 1001, 1001, 200000, 1001, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (1002, 'The Anthem',       1001, 1001, 200000, 1002, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (1003, 'Strokes',          1001, 1001, 200000, 1003, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
            [],
        )
        .expect("tracks");

        let results = search_with_audio_filters(
            &conn,
            "the strokes",
            &AudioFilters::default(),
            50,
        )
        .expect("library search");

        let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![1001], "expected only 1001 (both tokens in title, non-contiguous); got {ids:?}");
    }

    #[test]
    fn library_search_returns_track_when_album_title_matches() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (2001, 'Frank Ocean')", []).expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (2001, 'Blonde', 2001, 'tidal')", []).expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES (2001, 'Pink + White', 2001, 2001, 200000, 2001, 'LOSSLESS', 'tidal', 8, 0, 'tidal', 0)",
            [],
        )
        .expect("track");

        let results = search_with_audio_filters(&conn, "blonde", &AudioFilters::default(), 50).expect("search");
        let titles: Vec<&str> = results.iter().map(|r| r.title.as_str()).collect();
        assert!(
            titles.contains(&"Pink + White"),
            "expected 'Pink + White' (album 'Blonde' matches); got {titles:?}"
        );
    }

    #[test]
    fn library_search_audio_filter_composes_with_text() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (3001, 'Miles Davis')", []).expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (3001, 'Kind of Blue', 3001, 'tidal')", []).expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES
                (3001, 'So What (fast)', 3001, 3001, 540000, 3001, 'LOSSLESS', 'tidal', 9, 0, 'tidal', 0),
                (3002, 'Blue in Green',  3001, 3001, 330000, 3002, 'LOSSLESS', 'tidal', 9, 0, 'tidal', 0)",
            [],
        )
        .expect("tracks");
        conn.execute(
            "INSERT INTO audio_dsp_features (track_id, bpm) VALUES (3001, 120.0), (3002, 80.0)",
            [],
        )
        .expect("dsp features");

        let filters = AudioFilters { bpm_min: Some(100.0), ..Default::default() };

        let results = search_with_audio_filters(&conn, "miles", &filters, 50).expect("search");
        let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![3001], "expected only the 120-BPM track; got {ids:?}");
    }

    #[test]
    fn library_search_empty_query_with_filters_returns_filtered_set() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (4001, 'Test')", []).expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (4001, 'A', 4001, 'tidal')", []).expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES
                (4001, 'Fast', 4001, 4001, 200000, 4001, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (4002, 'Slow', 4001, 4001, 200000, 4002, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
            [],
        )
        .expect("tracks");
        conn.execute(
            "INSERT INTO audio_dsp_features (track_id, bpm) VALUES (4001, 130.0), (4002, 70.0)",
            [],
        )
        .expect("dsp");

        let filters = AudioFilters { bpm_min: Some(120.0), ..Default::default() };

        let results = search_with_audio_filters(&conn, "", &filters, 50).expect("search");
        let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![4001]);
    }

    #[test]
    fn library_search_empty_query_no_filters_respects_limit() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (5001, 'A')", []).expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (5001, 'A', 5001, 'tidal')", []).expect("album");
        for i in 0..5 {
            let id = 5001 + i;
            conn.execute(
                &format!(
                    "INSERT INTO tracks (
                        id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                        fidelity_score, is_favorite, source, play_count
                     ) VALUES ({id}, 'T{i}', 5001, 5001, 200000, {id}, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)"
                ),
                [],
            )
            .expect("track");
        }

        let results = search_with_audio_filters(&conn, "", &AudioFilters::default(), 3).expect("search");
        assert_eq!(results.len(), 3, "expected limit=3 to cap results");
    }

    #[test]
    fn library_search_favorites_lead_over_play_count() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (6001, 'Miles Davis')", []).expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (6001, 'Kind of Blue', 6001, 'tidal')", []).expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES
                (6001, 'Miles A', 6001, 6001, 200000, 6001, 'LOSSLESS', 'tidal', 5, 1, 'tidal', 0),
                (6002, 'Miles B', 6001, 6001, 200000, 6002, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 1000)",
            [],
        )
        .expect("tracks");

        // Old ordering (play_count DESC, last_played_at DESC) → B leads.
        // New ordering (is_favorite DESC, ...) → A leads.
        let results = search_with_audio_filters(&conn, "miles", &AudioFilters::default(), 50).expect("search");
        let ids: Vec<i64> = results.iter().map(|r| r.id).collect();
        assert_eq!(ids.first(), Some(&6001), "favorited track should lead despite zero plays; got {ids:?}");
    }

    #[test]
    fn library_search_non_track_track_type_returns_empty() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (7001, 'Anyone')", []).expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (7001, 'Anything', 7001, 'tidal')", []).expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES (7001, 'Anything', 7001, 7001, 200000, 7001, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
            [],
        )
        .expect("track");

        let filters = AudioFilters { track_type: Some("album".to_string()), ..Default::default() };

        let results = search_with_audio_filters(&conn, "anything", &filters, 50).expect("search");
        assert!(results.is_empty());
    }

    #[test]
    fn library_search_strips_fts_special_characters() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        // Punctuation in user queries (?, /, -) must not cause FTS to error.
        // to_fts_query strips non-alphanumerics; tokenization happens within a
        // single column, so fixtures keep all match tokens together in one column.
        conn.execute("INSERT INTO artists (id, name) VALUES (8001, 'AC/DC')", []).expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (8001, 'AC/DC Live', 8001, 'tidal')", []).expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES
                (8001, 'Thunderstruck', 8001, 8001, 200000, 8001, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0),
                (8002, 'Love Remix',    8001, 8001, 200000, 8002, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)",
            [],
        )
        .expect("tracks");

        // Query "AC/DC live?" → strips to "AC DC live" → tokens AC, DC, live must
        // all appear in some indexed column. Album "AC/DC Live" tokenizes to
        // ["ac", "dc", "live"] (unicode61 splits on /), satisfying all three.
        let r1 = search_with_audio_filters(&conn, "AC/DC live?", &AudioFilters::default(), 50)
            .expect("'AC/DC live?' must not error");
        assert!(r1.iter().any(|r| r.id == 8001), "expected Thunderstruck (album 'AC/DC Live' has all tokens); got ids {:?}", r1.iter().map(|r| r.id).collect::<Vec<_>>());

        // Query "love - remix" → "love remix" → tokens must both appear in same
        // column. Track 8002's title "Love Remix" satisfies that.
        let r2 = search_with_audio_filters(&conn, "love - remix", &AudioFilters::default(), 50)
            .expect("'love - remix' must not error");
        assert!(r2.iter().any(|r| r.id == 8002), "expected 'Love Remix' to match; got ids {:?}", r2.iter().map(|r| r.id).collect::<Vec<_>>());
    }

    #[test]
    fn global_search_tracks_fts_does_not_error_on_artist_match() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        // Setup designed to exercise the artists_fts UNION arm: track title
        // contains nothing of the query, but the artist name does.
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'The Cure')", []).expect("artist");
        conn.execute(
            "INSERT INTO albums (id, title, artist_id, source) VALUES (1, 'Disintegration', 1, 'tidal')",
            [],
        )
        .expect("album");
        conn.execute(
            "INSERT INTO tracks (
                id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                fidelity_score, is_favorite, source, play_count
             ) VALUES (1, 'Pictures of You', 1, 1, 420000, 101, 'LOSSLESS', 'tidal', 10, 1, 'tidal', 0)",
            [],
        )
        .expect("track");

        // Calls search_tracks_fts directly so the LIKE fallback in search() can't
        // mask an FTS-side error. If the UNION+ORDER-BY SQL is broken, this errors.
        let tracks = search_tracks_fts(&conn, "the* cure*", 10)
            .expect("search_tracks_fts must run without SQL errors");
        let titles: Vec<&str> = tracks.iter().map(|t| t.title.as_str()).collect();
        assert!(
            titles.contains(&"Pictures of You"),
            "FTS path should return the track via artists_fts arm; got {titles:?}"
        );
    }

    #[test]
    fn library_search_limit_is_respected_with_filters() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        conn.execute("INSERT INTO artists (id, name) VALUES (9101, 'Miles Davis')", []).expect("artist");
        conn.execute("INSERT INTO albums (id, title, artist_id, source) VALUES (9101, 'Kind of Blue', 9101, 'tidal')", []).expect("album");
        for i in 0..3 {
            let id = 9101 + i;
            conn.execute(
                &format!(
                    "INSERT INTO tracks (
                        id, title, artist_id, album_id, duration_ms, tidal_id, best_quality, best_source,
                        fidelity_score, is_favorite, source, play_count
                     ) VALUES ({id}, 'Miles {i}', 9101, 9101, 200000, {id}, 'LOSSLESS', 'tidal', 5, 0, 'tidal', 0)"
                ),
                [],
            )
            .expect("track");
        }
        conn.execute(
            "INSERT INTO audio_dsp_features (track_id, bpm) VALUES (9101, 120.0), (9102, 121.0), (9103, 122.0)",
            [],
        )
        .expect("dsp");

        let filters = AudioFilters { bpm_min: Some(100.0), ..Default::default() };

        let results = search_with_audio_filters(&conn, "miles", &filters, 2).expect("search");
        assert_eq!(results.len(), 2, "limit=2 with both FTS bind and audio-filter binds; off-by-one would return 0 or 3");
    }

    /// End-to-end Path A read API: cascade joins through `genre_paths` and
    /// returns ancestor-expanded paths with provenance. Mirrors the Path B
    /// fixture (in genre/filter.rs) — same artist/album/comp shape.
    #[test]
    fn get_genres_for_tracks_with_fallback_returns_provenance() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");

        // Genre tree: Electronic > Drum and Bass; Jazz; Rock.
        conn.execute_batch(
            "INSERT INTO genres (id, name, slug, parent_id) VALUES
                (1, 'Electronic', 'electronic', NULL),
                (2, 'Drum and Bass', 'drum-and-bass', 1),
                (3, 'Jazz', 'jazz', NULL),
                (4, 'Rock', 'rock', NULL);
             INSERT INTO artists (id, name) VALUES
                (1, 'CoherentArtist'),
                (2, 'PartiallyTaggedArtist'),
                (3, 'CompContributorA'),
                (4, 'CompContributorB');
             INSERT INTO albums (id, title, artist_id, source) VALUES
                (10, 'CoherentAlbum', 1, 'tidal'),
                (20, 'PartialAlbumA', 2, 'tidal'),
                (21, 'PartialAlbumB', 2, 'tidal'),
                (30, 'MultiArtistComp', 3, 'tidal');
             INSERT INTO tracks
                (id, title, artist_id, album_id, duration_ms, best_quality, best_source, fidelity_score, source)
             VALUES
                (100, 'Tagged on coherent', 1, 10, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (101, 'Empty on coherent', 1, 10, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (200, 'Tagged on partial A', 2, 20, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (201, 'Empty on partial B', 2, 21, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (300, 'Tagged on comp (artist 3)', 3, 30, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (301, 'Empty on comp (artist 4)', 4, 30, 1000, 'LOSSLESS', 'tidal', 10, 'tidal');
             INSERT INTO track_genres (track_id, genre_id, source, confidence) VALUES
                (100, 2, 'musicbrainz', 1.0),
                (200, 3, 'musicbrainz', 0.9),
                (300, 4, 'lastfm', 0.7);",
        )
        .expect("seed fixtures");

        let result =
            get_genres_for_tracks_with_fallback(&conn, &[100, 101, 201, 301]).expect("query");

        // Track 100: direct genre (Drum and Bass), ancestor-expanded into two paths
        // ("Electronic" via the parent walk and "Electronic > Drum and Bass" via the leaf).
        let r100 = result.get(&100).expect("track 100 present");
        assert!(r100.iter().all(|g| g.source == GenreSource::Track));
        assert!(
            r100.iter().any(|g| g.path == "Electronic > Drum and Bass"),
            "expected leaf path for direct genre, got {r100:?}"
        );

        // Track 101: empty, rescued from album sibling (track 100, genre 2).
        let r101 = result.get(&101).expect("track 101 present");
        assert!(r101.iter().all(|g| g.source == GenreSource::AlbumFallback));
        assert!(r101.iter().any(|g| g.path == "Electronic > Drum and Bass"));

        // Track 201: empty, no album sibling, rescued from artist (track 200, genre 3 = Jazz).
        let r201 = result.get(&201).expect("track 201 present");
        assert!(r201.iter().all(|g| g.source == GenreSource::ArtistFallback));
        assert!(r201.iter().any(|g| g.path == "Jazz"));

        // Track 301: empty on multi-artist comp; album tier MUST skip;
        // artist 4 has no other tagged tracks. Track stays unrescued, so it
        // should be absent from the returned map (per existing function's
        // contract: tracks with no genres are absent rather than empty Vec).
        assert!(
            !result.contains_key(&301),
            "track 301 must NOT inherit comp-mate genres; got {:?}",
            result.get(&301)
        );
    }

    #[test]
    fn paths_only_drops_provenance() {
        let rows = vec![
            ResolvedGenre {
                path: "Electronic > House".to_string(),
                source: GenreSource::Track,
            },
            ResolvedGenre {
                path: "Pop".to_string(),
                source: GenreSource::AlbumFallback,
            },
        ];
        let paths = ResolvedGenre::paths_only(&rows);
        assert_eq!(paths, vec!["Electronic > House", "Pop"]);
    }

    // ─── Analytics signals tests ──────────────────────────────────────────

    /// Rhythm: even routine = high score (low CV → rhythm near 100).
    #[test]
    fn compute_rhythm_even_routine_scores_high() {
        let mut days = Vec::new();
        for _ in 0..7 {
            // 1 listen each hour, every hour, every day = perfect routine.
            days.push([1i64; 24]);
        }
        let r = compute_rhythm(&days).expect("active days >= 5");
        assert!(r >= 95, "even routine should score near 100, got {r}");
    }

    /// Rhythm: spiky one-day pattern = low score (high CV → rhythm near 0).
    #[test]
    fn compute_rhythm_spiky_scores_low() {
        let mut days = Vec::new();
        for _ in 0..7 {
            // All 24 listens in hour 21, zero everywhere else.
            let mut h = [0i64; 24];
            h[21] = 24;
            days.push(h);
        }
        let r = compute_rhythm(&days).expect("active days >= 5");
        assert!(r <= 5, "spiky pattern should score near 0, got {r}");
    }

    /// Rhythm: <5 active days returns None (renders as `--` in the UI).
    #[test]
    fn compute_rhythm_returns_none_below_floor() {
        let days = vec![[1i64; 24]; 4];
        assert!(compute_rhythm(&days).is_none());
    }

    /// Rhythm: zero-listen days are excluded from the active count.
    #[test]
    fn compute_rhythm_ignores_empty_days() {
        let mut days = vec![[0i64; 24]; 30];
        for d in days.iter_mut().take(4) {
            d[12] = 5;
        }
        // 4 active days < 5 floor → None.
        assert!(compute_rhythm(&days).is_none());
    }

    /// Listen-weighted median: the canonical fixture from the plan —
    /// 200 plays of a 124 BPM track and 5 plays each of 10 other tracks
    /// MUST produce a median near 124, not the per-track median (~80-something).
    #[test]
    fn weighted_median_is_listen_weighted_not_per_track() {
        // 200 plays of one popular track at 124 BPM, 5 plays each of 10 tracks at
        // unrelated BPMs spanning the rest of the range.
        let mut weighted = vec![(124.0_f64, 200_i64)];
        for (i, bpm) in [62.0, 70.0, 78.0, 86.0, 94.0, 100.0, 108.0, 142.0, 160.0, 180.0]
            .iter()
            .enumerate()
        {
            weighted.push((*bpm, 5 + i as i64 * 0)); // 5 each
        }
        let med = weighted_median_bpm(&weighted).expect("non-empty");
        assert!(
            (med - 124.0).abs() < 0.01,
            "expected listen-weighted median near 124, got {med}"
        );
    }

    /// Listen-weighted stddev across the same per-listen vector.
    #[test]
    fn weighted_stddev_reflects_listen_weights() {
        // Two BPMs, equal weights → stddev should equal half the spread.
        let pairs = [(100.0, 5_i64), (140.0, 5_i64)];
        let s = weighted_stddev_bpm(&pairs).expect("non-empty");
        assert!((s - 20.0).abs() < 0.01, "expected ~20, got {s}");
    }

    /// Mode bucket centre: argmax bucket → returns lower-edge + step/2.
    #[test]
    fn mode_bucket_returns_centre() {
        let row = TempoRow {
            label: "row".to_string(),
            granularity: Granularity::Day,
            buckets: dense_buckets(),
        };
        let mut rows = vec![row.clone(), row];
        // Bump bucket 124 in the first row, bucket 100 in the second — total argmax = 124.
        rows[0]
            .buckets
            .iter_mut()
            .find(|b| b.bucket == 124)
            .unwrap()
            .listens = 50;
        rows[1]
            .buckets
            .iter_mut()
            .find(|b| b.bucket == 100)
            .unwrap()
            .listens = 30;
        let mode = mode_bucket_centre(&rows).expect("non-empty");
        // 124 + step/2 = 124 + 2 = 126.
        assert!((mode - 126.0).abs() < 0.01, "expected 126.0, got {mode}");
    }

    /// Granularity selection: short windows always pick Day.
    #[test]
    fn granularity_short_windows_pick_day() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        for d in [1, 7] {
            assert_eq!(select_granularity(&conn, d).expect("ok"), Granularity::Day);
        }
    }

    /// Granularity selection: 90d picks Week.
    #[test]
    fn granularity_90d_picks_week() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        assert_eq!(
            select_granularity(&conn, 90).expect("ok"),
            Granularity::Week
        );
    }

    /// Granularity selection: very long windows pick Month.
    #[test]
    fn granularity_all_picks_month() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        assert_eq!(
            select_granularity(&conn, 36500).expect("ok"),
            Granularity::Month
        );
    }

    /// Granularity selection: 30d with sparse data falls back to Week.
    /// Empty DB → distinct_days = 0 < 15 → Week.
    #[test]
    fn granularity_30d_sparse_falls_back_to_week() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        // No listen_history rows → sparse → Week fallback.
        assert_eq!(
            select_granularity(&conn, 30).expect("ok"),
            Granularity::Week
        );
    }

    /// Dense buckets cover the full BPM axis at step granularity, in order.
    #[test]
    fn dense_buckets_match_axis() {
        let buckets = dense_buckets();
        assert_eq!(buckets.len(), BPM_BUCKET_COUNT);
        assert_eq!(buckets.first().unwrap().bucket, BPM_MIN);
        assert_eq!(
            buckets.last().unwrap().bucket,
            BPM_MIN + ((BPM_BUCKET_COUNT - 1) as i32) * BPM_STEP
        );
        for w in buckets.windows(2) {
            assert_eq!(w[1].bucket - w[0].bucket, BPM_STEP);
        }
    }

    /// Empty signals on a fresh DB return zero everything, no panics, valid shape.
    #[test]
    fn analytics_signals_empty_db_returns_valid_shape() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        let s = get_analytics_signals(&conn, 30).expect("signals");
        assert_eq!(s.kpis.listened_ms.current, 0);
        assert_eq!(s.kpis.sessions.current, 0);
        assert!(s.kpis.completion.current.is_none());
        assert_eq!(s.tempo.bucket_axis.min, BPM_MIN);
        assert_eq!(s.tempo.bucket_axis.max, BPM_MAX);
        assert_eq!(s.tempo.bucket_axis.step, BPM_STEP);
        assert_eq!(s.sonic_field.total, 0);
        assert!(s.ridgeline.is_empty());
        assert_eq!(s.cohorts.len(), 3);
        assert!(s.audio_profile.loudness_lufs.is_none());
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

// ─── Audio Feature Search ─────────────────────────────────

#[derive(Debug, Default)]
pub struct AudioFilters {
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub energy_min: Option<f64>,
    pub energy_max: Option<f64>,
    pub danceability_min: Option<f64>,
    pub danceability_max: Option<f64>,
    pub key_signature: Option<String>, // exact match
    pub camelot_key: Option<String>,   // exact match
    pub year_min: Option<i64>,
    pub year_max: Option<i64>,
    pub genre_ids: Vec<i64>,           // track must belong to at least one
    pub track_type: Option<String>,    // placeholder, always "track"
    pub is_instrumental: Option<bool>, // true → vocal:false filter
}

#[derive(Debug, Serialize)]
pub struct AudioSearchResult {
    pub id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub bpm: Option<f64>,
    pub energy: Option<f64>,
    pub danceability: Option<f64>,
    pub key_signature: Option<String>,
    pub camelot_key: Option<String>,
    pub play_count: i64,
    pub is_favorite: bool,
    pub tidal_id: Option<i64>,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct VibeTrack {
    pub id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub bpm: Option<f64>,
    pub camelot_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BasicTrack {
    pub id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
}

pub fn get_same_vibe_tracks(
    conn: &Connection,
    track_id: i64,
    limit: i64,
) -> Result<Vec<VibeTrack>> {
    let src = conn.query_row(
        "SELECT d.bpm, d.camelot_key FROM audio_dsp_features d WHERE d.track_id = ?1",
        params![track_id],
        |row| {
            Ok((
                row.get::<_, Option<f64>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        },
    );
    let (bpm, camelot_key) = match src {
        Ok(v) => v,
        Err(_) => return Ok(vec![]),
    };
    let (Some(bpm), Some(camelot_key)) = (bpm, camelot_key) else {
        return Ok(vec![]);
    };

    let camelot_num: i64 = camelot_key
        .trim_end_matches(|c: char| c.is_alphabetic())
        .parse()
        .unwrap_or(0);
    let camelot_letter = camelot_key.chars().last().unwrap_or('A');

    let adjacent_nums: Vec<i64> = vec![
        if camelot_num == 1 {
            12
        } else {
            camelot_num - 1
        },
        camelot_num,
        if camelot_num == 12 {
            1
        } else {
            camelot_num + 1
        },
    ];
    let camelot_patterns: Vec<String> = adjacent_nums
        .iter()
        .map(|n| format!("{}{}%", n, camelot_letter))
        .collect();
    let camelot_clause = camelot_patterns
        .iter()
        .map(|p| format!("d.camelot_key LIKE '{}'", p.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(" OR ");

    let sql = format!(
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, d.bpm, d.camelot_key
         FROM tracks t
         LEFT JOIN artists a ON a.id = t.artist_id
         LEFT JOIN albums al ON al.id = t.album_id
         JOIN audio_dsp_features d ON d.track_id = t.id
         WHERE t.id != ?1
           AND d.bpm BETWEEN ?2 AND ?3
           AND ({camelot_clause})
         ORDER BY t.play_count DESC
         LIMIT ?4"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![track_id, bpm - 10.0, bpm + 10.0, limit], |row| {
        Ok(VibeTrack {
            id: row.get(0)?,
            title: row.get(1)?,
            artist_name: row.get(2)?,
            album_title: row.get(3)?,
            artwork_url: row.get(4)?,
            duration_ms: row.get(5)?,
            bpm: row.get(6)?,
            camelot_key: row.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_underrated_tracks(
    conn: &Connection,
    artist_id: i64,
    limit: i64,
) -> Result<Vec<BasicTrack>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms
         FROM tracks t
         LEFT JOIN artists a ON a.id = t.artist_id
         LEFT JOIN albums al ON al.id = t.album_id
         WHERE t.artist_id = ?1 AND t.play_count = 0
         ORDER BY RANDOM()
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![artist_id, limit], |row| {
        Ok(BasicTrack {
            id: row.get(0)?,
            title: row.get(1)?,
            artist_name: row.get(2)?,
            album_title: row.get(3)?,
            artwork_url: row.get(4)?,
            duration_ms: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// SQL subquery returning track ids that match `?1` via tracks_fts (title),
/// artists_fts (name), or albums_fts (title). Used inline as `t.id IN (...)`.
/// `?1` is reused in all three UNION arms — bind once.
fn track_fts_candidate_subquery() -> &'static str {
    "SELECT rowid FROM tracks_fts WHERE tracks_fts MATCH ?1 \
     UNION \
     SELECT t2.id FROM tracks t2 JOIN artists_fts ON artists_fts.rowid = t2.artist_id WHERE artists_fts MATCH ?1 \
     UNION \
     SELECT t3.id FROM tracks t3 JOIN albums_fts  ON albums_fts.rowid  = t3.album_id  WHERE albums_fts  MATCH ?1"
}

/// SQL fragment + bind params produced by `build_audio_filter_sql`. `next_idx`
/// is the first free `?N` slot — caller binds `LIMIT ?{next_idx}`.
struct AudioFilterSql {
    sql: String,
    params: Vec<Box<dyn rusqlite::ToSql>>,
    next_idx: usize,
}

/// Builds the audio-filter portion of the WHERE clause and its bind params,
/// starting bind indices at `start_idx`. Does NOT emit `LIMIT` — the caller does.
/// Shared by both the FTS-first path and the LIKE fallback so the two cannot
/// drift on filter handling.
fn build_audio_filter_sql(filters: &AudioFilters, start_idx: usize) -> AudioFilterSql {
    let mut sql = String::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut idx = start_idx;

    if let Some(v) = filters.bpm_min {
        sql.push_str(&format!(" AND d.bpm >= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(v) = filters.bpm_max {
        sql.push_str(&format!(" AND d.bpm <= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(v) = filters.energy_min {
        sql.push_str(&format!(" AND d.energy >= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(v) = filters.energy_max {
        sql.push_str(&format!(" AND d.energy <= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(v) = filters.danceability_min {
        sql.push_str(&format!(" AND d.danceability >= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(v) = filters.danceability_max {
        sql.push_str(&format!(" AND d.danceability <= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(ref v) = filters.key_signature {
        sql.push_str(&format!(" AND d.key_signature = ?{idx}"));
        params.push(Box::new(v.clone()));
        idx += 1;
    }
    if let Some(ref v) = filters.camelot_key {
        sql.push_str(&format!(" AND d.camelot_key = ?{idx}"));
        params.push(Box::new(v.clone()));
        idx += 1;
    }
    if let Some(v) = filters.year_min {
        sql.push_str(&format!(" AND al.year >= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if let Some(v) = filters.year_max {
        sql.push_str(&format!(" AND al.year <= ?{idx}"));
        params.push(Box::new(v));
        idx += 1;
    }
    if !filters.genre_ids.is_empty() {
        // i64 ids — safe to inline.
        let id_list: Vec<String> = filters.genre_ids.iter().map(|id| id.to_string()).collect();
        sql.push_str(&format!(
            " AND t.id IN (SELECT track_id FROM track_genres WHERE genre_id IN ({}))",
            id_list.join(", ")
        ));
    }
    if let Some(instrumental) = filters.is_instrumental {
        sql.push_str(&format!(" AND d.is_instrumental = ?{idx}"));
        params.push(Box::new(if instrumental { 1i64 } else { 0i64 }));
        idx += 1;
    }

    AudioFilterSql { sql, params, next_idx: idx }
}

pub fn search_with_audio_filters(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    limit: usize,
) -> Result<Vec<AudioSearchResult>> {
    match search_with_audio_filters_fts(conn, free_text, filters, limit) {
        Ok(results) => Ok(results),
        Err(err) => {
            tracing::warn!(?err, query = %free_text, "FTS library search failed; falling back to LIKE");
            search_with_audio_filters_like_fallback(conn, free_text, filters, limit)
        }
    }
}

fn search_with_audio_filters_fts(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    limit: usize,
) -> Result<Vec<AudioSearchResult>> {
    if filters
        .track_type
        .as_deref()
        .is_some_and(|t| t != "track")
    {
        return Ok(Vec::new());
    }

    let normalized = free_text.trim().to_ascii_lowercase();
    let has_text = !normalized.is_empty();

    let mut sql = String::from(
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, \
         d.bpm, d.energy, d.danceability, d.key_signature, d.camelot_key, \
         t.play_count, t.is_favorite, t.tidal_id, t.source \
         FROM tracks t \
         LEFT JOIN audio_dsp_features d ON d.track_id = t.id \
         LEFT JOIN artists a ON a.id = t.artist_id \
         LEFT JOIN albums al ON al.id = t.album_id \
         WHERE 1=1",
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut start_idx: usize = 1;

    if has_text {
        sql.push_str(&format!(
            " AND t.id IN ({sub})",
            sub = track_fts_candidate_subquery()
        ));
        params.push(Box::new(to_fts_query(&normalized)));
        start_idx += 1;
    }

    let filter_sql = build_audio_filter_sql(filters, start_idx);
    let limit_idx = filter_sql.next_idx;
    sql.push_str(&filter_sql.sql);
    params.extend(filter_sql.params);

    sql.push_str(&format!(
        " ORDER BY t.is_favorite DESC, t.play_count DESC, t.fidelity_score DESC, t.title ASC \
         LIMIT ?{limit_idx}"
    ));
    params.push(Box::new(limit as i64));

    let mut stmt = conn.prepare(&sql)?;
    let results = stmt
        .query_map(params_from_iter(params.iter().map(|p| p.as_ref())), |row| {
            Ok(AudioSearchResult {
                id: row.get(0)?,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                album_title: row.get(3)?,
                artwork_url: row.get(4)?,
                duration_ms: row.get(5)?,
                bpm: row.get(6)?,
                energy: row.get(7)?,
                danceability: row.get(8)?,
                key_signature: row.get(9)?,
                camelot_key: row.get(10)?,
                play_count: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
                is_favorite: row.get::<_, i64>(12)? != 0,
                tidal_id: row.get(13)?,
                source: row.get(14)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Verbatim copy of the pre-FTS LIKE-based library search, kept as a fallback
/// when FTS errors. Substring-contiguous LIKE on title/artist/album, ordered by
/// today's existing `t.play_count DESC, t.last_played_at DESC`. Renamed (not just
/// "fallback") so a future reader knows this is the OLD semantics, deliberately.
fn search_with_audio_filters_like_fallback(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    limit: usize,
) -> Result<Vec<AudioSearchResult>> {
    let normalized = free_text.trim().to_ascii_lowercase();

    if filters
        .track_type
        .as_deref()
        .is_some_and(|track_type| track_type != "track")
    {
        return Ok(Vec::new());
    }

    let mut sql = String::from(
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, \
         d.bpm, d.energy, d.danceability, d.key_signature, d.camelot_key, \
         t.play_count, t.is_favorite, t.tidal_id, t.source \
         FROM tracks t \
         LEFT JOIN audio_dsp_features d ON d.track_id = t.id \
         LEFT JOIN artists a ON a.id = t.artist_id \
         LEFT JOIN albums al ON al.id = t.album_id \
         WHERE 1=1",
    );

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut start_idx: usize = 1;

    if !normalized.is_empty() {
        let pattern = format!("%{normalized}%");
        sql.push_str(&format!(
            " AND (LOWER(t.title) LIKE ?{0} \
               OR LOWER(COALESCE(a.name, '')) LIKE ?{0} \
               OR LOWER(COALESCE(al.title, '')) LIKE ?{0})",
            start_idx
        ));
        params.push(Box::new(pattern));
        start_idx += 1;
    }

    let filter_sql = build_audio_filter_sql(filters, start_idx);
    let limit_idx = filter_sql.next_idx;
    sql.push_str(&filter_sql.sql);
    params.extend(filter_sql.params);

    sql.push_str(&format!(
        " ORDER BY t.play_count DESC, t.last_played_at DESC LIMIT ?{limit_idx}"
    ));
    params.push(Box::new(limit as i64));

    let mut stmt = conn.prepare(&sql)?;
    let results = stmt
        .query_map(params_from_iter(params.iter().map(|p| p.as_ref())), |row| {
            Ok(AudioSearchResult {
                id: row.get(0)?,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                album_title: row.get(3)?,
                artwork_url: row.get(4)?,
                duration_ms: row.get(5)?,
                bpm: row.get(6)?,
                energy: row.get(7)?,
                danceability: row.get(8)?,
                key_signature: row.get(9)?,
                camelot_key: row.get(10)?,
                play_count: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
                is_favorite: row.get::<_, i64>(12)? != 0,
                tidal_id: row.get(13)?,
                source: row.get(14)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(results)
}
