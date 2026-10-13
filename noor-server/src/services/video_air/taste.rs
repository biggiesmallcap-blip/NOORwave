//! Taste for On Air: roots from the music library and the watch history,
//! relevance spread over the artist graph, scene appetite, and per-video
//! watch facts. Computed per build; every number is neutral on missing data.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use rusqlite::Connection;

use super::universe::Universe;
use crate::services::video_discovery::graph::{self, Graph, Relevance};
use crate::services::video_discovery::roots;
use crate::services::video_stations::pool::Listener;

/// Relevance spreads three hops from the roots, as the crawler's does.
pub const RELEVANCE_HOPS: u8 = 3;
const APPETITE_FLOOR: f64 = 0.05;
const CURIOSITY_BONUS: f64 = 0.1;
const CURIOSITY_DAYS: f64 = 30.0;
const ZAP_MS: i64 = 15_000;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Root {
    pub weight: f64,
    pub plays: i64,
    pub library: bool,
    pub saved: bool,
    pub enjoyed: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VideoStats {
    pub plays: u32,
    pub zaps: u32,
    pub holds: u32,
    /// Days since the most recent watch.
    pub last_age_days: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ArtistStats {
    pub zaps: u32,
    pub holds: u32,
}

#[derive(Debug, Default)]
pub struct Taste {
    pub roots: HashMap<i64, Root>,
    pub names: HashMap<i64, String>,
    pub relevance: HashMap<i64, Relevance>,
    /// For artists one hop from a root: the root that pulls hardest.
    pub via: HashMap<i64, i64>,
    /// Scene id -> appetite, 0.05..1.1.
    pub appetite: HashMap<i64, f64>,
    pub watched_artists: HashSet<i64>,
    pub videos: HashMap<i64, VideoStats>,
    pub artists: HashMap<i64, ArtistStats>,
    /// Old-style skip counts (roots::is_skip), for the station Listener.
    pub skips: HashMap<i64, u32>,
    pub graph: Arc<Graph>,
}

/// A play stopped inside 15 s or under 10% of the video.
pub fn is_zap(watched_ms: i64, duration_ms: Option<i64>, completed: bool) -> bool {
    !completed
        && (watched_ms < ZAP_MS
            || duration_ms.is_some_and(|d| d > 0 && (watched_ms as f64) < 0.1 * d as f64))
}

/// A play that reached 70% or the end.
pub fn is_hold(watched_ms: i64, duration_ms: Option<i64>, completed: bool) -> bool {
    completed || duration_ms.is_some_and(|d| d > 0 && watched_ms as f64 >= 0.7 * d as f64)
}

/// Library artist 0.6 plus 0.1 per liked track (that part capped at 1),
/// listens add log1p(listens)/4, an enjoyed watch its roots.rs weight, a
/// saved video or a station on air 1.0. The whole root is capped at 1.
pub fn root_weight(
    library: bool,
    liked_tracks: i64,
    listens: i64,
    enjoyed: f64,
    saved: bool,
    station: bool,
) -> f64 {
    let base = if library || liked_tracks > 0 {
        (0.6 + 0.1 * liked_tracks.max(0) as f64).min(1.0)
    } else {
        0.0
    };
    let total = base
        + (listens.max(0) as f64).ln_1p() / 4.0
        + enjoyed.max(0.0)
        + if saved { 1.0 } else { 0.0 }
        + if station { 1.0 } else { 0.0 };
    total.min(1.0)
}

/// Root weight in a scene over sqrt(scene artists), normalised to the best
/// scene, floored at 0.05, plus 0.1 curiosity for scenes not played lately.
pub fn appetite(
    scene_weight: &HashMap<i64, f64>,
    scene_artists: &HashMap<i64, usize>,
    scenes: &[i64],
    played_recently: &HashSet<i64>,
) -> HashMap<i64, f64> {
    let raw: Vec<(i64, f64)> = scenes
        .iter()
        .map(|scene| {
            let weight = scene_weight.get(scene).copied().unwrap_or(0.0);
            let artists = scene_artists.get(scene).copied().unwrap_or(0).max(1) as f64;
            (*scene, weight / artists.sqrt())
        })
        .collect();
    let max = raw.iter().map(|(_, value)| *value).fold(0.0, f64::max);
    raw.into_iter()
        .map(|(scene, value)| {
            let normal = if max > 0.0 { value / max } else { 0.0 };
            let curiosity = if played_recently.contains(&scene) {
                0.0
            } else {
                CURIOSITY_BONUS
            };
            (scene, normal.max(APPETITE_FLOOR) + curiosity)
        })
        .collect()
}

struct HistoryRow {
    video: i64,
    artist: Option<i64>,
    name: Option<String>,
    watched_ms: Option<i64>,
    duration_ms: Option<i64>,
    completed: bool,
    age_days: f64,
}

fn history(conn: &Connection) -> Result<Vec<HistoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT tidal_video_id, artist_tidal_id, artist_name, duration_watched_ms,
                video_duration_ms, completed, julianday('now') - julianday(started_at)
           FROM video_history ORDER BY started_at, id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(HistoryRow {
            video: row.get(0)?,
            artist: row.get(1)?,
            name: row.get(2)?,
            watched_ms: row.get(3)?,
            duration_ms: row.get(4)?,
            completed: row.get::<_, Option<bool>>(5)?.unwrap_or(false),
            age_days: row.get::<_, Option<f64>>(6)?.unwrap_or(0.0),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

impl Taste {
    pub fn load(conn: &Connection, universe: &Universe) -> Result<Self> {
        let mut taste = Taste::default();
        let mut liked_tracks: HashMap<i64, i64> = HashMap::new();
        let mut library: HashSet<i64> = HashSet::new();
        {
            // Library tracks only (indexed); favorite albums come next. Every
            // track of a favorite album shares the album's artist, so the
            // album query covers them without scanning the wide tracks table.
            let mut stmt = conn.prepare(
                "SELECT a.tidal_id, MAX(a.name),
                        SUM(CASE WHEN t.is_favorite = 1 THEN 1 ELSE 0 END)
                   FROM tracks t
                   JOIN artists a ON a.id = t.artist_id
                  WHERE a.tidal_id > 0 AND (t.is_library = 1 OR t.is_favorite = 1)
                  GROUP BY a.tidal_id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })?;
            for row in rows {
                let (artist, name, liked) = row?;
                if let Some(name) = name {
                    taste.names.insert(artist, name);
                }
                let liked = liked.unwrap_or(0);
                if liked > 0 {
                    liked_tracks.insert(artist, liked);
                }
                library.insert(artist);
            }
            let mut stmt = conn.prepare(
                "SELECT a.tidal_id, a.name FROM artists a
                   JOIN albums al ON al.artist_id = a.id AND al.is_favorite = 1
                  WHERE a.tidal_id > 0",
            )?;
            for row in stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get(1)?)))? {
                let (artist, name): (i64, String) = row?;
                library.insert(artist);
                taste.names.entry(artist).or_insert(name);
            }
        }
        let mut listens: HashMap<i64, i64> = HashMap::new();
        let mut recent_listeners: HashSet<i64> = HashSet::new();
        {
            let mut stmt = conn.prepare(&format!(
                "SELECT a.tidal_id, COUNT(*),
                        MAX(lh.started_at >= datetime('now', '-{CURIOSITY_DAYS} days')),
                        MAX(a.name)
                   FROM listen_history lh
                   JOIN tracks t ON t.id = lh.track_id
                   JOIN artists a ON a.id = t.artist_id
                  WHERE a.tidal_id > 0 AND COALESCE(lh.source, '') NOT IN ('radio', 'automix')
                  GROUP BY a.tidal_id"
            ))?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<bool>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?;
            for row in rows {
                let (artist, count, recent, name) = row?;
                if let Some(name) = name {
                    taste.names.entry(artist).or_insert(name);
                }
                listens.insert(artist, count);
                if recent.unwrap_or(false) {
                    recent_listeners.insert(artist);
                }
            }
        }
        let mut saved: HashSet<i64> = HashSet::new();
        {
            let mut stmt = conn.prepare(
                "SELECT CAST(json_extract(item_json, '$.artist_id') AS INTEGER),
                        json_extract(item_json, '$.artist_name')
                   FROM saved_videos WHERE json_type(item_json, '$.artist_id') = 'integer'",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            for row in rows {
                let (artist, name) = row?;
                if artist > 0 {
                    saved.insert(artist);
                    if let Some(name) = name {
                        taste.names.entry(artist).or_insert(name);
                    }
                }
            }
        }
        let mut watched_recently: HashSet<i64> = HashSet::new();
        for row in history(conn)? {
            let artist = row.artist.filter(|a| *a > 0);
            if let Some(artist) = artist {
                taste.watched_artists.insert(artist);
                if row.age_days <= CURIOSITY_DAYS {
                    watched_recently.insert(artist);
                }
                if let Some(name) = row.name.clone() {
                    taste.names.entry(artist).or_insert(name);
                }
            }
            let stats = taste.videos.entry(row.video).or_default();
            stats.plays += 1;
            stats.last_age_days = if stats.plays == 1 {
                row.age_days
            } else {
                stats.last_age_days.min(row.age_days)
            };
            let Some(watched_ms) = row.watched_ms else {
                continue;
            };
            let zap = is_zap(watched_ms, row.duration_ms, row.completed);
            let hold = is_hold(watched_ms, row.duration_ms, row.completed);
            if zap {
                stats.zaps += 1;
            }
            if hold {
                stats.holds += 1;
            }
            if let Some(artist) = artist {
                let artist_stats = taste.artists.entry(artist).or_default();
                artist_stats.zaps += u32::from(zap);
                artist_stats.holds += u32::from(hold);
            }
            if roots::is_skip(&roots::WatchRow {
                artist_id: 0,
                age_days: 0.0,
                watched_ms,
                duration_ms: row.duration_ms,
                completed: false,
            }) {
                *taste.skips.entry(row.video).or_default() += 1;
            }
        }
        let known: HashSet<i64> = library.union(&saved).copied().collect();
        let enjoyed: HashMap<i64, f64> = roots::enjoyed_roots(conn, &known)?
            .into_iter()
            .map(|root| (root.artist_id, root.weight))
            .collect();
        let stations: HashSet<i64> = roots::station_roots()
            .into_iter()
            .map(|root| root.artist_id)
            .collect();
        let candidates: HashSet<i64> = known
            .iter()
            .chain(listens.keys())
            .chain(enjoyed.keys())
            .chain(stations.iter())
            .copied()
            .collect();
        for artist in candidates {
            let plays = listens.get(&artist).copied().unwrap_or(0);
            let weight = root_weight(
                library.contains(&artist),
                liked_tracks.get(&artist).copied().unwrap_or(0),
                plays,
                enjoyed.get(&artist).copied().unwrap_or(0.0),
                saved.contains(&artist),
                stations.contains(&artist),
            );
            if weight > 0.0 {
                taste.roots.insert(
                    artist,
                    Root {
                        weight,
                        plays,
                        library: library.contains(&artist),
                        saved: saved.contains(&artist),
                        enjoyed: enjoyed.contains_key(&artist),
                    },
                );
            }
        }
        taste.graph = graph::cached(conn)?;
        taste.spread();
        let played: HashSet<i64> = watched_recently.union(&recent_listeners).copied().collect();
        taste.appetite = taste.scene_appetite(universe, &played);
        Ok(taste)
    }

    /// Relevance over the graph and, for one-hop artists, the root that
    /// reaches them hardest.
    pub fn spread(&mut self) {
        let mut roots: Vec<(i64, f64)> = self
            .roots
            .iter()
            .map(|(artist, root)| (*artist, root.weight))
            .collect();
        roots.sort_by_key(|(artist, _)| *artist);
        self.relevance = self.graph.propagate(&roots, RELEVANCE_HOPS);
        let mut via: HashMap<i64, (f64, i64)> = HashMap::new();
        for (root, weight) in &roots {
            for (to, edge) in self.graph.neighbors(*root) {
                if self.roots.contains_key(to) {
                    continue;
                }
                let strength = weight * edge;
                let entry = via.entry(*to).or_insert((strength, *root));
                if strength > entry.0 || (strength == entry.0 && *root < entry.1) {
                    *entry = (strength, *root);
                }
            }
        }
        self.via = via
            .into_iter()
            .map(|(artist, (_, root))| (artist, root))
            .collect();
    }

    fn scene_appetite(&self, universe: &Universe, played: &HashSet<i64>) -> HashMap<i64, f64> {
        if universe.scenes.is_empty() {
            return HashMap::new();
        }
        let mut weight: HashMap<i64, f64> = HashMap::new();
        let mut played_scenes: HashSet<i64> = HashSet::new();
        let mut seen: HashSet<i64> = HashSet::new();
        for video in &universe.videos {
            let Some(scene) = video.scene else {
                continue;
            };
            if !seen.insert(video.artist) {
                continue;
            }
            if let Some(root) = self.roots.get(&video.artist) {
                *weight.entry(scene).or_default() += root.weight;
            }
            if played.contains(&video.artist) {
                played_scenes.insert(scene);
            }
        }
        let scenes: Vec<i64> = universe.scenes.iter().map(|scene| scene.id).collect();
        appetite(&weight, &universe.scene_artists, &scenes, &played_scenes)
    }

    pub fn relevance_of(&self, artist: i64) -> f64 {
        self.relevance.get(&artist).map_or(0.0, |r| r.score)
    }

    pub fn hops_of(&self, artist: i64) -> Option<u8> {
        self.relevance.get(&artist).map(|r| r.hops)
    }

    /// The station picker's view of the listener, for the shared spotlight.
    pub fn listener(&self) -> Listener {
        Listener {
            watched: self.videos.keys().copied().collect(),
            watched_artists: self.watched_artists.clone(),
            skips: self.skips.clone(),
            relevance: self
                .relevance
                .iter()
                .map(|(artist, r)| (*artist, r.score))
                .collect(),
            liked: self
                .roots
                .iter()
                .filter(|(_, root)| root.library || root.saved)
                .map(|(artist, _)| *artist)
                .collect(),
            artist_plays: self
                .roots
                .iter()
                .filter(|(_, root)| root.plays > 0)
                .map(|(artist, root)| (*artist, root.plays))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_air::test_support::*;

    #[test]
    fn root_weight_follows_the_plan() {
        assert!((root_weight(true, 0, 0, 0.0, false, false) - 0.6).abs() < 1e-9);
        assert!((root_weight(true, 2, 0, 0.0, false, false) - 0.8).abs() < 1e-9);
        assert!((root_weight(true, 9, 0, 0.0, false, false) - 1.0).abs() < 1e-9);
        let listens = (10f64).ln_1p() / 4.0;
        assert!((root_weight(false, 0, 10, 0.0, false, false) - listens).abs() < 1e-9);
        assert_eq!(root_weight(false, 0, 0, 0.0, true, false), 1.0);
        assert_eq!(root_weight(false, 0, 0, 0.0, false, true), 1.0);
        assert_eq!(root_weight(false, 0, 0, 0.0, false, false), 0.0);
        assert_eq!(root_weight(true, 3, 50, 0.8, true, true), 1.0, "capped");
    }

    #[test]
    fn zaps_and_holds_are_binary_ends() {
        assert!(is_zap(5_000, Some(200_000), false));
        assert!(is_zap(18_000, Some(200_000), false), "under 10%");
        assert!(!is_zap(25_000, Some(200_000), false));
        assert!(!is_zap(5_000, Some(200_000), true));
        assert!(is_hold(150_000, Some(200_000), false));
        assert!(is_hold(1_000, None, true));
        assert!(!is_hold(100_000, Some(200_000), false));
    }

    #[test]
    fn appetite_normalises_floors_and_rewards_curiosity() {
        let weight = HashMap::from([(1, 4.0), (2, 1.0)]);
        let artists = HashMap::from([(1, 4), (2, 1), (3, 9)]);
        let played = HashSet::from([1, 2]);
        let appetite = appetite(&weight, &artists, &[1, 2, 3], &played);
        assert!(
            (appetite[&1] - 1.0).abs() < 1e-9,
            "4 / sqrt(4) = 2 is the max"
        );
        assert!((appetite[&2] - 0.5).abs() < 1e-9);
        assert!((appetite[&3] - 0.15).abs() < 1e-9, "floor plus curiosity");
    }

    #[test]
    fn roots_come_from_library_likes_listens_saves_and_watches() {
        let conn = conn();
        add_library_artist(&conn, 10, "Library Liked", 2, 3);
        add_library_artist(&conn, 20, "Library Plain", 0, 0);
        conn.execute(
            "INSERT INTO saved_videos (tidal_video_id, item_json) VALUES (9, '{\"artist_id\":30,\"artist_name\":\"Saver\"}')",
            [],
        )
        .unwrap();
        add_watch(&conn, 501, 40, 200, 200, 2);
        add_watch(&conn, 502, 50, 3, 200, 1);
        add_edge(&conn, 10, 60, 0.9);
        let universe = Universe::default();
        let taste = Taste::load(&conn, &universe).unwrap();
        let expected = root_weight(true, 2, 3, 0.0, false, false);
        assert!((taste.roots[&10].weight - expected).abs() < 1e-9);
        assert_eq!(taste.roots[&10].plays, 3);
        assert!((taste.roots[&20].weight - 0.6).abs() < 1e-9);
        assert_eq!(taste.roots[&30].weight, 1.0);
        assert!(taste.roots[&40].enjoyed);
        assert!(!taste.roots.contains_key(&50), "a zap is no root");
        assert_eq!(taste.videos[&502].zaps, 1);
        assert_eq!(taste.videos[&501].holds, 1);
        assert_eq!(taste.via.get(&60), Some(&10));
        assert_eq!(taste.hops_of(60), Some(1));
        assert_eq!(taste.names[&10], "Library Liked");
        assert!(taste.watched_artists.contains(&50));
    }
}
