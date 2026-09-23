//! Reusable video candidates and bounded inputs for a replenishing video mix.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use super::video_sets::{AnchorArtist, VideoCandidate, VideoSetItem};

const ARTIST_CACHE_DAYS: i64 = 14;
const RELATED_CACHE_DAYS: i64 = 7;
const GENRE_CACHE_DAYS: i64 = 14;
pub const UNFAMILIAR_PER_BATCH: usize = 4;

pub fn cache_groups(
    conn: &Connection,
    groups: &[(AnchorArtist, Vec<VideoCandidate>)],
) -> Result<()> {
    for (anchor, videos) in groups {
        if anchor.tidal_id > 0 {
            mark_artist_scanned(conn, anchor.tidal_id)?;
        }
        for video in videos {
            let artist_id = video
                .artist_id
                .or((anchor.tidal_id > 0).then_some(anchor.tidal_id));
            let mut normalized = video.clone();
            normalized.artist_id = artist_id;
            if normalized.artist_name.is_none() && anchor.tidal_id > 0 {
                normalized.artist_name = Some(anchor.name.clone());
            }
            conn.execute(
                "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(tidal_video_id) DO UPDATE SET
                   artist_tidal_id = excluded.artist_tidal_id,
                   artist_name = excluded.artist_name,
                   item_json = excluded.item_json,
                   fetched_at = datetime('now')",
                params![
                    video.tidal_id,
                    artist_id,
                    normalized.artist_name.as_deref(),
                    serde_json::to_string(&normalized)?,
                ],
            )?;
        }
    }
    // The catalog is a bounded working index, not a permanent copy of TIDAL.
    conn.execute(
        "DELETE FROM video_catalog WHERE fetched_at < datetime('now', '-180 days')",
        [],
    )?;
    Ok(())
}

pub fn mark_artist_scanned(conn: &Connection, artist_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO video_artist_scans (artist_tidal_id) VALUES (?1)
         ON CONFLICT(artist_tidal_id) DO UPDATE SET scanned_at = datetime('now')",
        [artist_id],
    )?;
    Ok(())
}

pub fn artist_due(conn: &Connection, artist_id: i64) -> Result<bool> {
    let age: Option<i64> = conn
        .query_row(
            "SELECT CAST(julianday('now') - julianday(scanned_at) AS INTEGER)
         FROM video_artist_scans WHERE artist_tidal_id = ?1",
            [artist_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(age.is_none_or(|days| days >= ARTIST_CACHE_DAYS))
}

pub fn related_due(conn: &Connection, artist_id: i64) -> Result<bool> {
    let age: Option<i64> = conn
        .query_row(
            "SELECT CAST(julianday('now') - julianday(scanned_at) AS INTEGER)
         FROM video_related_scans WHERE seed_tidal_id = ?1",
            [artist_id],
            |row| row.get(0),
        )
        .optional()?;
    if age.is_some_and(|days| days < RELATED_CACHE_DAYS) {
        return Ok(false);
    }
    // A radio session moves seeds often. Limit the entire relationship runner
    // to one expansion per 15 minutes, not one per newly played artist.
    let recent: i64 = conn.query_row(
        "SELECT COUNT(*) FROM video_related_scans WHERE scanned_at >= datetime('now', '-15 minutes')",
        [], |row| row.get(0),
    )?;
    Ok(recent == 0)
}

pub fn store_related(
    conn: &Connection,
    seed_id: i64,
    related: &[(i64, String, &'static str)],
) -> Result<()> {
    conn.execute(
        "INSERT INTO video_related_scans (seed_tidal_id) VALUES (?1)
         ON CONFLICT(seed_tidal_id) DO UPDATE SET scanned_at = datetime('now')",
        [seed_id],
    )?;
    for (id, name, source) in related {
        if *id == seed_id || *id <= 0 {
            continue;
        }
        conn.execute(
            "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(seed_tidal_id, related_tidal_id) DO UPDATE SET
               name = excluded.name, source = excluded.source",
            params![seed_id, id, name, source],
        )?;
    }
    Ok(())
}

pub fn store_seed_genres(conn: &Connection, seed_id: i64, genres: &[String]) -> Result<()> {
    conn.execute(
        "DELETE FROM video_seed_genres WHERE seed_tidal_id = ?1",
        [seed_id],
    )?;
    for (rank, genre) in genres.iter().take(5).enumerate() {
        conn.execute(
            "INSERT OR IGNORE INTO video_seed_genres (seed_tidal_id, genre_name, rank) VALUES (?1, ?2, ?3)",
            params![seed_id, genre, rank as i64],
        )?;
    }
    Ok(())
}

pub fn local_artist_id(conn: &Connection, name: &str) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT tidal_id FROM artists WHERE name = ?1 COLLATE NOCASE AND tidal_id IS NOT NULL LIMIT 1",
        [name], |row| row.get(0),
    ).optional()?)
}

/// Presence in the catalog is not familiarity: imported discovery artists may
/// have local rows. Only deliberate listens and saves count as known taste.
pub fn familiar_artist_ids(conn: &Connection) -> Result<HashSet<i64>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT a.tidal_id FROM artists a
         JOIN tracks t ON t.artist_id = a.id
         WHERE a.tidal_id IS NOT NULL AND t.is_favorite = 1
         UNION
         SELECT DISTINCT a.tidal_id FROM artists a
         JOIN albums al ON al.artist_id = a.id
         WHERE a.tidal_id IS NOT NULL AND al.is_favorite = 1
         UNION
         SELECT DISTINCT a.tidal_id FROM listen_history lh
         JOIN tracks t ON t.id = lh.track_id
         JOIN artists a ON a.id = t.artist_id
         WHERE a.tidal_id IS NOT NULL AND COALESCE(lh.source, '') NOT IN ('radio', 'automix')",
    )?;
    Ok(stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<HashSet<_>, _>>()?)
}

