//! Searching tracks by audio features.

use super::*;

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
    pub genre_ids: Vec<i64>,             // track must belong to at least one
    pub track_type: Option<String>,      // placeholder, always "track"
    pub is_instrumental: Option<bool>,   // true → vocal:false filter
    pub liked_only: bool,                // restrict to user-liked tracks (Liked tab)
    pub artist_contains: Option<String>, // substring match on artist name
    pub album_contains: Option<String>,  // substring match on album title
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
pub(super) fn track_fts_candidate_subquery() -> &'static str {
    "SELECT rowid FROM tracks_fts WHERE tracks_fts MATCH ?1 \
     UNION \
     SELECT t2.id FROM tracks t2 JOIN artists_fts ON artists_fts.rowid = t2.artist_id WHERE artists_fts MATCH ?1 \
     UNION \
     SELECT t3.id FROM tracks t3 JOIN albums_fts  ON albums_fts.rowid  = t3.album_id  WHERE albums_fts  MATCH ?1"
}

/// Canonicalize a user-typed key filter into the analyzer's vocabulary
/// (sharps only, "Am" for minor / "Amaj" for major — see
/// services/audio_analysis/key.rs). "Bb" -> "A#maj", "bbm" -> "A#m",
/// "f# minor" -> "F#m", bare "A" -> "Amaj". Unrecognized input is returned
/// trimmed so the NOCASE equality match still gets a shot at it.
pub(super) fn normalize_key_signature(input: &str) -> String {
    let trimmed = input.trim();
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return trimmed.to_string();
    };
    let letter = first.to_ascii_uppercase();
    if !('A'..='G').contains(&letter) {
        return trimmed.to_string();
    }
    let rest: String = chars.collect();
    let (accidental, suffix) = match rest.chars().next() {
        Some('#') => ("#", &rest[1..]),
        Some('b') | Some('B') => ("b", &rest[1..]),
        _ => ("", rest.as_str()),
    };
    let note = match (letter, accidental) {
        (l, "") => l.to_string(),
        (l, "#") => match l {
            'E' => "F".to_string(),
            'B' => "C".to_string(),
            l => format!("{l}#"),
        },
        (l, "b") => match l {
            'C' => "B".to_string(),
            'F' => "E".to_string(),
            'D' => "C#".to_string(),
            'E' => "D#".to_string(),
            'G' => "F#".to_string(),
            'A' => "G#".to_string(),
            'B' => "A#".to_string(),
            l => l.to_string(),
        },
        (l, _) => l.to_string(),
    };
    match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "maj" | "major" => format!("{note}maj"),
        "m" | "min" | "minor" => format!("{note}m"),
        _ => trimmed.to_string(),
    }
}

/// Resolve user-typed genre tokens (from `genre:` filters) to genre ids.
/// Matches slug or name, case-insensitively; hyphens in the token also match
/// spaces in the name ("hip-hop" -> "Hip Hop"). Returns (matched ids,
/// unmatched tokens) so the route can surface unknown genres instead of
/// silently searching unfiltered.
pub fn resolve_genre_tokens(
    conn: &Connection,
    tokens: &[String],
) -> Result<(Vec<i64>, Vec<String>)> {
    let mut ids = Vec::new();
    let mut unmatched = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT id FROM genres \
         WHERE slug = ?1 OR LOWER(name) = ?1 OR LOWER(name) = REPLACE(?1, '-', ' ')",
    )?;
    for token in tokens {
        let normalized = token.trim().to_lowercase();
        if normalized.is_empty() {
            continue;
        }
        let found: Option<i64> = stmt
            .query_row(params![normalized], |row| row.get(0))
            .optional()?;
        match found {
            Some(id) => ids.push(id),
            None => unmatched.push(token.clone()),
        }
    }
    Ok((ids, unmatched))
}

/// Expand genre ids to the full descendant subtree (children, grandchildren,
/// ...) via the parent_id closure. Ids not present in `genres` drop out.
pub fn expand_genre_descendants(conn: &Connection, ids: &[i64]) -> Result<Vec<i64>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // i64 ids — safe to inline.
    let id_list: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    let sql = format!(
        "WITH RECURSIVE closure(id) AS (\
            SELECT id FROM genres WHERE id IN ({ids}) \
            UNION \
            SELECT g.id FROM genres g JOIN closure c ON g.parent_id = c.id\
         ) SELECT id FROM closure",
        ids = id_list.join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let expanded = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<i64>, _>>()?;
    Ok(expanded)
}

