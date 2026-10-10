//! Library search (FTS5 with a LIKE fallback).

use super::*;

// ─── Search (FTS5 + LIKE fallback) ────────────────────────────────────────

/// Strip FTS5 special chars and append `*` to each token for prefix matching.
///
/// The apostrophe is treated as a separator, NOT preserved: FTS5 parses a bare
/// `'` as the start of a string literal, so keeping it turned every query with
/// an apostrophe ("Don't", "Guns N' Roses") into an `fts5: syntax error` that
/// silently dropped search into the full-table LIKE scan (~13x slower here).
/// The unicode61 tokenizer already splits indexed text on apostrophes ("Don't"
/// -> "don","t"), so mapping `'` to a space ("don't" -> "don* t*") both parses
/// cleanly and matches how the content was indexed.
pub(super) fn to_fts_query(input: &str) -> String {
    let clean: String = input
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' {
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

pub(super) fn search_tracks_fts(
    conn: &Connection,
    fts_query: &str,
    limit: i64,
) -> Result<Vec<Track>> {
    // Positional ORDER BY (17/18/16/2) instead of named columns: SQLite rejects
    // bare column names in compound-SELECT (UNION) ORDER BY when the SELECTs
    // contain JOINs, with "1st ORDER BY term does not match any column in the
    // result set". Positional indices sidestep the resolver entirely. Mapping:
    //   2  = t.title
    //   16 = t.fidelity_score
    //   17 = t.is_favorite
    //   18 = t.play_count
    let filter = crate::db::tidal_content::browse_predicate(conn)?
        .map(|predicate| format!(" AND {predicate}"))
        .unwrap_or_default();
    let projection = track_projection("a");
    let mut stmt = conn.prepare(&format!(
        "SELECT {projection}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         JOIN tracks_fts ON tracks_fts.rowid = t.id
         WHERE tracks_fts MATCH ?1 {filter}
         UNION
         SELECT {projection}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         JOIN artists_fts ON artists_fts.rowid = t.artist_id
         WHERE artists_fts MATCH ?1 {filter}
         ORDER BY 17 DESC, 18 DESC, 16 DESC, 2 ASC
         LIMIT ?2"
    ))?;
    stmt.query_map(params![fts_query, limit], track_from_row)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub(super) fn search_tracks_like(
    conn: &Connection,
    normalized: &str,
    limit: i64,
) -> Result<Vec<Track>> {
    let filter = crate::db::tidal_content::browse_predicate(conn)?
        .map(|predicate| format!(" AND {predicate}"))
        .unwrap_or_default();
    let contains_pattern = format!("%{normalized}%");
    let prefix_pattern = format!("{normalized}%");
    let mut stmt = conn.prepare(&format!(
        "SELECT {}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE (LOWER(t.title) LIKE ?1
            OR LOWER(COALESCE(a.name, '')) LIKE ?1
            OR LOWER(COALESCE(al.title, '')) LIKE ?1) {filter}
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
        track_projection("a")
    ))?;
    stmt.query_map(
        params![contains_pattern, normalized, prefix_pattern, limit],
        track_from_row,
    )?
    .collect::<Result<Vec<_>, _>>()
    .map_err(Into::into)
}

pub(super) fn search_artists_fts(
    conn: &Connection,
    fts_query: &str,
    limit: i64,
) -> Result<Vec<Artist>> {
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

pub(super) fn search_artists_like(
    conn: &Connection,
    normalized: &str,
    limit: i64,
) -> Result<Vec<Artist>> {
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

pub(super) fn search_albums_fts(
    conn: &Connection,
    fts_query: &str,
    limit: i64,
) -> Result<Vec<Album>> {
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

pub(super) fn search_albums_like(
    conn: &Connection,
    normalized: &str,
    limit: i64,
) -> Result<Vec<Album>> {
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

pub(super) fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
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
        artist_tidal_id: None,
        album_tidal_id: None,
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

pub(super) fn genre_from_row(row: &Row<'_>) -> rusqlite::Result<Genre> {
    Ok(Genre {
        id: row.get(0)?,
        name: row.get(1)?,
        slug: row.get(2)?,
        parent_id: row.get(3)?,
        children: Vec::new(),
        track_count: Some(row.get(4)?),
    })
}

pub(super) fn build_genre_tree(genres: Vec<Genre>) -> Vec<Genre> {
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
        }

        children
    }

    attach_children(None, &mut children_by_parent)
}

pub(super) fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}
