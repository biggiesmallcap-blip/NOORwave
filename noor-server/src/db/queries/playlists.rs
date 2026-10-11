//! Playlist queries and edits.

use super::*;

// ─── Playlists ────────────────────────────────────────────

pub fn get_artist_tracks(conn: &Connection, artist_id: i64) -> Result<Vec<Track>> {
    get_artist_tracks_matching(conn, artist_id, "")
}

pub fn get_artist_library_tracks(conn: &Connection, artist_id: i64) -> Result<Vec<Track>> {
    get_artist_tracks_matching(
        conn,
        artist_id,
        &format!(" AND {}", artist_library_track_predicate()),
    )
}

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

/// Create a regular (non-smart, non-synced) playlist.
///
/// `is_synced = 0` because a locally created list has no TIDAL counterpart to
/// sync against. Three call sites used to inline this INSERT; they all come
/// through here now so the column defaults stay in one place.
pub fn create_playlist(
    conn: &Connection,
    name: &str,
    description: Option<&str>,
) -> Result<Playlist> {
    conn.execute(
        "INSERT INTO playlists (name, description, is_smart, is_synced, track_count)
         VALUES (?1, ?2, 0, 0, 0)",
        params![name, description],
    )?;
    let playlist_id = conn.last_insert_rowid();
    get_playlist(conn, playlist_id)?
        .ok_or_else(|| anyhow::anyhow!("playlist not found after insert"))
}

/// Rename a playlist and/or replace its description. Works on regular, smart,
/// and TIDAL-mirrored rows alike - unlike `update_smart_playlist`, which is
/// gated on `is_smart = 1` in SQL and so cannot touch a regular playlist.
pub fn rename_playlist(
    conn: &Connection,
    playlist_id: i64,
    name: &str,
    description: Option<&str>,
) -> Result<Playlist> {
    let changed = conn.execute(
        "UPDATE playlists SET name = ?2, description = ?3, updated_at = datetime('now')
         WHERE id = ?1",
        params![playlist_id, name, description],
    )?;
    if changed == 0 {
        anyhow::bail!("playlist not found");
    }
    get_playlist(conn, playlist_id)?.ok_or_else(|| anyhow::anyhow!("playlist not found"))
}

/// Delete any playlist. `playlist_tracks` rows go with it via ON DELETE CASCADE.
pub fn delete_playlist(conn: &Connection, playlist_id: i64) -> Result<()> {
    let deleted = conn.execute("DELETE FROM playlists WHERE id = ?1", params![playlist_id])?;
    if deleted == 0 {
        anyhow::bail!("playlist not found");
    }
    Ok(())
}

/// Recompute `track_count` from `playlist_tracks` and bump `updated_at`.
///
/// Every content mutation ends with this. Deliberately NOT called by
/// `toggle_playlist_favorite`: favouriting is not a content change and should
/// not reshuffle the "Last updated" sort.
pub fn touch_playlist(conn: &Connection, playlist_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE playlists SET track_count = (
            SELECT COUNT(*) FROM playlist_tracks WHERE playlist_id = ?1
         ),
         updated_at = datetime('now')
         WHERE id = ?1",
        params![playlist_id],
    )?;
    Ok(())
}

/// The playlist's track ids in position order. Positions may have gaps; this
/// returns the sequence, not the positions.
pub(super) fn playlist_track_order(conn: &Connection, playlist_id: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position ASC",
    )?;
    let ids = stmt
        .query_map(params![playlist_id], |row| row.get(0))?
        .collect::<Result<Vec<i64>, _>>()?;
    Ok(ids)
}

/// Rewrite a playlist's rows to exactly `ordered_track_ids`, at positions
/// `0..n-1`.
///
/// `playlist_tracks` has `PRIMARY KEY (playlist_id, position)`, so the obvious
/// "shift a range of positions by one" UPDATE can transiently collide with a
/// row that has not moved yet, and SQLite has no ORDER BY on UPDATE to control
/// the order rows are visited in. Deleting the whole playlist's rows and
/// re-inserting in order sidesteps that entirely, and is cheap at the scale a
/// playlist actually reaches. Callers must already be inside a transaction.
///
/// Duplicate track ids are preserved: the schema permits the same track at two
/// positions, so de-duplicating here would silently drop rows.
pub(super) fn rewrite_playlist_positions(
    conn: &Connection,
    playlist_id: i64,
    ordered_track_ids: &[i64],
) -> Result<()> {
    conn.execute(
        "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
        params![playlist_id],
    )?;
    let mut stmt = conn.prepare(
        "INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
    )?;
    for (index, &track_id) in ordered_track_ids.iter().enumerate() {
        stmt.execute(params![playlist_id, track_id, index as i64])?;
    }
    Ok(())
}