/// Genre filtering + ranking, expressed as an INNER JOIN so one pass over the
/// curated rowset both decides membership and yields the match confidence.
/// Returns the JOIN clause to splice in after the base table's LEFT JOINs, or
/// "" when no genre filter is active. Exposes column `gm.genre_match_conf`.
///
/// Uses the same rowset the genre galaxy uses (confidence floor + weakest-tag
/// rescue) instead of raw track_genres, so low-confidence junk tags (e.g.
/// "Psychedelic Rock" @ 0.29 on psytrance tracks) neither match nor rank.
/// confidence is clamped to 1.0 for ranking so the handful of miscalibrated
/// >1.0 rows (docs/genre-data-quality-2026-05-07.md) can't outrank a clean
/// > 1.0 tag.
pub(super) fn genre_match_join_sql(filters: &AudioFilters) -> String {
    if filters.genre_ids.is_empty() {
        return String::new();
    }
    // i64 ids — safe to inline.
    let id_list: Vec<String> = filters.genre_ids.iter().map(|id| id.to_string()).collect();
    let rowset = crate::genre::filter::filter_subquery(
        crate::genre::filter::GalaxyFilterRule::default_rule(),
    );
    format!(
        " JOIN (SELECT track_id, MAX(MIN(confidence, 1.0)) AS genre_match_conf \
         FROM ({rowset}) WHERE genre_id IN ({}) GROUP BY track_id) gm \
         ON gm.track_id = t.id",
        id_list.join(", ")
    )
}

/// SQL fragment + bind params produced by `build_audio_filter_sql`. `next_idx`
/// is the first free `?N` slot — caller binds `LIMIT ?{next_idx}`.
pub(super) struct AudioFilterSql {
    pub(super) sql: String,
    pub(super) params: Vec<Box<dyn rusqlite::ToSql>>,
    pub(super) next_idx: usize,
}

/// Builds the audio-filter portion of the WHERE clause and its bind params,
/// starting bind indices at `start_idx`. Does NOT emit `LIMIT` — the caller does.
/// Shared by both the FTS-first path and the LIKE fallback so the two cannot
/// drift on filter handling.
pub(super) fn build_audio_filter_sql(filters: &AudioFilters, start_idx: usize) -> AudioFilterSql {
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
        // NOCASE + canonicalization so "key:am", "key:A", "key:Bb" all hit the
        // analyzer's "Am"/"Amaj"/"A#maj" vocabulary instead of matching nothing.
        sql.push_str(&format!(" AND d.key_signature = ?{idx} COLLATE NOCASE"));
        params.push(Box::new(normalize_key_signature(v)));
        idx += 1;
    }
    if let Some(ref v) = filters.camelot_key {
        sql.push_str(&format!(" AND d.camelot_key = ?{idx} COLLATE NOCASE"));
        params.push(Box::new(v.trim().to_string()));
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
    // NOTE: genre filtering is NOT a WHERE clause. It is expressed as an INNER
    // JOIN (see genre_match_join_sql) so the same curated rowset that decides
    // membership also yields the matched confidence used to rank strongest
    // matches first. Callers splice that JOIN into the FROM before this WHERE.
    if let Some(instrumental) = filters.is_instrumental {
        sql.push_str(&format!(" AND d.is_instrumental = ?{idx}"));
        params.push(Box::new(if instrumental { 1i64 } else { 0i64 }));
        idx += 1;
    }
    if let Some(ref v) = filters.artist_contains {
        sql.push_str(&format!(" AND LOWER(COALESCE(a.name, '')) LIKE ?{idx}"));
        params.push(Box::new(format!("%{}%", v.trim().to_lowercase())));
        idx += 1;
    }
    if let Some(ref v) = filters.album_contains {
        sql.push_str(&format!(" AND LOWER(COALESCE(al.title, '')) LIKE ?{idx}"));
        params.push(Box::new(format!("%{}%", v.trim().to_lowercase())));
        idx += 1;
    }
    if filters.liked_only {
        // Mirrors the Liked tab's client-side is_favorite filter so a filtered
        // Shuffle on that tab samples only liked tracks, not the whole match set.
        sql.push_str(" AND t.is_favorite = 1");
    }

    AudioFilterSql {
        sql,
        params,
        next_idx: idx,
    }
}

/// Deterministic display ranking. `offset` pages past the 50-row display cap
/// ("Show more"); pass 0 for the first page.
pub fn search_with_audio_filters(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    limit: usize,
    offset: usize,
) -> Result<Vec<AudioSearchResult>> {
    search_with_audio_filters_ordered(conn, free_text, filters, limit, offset, false)
}

/// Total number of tracks matching the query + filters, independent of the
/// display LIMIT, so the UI can say "top 50 of N" instead of looking capped.
pub fn count_audio_filter_matches(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
) -> Result<i64> {
    match count_audio_filter_matches_inner(conn, free_text, filters, true) {
        Ok(count) => Ok(count),
        Err(err) => {
            tracing::warn!(?err, query = %free_text, "FTS count failed; falling back to LIKE");
            count_audio_filter_matches_inner(conn, free_text, filters, false)
        }
    }
}

