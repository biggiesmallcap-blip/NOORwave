//! Reusable video candidates and bounded inputs for a replenishing video mix.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use super::video_sets::{AnchorArtist, VideoCandidate, VideoSetItem};

const GENRE_CACHE_DAYS: i64 = 14;
pub const UNFAMILIAR_ARTISTS_FOR_HEALTHY_QUEUE: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceLane {
    Seed,
    Direct,
    Genre,
    Library,
    Bridge,
}

impl SourceLane {
    fn score(self) -> i32 {
        match self {
            Self::Seed => 0,
            Self::Direct => 2,
            Self::Genre => 4,
            Self::Library => 6,
            Self::Bridge => 8,
        }
    }
    pub fn is_close(self) -> bool {
        matches!(self, Self::Seed | Self::Direct | Self::Genre)
    }
    fn seeded_priority(self) -> u8 {
        match self {
            Self::Seed => 0,
            Self::Direct => 1,
            Self::Genre => 2,
            Self::Library => 3,
            Self::Bridge => 4,
        }
    }
}

pub fn unfamiliar_artist_count(
    items: &[VideoSetItem],
    familiar: &HashSet<i64>,
    seed: Option<i64>,
) -> usize {
    items
        .iter()
        .filter_map(|item| item.artist_id)
        .filter(|id| Some(*id) != seed && !familiar.contains(id))
        .collect::<HashSet<_>>()
        .len()
}

pub fn queue_needs_discovery(total: usize, unfamiliar_artists: usize) -> bool {
    total < 8 || unfamiliar_artists < UNFAMILIAR_ARTISTS_FOR_HEALTHY_QUEUE
}

pub fn cache_groups(
    conn: &Connection,
    groups: &[(AnchorArtist, Vec<VideoCandidate>)],
) -> Result<()> {
    cache_groups_without_prune(conn, groups)?;
    prune_catalog(conn)
}

/// The liked-video sweep may write thousands of artists in one pass. It prunes
/// once at the end instead of scanning the entire catalog after every artist.
pub fn cache_groups_without_prune(
    conn: &Connection,
    groups: &[(AnchorArtist, Vec<VideoCandidate>)],
) -> Result<()> {
    use crate::services::video_discovery::artist_state::{self, CheckResult};
    use crate::services::video_discovery::harvest::{self, HarvestContext};
    for (anchor, videos) in groups {
        if anchor.tidal_id <= 0 {
            harvest::ingest(conn, videos, HarvestContext::Search)?;
            continue;
        }
        let summary = harvest::ingest(
            conn,
            videos,
            HarvestContext::ArtistPage {
                artist_id: anchor.tidal_id,
                name: &anchor.name,
            },
        )?;
        artist_state::record_page(conn, anchor.tidal_id, 0, videos.len() as i64, None)?;
        let result = if videos.is_empty() {
            CheckResult::Empty
        } else {
            CheckResult::Found {
                new_videos: summary.new_videos as i64,
            }
        };
        artist_state::record_check(conn, anchor.tidal_id, result)?;
    }
    Ok(())
}

