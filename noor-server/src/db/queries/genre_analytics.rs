//! Genre co-listening, personal cohorts and evolution over time.

use super::*;

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
    let sub = galaxy_library_gate(&crate::genre::filter::filter_subquery(filter));
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
    let inner = if with_fallback {
        crate::genre::filter::filter_subquery_with_fallback(filter)
    } else {
        crate::genre::filter::filter_subquery(filter)
    };
    let sub = galaxy_library_gate(&inner);
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
    cohorts.sort_by_key(|a| std::cmp::Reverse(a.listen_count));

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

/// Album release year per track, for the discovery era filter. Tracks whose
/// album has no year are simply absent from the map (the filter passes them).
pub fn get_album_years_for_tracks(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<HashMap<i64, i64>> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids_csv: String = track_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT t.id, al.year
         FROM tracks t
         JOIN albums al ON al.id = t.album_id
         WHERE t.id IN ({ids_csv}) AND al.year IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
    let mut map = HashMap::new();
    for r in rows {
        let (track_id, year) = r?;
        map.insert(track_id, year);
    }
    Ok(map)
}

/// Minimal DSP triple (bpm, camelot, energy) per track, batched for discovery
/// ranking. Only rows that exist come back; absent tracks mean "unanalyzed".
pub fn get_dsp_lite_for_tracks(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<HashMap<i64, (Option<f64>, Option<String>, Option<f64>)>> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids_csv: String = track_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT track_id, bpm, camelot_key, energy
         FROM audio_dsp_features WHERE track_id IN ({ids_csv})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<f64>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<f64>>(3)?,
        ))
    })?;
    let mut map = HashMap::new();
    for r in rows {
        let (track_id, bpm, camelot, energy) = r?;
        map.insert(track_id, (bpm, camelot, energy));
    }
    Ok(map)
}

/// Flat genre names per track, batched for discovery ranking's weighted
/// Jaccard. Names, not paths: plain-name sets forgo the ancestor bonus but
/// avoid a second, heavier path-resolution query on the interactive path.
pub fn get_genre_names_for_tracks(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids_csv: String = track_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT tg.track_id, g.name
         FROM track_genres tg
         JOIN genres g ON g.id = tg.genre_id
         WHERE tg.track_id IN ({ids_csv})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut map: HashMap<i64, Vec<String>> = HashMap::new();
    for r in rows {
        let (track_id, name) = r?;
        map.entry(track_id).or_default().push(name);
    }
    Ok(map)
}