/// One seed genre for the external video-search lane. Library tags win; tags
/// fetched from Last.fm fill in unfamiliar artists with no local tracks.
pub fn seed_genre(conn: &Connection, seed_id: i64) -> Result<Option<String>> {
    let local = conn
        .query_row(
            "SELECT g.name FROM artists a
         JOIN tracks t ON t.artist_id = a.id
         JOIN track_genres tg ON tg.track_id = t.id
         JOIN genres g ON g.id = tg.genre_id
         WHERE a.tidal_id = ?1 GROUP BY g.id ORDER BY COUNT(*) DESC LIMIT 1",
            [seed_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if local.is_some() {
        return Ok(local);
    }
    Ok(conn
        .query_row(
            "SELECT vg.genre_name FROM video_seed_genres vg
             JOIN genres g ON g.name = vg.genre_name COLLATE NOCASE
             WHERE vg.seed_tidal_id = ?1 ORDER BY vg.rank LIMIT 1",
            [seed_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

pub fn genre_due(conn: &Connection, genre: &str) -> Result<bool> {
    let age: Option<i64> = conn
        .query_row(
            "SELECT CAST(julianday('now') - julianday(scanned_at) AS INTEGER)
         FROM video_genre_scans WHERE genre_name = ?1",
            [genre],
            |row| row.get(0),
        )
        .optional()?;
    if age.is_some_and(|days| days < GENRE_CACHE_DAYS) {
        return Ok(false);
    }
    let recent: i64 = conn.query_row(
        "SELECT COUNT(*) FROM video_genre_scans WHERE scanned_at >= datetime('now', '-30 minutes')",
        [],
        |row| row.get(0),
    )?;
    Ok(recent == 0)
}

pub fn mark_genre_scanned(conn: &Connection, genre: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO video_genre_scans (genre_name) VALUES (?1)
         ON CONFLICT(genre_name) DO UPDATE SET scanned_at = datetime('now')",
        [genre],
    )?;
    Ok(())
}

pub fn store_genre_artists(
    conn: &Connection,
    seed_id: i64,
    videos: &[VideoCandidate],
) -> Result<()> {
    let mut seen = HashSet::new();
    for video in videos {
        let Some(id) = video.artist_id.filter(|id| *id > 0) else {
            continue;
        };
        if !seen.insert(id) || id == seed_id {
            continue;
        }
        let Some(name) = video.artist_name.as_deref() else {
            continue;
        };
        conn.execute(
            "INSERT OR IGNORE INTO video_related_artists
             (seed_tidal_id, related_tidal_id, name, source) VALUES (?1, ?2, ?3, 'genre')",
            params![seed_id, id, name],
        )?;
    }
    Ok(())
}

/// First the seed and its direct relationships, then genre neighbours and
/// library taste. Recent seeds keep the relationship graph connected as the
/// current artist changes during playback.
pub fn artist_pool(
    conn: &Connection,
    seed_id: Option<i64>,
    recent_seeds: &[i64],
    library_anchors: &[AnchorArtist],
) -> Result<Vec<(i64, String, u8)>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    if let Some(id) = seed_id.filter(|id| *id > 0) {
        let name: Option<String> = conn.query_row(
            "SELECT COALESCE((SELECT name FROM artists WHERE tidal_id = ?1),
                             (SELECT artist_name FROM video_catalog WHERE artist_tidal_id = ?1 LIMIT 1))",
            [id], |row| row.get(0),
        ).optional()?.flatten();
        out.push((id, name.unwrap_or_default(), 0));
        seen.insert(id);
        let mut stmt = conn.prepare(
            "SELECT related_tidal_id, name, source FROM video_related_artists
             WHERE seed_tidal_id = ?1
             ORDER BY CASE source WHEN 'lastfm' THEN 0 WHEN 'tidal' THEN 1 ELSE 2 END,
                      related_tidal_id LIMIT 30",
        )?;
        for row in stmt.query_map([id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })? {
            let (related_id, name, source) = row?;
            if seen.insert(related_id) {
                out.push((related_id, name, if source == "genre" { 2 } else { 1 }));
            }
        }
        let mut stmt = conn.prepare(
            "SELECT a.tidal_id, a.name FROM artists a
             JOIN tracks t ON t.artist_id = a.id
             JOIN track_genres tg ON tg.track_id = t.id
             WHERE a.tidal_id IS NOT NULL AND tg.genre_id IN (
               SELECT DISTINCT tg2.genre_id FROM artists seed
               JOIN tracks st ON st.artist_id = seed.id
               JOIN track_genres tg2 ON tg2.track_id = st.id
               WHERE seed.tidal_id = ?1)
             GROUP BY a.id ORDER BY COUNT(DISTINCT tg.genre_id) DESC, a.name LIMIT 20",
        )?;
        for row in stmt.query_map([id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })? {
            let (genre_id, name) = row?;
            if seen.insert(genre_id) {
                out.push((genre_id, name, 2));
            }
        }
        // Last.fm tags give an unfamiliar TIDAL artist a route back into the
        // listener's local genre graph, even when that artist has no tracks here.
        let mut stmt = conn.prepare(
            "SELECT a.tidal_id, a.name FROM artists a
             JOIN tracks t ON t.artist_id = a.id
             JOIN track_genres tg ON tg.track_id = t.id
             JOIN genres g ON g.id = tg.genre_id
             JOIN video_seed_genres vg ON vg.genre_name = g.name COLLATE NOCASE
             WHERE vg.seed_tidal_id = ?1 AND a.tidal_id IS NOT NULL
             GROUP BY a.id ORDER BY COUNT(DISTINCT g.id) DESC, a.name LIMIT 20",
        )?;
        for row in stmt.query_map([id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })? {
            let (genre_id, name) = row?;
            if seen.insert(genre_id) {
                out.push((genre_id, name, 2));
            }
        }
    }
    let mut prior_seeds = HashSet::new();
    for id in recent_seeds
        .iter()
        .rev()
        .copied()
        .filter(|id| *id > 0 && Some(*id) != seed_id)
    {
        if !prior_seeds.insert(id) {
            continue;
        }
        let mut stmt = conn.prepare(
            "SELECT related_tidal_id, name, source FROM video_related_artists
             WHERE seed_tidal_id = ?1 ORDER BY related_tidal_id LIMIT 20",
        )?;
        for row in stmt.query_map([id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })? {
            let (related_id, name, source) = row?;
            if seen.insert(related_id) {
                out.push((related_id, name, if source == "genre" { 2 } else { 1 }));
            }
        }
        if prior_seeds.len() >= 3 {
            break;
        }
    }
    for anchor in library_anchors.iter().take(30) {
        if seen.insert(anchor.tidal_id) {
            out.push((anchor.tidal_id, anchor.name.clone(), 3));
        }
    }
    Ok(out)
}

pub fn load_candidates(
    conn: &Connection,
    artists: &[(i64, String, u8)],
) -> Result<Vec<(VideoCandidate, u8)>> {
    let ids: Vec<String> = artists.iter().map(|a| a.0.to_string()).collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let lane: HashMap<i64, u8> = artists.iter().map(|a| (a.0, a.2)).collect();
    let sql = format!(
        "SELECT artist_tidal_id, item_json FROM video_catalog WHERE artist_tidal_id IN ({})
         ORDER BY fetched_at DESC LIMIT 600",
        ids.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for row in rows {
        let (artist_id, json) = row?;
        if let (Some(priority), Ok(video)) = (
            artist_id.and_then(|id| lane.get(&id)),
            serde_json::from_str::<VideoCandidate>(&json),
        ) {
            if seen.insert(video.tidal_id) {
                out.push((video, *priority));
            }
        }
    }
    // Liked-song video matches are already indexed by the library scanner.
    // Reuse them directly; duplicating those rows in video_catalog adds no value.
    let sql = format!(
        "SELECT a.tidal_id, a.name, lv.tidal_video_id, lv.video_title,
                lv.duration_seconds, lv.image_id, lv.release_year
         FROM library_videos lv
         JOIN tracks t ON t.id = lv.track_id
         JOIN artists a ON a.id = t.artist_id
         WHERE lv.suppressed = 0 AND a.tidal_id IN ({})
         ORDER BY lv.match_score DESC LIMIT 300",
        ids.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<i32>>(6)?,
        ))
    })?;
    for row in rows {
        let (artist_id, artist_name, video_id, title, duration_s, image_id, release_year) = row?;
        if !seen.insert(video_id) {
            continue;
        }
        out.push((
            VideoCandidate {
                tidal_id: video_id,
                title,
                duration_s,
                artist_id: Some(artist_id),
                artist_name: Some(artist_name),
                album_tidal_id: None,
                artwork_url: crate::services::tidal::client::TidalClient::get_artwork_url(
                    &image_id, 640,
                ),
                release_year,
            },
            *lane.get(&artist_id).unwrap_or(&3),
        ));
    }
    Ok(out)
}

pub fn select_batch(
    candidates: &[(VideoCandidate, u8)],
    excluded: &HashSet<i64>,
    watched: &HashSet<i64>,
    recent_artists: &[i64],
    familiar_artists: &HashSet<i64>,
    limit: usize,
) -> Vec<VideoSetItem> {
    let recent: HashSet<i64> = recent_artists.iter().copied().collect();
    let mut scored: Vec<_> = candidates
        .iter()
        .filter(|(v, _)| !excluded.contains(&v.tidal_id))
        .map(|(v, lane)| {
            let watched_penalty = if watched.contains(&v.tidal_id) { 10 } else { 0 };
            let artist_penalty = if v.artist_id.is_some_and(|id| recent.contains(&id)) {
                5
            } else {
                0
            };
            (v, *lane as i32 * 2 + watched_penalty + artist_penalty)
        })
        .collect();
    scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.tidal_id.cmp(&b.0.tidal_id)));
    let mut out = Vec::new();
    let mut artist_count = HashMap::<i64, usize>::new();
    let mut used = HashSet::new();
    // Familiarity is the spine of the mix, with a related/genre discovery pick
    // in every third slot. Relax the artist cap before switching lanes so a
    // large unfamiliar pool cannot crowd out the familiar share.
    while out.len() < limit {
        let last_artist = out
            .last()
            .and_then(|v: &VideoSetItem| v.artist_id)
            .or_else(|| recent_artists.last().copied());
        let prefer_unfamiliar = out.len() % 3 == 2;
        let mut next = None;
        for avoid_adjacent in [true, false] {
            for want_unfamiliar in [prefer_unfamiliar, !prefer_unfamiliar] {
                for cap in [1, 2, 3] {
                    next = scored.iter().find(|(video, _)| {
                        let artist = video.artist_id.unwrap_or(-video.tidal_id);
                        let unfamiliar = video
                            .artist_id
                            .is_some_and(|id| !familiar_artists.contains(&id));
                        !used.contains(&video.tidal_id)
                            && artist_count.get(&artist).copied().unwrap_or(0) < cap
                            && unfamiliar == want_unfamiliar
                            && (!avoid_adjacent || Some(artist) != last_artist)
                    });
                    if next.is_some() {
                        break;
                    }
                }
                if next.is_some() {
                    break;
                }
            }
            if next.is_some() {
                break;
            }
        }
        let Some((video, _)) = next else {
            break;
        };
        used.insert(video.tidal_id);
        let artist = video.artist_id.unwrap_or(-video.tidal_id);
        *artist_count.entry(artist).or_default() += 1;
        out.push(VideoSetItem {
            tidal_id: video.tidal_id,
            title: video.title.clone(),
            duration_ms: video.duration_s.map(|d| d * 1000),
            artist_id: video.artist_id,
            artist_name: video.artist_name.clone(),
            album_tidal_id: video.album_tidal_id,
            artwork_url: video.artwork_url.clone(),
            quality: None,
            explicit: None,
            kind: "Music Video".into(),
            why: String::new(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_video_and_relationship_are_reused_without_a_scan() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        let anchor = AnchorArtist {
            tidal_id: 42,
            name: "Seed".into(),
            listens: 1,
            via: None,
        };
        let video = VideoCandidate {
            tidal_id: 901,
            title: "Live".into(),
            duration_s: Some(220),
            artist_id: Some(42),
            artist_name: Some("Seed".into()),
            album_tidal_id: None,
            artwork_url: None,
            release_year: Some(2024),
        };
        cache_groups(&conn, &[(anchor, vec![video])]).unwrap();
        store_related(&conn, 42, &[(43, "Neighbour".into(), "tidal")]).unwrap();
        assert!(!artist_due(&conn, 42).unwrap());
        assert!(!related_due(&conn, 42).unwrap());
        let pool = artist_pool(&conn, Some(42), &[], &[]).unwrap();
        assert!(pool.iter().any(|(id, _, _)| *id == 43));
        let flowing_pool = artist_pool(&conn, Some(44), &[42], &[]).unwrap();
        assert!(flowing_pool.iter().any(|(id, _, _)| *id == 43));
        let cached = load_candidates(&conn, &pool).unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].0.tidal_id, 901);
    }

    #[test]
    fn genre_search_can_add_an_unfamiliar_artist_to_the_pool() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO genres (id, name, slug) VALUES (1, 'Electronic', 'electronic')",
            [],
        )
        .unwrap();
        store_seed_genres(&conn, 42, &["Seen live".into(), "Electronic".into()]).unwrap();
        assert_eq!(
            seed_genre(&conn, 42).unwrap().as_deref(),
            Some("Electronic")
        );
        let video = VideoCandidate {
            tidal_id: 902,
            title: "Discovery".into(),
            duration_s: Some(210),
            artist_id: Some(45),
            artist_name: Some("New Artist".into()),
            album_tidal_id: None,
            artwork_url: None,
            release_year: None,
        };
        cache_groups(
            &conn,
            &[(
                AnchorArtist {
                    tidal_id: -1,
                    name: "Electronic music video".into(),
                    listens: 1,
                    via: None,
                },
                vec![video.clone()],
            )],
        )
        .unwrap();
        store_genre_artists(&conn, 42, &[video]).unwrap();
        mark_genre_scanned(&conn, "Electronic").unwrap();
        assert!(!genre_due(&conn, "Electronic").unwrap());
        let pool = artist_pool(&conn, Some(42), &[], &[]).unwrap();
        assert!(pool.iter().any(|(id, _, lane)| *id == 45 && *lane == 2));
        assert!(
            load_candidates(&conn, &pool)
                .unwrap()
                .iter()
                .any(|(video, _)| video.tidal_id == 902)
        );
    }

    #[test]
    fn batch_prefers_new_artists_and_excludes_queue() {
        let candidate = |id, artist| VideoCandidate {
            tidal_id: id,
            title: format!("Video {id}"),
            duration_s: Some(180),
            artist_id: Some(artist),
            artist_name: Some(format!("Artist {artist}")),
            album_tidal_id: None,
            artwork_url: None,
            release_year: None,
        };
        let pool = vec![
            (candidate(1, 1), 0),
            (candidate(2, 1), 0),
            (candidate(3, 2), 1),
            (candidate(4, 3), 2),
        ];
        let selected = select_batch(
            &pool,
            &HashSet::from([1]),
            &HashSet::from([3]),
            &[1],
            &HashSet::new(),
            3,
        );
        assert_eq!(
            selected.iter().map(|v| v.tidal_id).collect::<Vec<_>>(),
            vec![4, 2, 3]
        );
    }

    #[test]
    fn familiar_videos_lead_and_unfamiliar_artists_are_interleaved() {
        let candidates: Vec<_> = (1..=12)
            .map(|artist| {
                (
                    VideoCandidate {
                        tidal_id: 100 + artist,
                        title: format!("Video {artist}"),
                        duration_s: Some(180),
                        artist_id: Some(artist),
                        artist_name: Some(format!("Artist {artist}")),
                        album_tidal_id: None,
                        artwork_url: None,
                        release_year: None,
                    },
                    if artist <= 8 { 0 } else { 1 },
                )
            })
            .collect();
        let familiar: HashSet<i64> = (1..=8).collect();
        let items = select_batch(
            &candidates,
            &HashSet::new(),
            &HashSet::new(),
            &[],
            &familiar,
            12,
        );
        assert_eq!(items.len(), 12);
        for (index, item) in items.iter().enumerate() {
            let is_unfamiliar = !familiar.contains(&item.artist_id.unwrap());
            assert_eq!(is_unfamiliar, index % 3 == 2, "slot {index}");
        }
        assert_eq!(
            items
                .iter()
                .map(|item| item.tidal_id)
                .collect::<HashSet<_>>()
                .len(),
            12
        );
    }
}
