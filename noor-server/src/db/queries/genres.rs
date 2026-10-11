//! Genre taxonomy and track genre queries.

use super::*;

// ─── Genres ───────────────────────────────────────────────

pub fn get_genres_filtered(
    conn: &Connection,
    filter: crate::genre::filter::GalaxyFilterRule,
) -> Result<Vec<Genre>> {
    let sub = galaxy_library_gate(&crate::genre::filter::filter_subquery(filter));
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
    let mut genres = get_genres_filtered(conn, filter)?;
    let subtree_counts = genre_subtree_track_counts(conn, filter)?;
    for genre in &mut genres {
        genre.track_count = Some(subtree_counts.get(&genre.id).copied().unwrap_or(0));
    }
    Ok(build_genre_tree(genres))
}

/// Distinct library tracks under each genre's whole subtree, using the same
/// membership rules as `get_tracks_by_genre_filtered(.., include_descendants =
/// true)`: the galaxy library gate, the confidence filter, and the Spotify
/// dominance rule. A node's count always equals the list its genre page shows.
pub(super) fn genre_subtree_track_counts(
    conn: &Connection,
    filter: crate::genre::filter::GalaxyFilterRule,
) -> Result<HashMap<i64, i64>> {
    let mut parent_of: HashMap<i64, Option<i64>> = HashMap::new();
    let mut stmt = conn.prepare("SELECT id, parent_id FROM genres")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
    })?;
    for row in rows {
        let (id, parent) = row?;
        parent_of.insert(id, parent);
    }
    // Self first, then each ancestor up to the root.
    let ancestors_of = |genre_id: i64| -> Vec<i64> {
        let mut chain = Vec::new();
        let mut cursor = Some(genre_id);
        while let Some(id) = cursor {
            if chain.contains(&id) {
                break; // cycle guard
            }
            chain.push(id);
            cursor = parent_of.get(&id).copied().flatten();
        }
        chain
    };

    // A track Spotify tagged at all only counts under subtrees that contain one
    // of its Spotify tags (raw table, independent of the confidence filter).
    let mut spotify_ancestors: HashMap<i64, HashSet<i64>> = HashMap::new();
    let mut stmt =
        conn.prepare("SELECT track_id, genre_id FROM track_genres WHERE source = 'spotify'")?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
    for row in rows {
        let (track_id, genre_id) = row?;
        spotify_ancestors
            .entry(track_id)
            .or_default()
            .extend(ancestors_of(genre_id));
    }

    let sub = galaxy_library_gate(&crate::genre::filter::filter_subquery(filter));
    let mut members: HashMap<i64, HashSet<i64>> = HashMap::new();
    let mut stmt = conn.prepare(&format!("SELECT track_id, genre_id FROM ({sub})"))?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
    for row in rows {
        let (track_id, genre_id) = row?;
        let spotify = spotify_ancestors.get(&track_id);
        for ancestor in ancestors_of(genre_id) {
            if spotify.is_some_and(|allowed| !allowed.contains(&ancestor)) {
                continue;
            }
            members.entry(ancestor).or_default().insert(track_id);
        }
    }

    Ok(members
        .into_iter()
        .map(|(id, tracks)| (id, tracks.len() as i64))
        .collect())
}

pub fn get_genre_heat_filtered(
    conn: &Connection,
    days: i64,
    filter: crate::genre::filter::GalaxyFilterRule,
) -> Result<Vec<GenreHeat>> {
    let sub = galaxy_library_gate(&crate::genre::filter::filter_subquery(filter));
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
            // rusqlite 0.40 dropped `FromSql for usize`; read as i64 and cast.
            child_count: row.get::<_, i64>(6)? as usize,
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
    let projection = track_projection("a");
    let lib_where = ARTIST_LIBRARY_TRACK_WHERE;
    // The Spotify-dominance EXISTS check still queries raw `track_genres` —
    // it's a "did Spotify ever tag this track at all" predicate, independent
    // of the confidence filter that decides which clusters the track is
    // visible in. The MAIN membership join uses the filtered rowset. Both
    // branches also gate on the Library predicate so hidden enrichment fill
    // (is_library = 0) never surfaces in galaxy track lists.
    let sql = if include_descendants {
        format!(
            "WITH RECURSIVE selected_genres(id) AS (
                SELECT id FROM genres WHERE id = ?1
                UNION ALL
                SELECT g.id
                FROM genres g
                JOIN selected_genres sg ON g.parent_id = sg.id
            )
            SELECT DISTINCT {projection}
             FROM selected_genres sg
             JOIN ({sub}) tg ON tg.genre_id = sg.id
             JOIN tracks t ON tg.track_id = t.id
             LEFT JOIN artists a ON t.artist_id = a.id
             LEFT JOIN albums al ON t.album_id = al.id
             WHERE {lib_where}
               AND (
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
            "SELECT DISTINCT {projection}
             FROM ({sub}) tg
             JOIN tracks t ON tg.track_id = t.id
             LEFT JOIN artists a ON t.artist_id = a.id
             LEFT JOIN albums al ON t.album_id = al.id
             WHERE tg.genre_id = ?1
               AND {lib_where}
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
    } else {
        // Always stamp last_played_at even for partial listens so freshness
        // weighting can distinguish "heard recently" from "never heard."
        conn.execute(
            "UPDATE tracks SET last_played_at = ?2 WHERE id = ?1",
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
                completion_rate: None,
                share_of_window_listened_ms: None,
                previous_rank: None,
                rank_delta: None,
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
                completion_rate: None,
                share_of_window_listened_ms: None,
                previous_rank: None,
                rank_delta: None,
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
                share_of_window_listens: None,
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