/// Genre tag lists for external track candidates (parsed from their
/// `genre_tags_json` sidecar column), batched by candidate id.
pub fn get_external_candidate_genre_tags(
    conn: &Connection,
    candidate_ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>> {
    if candidate_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids_csv: String = candidate_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id, genre_tags_json FROM external_track_candidates
         WHERE id IN ({ids_csv}) AND genre_tags_json IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut map = HashMap::new();
    for r in rows {
        let (id, raw) = r?;
        if let Ok(tags) = serde_json::from_str::<Vec<String>>(&raw)
            && !tags.is_empty()
        {
            map.insert(id, tags);
        }
    }
    Ok(map)
}

/// Track ids heard in a listening session, for the discovery exclude-heard
/// filter. `session_id: None` falls back to the most recent session so the
/// filter still means something when the client has not minted one yet.
pub fn get_session_heard_track_ids(
    conn: &Connection,
    session_id: Option<&str>,
) -> Result<HashSet<i64>> {
    let mut out = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT DISTINCT track_id FROM listen_history
         WHERE session_id = COALESCE(
             ?1,
             (SELECT session_id FROM listen_history
              WHERE session_id IS NOT NULL
              ORDER BY started_at DESC LIMIT 1)
         )",
    )?;
    let rows = stmt.query_map(params![session_id], |row| row.get::<_, i64>(0))?;
    for r in rows {
        out.insert(r?);
    }
    Ok(out)
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
    let filter = crate::db::tidal_content::browse_predicate(conn)?
        .map(|predicate| format!(" WHERE {predicate}"))
        .unwrap_or_default();
    let mut stmt = conn.prepare(&format!(
        "SELECT {}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         {filter}
         ORDER BY t.is_favorite DESC, t.play_count DESC, t.date_added DESC, t.title ASC
         LIMIT ?1",
        track_projection("a")
    ))?;

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
    let mut sql = format!(
        "SELECT {}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id",
        track_projection("a")
    );

    if !excluded_track_ids.is_empty() {
        sql.push_str(" WHERE t.id NOT IN (");
        sql.push_str(&placeholders(excluded_track_ids.len()));
        sql.push(')');
    }

    if let Some(predicate) = crate::db::tidal_content::browse_predicate(conn)? {
        sql.push_str(if excluded_track_ids.is_empty() {
            " WHERE "
        } else {
            " AND "
        });
        sql.push_str(predicate);
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

/// Artist-diverse recall over the seed's genres: one representative track per
/// artist that shares any genre with the seed. Used to widen an automix pool that
/// has collapsed to a handful of artists - a precomputed similar-pool is often the
/// seed's own catalogue plus one over-connected neighbour, and no per-artist cap
/// can create diversity the pool doesn't contain. Returning a single track per
/// artist makes this a breadth-first artist sample (no deep runs), which the
/// scorer's shared-genre boost then keeps on-vibe. Empty when the seed is
/// untagged. Caller dedupes against the existing pool and exclusions.
pub fn get_genre_diverse_candidates(
    conn: &Connection,
    seed_track_id: i64,
    limit: usize,
) -> Result<Vec<Track>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    // One track per artist sharing a seed genre. Track and artist order come
    // from a stable per-seed hash, not the lowest ids, so different seeds widen
    // with different tracks instead of the same old rows every time.
    let sql = format!(
        "WITH pool AS (
             SELECT DISTINCT t2.id, t2.artist_id,
                    (((t2.id * 2654435761) % 1000003) * ((?1 * 40503) % 1000003 + 1)) % 1000003 AS k
             FROM tracks t2
             JOIN track_genres tg ON tg.track_id = t2.id
             WHERE tg.genre_id IN (
                 SELECT genre_id FROM track_genres WHERE track_id = ?1
             )
               AND t2.id != ?1
         ),
         pick AS (
             SELECT id, k, ROW_NUMBER() OVER (PARTITION BY artist_id ORDER BY k, id) AS rn
             FROM pool
         )
         SELECT {proj}
         FROM pick
         JOIN tracks t ON t.id = pick.id
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE pick.rn = 1
         ORDER BY pick.k, t.id
         LIMIT {limit}",
        proj = track_projection("a"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(params![seed_track_id], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tracks)
}

pub fn get_existing_tidal_track_ids(conn: &Connection, tidal_ids: &[i64]) -> Result<HashSet<i64>> {
    if tidal_ids.is_empty() {
        return Ok(HashSet::new());
    }

    let placeholders = placeholders(tidal_ids.len());
    let table = if crate::db::catalogue::enabled(conn)? {
        "tidal_track_aliases"
    } else {
        "tracks"
    };
    let query = format!("SELECT tidal_id FROM {table} WHERE tidal_id IN ({placeholders})");
    let params = params_from_iter(tidal_ids.iter().copied());
    let mut stmt = conn.prepare(&query)?;
    let ids = stmt
        .query_map(params, |row| row.get::<_, i64>(0))?
        .collect::<Result<HashSet<_>, _>>()?;

    Ok(ids)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TidalTrackLibraryState {
    pub local_id: i64,
    pub is_favorite: bool,
}

pub fn get_tidal_track_library_states(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, TidalTrackLibraryState>> {
    if tidal_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = placeholders(tidal_ids.len());
    let sql = if crate::db::catalogue::enabled(conn)? {
        format!(
            "SELECT a.tidal_id,t.id,t.is_favorite FROM tidal_track_aliases a JOIN tracks t ON t.id=a.track_id WHERE a.tidal_id IN ({placeholders})"
        )
    } else {
        format!("SELECT tidal_id,id,is_favorite FROM tracks WHERE tidal_id IN ({placeholders})")
    };
    let params = params_from_iter(tidal_ids.iter().copied());
    let mut stmt = conn.prepare(&sql)?;
    let mut map = HashMap::new();
    let rows = stmt.query_map(params, |row| {
        Ok((
            row.get::<_, i64>(0)?,
            TidalTrackLibraryState {
                local_id: row.get::<_, i64>(1)?,
                is_favorite: row.get::<_, i64>(2)? != 0,
            },
        ))
    })?;
    for row in rows {
        let (tidal_id, state) = row?;
        map.insert(tidal_id, state);
    }
    Ok(map)
}

pub fn get_tidal_track_local_ids(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, i64>> {
    Ok(get_tidal_track_library_states(conn, tidal_ids)?
        .into_iter()
        .map(|(tidal_id, state)| (tidal_id, state.local_id))
        .collect())
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

pub(super) fn parse_discovery_services(raw: &str) -> Vec<String> {
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
