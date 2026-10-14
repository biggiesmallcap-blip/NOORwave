//! Steering a channel: More and Less like this (the panel's buttons, + and -
//! on the stage). A press works at three strengths, each gentler than the
//! one before:
//!
//! - Now. More lines up two of the artist's nearest neighbours and the
//!   artist itself right after the playing video (`more_picks`). Less drops
//!   the artist's upcoming videos; the client does that from its queue.
//! - This sitting. The session's refills double (More) or halve (Less) the
//!   artist and lean its nearest neighbours the same way, a little
//!   (`session_boosts`, folded into `session::boosts`).
//! - Afterwards. A remembered lean on the artist alone, a quarter either way
//!   at most, that halves every week and is gone after four. It only weighs
//!   On Air programming (channel pools, so tomorrow's schedule and refills),
//!   never the library, likes, the Archive, the Atlas or radio. One entry per
//!   artist, the latest press wins, and the Logbook lists every one with an
//!   undo (`forget`). Today's schedule is frozen and never moves.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::universe::{Cut, Universe, VideoRow};
use crate::services::video_discovery::graph::Graph;

pub const STEER_KEY: &str = "video_air.steer";
/// Entries kept; the oldest go first.
const MAX_ENTRIES: usize = 200;
const HALF_LIFE_DAYS: f64 = 7.0;
/// After this a lean is forgotten outright (strength under 1/16).
pub const FORGET_DAYS: f64 = 28.0;
/// The remembered lean at full strength: x1.25 for More, x0.75 for Less.
const LEAN: f64 = 0.25;
/// Nearest neighbours a press reaches.
pub const NEIGHBORS: usize = 4;
/// This sitting: the artist doubles or halves (`session::boosts`), its
/// nearest neighbours move this much.
pub const NEIGHBOR_MORE: f64 = 1.5;
pub const NEIGHBOR_LESS: f64 = 0.85;
/// Longest video More will line up (no long form).
const MAX_PICK_S: i64 = 20 * 60;
const DAY_S: f64 = 86_400.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Steer {
    pub artist: i64,
    #[serde(default)]
    pub name: Option<String>,
    pub more: bool,
    /// Unix seconds of the press.
    pub at: i64,
}

impl Steer {
    fn age_days(&self, now: i64) -> f64 {
        ((now - self.at).max(0) as f64) / DAY_S
    }

    /// 1 at the press, half a week later, 0 once forgotten.
    pub fn strength(&self, now: i64) -> f64 {
        let age = self.age_days(now);
        if age >= FORGET_DAYS {
            0.0
        } else {
            0.5f64.powf(age / HALF_LIFE_DAYS)
        }
    }

    /// The multiplier on this artist's weight in a channel pool.
    pub fn lean(&self, now: i64) -> f64 {
        let s = LEAN * self.strength(now);
        if self.more { 1.0 + s } else { 1.0 - s }
    }

    /// Unix seconds when it is forgotten.
    pub fn forgotten_at(&self) -> i64 {
        self.at + (FORGET_DAYS * DAY_S) as i64
    }
}

/// Every remembered press still in force, newest first.
pub fn load(conn: &Connection, now: i64) -> Result<Vec<Steer>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [STEER_KEY],
            |row| row.get(0),
        )
        .optional()?;
    let mut steers: Vec<Steer> = raw
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    steers.retain(|steer| steer.artist > 0 && steer.strength(now) > 0.0);
    // Stable: presses in the same second keep the order they were stored in
    // (newest first, see record).
    steers.sort_by_key(|steer| std::cmp::Reverse(steer.at));
    Ok(steers)
}

fn save(conn: &Connection, steers: &[Steer]) -> Result<()> {
    conn.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![STEER_KEY, serde_json::to_string(steers)?],
    )?;
    Ok(())
}

/// Remember a press: it replaces any earlier one for the artist, so More
/// then Less leaves Less, and pressing again only refreshes it.
pub fn record(
    conn: &Connection,
    artist: i64,
    name: Option<&str>,
    more: bool,
    now: i64,
) -> Result<()> {
    if artist <= 0 {
        return Ok(());
    }
    let mut steers = load(conn, now)?;
    steers.retain(|steer| steer.artist != artist);
    let name = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| name.chars().take(120).collect());
    steers.insert(
        0,
        Steer {
            artist,
            name,
            more,
            at: now,
        },
    );
    steers.truncate(MAX_ENTRIES);
    save(conn, &steers)
}

