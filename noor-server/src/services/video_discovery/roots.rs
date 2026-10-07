//! Where relevance starts: artists you like, discoveries you enjoyed, and the
//! station currently on air.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::Connection;

pub const STATION_TTL: Duration = Duration::from_secs(2 * 3600);
const ENJOYED_WINDOW_DAYS: f64 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RootKind {
    Liked,
    Enjoyed,
    Station,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Root {
    pub artist_id: i64,
    pub weight: f64,
    pub kind: RootKind,
}

/// Liked-track artists, favorite-album artists and saved-video artists.
pub const LIKED_ARTISTS_SQL: &str = "
WITH liked_sources AS (
    SELECT a.tidal_id AS artist_id
      FROM artists a JOIN tracks t ON t.artist_id = a.id AND t.is_favorite = 1
    UNION ALL
    SELECT a.tidal_id
      FROM artists a JOIN albums al ON al.artist_id = a.id AND al.is_favorite = 1
    UNION ALL
    SELECT CAST(json_extract(s.item_json, '$.artist_id') AS INTEGER)
      FROM saved_videos s
     WHERE json_type(s.item_json, '$.artist_id') = 'integer'
)
SELECT DISTINCT artist_id FROM liked_sources WHERE artist_id > 0";

pub fn liked_roots(conn: &Connection) -> Result<Vec<Root>> {
    let mut stmt = conn.prepare(LIKED_ARTISTS_SQL)?;
    let ids = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids
        .into_iter()
        .map(|artist_id| Root {
            artist_id,
            weight: 1.0,
            kind: RootKind::Liked,
        })
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WatchRow {
    pub artist_id: i64,
    pub age_days: f64,
    pub watched_ms: i64,
    pub duration_ms: Option<i64>,
    pub completed: bool,
}

pub fn is_enjoyed(row: &WatchRow) -> bool {
    row.completed
        || row
            .duration_ms
            .is_some_and(|duration| duration > 0 && row.watched_ms as f64 >= 0.7 * duration as f64)
}

pub fn is_skip(row: &WatchRow) -> bool {
    row.watched_ms < 30_000
        && row.duration_ms.is_none_or(|duration| {
            duration <= 0 || (row.watched_ms as f64) < 0.25 * duration as f64
        })
}

/// 0.8 after one enjoyed watch, 1.0 after two, fading to 0 over 60 days. Skips
/// only count from the third one after the last enjoyed watch, 25% each, never
/// below half.
pub fn enjoyed_weight(enjoyed_count: i64, days_since_last: f64, skips_since: i64) -> f64 {
    if enjoyed_count <= 0 || days_since_last >= ENJOYED_WINDOW_DAYS {
        return 0.0;
    }
    let base = (0.6 + 0.2 * enjoyed_count as f64).min(1.0);
    let fade = 1.0 - days_since_last.max(0.0) / ENJOYED_WINDOW_DAYS;
    let skip_factor = if skips_since >= 3 {
        (1.0 - 0.25 * (skips_since - 2) as f64).max(0.5)
    } else {
        1.0
    };
    base * fade * skip_factor
}

/// Weights from oldest-first watch rows of non-liked artists.
pub fn enjoyed_from_rows(rows: &[WatchRow], liked: &HashSet<i64>) -> Vec<Root> {
    #[derive(Default)]
    struct Tally {
        enjoyed: i64,
        last_age: f64,
        skips: i64,
    }
    let mut tallies: HashMap<i64, Tally> = HashMap::new();
    for row in rows {
        if liked.contains(&row.artist_id) {
            continue;
        }
        let tally = tallies.entry(row.artist_id).or_default();
        if is_enjoyed(row) {
            tally.enjoyed += 1;
            tally.last_age = row.age_days;
            tally.skips = 0;
        } else if is_skip(row) && tally.enjoyed > 0 {
            tally.skips += 1;
        }
    }
    let mut roots: Vec<Root> = tallies
        .into_iter()
        .filter_map(|(artist_id, tally)| {
            let weight = enjoyed_weight(tally.enjoyed, tally.last_age, tally.skips);
            (weight > 0.0).then_some(Root {
                artist_id,
                weight,
                kind: RootKind::Enjoyed,
            })
        })
        .collect();
    roots.sort_by_key(|root| root.artist_id);
    roots
}

/// Liked-wall plays used to log the local `artists.id` as `artist_tidal_id`, so
/// a Bee Gees watch credited TIDAL artist 126 (Richie Havens) and seeded his
/// radio. Rewrites rows whose id names a local artist of the same name with a
/// different TIDAL id. Idempotent: a repaired row no longer matches.
pub fn repair_local_artist_ids(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE video_history
            SET artist_tidal_id = (SELECT a.tidal_id FROM artists a
                                    WHERE a.id = video_history.artist_tidal_id)
          WHERE EXISTS (SELECT 1 FROM artists a
                         WHERE a.id = video_history.artist_tidal_id
                           AND a.name = video_history.artist_name
                           AND a.tidal_id > 0
                           AND a.tidal_id <> video_history.artist_tidal_id)",
        [],
    )?)
}

