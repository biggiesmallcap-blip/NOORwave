//! Track list, count and detail queries.

use super::*;

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
pub(super) fn favorite_predicate(favorite_only: bool, liked_only: bool) -> Option<&'static str> {
    if liked_only {
        Some("t.is_favorite = 1")
    } else if favorite_only {
        // The album-favorite branch is gated on `is_library = 1` so transient
        // resolver/discovery imports that happen to land in a favorited album
        // don't leak into the library. See MIGRATION_052 and the canonical
        // sibling `ARTIST_LIBRARY_TRACK_WHERE` (keep both in sync).
        Some(
            "(t.is_favorite = 1 OR (t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1) AND t.is_library = 1))",
        )
    } else {
        None
    }
}

// SQLite accepts ISO offsets with a colon; older TIDAL timestamps omit it.
// Keep labels and ordering consistent across those and UTC SQL timestamps.
pub(super) const SAVED_DATE_INSTANT: &str = "julianday(CASE WHEN t.date_added GLOB '*[+-][0-9][0-9][0-9][0-9]' THEN substr(t.date_added,1,length(t.date_added)-2) || ':' || substr(t.date_added,-2) ELSE t.date_added END)";

pub(super) fn saved_date_order(dir: &str) -> String {
    format!("({SAVED_DATE_INSTANT} IS NULL) ASC, {SAVED_DATE_INSTANT} {dir}, t.id {dir}")
}

pub(super) fn track_order_clause(sort_by: &str, sort_dir: &str) -> String {
    let dir = if sort_dir == "asc" { "ASC" } else { "DESC" };
    match sort_by {
        // True random sample across the whole matching set. Direction is
        // meaningless for RANDOM(), so it's ignored. Used by the library
        // Shuffle button so the queue isn't stuck reordering the newest 200
        // rows; SQLite draws a fresh sample on every request.
        "random" => "RANDOM()".to_string(),
        "title" => format!("t.title {dir}"),
        "artist" => format!("a_artists.name {dir}"),
        "album" => format!("al.title {dir}"),
        "year" => format!("al.year {dir}"),
        "date_added" => saved_date_order(dir),
        "duration" => format!("t.duration_ms {dir}"),
        "play_count" => format!("t.play_count {dir}"),
        "fidelity" => format!("t.fidelity_score {dir}"),
        "bpm" => format!("COALESCE(a.bpm, 0) {dir}"),
        "energy" => format!("COALESCE(a.energy, 0) {dir}"),
        "danceability" => format!("COALESCE(a.danceability, 0) {dir}"),
        "last_played_at" => format!("COALESCE(t.last_played_at, '') {dir}"),
        _ => saved_date_order(dir),
    }
}

