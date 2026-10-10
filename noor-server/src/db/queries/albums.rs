//! Album list and detail queries.

use super::*;

// ─── Albums ───────────────────────────────────────────────

pub fn get_albums(
    conn: &Connection,
    sort_by: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
    favorite_only: bool,
    decade: Option<i64>,
) -> Result<Vec<Album>> {
    let order_col = match sort_by {
        "title" => "al.title",
        "artist" => "a.name",
        "year" => "al.year",
        _ => "al.title",
    };
    let dir = if sort_dir == "asc" { "ASC" } else { "DESC" };

    let where_clause = album_filter_clause("al", favorite_only, decade);

    let sql = format!(
        "SELECT al.id, al.tidal_id, al.ytmusic_id, al.title, al.artist_id,
                a.name as artist_name, al.year, al.artwork_url,
                al.release_type, al.label, al.track_count, al.is_favorite, al.source
         FROM albums al
         LEFT JOIN artists a ON al.artist_id = a.id
         {where_clause}
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

/// Ordering key for a deterministic pseudo-random sample.
///
/// `salt` selects which sample you get: the same salt always yields the same
/// rows, so a caller that buckets time (the library home shuffle murals rotate
/// every five minutes) keeps a stable panel across remounts instead of
/// reshuffling on every navigation. `ORDER BY RANDOM()` cannot do that - its
/// output is unseedable - and the old client-side workaround was one paginated
/// request per pick.
pub(super) fn shuffled_order_clause(column: &str) -> String {
    format!("(({column} + ?1) * 2654435761) % 1000003")
}

/// Deterministic pseudo-random sample of library tracks. See
/// [`shuffled_order_clause`] for what `salt` means.
pub fn get_shuffled_tracks(
    conn: &Connection,
    salt: i64,
    limit: i64,
    favorite_only: bool,
) -> Result<Vec<Track>> {
    let projection = track_projection("a_artists");
    let order_clause = shuffled_order_clause("t.id");
    let where_clause = match favorite_predicate(favorite_only, false) {
        Some(pred) => format!(" WHERE {pred}"),
        None => String::new(),
    };
    let sql = format!(
        "SELECT {projection}
         FROM tracks t
         LEFT JOIN artists a_artists ON t.artist_id = a_artists.id
         LEFT JOIN albums al ON t.album_id = al.id
         {where_clause}
         ORDER BY {order_clause}
         LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(params![salt, limit], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tracks)
}

/// Deterministic pseudo-random sample of library albums. Same `salt` contract as
/// [`get_shuffled_tracks`], so both murals rotate on the same bucket.
pub fn get_shuffled_albums(
    conn: &Connection,
    salt: i64,
    limit: i64,
    favorite_only: bool,
) -> Result<Vec<Album>> {
    let where_clause = album_filter_clause("al", favorite_only, None);
    let order_clause = shuffled_order_clause("al.id");
    let sql = format!(
        "SELECT al.id, al.tidal_id, al.ytmusic_id, al.title, al.artist_id,
                a.name as artist_name, al.year, al.artwork_url,
                al.release_type, al.label, al.track_count, al.is_favorite, al.source
         FROM albums al
         LEFT JOIN artists a ON al.artist_id = a.id
         {where_clause}
         ORDER BY {order_clause}
         LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let albums = stmt
        .query_map(params![salt, limit], |row| {
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

/// One artist on the Library hero: play totals across every library track by
/// that artist, not just the ones that make an individual-track cutoff.
#[derive(Debug, Clone, Serialize)]
pub struct LibraryTopArtist {
    pub id: i64,
    pub name: String,
    pub photo_url: Option<String>,
    /// Artwork of the artist's most played track that has any.
    pub fallback_art_url: Option<String>,
    pub play_count: i64,
    pub track_count: i64,
    pub album_count: i64,
}

/// Library artists ranked by total plays, summed per artist before the cut.
/// Ranking the most played individual tracks first undercounts an artist whose
/// plays are spread over many tracks, and can leave them off the hero entirely.
/// Same library scope as the Library track list (favorites plus library tracks
/// on favorited albums, minus hidden TIDAL content).
pub fn get_library_top_artists(conn: &Connection, limit: i64) -> Result<Vec<LibraryTopArtist>> {
    let mut conditions = vec!["t.artist_id IS NOT NULL".to_string()];
    if let Some(predicate) = crate::db::tidal_content::browse_predicate(conn)? {
        conditions.push(predicate.to_string());
    }
    if let Some(pred) = favorite_predicate(true, false) {
        conditions.push(pred.to_string());
    }
    let where_clause = conditions.join(" AND ");
    let sql = format!(
        "WITH lib AS (
             SELECT t.artist_id, t.album_id, t.play_count, al.artwork_url,
                    ROW_NUMBER() OVER (
                        PARTITION BY t.artist_id
                        ORDER BY al.artwork_url IS NULL, t.play_count DESC, t.id
                    ) AS art_rank
             FROM tracks t
             LEFT JOIN albums al ON t.album_id = al.id
             WHERE {where_clause}
         )
         SELECT lib.artist_id, a.name, a.photo_url,
                MAX(CASE WHEN lib.art_rank = 1 THEN lib.artwork_url END),
                SUM(lib.play_count) AS plays,
                COUNT(*),
                COUNT(DISTINCT lib.album_id)
         FROM lib
         JOIN artists a ON a.id = lib.artist_id
         GROUP BY lib.artist_id
         HAVING plays > 0
         ORDER BY plays DESC, a.name ASC
         LIMIT ?1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let artists = stmt
        .query_map(params![limit], |row| {
            Ok(LibraryTopArtist {
                id: row.get(0)?,
                name: row.get(1)?,
                photo_url: row.get(2)?,
                fallback_art_url: row.get(3)?,
                play_count: row.get(4)?,
                track_count: row.get(5)?,
                album_count: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(artists)
}

pub fn get_album_count(conn: &Connection, favorite_only: bool, decade: Option<i64>) -> Result<i64> {
    // Count uses the bare `albums` table (no alias), so build the clause without one.
    let filter = album_filter_clause("", favorite_only, decade);
    Ok(
        conn.query_row(&format!("SELECT COUNT(*) FROM albums{filter}"), [], |row| {
            row.get(0)
        })?,
    )
}

/// Build a ` WHERE ...` clause for album list/count queries from the optional
/// favorite and decade filters. `prefix` is the table alias (e.g. "al") or ""
/// for an unaliased table. A decade of 1990 matches years [1990, 2000).
/// Returns an empty string when no filter applies. Values are integers/booleans,
/// so inlining them is injection-safe and matches the surrounding style.
pub(super) fn album_filter_clause(
    prefix: &str,
    favorite_only: bool,
    decade: Option<i64>,
) -> String {
    let col = |name: &str| {
        if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}.{name}")
        }
    };
    let mut conditions: Vec<String> = Vec::new();
    if favorite_only {
        conditions.push(format!("{} = 1", col("is_favorite")));
    }
    if let Some(d) = decade {
        conditions.push(format!(
            "{year} >= {d} AND {year} < {}",
            d + 10,
            year = col("year")
        ));
    }
    if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    }
}

/// Distinct decades (1950, 1960, ...) present in the album library, ascending.
/// Powers the library's decade-filter chips independently of what has been paged
/// into the client, so selecting a decade resolves to a complete server-side set.
pub fn get_album_decades(conn: &Connection, favorite_only: bool) -> Result<Vec<i64>> {
    let fav_filter = if favorite_only {
        " AND al.is_favorite = 1"
    } else {
        ""
    };
    let sql = format!(
        "SELECT DISTINCT (al.year / 10) * 10 AS decade
         FROM albums al
         WHERE al.year IS NOT NULL{fav_filter}
         ORDER BY decade ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let decades = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(decades)
}

pub fn get_album_tracks(conn: &Connection, album_id: i64) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.album_id = ?1
         ORDER BY
            COALESCE(t.disc_number, 1) ASC,
            COALESCE(t.track_number, 999999) ASC,
            t.title COLLATE NOCASE ASC",
        track_projection("a")
    ))?;

    let tracks = stmt
        .query_map(params![album_id], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}