pub fn prune_catalog(conn: &Connection) -> Result<()> {
    // The catalog is a bounded working index, not a permanent copy of TIDAL.
    conn.execute(
        "DELETE FROM video_catalog WHERE fetched_at < datetime('now', '-180 days')",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
pub fn store_related(
    conn: &Connection,
    seed_id: i64,
    related: &[(i64, String, &'static str)],
) -> Result<()> {
    for (rank, (id, name, source)) in related.iter().enumerate() {
        if *id == seed_id || *id <= 0 {
            continue;
        }
        let weight = if *source == "tidal" {
            crate::services::video_discovery::graph::tidal_weight(rank)
        } else {
            crate::services::video_discovery::graph::lastfm_weight(None, rank)
        };
        conn.execute(
            "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, rank, weight)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(seed_tidal_id, related_tidal_id, source) DO UPDATE SET
               name = excluded.name, weight = excluded.weight",
            params![seed_id, id, name, source, rank as i64, weight],
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

const DIRECT_POOL: usize = 30;
const BRIDGE_POOL: usize = 30;

fn display_name(conn: &Connection, artist_id: i64) -> Result<String> {
    Ok(conn
        .query_row(
            "SELECT COALESCE(
                (SELECT name FROM artists WHERE tidal_id = ?1 AND name <> '' LIMIT 1),
                (SELECT name FROM video_artist_state WHERE artist_tidal_id = ?1 AND name <> ''),
                (SELECT artist_name FROM video_catalog WHERE artist_tidal_id = ?1 AND artist_name <> '' LIMIT 1),
                '')",
            [artist_id],
            |row| row.get(0),
        )
        .unwrap_or_default())
}

/// Seeded stations follow the weighted graph from the seed alone: hop one is
/// the Direct lane and hops two and three the Bridge lane, each in relevance
/// order. Library radio flows from recent seeds' strongest neighbors and the
/// listener's anchors.
pub fn station_pool(
    conn: &Connection,
    graph: &crate::services::video_discovery::graph::Graph,
    seed_id: Option<i64>,
    recent_seeds: &[i64],
    library_anchors: &[AnchorArtist],
) -> Result<Vec<(i64, String, SourceLane)>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    if let Some(seed) = seed_id.filter(|id| *id > 0) {
        out.push((seed, display_name(conn, seed)?, SourceLane::Seed));
        seen.insert(seed);
        let mut ranked: Vec<_> = graph
            .propagate(&[(seed, 1.0)], 3)
            .into_iter()
            .filter(|(id, _)| *id != seed)
            .collect();
        ranked.sort_by(|a, b| b.1.score.total_cmp(&a.1.score).then(a.0.cmp(&b.0)));
        let direct = ranked.iter().filter(|(_, r)| r.hops == 1).take(DIRECT_POOL);
        let bridge = ranked.iter().filter(|(_, r)| r.hops >= 2).take(BRIDGE_POOL);
        for ((id, _), lane) in direct
            .map(|entry| (entry, SourceLane::Direct))
            .chain(bridge.map(|entry| (entry, SourceLane::Bridge)))
        {
            if seen.insert(*id) {
                out.push((*id, display_name(conn, *id)?, lane));
            }
        }
        if let Some(genre) = seed_genre(conn, seed)? {
            let mut stmt = conn.prepare(
                "SELECT a.tidal_id, a.name FROM artists a
                 JOIN tracks t ON t.artist_id = a.id
                 JOIN track_genres tg ON tg.track_id = t.id
                 JOIN genres g ON g.id = tg.genre_id
                 WHERE a.tidal_id IS NOT NULL AND g.name = ?1 COLLATE NOCASE
                 GROUP BY a.id HAVING COUNT(DISTINCT t.id) >= 2
                 ORDER BY COUNT(DISTINCT t.id) DESC, a.name LIMIT 20",
            )?;
            for row in stmt.query_map([genre], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })? {
                let (genre_id, name) = row?;
                if seen.insert(genre_id) {
                    out.push((genre_id, name, SourceLane::Genre));
                }
            }
        }
    }
    let mut prior = HashSet::new();
    for id in recent_seeds
        .iter()
        .rev()
        .copied()
        .filter(|id| *id > 0 && Some(*id) != seed_id)
    {
        if !prior.insert(id) {
            continue;
        }
        for (neighbor, _) in graph.neighbors(id).iter().take(20) {
            if seen.insert(*neighbor) {
                out.push((
                    *neighbor,
                    display_name(conn, *neighbor)?,
                    SourceLane::Direct,
                ));
            }
        }
        if prior.len() >= 3 {
            break;
        }
    }
    for anchor in library_anchors.iter().take(30) {
        if seen.insert(anchor.tidal_id) {
            out.push((anchor.tidal_id, anchor.name.clone(), SourceLane::Library));
        }
    }
    Ok(out)
}

pub fn load_candidates(
    conn: &Connection,
    artists: &[(i64, String, SourceLane)],
) -> Result<Vec<(VideoCandidate, SourceLane)>> {
    let ids: Vec<String> = artists.iter().map(|a| a.0.to_string()).collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let priority = artists
        .iter()
        .enumerate()
        .map(|(rank, (id, _, _))| format!("WHEN {id} THEN {rank}"))
        .collect::<Vec<_>>()
        .join(" ");
    let lane: HashMap<i64, SourceLane> = artists.iter().map(|a| (a.0, a.2)).collect();
    let lead = artists[0].0;
    let sql = format!(
        "SELECT artist_tidal_id, item_json FROM video_catalog
         WHERE artist_tidal_id IN ({})
         ORDER BY CASE WHEN artist_tidal_id = {lead} THEN 0 ELSE 1 END,
                  CASE artist_tidal_id {priority} ELSE 999 END,
                  COALESCE(json_extract(item_json, '$.popularity'), -1) DESC,
                  tidal_video_id DESC",
        ids.join(","),
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
         ORDER BY CASE a.tidal_id {priority} ELSE 999 END, lv.match_score DESC",
        ids.join(","),
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
                ..Default::default()
            },
            *lane.get(&artist_id).unwrap_or(&SourceLane::Library),
        ));
    }
    Ok(out)
}

/// Collapse alternate cuts of the same song while keeping the artist in the
/// key. A live/visualizer suffix should not make one song fill the queue twice.
pub fn video_song_key(artist_id: Option<i64>, artist_name: Option<&str>, title: &str) -> String {
    let artist = artist_id
        .map(|id| format!("id:{id}"))
        .unwrap_or_else(|| format!("name:{}", artist_name.unwrap_or("").trim().to_lowercase()));
    let base = title.split(['(', '[']).next().unwrap_or(title);
    let normalize = |text: &str| {
        text.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let normalized_title = normalize(base);
    let normalized_title = if normalized_title.is_empty() {
        normalize(title)
    } else {
        normalized_title
    };
    format!("{artist}:{normalized_title}")
}

/// (position among this artist's videos, artist's first position) for each
/// video, so a lane offers every artist's best video in relevance order
/// before anyone's second.
fn breadth_order(candidates: &[(VideoCandidate, SourceLane)]) -> HashMap<i64, (usize, usize)> {
    let mut first_seen = HashMap::new();
    let mut depth = HashMap::new();
    let mut out = HashMap::new();
    for (index, (video, _)) in candidates.iter().enumerate() {
        let artist = video.artist_id.unwrap_or(-video.tidal_id);
        let first = *first_seen.entry(artist).or_insert(index);
        let position = depth.entry(artist).or_insert(0usize);
        out.entry(video.tidal_id).or_insert((*position, first));
        *position += 1;
    }
    out
}

/// Artist radio stays inside the seed's own videos and its direct graph.
/// Related artists provide discovery; broad library anchors never fill gaps.
pub fn select_seeded_batch(
    candidates: &[(VideoCandidate, SourceLane)],
    excluded_ids: &HashSet<i64>,
    recent_song_keys: &HashSet<String>,
    watched: &HashSet<i64>,
    recent_artists: &[i64],
    limit: usize,
) -> Vec<VideoSetItem> {
    let mut scored: Vec<_> = candidates
        .iter()
        .filter(|(video, lane)| {
            *lane != SourceLane::Library && !excluded_ids.contains(&video.tidal_id)
        })
        .filter_map(|(video, lane)| {
            let key = video_song_key(video.artist_id, video.artist_name.as_deref(), &video.title);
            (!recent_song_keys.contains(&key)).then_some((video, *lane, key))
        })
        .collect();
    let order = breadth_order(candidates);
    scored.sort_by_key(|(video, lane, _)| {
        (
            watched.contains(&video.tidal_id) as u8,
            lane.seeded_priority(),
            (*lane != SourceLane::Seed
                && video
                    .artist_id
                    .is_some_and(|id| recent_artists.contains(&id))) as u8,
            order
                .get(&video.tidal_id)
                .copied()
                .unwrap_or((usize::MAX, usize::MAX)),
            video.tidal_id,
        )
    });
    let mut out = Vec::new();
    let mut used_songs = HashSet::<String>::new();
    let mut artist_counts = HashMap::<i64, usize>::new();
    let mut genre_count = 0;
    let mut bridge_count = 0;
    while out.len() < limit {
        let lanes = match out.len() % 4 {
            0 | 2 => [
                SourceLane::Seed,
                SourceLane::Direct,
                SourceLane::Bridge,
                SourceLane::Genre,
            ],
            1 => [
                SourceLane::Direct,
                SourceLane::Seed,
                SourceLane::Bridge,
                SourceLane::Genre,
            ],
            _ => [
                SourceLane::Bridge,
                SourceLane::Seed,
                SourceLane::Direct,
                SourceLane::Genre,
            ],
        };
        let last_artist = out
            .last()
            .and_then(|video: &VideoSetItem| video.artist_id)
            .or_else(|| recent_artists.last().copied());
        let mut chosen = None;
        for allow_watched in [false, true] {
            for lane in lanes {
                if (lane == SourceLane::Genre && genre_count >= 2)
                    || (lane == SourceLane::Bridge && bridge_count >= 4)
                {
                    continue;
                }
                for avoid_adjacent in [true, false] {
                    for artist_cap in [2, 4, limit] {
                        chosen = scored.iter().find(|(video, candidate_lane, key)| {
                            let artist = video.artist_id.unwrap_or(-video.tidal_id);
                            *candidate_lane == lane
                                && (allow_watched || !watched.contains(&video.tidal_id))
                                && !used_songs.contains(key.as_str())
                                && (lane == SourceLane::Seed
                                    || artist_counts.get(&artist).copied().unwrap_or(0)
                                        < artist_cap)
                                && (!avoid_adjacent || Some(artist) != last_artist)
                        });
                        if chosen.is_some() {
                            break;
                        }
                    }
                    if chosen.is_some() {
                        break;
                    }
                }
                if chosen.is_some() {
                    break;
                }
            }
            if chosen.is_some() {
                break;
            }
        }
        let Some((video, lane, key)) = chosen else {
            break;
        };
        used_songs.insert((*key).clone());
        let artist = video.artist_id.unwrap_or(-video.tidal_id);
        *artist_counts.entry(artist).or_default() += 1;
        if *lane == SourceLane::Genre {
            genre_count += 1;
        } else if *lane == SourceLane::Bridge {
            bridge_count += 1;
        }
        out.push(VideoSetItem {
            tidal_id: video.tidal_id,
            title: video.title.clone(),
            duration_ms: video.duration_s.map(|duration| duration * 1000),
            artist_id: video.artist_id,
            artist_name: video.artist_name.clone(),
            album_tidal_id: video.album_tidal_id,
            artwork_url: video.artwork_url.clone(),
            quality: video.quality.clone(),
            explicit: video.explicit,
            kind: "Music Video".into(),
            why: match lane {
                SourceLane::Seed => "More from this artist",
                SourceLane::Direct => "Related artist",
                SourceLane::Genre => "Shared genre",
                SourceLane::Bridge => "Recommended by related artists",
                SourceLane::Library => unreachable!("seeded selection excludes library"),
            }
            .into(),
        });
    }
    out
}

pub fn select_batch(
    candidates: &[(VideoCandidate, SourceLane)],
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
            (v, lane.score() + watched_penalty + artist_penalty)
        })
        .collect();
    let order = breadth_order(candidates);
    scored.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| order.get(&a.0.tidal_id).cmp(&order.get(&b.0.tidal_id)))
            .then_with(|| a.0.tidal_id.cmp(&b.0.tidal_id))
    });
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
            quality: video.quality.clone(),
            explicit: video.explicit,
            kind: "Music Video".into(),
            why: String::new(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_discovery::graph::Graph;

    fn store_related_weighted(conn: &Connection, seed: i64, related: &[(i64, &str, &str, f64)]) {
        for (rank, (id, name, source, weight)) in related.iter().enumerate() {
            conn.execute(
                "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, rank, weight)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![seed, id, name, source, rank as i64, weight],
            )
            .unwrap();
        }
    }

    #[test]
    fn station_pool_reads_cached_relationships_and_catalog() {
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
            artist_id: Some(42),
            artist_name: Some("Seed".into()),
            ..Default::default()
        };
        cache_groups(&conn, &[(anchor, vec![video])]).unwrap();
        store_related(&conn, 42, &[(43, "Neighbour".into(), "tidal")]).unwrap();
        let graph = Graph::load(&conn).unwrap();
        let pool = station_pool(&conn, &graph, Some(42), &[], &[]).unwrap();
        assert_eq!(pool[0], (42, "Seed".to_string(), SourceLane::Seed));
        assert!(
            pool.iter()
                .any(|(id, _, lane)| *id == 43 && *lane == SourceLane::Direct)
        );
        let flowing = station_pool(&conn, &graph, None, &[42], &[]).unwrap();
        assert!(flowing.iter().any(|(id, _, _)| *id == 43));
        let cached = load_candidates(&conn, &pool).unwrap();
        assert_eq!(cached[0].0.tidal_id, 901);
    }

    #[test]
    fn a_strong_second_hop_becomes_a_bridge_and_a_weak_chain_does_not() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        store_related_weighted(
            &conn,
            10,
            &[
                (20, "Neighbor A", "lastfm", 1.0),
                (30, "Loose", "tidal", 0.3),
            ],
        );
        store_related_weighted(&conn, 20, &[(40, "Strong hop", "lastfm", 1.0)]);
        store_related_weighted(&conn, 30, &[(50, "Weak hop", "tidal", 0.3)]);
        let graph = Graph::load(&conn).unwrap();
        let pool = station_pool(&conn, &graph, Some(10), &[], &[]).unwrap();
        assert!(
            pool.iter()
                .any(|(id, _, lane)| *id == 40 && *lane == SourceLane::Bridge)
        );
        assert!(!pool.iter().any(|(id, _, _)| *id == 50));
    }

    #[test]
    fn within_a_lane_relevance_order_beats_video_ids() {
        let candidate = |id, artist| {
            (
                VideoCandidate {
                    tidal_id: id,
                    title: format!("Song {id}"),
                    artist_id: Some(artist),
                    artist_name: Some(format!("Artist {artist}")),
                    ..Default::default()
                },
                SourceLane::Direct,
            )
        };
        // Pool order says artist 7 is the closer neighbor; its ids are larger.
        let candidates = vec![candidate(50, 7), candidate(60, 7), candidate(10, 8)];
        let picked: Vec<i64> = select_seeded_batch(
            &candidates,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            &[],
            3,
        )
        .iter()
        .map(|v| v.tidal_id)
        .collect();
        assert_eq!(picked, vec![50, 10, 60]);
    }

    #[test]
    fn queue_health_counts_distinct_unfamiliar_artists() {
        let item = |artist_id| VideoSetItem {
            tidal_id: artist_id * 10,
            title: "Video".into(),
            duration_ms: None,
            artist_id: Some(artist_id),
            artist_name: None,
            album_tidal_id: None,
            artwork_url: None,
            quality: None,
            explicit: None,
            kind: "Music Video".into(),
            why: String::new(),
        };
        let one_artist = vec![item(20), item(20), item(20), item(20)];
        assert_eq!(
            unfamiliar_artist_count(&one_artist, &HashSet::new(), Some(10)),
            1
        );
        assert!(queue_needs_discovery(12, 1));
        let three = vec![item(20), item(30), item(40), item(10)];
        assert_eq!(
            unfamiliar_artist_count(&three, &HashSet::new(), Some(10)),
            3
        );
        assert!(!queue_needs_discovery(12, 3));
        assert!(queue_needs_discovery(7, 3));
    }

    #[test]
    fn all_cached_neighbor_videos_remain_candidates() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        let candidate = |id, artist, name: &str| VideoCandidate {
            tidal_id: id,
            title: format!("Video {id}"),
            duration_s: Some(180),
            artist_id: Some(artist),
            artist_name: Some(name.into()),
            album_tidal_id: None,
            artwork_url: None,
            release_year: None,
            ..Default::default()
        };
        cache_groups(
            &conn,
            &[(
                AnchorArtist {
                    tidal_id: 10,
                    name: "Seed".into(),
                    listens: 1,
                    via: None,
                },
                vec![candidate(1, 10, "Seed")],
            )],
        )
        .unwrap();
        let neighbor_videos = (1000..=1600)
            .map(|id| candidate(id, 20, "Neighbor"))
            .collect();
        cache_groups(
            &conn,
            &[(
                AnchorArtist {
                    tidal_id: 20,
                    name: "Neighbor".into(),
                    listens: 1,
                    via: None,
                },
                neighbor_videos,
            )],
        )
        .unwrap();
        let candidates = load_candidates(
            &conn,
            &[
                (10, "Seed".into(), SourceLane::Seed),
                (20, "Neighbor".into(), SourceLane::Direct),
            ],
        )
        .unwrap();
        assert_eq!(candidates.len(), 602);
        assert!(candidates.iter().any(|(video, _)| video.tidal_id == 1));
    }

    #[test]
    fn lastfm_genre_tag_can_find_a_local_neighbor_with_cached_video() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO genres (id, name, slug) VALUES (1, 'Electronic', 'electronic')",
            [],
        )
        .unwrap();
        store_seed_genres(&conn, 42, &["Seen live".into(), "Electronic".into()]).unwrap();
        conn.execute_batch(
            "INSERT INTO artists (id, tidal_id, name) VALUES
                (1, 45, 'New Artist'), (2, 46, 'One-off tag');
             INSERT INTO tracks (id, artist_id, title) VALUES
                (1, 1, 'Discovery'), (2, 1, 'Another'), (3, 2, 'Noise');
             INSERT INTO track_genres (track_id, genre_id) VALUES
                (1, 1), (2, 1), (3, 1);",
        )
        .unwrap();
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
            ..Default::default()
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
        assert!(genre_due(&conn, "Electronic").unwrap());
        let pool = station_pool(&conn, &Graph::load(&conn).unwrap(), Some(42), &[], &[]).unwrap();
        assert!(
            pool.iter()
                .any(|(id, _, lane)| *id == 45 && *lane == SourceLane::Genre)
        );
        assert!(!pool.iter().any(|(id, _, _)| *id == 46));
        assert!(
            load_candidates(&conn, &pool)
                .unwrap()
                .iter()
                .any(|(video, _)| video.tidal_id == 902)
        );
    }

    #[test]
    fn green_day_style_genre_fallback_excludes_broad_rock_only_artists() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO genres (id, name, slug) VALUES
                (1, 'Punk Rock', 'punk-rock'), (2, 'Rock', 'rock');
             INSERT INTO artists (id, tidal_id, name) VALUES
                (1, 10, 'Green Day'), (2, 20, 'Punk Neighbor'), (3, 30, 'Broad Rock');
             INSERT INTO tracks (id, artist_id, title) VALUES
                (1, 1, 'Seed One'), (2, 1, 'Seed Two'),
                (3, 2, 'Punk One'), (4, 2, 'Punk Two'),
                (5, 3, 'Rock One'), (6, 3, 'Rock Two');
             INSERT INTO track_genres (track_id, genre_id) VALUES
                (1, 1), (2, 1), (1, 2),
                (3, 1), (4, 1), (5, 2), (6, 2);",
        )
        .unwrap();
        let pool = station_pool(&conn, &Graph::load(&conn).unwrap(), Some(10), &[], &[]).unwrap();
        assert!(
            pool.iter()
                .any(|(id, _, lane)| *id == 20 && *lane == SourceLane::Genre)
        );
        assert!(!pool.iter().any(|(id, _, _)| *id == 30));
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
            ..Default::default()
        };
        let pool = vec![
            (candidate(1, 1), SourceLane::Seed),
            (candidate(2, 1), SourceLane::Seed),
            (candidate(3, 2), SourceLane::Direct),
            (candidate(4, 3), SourceLane::Genre),
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
                        ..Default::default()
                    },
                    if artist <= 8 {
                        SourceLane::Seed
                    } else {
                        SourceLane::Direct
                    },
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

    #[test]
    fn seeded_radio_stays_with_seed_and_direct_neighbors_without_repeating_songs() {
        let candidate = |id, artist, title: &str, lane| {
            (
                VideoCandidate {
                    tidal_id: id,
                    title: title.into(),
                    duration_s: Some(180),
                    artist_id: Some(artist),
                    artist_name: Some(format!("Artist {artist}")),
                    album_tidal_id: None,
                    artwork_url: None,
                    release_year: None,
                    ..Default::default()
                },
                lane,
            )
        };
        let candidates = vec![
            candidate(1, 10, "Basket Case (Live)", SourceLane::Seed),
            candidate(2, 10, "Basket Case [Official Video]", SourceLane::Seed),
            candidate(3, 10, "Holiday", SourceLane::Seed),
            candidate(4, 10, "American Idiot", SourceLane::Seed),
            candidate(5, 10, "Jesus of Suburbia", SourceLane::Seed),
            candidate(6, 20, "Related one", SourceLane::Direct),
            candidate(7, 20, "Related two", SourceLane::Direct),
            candidate(8, 30, "Another neighbor", SourceLane::Direct),
            candidate(9, 40, "Genre neighbor", SourceLane::Genre),
            candidate(12, 60, "Corroborated neighbor", SourceLane::Bridge),
            candidate(10, 50, "Unrelated favorite", SourceLane::Library),
            candidate(11, 51, "Another unrelated favorite", SourceLane::Library),
        ];
        let recent_song = video_song_key(Some(10), None, "American Idiot (Visualizer)");
        let selected = select_seeded_batch(
            &candidates,
            &HashSet::new(),
            &HashSet::from([recent_song]),
            &HashSet::new(),
            &[10],
            12,
        );
        let ids: HashSet<_> = selected.iter().map(|video| video.tidal_id).collect();
        assert!(!ids.contains(&4), "a recently queued song must stay out");
        assert!(
            !(ids.contains(&1) && ids.contains(&2)),
            "alternate cuts of one song must collapse"
        );
        assert!(
            !ids.contains(&10) && !ids.contains(&11),
            "library anchors are unrelated to this seed"
        );
        assert!(
            selected
                .iter()
                .filter(|video| video.artist_id == Some(10))
                .count()
                >= 3
        );
        assert!(selected.iter().any(|video| video.artist_id == Some(20)));
        assert!(
            selected
                .iter()
                .any(|video| video.why == "Recommended by related artists")
        );
        assert!(
            selected
                .iter()
                .filter(|video| video.why == "Shared genre")
                .count()
                <= 2
        );
    }

    #[test]
    fn corroborated_artists_are_blended_through_a_seeded_mix() {
        let candidates = (1..=36)
            .map(|id| {
                let (artist, lane) = if id <= 12 {
                    (10, SourceLane::Seed)
                } else if id <= 24 {
                    (20, SourceLane::Direct)
                } else {
                    (30, SourceLane::Bridge)
                };
                (
                    VideoCandidate {
                        tidal_id: id,
                        title: format!("Song {id}"),
                        duration_s: Some(180),
                        artist_id: Some(artist),
                        artist_name: Some(format!("Artist {artist}")),
                        album_tidal_id: None,
                        artwork_url: None,
                        release_year: None,
                        ..Default::default()
                    },
                    lane,
                )
            })
            .collect::<Vec<_>>();
        let selected = select_seeded_batch(
            &candidates,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            &[],
            12,
        );
        assert_eq!(selected.len(), 12);
        assert!(
            selected
                .iter()
                .filter(|video| video.artist_id == Some(10))
                .count()
                >= 5
        );
        assert_eq!(
            selected
                .iter()
                .filter(|video| video.artist_id == Some(30))
                .count(),
            3
        );
    }

    #[test]
    fn a_related_artist_does_not_dominate_when_other_neighbors_have_videos() {
        let candidates: Vec<_> = (1..=32)
            .map(|id| {
                let (artist, lane) = if id <= 8 {
                    (10, SourceLane::Seed)
                } else if id <= 20 {
                    (20, SourceLane::Direct)
                } else {
                    (30, SourceLane::Direct)
                };
                (
                    VideoCandidate {
                        tidal_id: id,
                        title: format!("Song {id}"),
                        duration_s: Some(180),
                        artist_id: Some(artist),
                        artist_name: Some(format!("Artist {artist}")),
                        album_tidal_id: None,
                        artwork_url: None,
                        release_year: None,
                        ..Default::default()
                    },
                    lane,
                )
            })
            .collect();
        let selected = select_seeded_batch(
            &candidates,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            &[],
            12,
        );
        assert_eq!(selected.len(), 12);
        assert!(
            selected
                .iter()
                .filter(|item| item.artist_id == Some(10))
                .count()
                >= 5
        );
        assert!(
            selected
                .iter()
                .filter(|item| item.artist_id == Some(20))
                .count()
                <= 4
        );
        assert!(selected.iter().any(|item| item.artist_id == Some(30)));
    }

    #[test]
    fn large_seed_catalog_does_not_hide_new_artist_videos() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        let seed = AnchorArtist {
            tidal_id: 10,
            name: "Seed".into(),
            listens: 1,
            via: None,
        };
        let neighbor = AnchorArtist {
            tidal_id: 20,
            name: "Neighbor".into(),
            listens: 1,
            via: None,
        };
        let make_video = |id, artist_id, name: &str| VideoCandidate {
            tidal_id: id,
            title: format!("Song {id}"),
            duration_s: Some(180),
            artist_id: Some(artist_id),
            artist_name: Some(name.into()),
            album_tidal_id: None,
            artwork_url: None,
            release_year: None,
            ..Default::default()
        };
        let seed_videos = (1..=650).map(|id| make_video(id, 10, "Seed")).collect();
        cache_groups_without_prune(
            &conn,
            &[
                (seed, seed_videos),
                (neighbor, vec![make_video(999, 20, "Neighbor")]),
            ],
        )
        .unwrap();
        let candidates = load_candidates(
            &conn,
            &[
                (10, "Seed".into(), SourceLane::Seed),
                (20, "Neighbor".into(), SourceLane::Direct),
            ],
        )
        .unwrap();
        assert_eq!(candidates.len(), 651);
        assert!(
            candidates
                .iter()
                .any(|(video, lane)| video.tidal_id == 999 && *lane == SourceLane::Direct)
        );
    }
}