/// The 22-column track projection shared by every query whose rows are mapped
/// by `track_from_row`. Pass the alias the query uses for the joined `artists`
/// table: `a` everywhere except `get_tracks_with_dsp`, which joins
/// `audio_dsp_features` as `a` and so must alias artists as `a_artists`.
///
/// Column ORDER is load-bearing: `track_from_row` reads by index, and
/// `search_tracks_fts` does positional ORDER BY against these columns. Only
/// ever append a column here, and update `track_from_row` to match.
pub(super) fn track_projection(artist_alias: &str) -> String {
    format!(
        "t.id, t.title, t.artist_id, {artist_alias}.name as artist_name,
                t.album_id, al.title as album_title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc,
                t.tidal_id, t.ytmusic_id, t.soundcloud_id,
                t.best_quality, t.best_source, t.fidelity_score,
                t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url"
    )
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

    let order_clause = track_order_clause(sort_by, sort_dir);

    let mut conditions = Vec::new();
    if let Some(predicate) = crate::db::tidal_content::browse_predicate(conn)? {
        conditions.push(predicate.to_string());
    }
    if let Some(pred) = favorite_predicate(favorite_only, liked_only) {
        conditions.push(pred.to_string());
    }

    let join_clause = if has_dsp {
        " LEFT JOIN audio_dsp_features a ON t.id = a.track_id"
    } else {
        ""
    };

    let mut bind_values = Vec::new();
    if let Some(min) = dsp.bpm_min {
        bind_values.push(SqlValue::Real(min));
        conditions.push(format!("a.bpm >= ?{}", bind_values.len()));
    }
    if let Some(max) = dsp.bpm_max {
        bind_values.push(SqlValue::Real(max));
        conditions.push(format!("a.bpm <= ?{}", bind_values.len()));
    }
    if let Some(min) = dsp.energy_min {
        bind_values.push(SqlValue::Real(min));
        conditions.push(format!("a.energy >= ?{}", bind_values.len()));
    }
    if let Some(max) = dsp.energy_max {
        bind_values.push(SqlValue::Real(max));
        conditions.push(format!("a.energy <= ?{}", bind_values.len()));
    }
    if let Some(ref key) = dsp.key_signature {
        bind_values.push(SqlValue::Text(key.clone()));
        conditions.push(format!("a.key_signature = ?{}", bind_values.len()));
    }
    if dsp.instrumental_only {
        conditions.push("a.is_instrumental = 1".to_string());
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let projection = track_projection("a_artists");
    bind_values.push(SqlValue::Integer(limit));
    let limit_param = bind_values.len();
    bind_values.push(SqlValue::Integer(offset));
    let offset_param = bind_values.len();
    let sql = format!(
        "SELECT {projection}
         FROM tracks t
         LEFT JOIN artists a_artists ON t.artist_id = a_artists.id
         LEFT JOIN albums al ON t.album_id = al.id
         {join_clause}
         {where_clause}
         ORDER BY {order_clause}
         LIMIT ?{limit_param} OFFSET ?{offset_param}"
    );

    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(params_from_iter(bind_values.iter()), track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}

pub fn get_track_count(conn: &Connection, favorite_only: bool, liked_only: bool) -> Result<i64> {
    // FROM tracks t alias is required so favorite_predicate's "t."-prefixed SQL applies.
    let mut conditions = Vec::new();
    if let Some(predicate) = favorite_predicate(favorite_only, liked_only) {
        conditions.push(predicate);
    }
    if let Some(predicate) = crate::db::tidal_content::browse_predicate(conn)? {
        conditions.push(predicate);
    }
    let filter = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    Ok(conn.query_row(
        &format!("SELECT COUNT(*) FROM tracks t{filter}"),
        [],
        |row| row.get(0),
    )?)
}

/// Play history collapsed to one row per track, most-recently-played first.
/// Unlike `get_tracks(favorite_only=true)` (which powers the library "Recent
/// Tracks" shelf), this reflects what was actually *played*: radio, discover,
/// and other external tracks that were imported into `tracks` on play but never
/// favorited surface here too. `listen_history.track_id` is a NOT NULL FK to
/// `tracks`, so an external track appears once its first listen was recorded.
///
/// GROUP BY collapses repeat plays; `MAX(lh.started_at)` is the ordering key.
/// The bare `t.*` columns in the projection are safe under GROUP BY because the
/// join keys them all to the single track behind `lh.track_id`.
pub fn get_listen_history_tracks(conn: &Connection, limit: i64, offset: i64) -> Result<Vec<Track>> {
    let projection = track_projection("a");
    let sql = format!(
        "SELECT {projection}, MAX(lh.started_at) AS last_listen
         FROM listen_history lh
         JOIN tracks t ON t.id = lh.track_id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         GROUP BY lh.track_id
         ORDER BY last_listen DESC
         LIMIT ?1 OFFSET ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(params![limit, offset], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tracks)
}

/// Count of distinct tracks that appear in play history (pagination total for
/// [`get_listen_history_tracks`]).
pub fn get_listen_history_track_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(DISTINCT track_id) FROM listen_history",
        [],
        |row| row.get(0),
    )?)
}