pub(super) fn count_audio_filter_matches_inner(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    use_fts: bool,
) -> Result<i64> {
    if filters.track_type.as_deref().is_some_and(|t| t != "track") {
        return Ok(0);
    }

    let normalized = free_text.trim().to_ascii_lowercase();

    let mut sql = String::from(
        "SELECT COUNT(*) \
         FROM tracks t \
         LEFT JOIN audio_dsp_features d ON d.track_id = t.id \
         LEFT JOIN artists a ON a.id = t.artist_id \
         LEFT JOIN albums al ON al.id = t.album_id",
    );
    sql.push_str(&genre_match_join_sql(filters));
    sql.push_str(" WHERE 1=1");

    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut start_idx: usize = 1;

    if !normalized.is_empty() {
        if use_fts {
            sql.push_str(&format!(
                " AND t.id IN ({sub})",
                sub = track_fts_candidate_subquery()
            ));
            params.push(Box::new(to_fts_query(&normalized)));
        } else {
            let pattern = format!("%{normalized}%");
            sql.push_str(&format!(
                " AND (LOWER(t.title) LIKE ?{0} \
                   OR LOWER(COALESCE(a.name, '')) LIKE ?{0} \
                   OR LOWER(COALESCE(al.title, '')) LIKE ?{0})",
                start_idx
            ));
            params.push(Box::new(pattern));
        }
        start_idx += 1;
    }

    let filter_sql = build_audio_filter_sql(filters, start_idx);
    sql.push_str(&filter_sql.sql);
    params.extend(filter_sql.params);

    let mut stmt = conn.prepare(&sql)?;
    let count: i64 = stmt
        .query_row(params_from_iter(params.iter().map(|p| p.as_ref())), |row| {
            row.get(0)
        })?;
    Ok(count)
}

/// Same matching as `search_with_audio_filters`, but returns a true random
/// sample of the full matching set (`ORDER BY RANDOM()`) instead of the
/// deterministic favorite / play-count ranking. Backs the library Shuffle
/// button on a filtered view, so Shuffle randomizes across every matching track
/// instead of reshuffling the same top rows the display query returns.
pub fn search_with_audio_filters_shuffled(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    limit: usize,
) -> Result<Vec<AudioSearchResult>> {
    search_with_audio_filters_ordered(conn, free_text, filters, limit, 0, true)
}

pub(super) fn search_with_audio_filters_ordered(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    limit: usize,
    offset: usize,
    shuffle: bool,
) -> Result<Vec<AudioSearchResult>> {
    match search_with_audio_filters_fts(conn, free_text, filters, limit, offset, shuffle) {
        Ok(results) => Ok(results),
        Err(err) => {
            tracing::warn!(?err, query = %free_text, "FTS library search failed; falling back to LIKE");
            search_with_audio_filters_like_fallback(
                conn, free_text, filters, limit, offset, shuffle,
            )
        }
    }
}

pub(super) fn search_with_audio_filters_fts(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    limit: usize,
    offset: usize,
    shuffle: bool,
) -> Result<Vec<AudioSearchResult>> {
    if filters.track_type.as_deref().is_some_and(|t| t != "track") {
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
         LEFT JOIN albums al ON al.id = t.album_id",
    );
    sql.push_str(&genre_match_join_sql(filters));
    sql.push_str(" WHERE 1=1");

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

    let order = if shuffle {
        "RANDOM()"
    } else if filters.genre_ids.is_empty() {
        "t.is_favorite DESC, t.play_count DESC, t.fidelity_score DESC, t.title ASC"
    } else {
        // Strongest genre match first (a definitive-rock track outranks a
        // barely-tagged favorite), then the usual favorite/play ranking.
        "gm.genre_match_conf DESC, t.is_favorite DESC, t.play_count DESC, \
         t.fidelity_score DESC, t.title ASC"
    };
    sql.push_str(&format!(
        " ORDER BY {order} LIMIT ?{limit_idx} OFFSET ?{offset_idx}",
        offset_idx = limit_idx + 1
    ));
    params.push(Box::new(limit as i64));
    params.push(Box::new(offset as i64));

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
pub(super) fn search_with_audio_filters_like_fallback(
    conn: &Connection,
    free_text: &str,
    filters: &AudioFilters,
    limit: usize,
    offset: usize,
    shuffle: bool,
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
         LEFT JOIN albums al ON al.id = t.album_id",
    );
    sql.push_str(&genre_match_join_sql(filters));
    sql.push_str(" WHERE 1=1");

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

    let order = if shuffle {
        "RANDOM()"
    } else if filters.genre_ids.is_empty() {
        "t.play_count DESC, t.last_played_at DESC"
    } else {
        "gm.genre_match_conf DESC, t.play_count DESC, t.last_played_at DESC"
    };
    sql.push_str(&format!(
        " ORDER BY {order} LIMIT ?{limit_idx} OFFSET ?{offset_idx}",
        offset_idx = limit_idx + 1
    ));
    params.push(Box::new(limit as i64));
    params.push(Box::new(offset as i64));

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