/// Undo a remembered press. False when there was none.
pub fn forget(conn: &Connection, artist: i64, now: i64) -> Result<bool> {
    let mut steers = load(conn, now)?;
    let before = steers.len();
    steers.retain(|steer| steer.artist != artist);
    if steers.len() == before {
        return Ok(false);
    }
    save(conn, &steers)?;
    Ok(true)
}

/// Artist -> remembered lean, for channel pools.
pub fn leans(steers: &[Steer], now: i64) -> HashMap<i64, f64> {
    steers
        .iter()
        .map(|steer| (steer.artist, steer.lean(now)))
        .filter(|(_, lean)| (*lean - 1.0).abs() > 1e-6)
        .collect()
}

fn artist_videos(universe: &Universe, artist: i64) -> &[VideoRow] {
    let start = universe.videos.partition_point(|v| v.artist < artist);
    let end = universe.videos.partition_point(|v| v.artist <= artist);
    &universe.videos[start..end]
}

fn pickable(video: &VideoRow) -> bool {
    video.cut != Cut::Longform && video.duration_s <= MAX_PICK_S
}

/// The artist's nearest neighbours on the graph, linked either way and
/// strongest edge first, that have something to air.
pub fn neighbors(graph: &Graph, universe: &Universe, artist: i64, limit: usize) -> Vec<i64> {
    graph
        .related(artist)
        .into_iter()
        .map(|(to, _)| to)
        .filter(|to| *to != artist && artist_videos(universe, *to).iter().any(pickable))
        .take(limit)
        .collect()
}

/// The artist's best video not yet queued: highest quality, a song not
/// already in the queue, official cuts first on a tie.
fn best_of(
    universe: &Universe,
    artist: i64,
    skip: &HashSet<i64>,
    songs: &HashSet<u64>,
) -> Option<i64> {
    artist_videos(universe, artist)
        .iter()
        .filter(|v| pickable(v) && !skip.contains(&v.id) && !songs.contains(&v.song))
        .max_by(|a, b| {
            a.quality
                .total_cmp(&b.quality)
                .then((a.cut == Cut::Official).cmp(&(b.cut == Cut::Official)))
                .then(b.id.cmp(&a.id))
        })
        .map(|v| v.id)
}

/// What More lines up right after the playing video: a neighbour, the
/// artist, another neighbour, so it never plays the same artist twice in a
/// row. Each with why it is there. Skips every queued video and song.
pub fn more_picks(
    universe: &Universe,
    graph: &Graph,
    artist: i64,
    name: &str,
    queued: &[i64],
) -> Vec<(i64, String)> {
    let mut skip: HashSet<i64> = queued.iter().copied().collect();
    let mut songs: HashSet<u64> = queued
        .iter()
        .filter_map(|id| universe.video(*id))
        .map(|video| video.song)
        .collect();
    let mut take = |who: i64, why: String, out: &mut Vec<(i64, String)>| {
        if let Some(id) = best_of(universe, who, &skip, &songs) {
            skip.insert(id);
            if let Some(video) = universe.video(id) {
                songs.insert(video.song);
            }
            out.push((id, why));
        }
    };
    let name = if name.trim().is_empty() {
        "this artist"
    } else {
        name.trim()
    };
    let near = neighbors(graph, universe, artist, NEIGHBORS);
    let mut out = Vec::new();
    let mut near_iter = near.iter();
    if let Some(first) = near_iter.next() {
        take(*first, format!("Next to {name}"), &mut out);
    }
    take(artist, format!("More {name}"), &mut out);
    for other in near_iter {
        if out.len() >= 3 {
            break;
        }
        let before = out.len();
        take(*other, format!("Next to {name}"), &mut out);
        if out.len() > before {
            break;
        }
    }
    out
}