/// Remove the rows at `positions` and close the resulting gaps.
///
/// Takes positions rather than track ids because the schema allows the same
/// track twice; a track id would be ambiguous about which copy to drop.
/// Positions that do not exist are ignored. Returns the number removed.
pub fn remove_playlist_positions(
    conn: &Connection,
    playlist_id: i64,
    positions: &[i64],
) -> Result<usize> {
    if positions.is_empty() {
        return Ok(0);
    }
    let tx = conn.unchecked_transaction()?;
    let doomed: std::collections::HashSet<i64> = positions.iter().copied().collect();

    let rows = {
        let mut stmt = tx.prepare(
            "SELECT position, track_id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position ASC",
        )?;
        stmt.query_map(params![playlist_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<(i64, i64)>, _>>()?
    };

    let kept: Vec<i64> = rows
        .iter()
        .filter(|(position, _)| !doomed.contains(position))
        .map(|(_, track_id)| *track_id)
        .collect();
    let removed = rows.len() - kept.len();
    if removed == 0 {
        return Ok(0);
    }

    rewrite_playlist_positions(&tx, playlist_id, &kept)?;
    touch_playlist(&tx, playlist_id)?;
    tx.commit()?;
    Ok(removed)
}

/// Move the row at `from` to index `to`, where `to` is measured AFTER the moved
/// row has been lifted out - the same convention the queue's move endpoint and
/// the frontend's `reorderDropIndex` use. Out-of-range indices are clamped.
pub fn move_playlist_track(conn: &Connection, playlist_id: i64, from: i64, to: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let mut order = playlist_track_order(&tx, playlist_id)?;
    let len = order.len() as i64;
    if from < 0 || from >= len {
        anyhow::bail!("playlist position out of range");
    }
    let track_id = order.remove(from as usize);
    let target = to.clamp(0, order.len() as i64) as usize;
    order.insert(target, track_id);

    rewrite_playlist_positions(&tx, playlist_id, &order)?;
    touch_playlist(&tx, playlist_id)?;
    tx.commit()?;
    Ok(())
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
    touch_playlist(conn, playlist_id)?;

    Ok(to_insert.len())
}

/// Up to `limit` distinct album-artwork URLs for a regular playlist, ordered
/// by the earliest position the URL appears at. Built for the `/cover-sample`
/// endpoint - returning four URLs as a JSON array is cheaper than evaluating
/// the playlist and discarding everything but the first four.
pub fn sample_playlist_artwork(
    conn: &Connection,
    playlist_id: i64,
    limit: i64,
) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT al.artwork_url, MIN(pt.position) AS first_pos
         FROM playlist_tracks pt
         JOIN tracks t ON pt.track_id = t.id
         JOIN albums al ON t.album_id = al.id
         WHERE pt.playlist_id = ?1 AND al.artwork_url IS NOT NULL
         GROUP BY al.artwork_url
         ORDER BY first_pos ASC
         LIMIT ?2",
    )?;
    let urls = stmt
        .query_map(params![playlist_id, limit], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(urls)
}

/// Lightweight artist-name search for autocomplete: returns `(id, name)` pairs
/// only. Reuses the FTS-then-LIKE fallback that powers the global `search`
/// endpoint so it picks up the same matches.
pub fn search_library_artist_names(
    conn: &Connection,
    query: &str,
    limit: i64,
) -> Result<Vec<(i64, String)>> {
    let normalized = query.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.max(1);
    let fts_query = to_fts_query(&normalized);
    let artists = search_artists_fts(conn, &fts_query, limit)
        .unwrap_or_else(|_| search_artists_like(conn, &normalized, limit).unwrap_or_default());
    Ok(artists.into_iter().map(|a| (a.id, a.name)).collect())
}

pub fn get_playlist_tracks(conn: &Connection, playlist_id: i64) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {}
         FROM playlist_tracks pt
         JOIN tracks t ON pt.track_id = t.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE pt.playlist_id = ?1
         ORDER BY pt.position ASC",
        track_projection("a")
    ))?;

    let tracks = stmt
        .query_map(params![playlist_id], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

pub fn get_all_tracks(conn: &Connection) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         ORDER BY {}",
        track_projection("a"),
        saved_date_order("DESC")
    ))?;

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
    let cascade = crate::genre::filter::filter_subquery_with_fallback(
        crate::genre::filter::GalaxyFilterRule::All,
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