pub fn enjoyed_roots(conn: &Connection, liked: &HashSet<i64>) -> Result<Vec<Root>> {
    let mut stmt = conn.prepare(
        "SELECT artist_tidal_id, julianday('now') - julianday(started_at),
                duration_watched_ms, video_duration_ms, completed
           FROM video_history
          WHERE artist_tidal_id > 0 AND duration_watched_ms IS NOT NULL
            AND started_at >= datetime('now', '-60 days')
          ORDER BY started_at, id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(WatchRow {
                artist_id: row.get(0)?,
                age_days: row.get(1)?,
                watched_ms: row.get(2)?,
                duration_ms: row.get(3)?,
                completed: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(enjoyed_from_rows(&rows, liked))
}

static STATIONS: Mutex<Vec<(i64, Instant)>> = Mutex::new(Vec::new());

/// A radio refill for this seed just happened; keep it a root for two hours.
pub fn touch_station(seed: i64) {
    touch_station_at(seed, Instant::now());
}

fn touch_station_at(seed: i64, now: Instant) {
    if seed <= 0 {
        return;
    }
    if let Ok(mut stations) = STATIONS.lock() {
        stations.retain(|(id, _)| *id != seed);
        stations.push((seed, now));
    }
}

pub fn station_roots() -> Vec<Root> {
    station_roots_at(Instant::now())
}

fn station_roots_at(now: Instant) -> Vec<Root> {
    let Ok(mut stations) = STATIONS.lock() else {
        return Vec::new();
    };
    stations.retain(|(_, touched)| now.saturating_duration_since(*touched) < STATION_TTL);
    stations
        .iter()
        .map(|(artist_id, _)| Root {
            artist_id: *artist_id,
            weight: 1.0,
            kind: RootKind::Station,
        })
        .collect()
}

pub fn station_active(seed: i64) -> bool {
    station_roots().iter().any(|root| root.artist_id == seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn
    }

    fn watch(artist_id: i64, age_days: f64, watched_ms: i64, completed: bool) -> WatchRow {
        WatchRow {
            artist_id,
            age_days,
            watched_ms,
            duration_ms: Some(200_000),
            completed,
        }
    }

    #[test]
    fn repair_rewrites_local_artist_ids_only() {
        let conn = conn();
        conn.execute_batch(
            "INSERT INTO artists (id, tidal_id, name) VALUES (126, 15096, 'Bee Gees'), (4149, 126, 'Richie Havens');
             INSERT INTO video_history (tidal_video_id, artist_tidal_id, artist_name) VALUES
                 (1, 126, 'Bee Gees'), (2, 126, 'Richie Havens'), (3, 15096, 'Bee Gees');",
        )
        .unwrap();
        assert_eq!(repair_local_artist_ids(&conn).unwrap(), 1);
        assert_eq!(repair_local_artist_ids(&conn).unwrap(), 0);
        let ids: Vec<i64> = conn
            .prepare("SELECT artist_tidal_id FROM video_history ORDER BY tidal_video_id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(ids, vec![15096, 126, 15096]);
    }

    #[test]
    fn liked_roots_cover_tracks_albums_and_saved_videos() {
        let conn = conn();
        conn.execute_batch(
            "INSERT INTO artists (id, tidal_id, name) VALUES (1, 101, 'Track'), (2, 102, 'Album'), (3, 103, 'Neither');
             INSERT INTO tracks (id, artist_id, title, is_favorite) VALUES (1, 1, 'Liked', 1), (2, 3, 'Not', 0);
             INSERT INTO albums (id, artist_id, title, is_favorite) VALUES (1, 2, 'Fav', 1);
             INSERT INTO saved_videos (tidal_video_id, item_json) VALUES (9, '{\"artist_id\":104}');",
        )
        .unwrap();
        let mut ids: Vec<i64> = liked_roots(&conn)
            .unwrap()
            .iter()
            .map(|r| r.artist_id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec![101, 102, 104]);
    }

    #[test]
    fn enjoyment_and_skips_follow_the_spec() {
        assert!((enjoyed_weight(1, 0.0, 0) - 0.8).abs() < 1e-9);
        assert!((enjoyed_weight(2, 0.0, 0) - 1.0).abs() < 1e-9);
        assert!((enjoyed_weight(1, 30.0, 0) - 0.4).abs() < 1e-9);
        assert_eq!(enjoyed_weight(1, 61.0, 0), 0.0);
        assert!(
            (enjoyed_weight(2, 0.0, 2) - 1.0).abs() < 1e-9,
            "two skips change nothing"
        );
        assert!((enjoyed_weight(2, 0.0, 3) - 0.75).abs() < 1e-9);
        assert!(
            (enjoyed_weight(2, 0.0, 9) - 0.5).abs() < 1e-9,
            "never below half"
        );
    }

    #[test]
    fn enjoyed_roots_skip_liked_artists_and_reset_skips_on_enjoyment() {
        let liked = HashSet::from([1]);
        let rows = [
            watch(1, 5.0, 200_000, true),
            watch(2, 10.0, 150_000, false),
            watch(2, 9.0, 5_000, false),
            watch(2, 8.0, 5_000, false),
            watch(2, 7.0, 5_000, false),
            watch(3, 6.0, 5_000, false),
        ];
        let roots = enjoyed_from_rows(&rows, &liked);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].artist_id, 2);
        let expected = 0.8 * (1.0 - 10.0 / 60.0) * 0.75;
        assert!((roots[0].weight - expected).abs() < 1e-9);
    }

    #[test]
    fn stations_expire_after_two_hours() {
        let start = Instant::now();
        touch_station_at(777_001, start);
        assert!(
            station_roots_at(start)
                .iter()
                .any(|r| r.artist_id == 777_001)
        );
        assert!(
            !station_roots_at(start + STATION_TTL)
                .iter()
                .any(|r| r.artist_id == 777_001)
        );
    }
}