/// This sitting's lean on the nearest neighbours of every More and Less
/// press (the pressed artists themselves are handled by the caller).
pub fn session_boosts(
    graph: &Graph,
    universe: &Universe,
    more: &[i64],
    less: &[i64],
) -> HashMap<i64, f64> {
    let pressed: HashSet<i64> = more.iter().chain(less).copied().collect();
    let mut out: HashMap<i64, f64> = HashMap::new();
    for (list, factor) in [(more, NEIGHBOR_MORE), (less, NEIGHBOR_LESS)] {
        for artist in list {
            for near in neighbors(graph, universe, *artist, NEIGHBORS) {
                if !pressed.contains(&near) {
                    *out.entry(near).or_insert(1.0) *= factor;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_air::test_support::*;

    const NOW: i64 = 1_800_000_000;

    fn universe(conn: &Connection) -> Universe {
        Universe::load(conn).unwrap()
    }

    fn seed(conn: &Connection) {
        // Artist 1 and three neighbours (2, 3, 4), each with a few songs.
        for artist in 1..=5 {
            for n in 0..3 {
                add_video(
                    conn,
                    artist * 100 + n,
                    artist,
                    "official",
                    &format!("song {artist}-{n}"),
                    240,
                );
            }
        }
        add_video(conn, 199, 1, "longform", "concert film", 3600);
    }

    fn graph() -> Graph {
        Graph::from_edges([(1, 2, 0.9), (1, 3, 0.7), (1, 4, 0.5), (1, 9, 0.95)])
    }

    #[test]
    fn a_remembered_press_fades_by_half_a_week_and_is_gone_after_four() {
        let steer = Steer {
            artist: 7,
            name: None,
            more: true,
            at: NOW,
        };
        assert!((steer.lean(NOW) - 1.25).abs() < 1e-9);
        assert!((steer.strength(NOW + 7 * 86_400) - 0.5).abs() < 1e-9);
        assert_eq!(steer.strength(NOW + 28 * 86_400), 0.0);
        let less = Steer {
            more: false,
            ..steer.clone()
        };
        assert!((less.lean(NOW) - 0.75).abs() < 1e-9);
        assert!(less.lean(NOW + 14 * 86_400) > 0.93);
    }

    #[test]
    fn the_latest_press_wins_and_undo_forgets_it() {
        let conn = conn();
        record(&conn, 7, Some("Seven"), true, NOW).unwrap();
        record(&conn, 8, None, false, NOW + 10).unwrap();
        record(&conn, 7, Some("Seven"), false, NOW + 20).unwrap();
        let steers = load(&conn, NOW + 30).unwrap();
        assert_eq!(
            steers
                .iter()
                .map(|s| (s.artist, s.more))
                .collect::<Vec<_>>(),
            vec![(7, false), (8, false)]
        );
        assert!(forget(&conn, 7, NOW + 30).unwrap());
        assert!(!forget(&conn, 7, NOW + 30).unwrap());
        assert_eq!(load(&conn, NOW + 30).unwrap().len(), 1);
        // Forgotten after four weeks without anyone pressing undo.
        assert!(load(&conn, NOW + 29 * 86_400).unwrap().is_empty());
    }

    #[test]
    fn a_broken_store_reads_as_nothing_remembered() {
        let conn = conn();
        conn.execute(
            "INSERT INTO server_config (key, value) VALUES (?1, 'nope')",
            [STEER_KEY],
        )
        .unwrap();
        assert!(load(&conn, NOW).unwrap().is_empty());
        record(&conn, 3, None, true, NOW).unwrap();
        assert_eq!(load(&conn, NOW).unwrap().len(), 1);
    }

    #[test]
    fn neighbours_need_something_to_air() {
        let conn = conn();
        seed(&conn);
        let u = universe(&conn);
        // 9 has the strongest edge but no videos.
        assert_eq!(neighbors(&graph(), &u, 1, 4), vec![2, 3, 4]);
    }

    #[test]
    fn an_artist_only_others_point_at_still_has_neighbours() {
        let conn = conn();
        seed(&conn);
        let u = universe(&conn);
        // 5 has no edges of its own; 2 and 4 list it.
        let g = Graph::from_edges([(2, 5, 0.4), (4, 5, 0.8), (5, 4, 0.3)]);
        assert_eq!(neighbors(&g, &u, 5, 4), vec![4, 2]);
    }

    #[test]
    fn more_lines_up_a_neighbour_the_artist_and_another_neighbour() {
        let conn = conn();
        seed(&conn);
        let u = universe(&conn);
        let picks = more_picks(&u, &graph(), 1, "One", &[100]);
        let artists: Vec<i64> = picks
            .iter()
            .map(|(id, _)| u.video(*id).unwrap().artist)
            .collect();
        assert_eq!(artists, vec![2, 1, 3]);
        assert_eq!(picks[0].1, "Next to One");
        assert_eq!(picks[1].1, "More One");
        // Never the queued video, its song or the long form.
        assert!(picks.iter().all(|(id, _)| *id != 100 && *id != 199));
    }

    #[test]
    fn neighbours_of_a_press_lean_a_little_this_sitting() {
        let conn = conn();
        seed(&conn);
        let u = universe(&conn);
        let leans = session_boosts(&graph(), &u, &[1], &[3]);
        assert_eq!(leans.get(&2), Some(&NEIGHBOR_MORE));
        assert_eq!(leans.get(&4), Some(&NEIGHBOR_MORE));
        // A pressed artist is left to its own press.
        assert_eq!(leans.get(&3), None);
        assert_eq!(leans.get(&1), None);
    }
}
