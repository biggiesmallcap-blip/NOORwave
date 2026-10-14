//! Atlas You: the listener's watching drawn on the map. Footprints are the
//! artists watched or saved in the last 180 days (the client fades them by
//! recency), trails are the artists watched in a row, and the frontier is
//! the unexplored artists right next to the footprints. Computed per
//! request from video_history, saved_videos and the discovery graph.
//!
//! A play counts as a watch at 15 s watched or completed. Plays whose watch
//! time was never reported (every play before watch time was recorded, and
//! some since) count by the time until the next play started, so a skip
//! stays a skip and a full watch is not lost.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use super::scenes::VARIOUS_ARTISTS_ID;
use crate::services::video_discovery::graph::{self, Graph};

const DAY_S: i64 = 86_400;
pub const FOOTPRINT_DAYS: i64 = 180;
pub const TRAIL_DAYS: i64 = 60;
/// A longer pause between watches starts a new trail.
pub const TRAIL_GAP_S: i64 = 45 * 60;
pub const MAX_TRAILS: usize = 40;
pub const MAX_FRONTIER: usize = 300;
pub const MIN_WATCH_MS: i64 = 15_000;
/// A footprint's pull on its neighbours halves every two weeks.
pub const HALF_LIFE_DAYS: f64 = 14.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Footprint {
    pub artist_id: i64,
    /// Unix seconds of the newest watch or save.
    pub last_at: i64,
    /// Watches in the window; a saved-only artist has none.
    pub plays: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Trail {
    pub ended_at: i64,
    pub artists: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Frontier {
    pub artist_id: i64,
    pub score: f64,
    /// The footprint artist this one is next to.
    pub via: i64,
}

/// GET /api/videos/atlas/you
#[derive(Debug, Default, Serialize)]
pub struct You {
    pub footprints: Vec<Footprint>,
    pub trails: Vec<Trail>,
    pub frontier: Vec<Frontier>,
}

/// One row of video history, oldest first.
#[derive(Debug, Clone, PartialEq)]
pub struct Play {
    pub artist_id: Option<i64>,
    pub at: i64,
    pub watched_ms: Option<i64>,
    pub video_ms: Option<i64>,
    pub completed: bool,
}

impl Play {
    /// Watch time reported, else the full video when completed.
    fn known_ms(&self) -> i64 {
        match (self.watched_ms, self.completed) {
            (Some(ms), false) => ms,
            (Some(ms), true) => ms.max(self.video_ms.unwrap_or(0)),
            (None, true) => self.video_ms.unwrap_or(0),
            (None, false) => 0,
        }
    }

    /// End of what is known to have been watched, unix seconds.
    fn ended_at(&self) -> i64 {
        self.at + self.known_ms().max(0) / 1000
    }
}

/// Whether each play counts as a watch. `next_at` is when the play after
/// it started (now for the newest).
fn counts(play: &Play, next_at: i64) -> bool {
    if play.completed {
        return true;
    }
    match play.watched_ms {
        Some(ms) => ms >= MIN_WATCH_MS,
        None => {
            let until_next = (next_at - play.at).saturating_mul(1000);
            until_next.min(play.video_ms.unwrap_or(i64::MAX)) >= MIN_WATCH_MS
        }
    }
}

/// The plays that count as watches, with an artist, oldest first.
pub fn watches(plays: &[Play], now: i64) -> Vec<Play> {
    plays
        .iter()
        .enumerate()
        .filter(|(i, play)| {
            let next_at = plays.get(i + 1).map_or(now, |next| next.at);
            play.artist_id.is_some_and(|id| id > 0) && counts(play, next_at)
        })
        .map(|(_, play)| play.clone())
        .collect()
}

/// Footprints from watches and saves (artist, unix seconds), by artist id.
pub fn footprints(watches: &[Play], saves: &[(i64, i64)]) -> Vec<Footprint> {
    let mut by_artist: BTreeMap<i64, Footprint> = BTreeMap::new();
    let mut touch = |artist_id: i64, at: i64, plays: i64| {
        let entry = by_artist.entry(artist_id).or_insert(Footprint {
            artist_id,
            last_at: at,
            plays: 0,
        });
        entry.last_at = entry.last_at.max(at);
        entry.plays += plays;
    };
    for watch in watches {
        if let Some(artist) = watch.artist_id {
            touch(artist, watch.at, 1);
        }
    }
    for &(artist, at) in saves {
        if artist > 0 {
            touch(artist, at, 0);
        }
    }
    by_artist.into_values().collect()
}

/// Session trails over watches since `since`: a pause over 45 minutes
/// between one watch's end and the next start starts a new trail; repeats
/// of the same artist collapse; trails of one artist are dropped. Newest
/// last, at most 40.
pub fn trails(watches: &[Play], since: i64) -> Vec<Trail> {
    let mut out = Vec::new();
    let mut current: Vec<i64> = Vec::new();
    let mut ended_at = 0i64;
    let mut close = |artists: &mut Vec<i64>, ended_at: i64| {
        let distinct: HashSet<&i64> = artists.iter().collect();
        if distinct.len() >= 2 {
            out.push(Trail {
                ended_at,
                artists: std::mem::take(artists),
            });
        } else {
            artists.clear();
        }
    };
    for watch in watches.iter().filter(|w| w.at >= since) {
        let Some(artist) = watch.artist_id else {
            continue;
        };
        if !current.is_empty() && watch.at - ended_at > TRAIL_GAP_S {
            close(&mut current, ended_at);
        }
        if current.last() != Some(&artist) {
            current.push(artist);
        }
        ended_at = ended_at.max(watch.ended_at());
    }
    close(&mut current, ended_at);
    let skip = out.len().saturating_sub(MAX_TRAILS);
    out.split_off(skip)
}

/// How much a footprint made `age_s` ago still pulls: 0.5^(days / 14).
pub fn recency(age_s: i64) -> f64 {
    0.5f64.powf(age_s.max(0) as f64 / DAY_S as f64 / HALF_LIFE_DAYS)
}

/// Unexplored neighbours (either edge direction) of the footprints, on the
/// map and never watched or saved, scored by edge weight x the footprint's
/// recency; `via` is the footprint giving the best score. Top 300.
pub fn frontier(
    footprints: &[Footprint],
    graph: &Graph,
    on_map: &HashSet<i64>,
    touched: &HashSet<i64>,
    now: i64,
) -> Vec<Frontier> {
    let mut best: HashMap<i64, (f64, i64)> = HashMap::new();
    for footprint in footprints {
        if footprint.artist_id == VARIOUS_ARTISTS_ID {
            continue;
        }
        let pull = recency(now - footprint.last_at);
        for (neighbor, weight) in graph.related(footprint.artist_id) {
            if neighbor == VARIOUS_ARTISTS_ID
                || touched.contains(&neighbor)
                || !on_map.contains(&neighbor)
            {
                continue;
            }
            let score = weight * pull;
            let entry = best.entry(neighbor).or_insert((0.0, footprint.artist_id));
            if score > entry.0 || (score == entry.0 && footprint.artist_id < entry.1) {
                *entry = (score, footprint.artist_id);
            }
        }
    }
    let mut out: Vec<Frontier> = best
        .into_iter()
        .filter(|(_, (score, _))| *score > 0.0)
        .map(|(artist_id, (score, via))| Frontier {
            artist_id,
            score,
            via,
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.artist_id.cmp(&b.artist_id))
    });
    out.truncate(MAX_FRONTIER);
    for item in &mut out {
        item.score = (item.score * 1000.0).round() / 1000.0;
    }
    out
}

fn plays_since(conn: &Connection, since: i64) -> Result<Vec<Play>> {
    let mut stmt = conn.prepare(
        "SELECT artist_tidal_id, CAST(strftime('%s', started_at) AS INTEGER),
                duration_watched_ms, video_duration_ms, completed
           FROM video_history
          WHERE started_at >= datetime(?1, 'unixepoch')
            AND strftime('%s', started_at) IS NOT NULL
          ORDER BY started_at, id",
    )?;
    let rows = stmt
        .query_map([since], |row| {
            Ok(Play {
                artist_id: row.get(0)?,
                at: row.get(1)?,
                watched_ms: row.get(2)?,
                video_ms: row.get(3)?,
                completed: row.get::<_, i64>(4)? != 0,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

fn saves_since(conn: &Connection, since: i64) -> Result<Vec<(i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(json_extract(item_json, '$.artist_id') AS INTEGER),
                CAST(strftime('%s', saved_at) AS INTEGER)
           FROM saved_videos
          WHERE saved_at >= datetime(?1, 'unixepoch')
            AND strftime('%s', saved_at) IS NOT NULL",
    )?;
    let rows = stmt
        .query_map([since], |row| {
            Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?))
        })?
        .filter_map(|row| match row {
            Ok((Some(artist), at)) => Some(Ok((artist, at))),
            Ok((None, _)) => None,
            Err(err) => Some(Err(err)),
        })
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

fn id_set(conn: &Connection, sql: &str) -> Result<HashSet<i64>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, Option<i64>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().flatten().filter(|id| *id > 0).collect())
}

/// Everything the You look draws, as of `now` (unix seconds).
pub fn read(conn: &Connection, now: i64) -> Result<You> {
    let plays = plays_since(conn, now - FOOTPRINT_DAYS * DAY_S)?;
    let watches = watches(&plays, now);
    let saves = saves_since(conn, now - FOOTPRINT_DAYS * DAY_S)?;
    let footprints = footprints(&watches, &saves);
    let trails = trails(&watches, now - TRAIL_DAYS * DAY_S);
    // Ever watched (any play, however short) or saved is explored.
    let mut touched = id_set(
        conn,
        "SELECT DISTINCT artist_tidal_id FROM video_history WHERE artist_tidal_id > 0",
    )?;
    touched.extend(id_set(
        conn,
        "SELECT CAST(json_extract(item_json, '$.artist_id') AS INTEGER) FROM saved_videos",
    )?);
    let on_map = id_set(
        conn,
        "SELECT artist_tidal_id FROM video_artist_facets WHERE x IS NOT NULL",
    )?;
    let graph = graph::cached(conn)?;
    let frontier = frontier(&footprints, &graph, &on_map, &touched, now);
    Ok(You {
        footprints,
        trails,
        frontier,
    })
}

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use super::*;
    use crate::services::video_intel::songs::tests::conn;

    const NOW: i64 = 1_791_900_000;
    const MIN: i64 = 60;

    fn play(artist: i64, at: i64, watched_s: Option<i64>) -> Play {
        Play {
            artist_id: Some(artist),
            at,
            watched_ms: watched_s.map(|s| s * 1000),
            video_ms: Some(240_000),
            completed: false,
        }
    }

    #[test]
    fn short_plays_are_skips_and_unreported_ones_count_by_the_next_start() {
        let plays = vec![
            play(1, NOW - 60 * MIN, Some(200)),
            play(2, NOW - 50 * MIN, Some(5)),
            Play {
                completed: true,
                ..play(3, NOW - 49 * MIN, Some(5))
            },
            // Unreported: 8 s until the next play is a skip, 4 minutes is a watch.
            play(4, NOW - 40 * MIN, None),
            play(5, NOW - 40 * MIN + 8, None),
            play(6, NOW - 36 * MIN, None),
            Play {
                artist_id: None,
                ..play(0, NOW - 30 * MIN, Some(200))
            },
            // The newest, unreported, counts against now.
            play(7, NOW - 10, None),
        ];
        let ids: Vec<i64> = watches(&plays, NOW)
            .iter()
            .filter_map(|w| w.artist_id)
            .collect();
        assert_eq!(ids, vec![1, 3, 5, 6]);
    }

    #[test]
    fn footprints_merge_watches_and_saves() {
        let watches = vec![
            play(1, NOW - 3 * DAY_S, Some(200)),
            play(1, NOW - DAY_S, Some(200)),
            play(2, NOW - 2 * DAY_S, Some(200)),
        ];
        let saves = vec![(2, NOW - 10), (3, NOW - 5 * DAY_S), (0, NOW)];
        assert_eq!(
            footprints(&watches, &saves),
            vec![
                Footprint {
                    artist_id: 1,
                    last_at: NOW - DAY_S,
                    plays: 2
                },
                Footprint {
                    artist_id: 2,
                    last_at: NOW - 10,
                    plays: 1
                },
                Footprint {
                    artist_id: 3,
                    last_at: NOW - 5 * DAY_S,
                    plays: 0
                },
            ]
        );
    }

    #[test]
    fn trails_break_on_long_pauses_collapse_repeats_and_drop_singles() {
        let day = NOW - 2 * DAY_S;
        let watches = vec![
            // Too old.
            play(9, NOW - 70 * DAY_S, Some(200)),
            play(8, NOW - 70 * DAY_S + 300, Some(200)),
            // One session: 1, 1, 2, 3 (a long concert does not break it).
            play(1, day, Some(200)),
            play(1, day + 4 * MIN, Some(200)),
            play(2, day + 8 * MIN, Some(3600)),
            play(3, day + 8 * MIN + 3600 + 30 * MIN, Some(200)),
            // Over an hour later: a lone artist, dropped.
            play(4, day + 3 * 3600, Some(200)),
            play(4, day + 3 * 3600 + 5 * MIN, Some(200)),
            // A new session the next day: 5, 6, 5.
            play(5, NOW - DAY_S, Some(200)),
            play(6, NOW - DAY_S + 4 * MIN, Some(200)),
            play(5, NOW - DAY_S + 8 * MIN, Some(100)),
        ];
        let trails = trails(&watches, NOW - TRAIL_DAYS * DAY_S);
        assert_eq!(
            trails,
            vec![
                Trail {
                    ended_at: day + 8 * MIN + 3600 + 30 * MIN + 200,
                    artists: vec![1, 2, 3]
                },
                Trail {
                    ended_at: NOW - DAY_S + 8 * MIN + 100,
                    artists: vec![5, 6, 5]
                },
            ]
        );
        let many: Vec<Play> = (0..100)
            .flat_map(|i| {
                let at = NOW - DAY_S + i * 2 * 3600;
                [play(1, at, Some(200)), play(2, at + 300, Some(200))]
            })
            .collect();
        let kept = super::trails(&many, 0);
        assert_eq!(kept.len(), MAX_TRAILS);
        assert_eq!(kept.last().unwrap().ended_at, NOW - DAY_S + 99 * 7200 + 500);
    }

    #[test]
    fn the_frontier_is_unexplored_neighbours_by_weight_and_recency() {
        // 1 was watched today, 2 four weeks ago. 10 hangs off both, 11 only
        // off 2 (reverse edge), 3 is explored, 12 is off the map.
        let graph = Graph::from_edges([
            (1, 10, 0.5),
            (2, 10, 0.9),
            (11, 2, 0.8),
            (1, 3, 0.9),
            (1, 12, 0.9),
        ]);
        let footprints = vec![
            Footprint {
                artist_id: 1,
                last_at: NOW,
                plays: 1,
            },
            Footprint {
                artist_id: 2,
                last_at: NOW - 28 * DAY_S,
                plays: 1,
            },
        ];
        let on_map = HashSet::from([1, 2, 3, 10, 11]);
        let touched = HashSet::from([1, 2, 3]);
        let frontier = frontier(&footprints, &graph, &on_map, &touched, NOW);
        assert_eq!(
            frontier,
            vec![
                Frontier {
                    artist_id: 10,
                    score: 0.5,
                    via: 1
                },
                Frontier {
                    artist_id: 11,
                    score: 0.2,
                    via: 2
                },
            ]
        );
        assert!((recency(14 * DAY_S) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn read_draws_from_history_saves_and_the_graph() {
        let conn = conn();
        conn.execute_batch(
            "INSERT INTO video_history (tidal_video_id, artist_tidal_id, started_at, duration_watched_ms, completed) VALUES
                 (1, 101, datetime('now', '-3 days'), 200000, 0),
                 (2, 102, datetime('now', '-3 days', '+4 minutes'), 200000, 0),
                 (3, 103, datetime('now', '-3 days', '+8 minutes'), 2000, 0),
                 (4, 104, datetime('now', '-300 days'), 200000, 1);
             INSERT INTO saved_videos (tidal_video_id, item_json, saved_at)
                 VALUES (9, '{\"artist_id\":105}', datetime('now', '-1 day'));
             INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, weight) VALUES
                 (101, 201, 'x', 'lastfm', 0.9),
                 (202, 105, 'x', 'tidal', 0.6),
                 (101, 103, 'x', 'lastfm', 0.9),
                 (102, 104, 'x', 'lastfm', 0.9),
                 (102, 203, 'x', 'lastfm', 0.9);",
        )
        .unwrap();
        for id in [101i64, 102, 103, 104, 105, 201, 202] {
            conn.execute(
                "INSERT INTO video_artist_facets (artist_tidal_id, name, x, y) VALUES (?1, 'a', 1, 1)",
                params![id],
            )
            .unwrap();
        }
        let now = chrono::Utc::now().timestamp();
        let you = read(&conn, now).unwrap();
        let ids: Vec<(i64, i64)> = you
            .footprints
            .iter()
            .map(|f| (f.artist_id, f.plays))
            .collect();
        assert_eq!(
            ids,
            vec![(101, 1), (102, 1), (105, 0)],
            "a skip and a 300 day old watch are not footprints"
        );
        assert_eq!(you.trails.len(), 1);
        assert_eq!(you.trails[0].artists, vec![101, 102]);
        let frontier: Vec<(i64, i64)> = you.frontier.iter().map(|f| (f.artist_id, f.via)).collect();
        // 103 was played (a skip) and 104 watched long ago: both explored.
        // 203 is off the map.
        assert_eq!(frontier, vec![(201, 101), (202, 105)]);
        assert!(you.frontier[0].score > 0.75 && you.frontier[0].score <= 0.9);
        let json = serde_json::to_value(&you).unwrap();
        assert!(json["footprints"][0]["last_at"].as_i64().unwrap() > now - 4 * DAY_S);
        assert!(json["trails"][0]["ended_at"].is_i64());
        assert!(json["frontier"][0]["score"].is_f64());
    }
}
