//! The census: every coverage number the design's "done when" checks, in
//! one read. Served at GET /api/videos/census.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use super::archive::{self, Lights};
use super::cuts::Cut;
use super::era::TRUSTED_CONF;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StageRun {
    pub stage: String,
    pub finished_at: Option<String>,
    pub rows: Option<i64>,
    pub error: Option<String>,
}

/// One scene's coverage for the Logbook's "seen N of M" bars, so it needs
/// no atlas payload.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SceneRow {
    pub id: i64,
    pub name: String,
    pub hue: f64,
    pub videos: i64,
    pub artists: i64,
    /// Distinct watched videos by the scene's artists.
    pub seen_videos: i64,
    /// Scene artists the listener watched or saved (light 3).
    pub lit_artists: i64,
    pub channel_no: Option<i64>,
}

/// `VideoCensus` on the wire.
#[derive(Debug, Clone, Serialize)]
pub struct Census {
    pub building: bool,
    pub built_at: Option<String>,
    pub videos: i64,
    pub hours: f64,
    pub programmable: i64,
    pub artists: i64,
    pub charted_artists: i64,
    pub uncharted_artists: i64,
    pub scenes: i64,
    pub channel_scenes: i64,
    pub songs: i64,
    pub multi_cut_songs: i64,
    pub concerts: i64,
    pub cuts: BTreeMap<&'static str, i64>,
    /// Share of charted artists with at least one genre.
    pub genre_coverage: f64,
    /// Share of songs whose year is trusted to the decade (era_conf >= 0.5):
    /// library and tag years, and cut years the artist's range backs. A bare
    /// upload year does not count, so Rewind unlocks on real evidence.
    pub era_coverage: f64,
    pub watched_videos: i64,
    pub lit_artists: i64,
    pub stages: Vec<StageRun>,
    /// Per-scene coverage, biggest scene first; empty before scenes exist.
    pub scene_rows: Vec<SceneRow>,
}

/// The catalog-wide sums: every video's runtime, every song's cuts and
/// years. They are most of the census's cost (about 170 ms of aggregates
/// over the real catalog) and change only when a build stage runs, so they
/// are kept against the Archive's build signature.
#[derive(Debug, Clone)]
struct Sums {
    seconds: i64,
    programmable: i64,
    artists: i64,
    songs: i64,
    multi_cut: i64,
    trusted: i64,
    cuts: BTreeMap<&'static str, i64>,
}

static SUMS: LazyLock<Mutex<Option<(String, Sums)>>> = LazyLock::new(|| Mutex::new(None));

fn sums(conn: &Connection) -> Result<Sums> {
    // Tests open many databases in one process; they always count afresh.
    if cfg!(test) {
        return count_sums(conn);
    }
    let key = format!(
        "{}|{}",
        conn.path().unwrap_or(""),
        archive::signature(conn)?
    );
    if let Some(hit) = SUMS.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .filter(|(at, _)| *at == key)
            .map(|(_, sums)| sums.clone())
    }) {
        return Ok(hit);
    }
    let fresh = count_sums(conn)?;
    if let Ok(mut guard) = SUMS.lock() {
        *guard = Some((key, fresh.clone()));
    }
    Ok(fresh)
}

fn count_sums(conn: &Connection) -> Result<Sums> {
    let (seconds, programmable, artists): (i64, i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(duration_s), 0), COALESCE(SUM(programmable), 0),
                COUNT(DISTINCT artist_tidal_id)
           FROM video_facets",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let (songs, multi_cut, trusted): (i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(cut_count >= 2), 0),
                COALESCE(SUM(song_year IS NOT NULL AND era_conf >= ?1), 0)
           FROM video_songs",
        [TRUSTED_CONF],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let mut cuts: BTreeMap<&'static str, i64> =
        Cut::ALL.iter().map(|cut| (cut.as_str(), 0)).collect();
    let mut stmt = conn.prepare("SELECT cut, COUNT(*) FROM video_facets GROUP BY cut")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        if let Some(cut) = Cut::parse(&row.get::<_, String>(0)?) {
            cuts.insert(cut.as_str(), row.get(1)?);
        }
    }
    Ok(Sums {
        seconds,
        programmable,
        artists,
        songs,
        multi_cut,
        trusted,
        cuts,
    })
}

/// The last finished build's timestamp, if any.
pub fn built_at(conn: &Connection) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT finished_at FROM video_intel_runs
              WHERE stage = 'build' AND finished_at IS NOT NULL
              ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

fn ratio(part: i64, whole: i64) -> f64 {
    if whole <= 0 {
        0.0
    } else {
        ((part as f64 / whole as f64) * 10_000.0).round() / 10_000.0
    }
}

pub fn read(conn: &Connection, running: bool) -> Result<Census> {
    let one = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [], |row| row.get(0))?) };
    let built_at = built_at(conn)?;
    let videos = one("SELECT COUNT(*) FROM video_catalog")?;
    let Sums {
        seconds,
        programmable,
        artists,
        songs,
        multi_cut,
        trusted,
        cuts,
    } = sums(conn)?;
    let (charted, genred): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(genres_json <> '[]'), 0) FROM video_artist_facets
          WHERE charted = 1 AND (videos > 0 OR playable > 0)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let (scenes, channel_scenes): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COUNT(channel_no) FROM video_scenes",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let stages: Vec<StageRun> = {
        let mut stmt = conn.prepare(
            "SELECT stage, finished_at, rows, error FROM video_intel_runs
              WHERE id IN (SELECT MAX(id) FROM video_intel_runs GROUP BY stage)
              ORDER BY id",
        )?;
        stmt.query_map([], |row| {
            Ok(StageRun {
                stage: row.get(0)?,
                finished_at: row.get(1)?,
                rows: row.get(2)?,
                error: row.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?
    };
    let lights = Lights::load(conn)?;
    let scene_rows = scene_rows(conn, &lights)?;
    Ok(Census {
        building: running || built_at.is_none(),
        built_at,
        videos,
        hours: (seconds as f64 / 36.0).round() / 100.0,
        programmable,
        artists,
        charted_artists: charted,
        uncharted_artists: (artists - charted).max(0),
        scenes,
        channel_scenes,
        songs,
        multi_cut_songs: multi_cut,
        concerts: one("SELECT COUNT(*) FROM video_concerts")?,
        cuts,
        genre_coverage: ratio(genred, charted),
        era_coverage: ratio(trusted, songs),
        watched_videos: one("SELECT COUNT(DISTINCT tidal_video_id) FROM video_history")?,
        lit_artists: lights.lit.len() as i64,
        stages,
        scene_rows,
    })
}

/// Distinct watched videos per scene, by the scene of each video's artist.
pub fn seen_per_scene(conn: &Connection) -> Result<HashMap<i64, i64>> {
    let mut seen: HashMap<i64, i64> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT a.scene_id, COUNT(DISTINCT h.tidal_video_id)
           FROM video_history h
           LEFT JOIN video_facets f ON f.tidal_video_id = h.tidal_video_id
           JOIN video_artist_facets a
             ON a.artist_tidal_id = COALESCE(f.artist_tidal_id, h.artist_tidal_id)
          WHERE a.scene_id IS NOT NULL
          GROUP BY a.scene_id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        seen.insert(row.get(0)?, row.get(1)?);
    }
    Ok(seen)
}

/// Every scene with what the listener has seen and lit in it.
pub fn scene_rows(conn: &Connection, lights: &Lights) -> Result<Vec<SceneRow>> {
    let seen = seen_per_scene(conn)?;
    let mut lit: HashMap<i64, i64> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT artist_tidal_id, scene_id FROM video_artist_facets
              WHERE scene_id IS NOT NULL",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            if lights.lit.contains(&row.get::<_, i64>(0)?) {
                *lit.entry(row.get(1)?).or_default() += 1;
            }
        }
    }
    let mut stmt = conn.prepare(
        "SELECT scene_id, name, hue, video_count, artist_count, channel_no
           FROM video_scenes ORDER BY video_count DESC, scene_id",
    )?;
    let rows = stmt.query_map([], |row| {
        let id: i64 = row.get(0)?;
        Ok(SceneRow {
            id,
            name: row.get(1)?,
            hue: row.get(2)?,
            videos: row.get(3)?,
            artists: row.get(4)?,
            seen_videos: seen.get(&id).copied().unwrap_or(0),
            lit_artists: lit.get(&id).copied().unwrap_or(0),
            channel_no: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_intel::songs::tests::{add, conn};

    #[test]
    fn an_unbuilt_catalog_reports_building_and_zeros() {
        let conn = conn();
        add(&conn, 1, 7, "Song", 200, None, None);
        let census = read(&conn, false).unwrap();
        assert!(census.building);
        assert_eq!(census.videos, 1);
        assert_eq!(census.songs, 0);
        assert_eq!(census.cuts.len(), 11);
        assert_eq!(census.era_coverage, 0.0);
        assert!(census.scene_rows.is_empty());
    }

    #[test]
    fn a_built_catalog_counts_everything() {
        let conn = conn();
        add(&conn, 1, 7, "Song", 200, Some(3), Some(2020));
        add(&conn, 2, 7, "Song (Live)", 200, None, Some(2021));
        add(&conn, 3, 8, "Teaser (Official Trailer)", 100, None, None);
        conn.execute(
            "INSERT INTO video_history (tidal_video_id, artist_tidal_id) VALUES (1, 7)",
            [],
        )
        .unwrap();
        crate::services::video_intel::build::build_all(&conn).unwrap();
        let census = read(&conn, false).unwrap();
        assert!(!census.building);
        assert!(census.built_at.is_some());
        assert_eq!(
            (census.videos, census.programmable, census.artists),
            (3, 2, 2)
        );
        assert_eq!((census.songs, census.multi_cut_songs), (2, 1));
        assert_eq!(census.cuts["official"], 1);
        assert_eq!(census.cuts["live"], 1);
        assert_eq!(census.cuts["trailer"], 1);
        assert_eq!(census.watched_videos, 1);
        assert_eq!(census.lit_artists, 1);
        assert!((census.hours - 0.14).abs() < 1e-9);
        assert!(
            census
                .stages
                .iter()
                .any(|s| s.stage == "cuts" && s.rows == Some(3))
        );
        assert!(read(&conn, true).unwrap().building);
    }

    #[test]
    fn scene_rows_count_what_was_seen_and_lit() {
        let conn = conn();
        add(&conn, 1, 7, "One", 200, None, None);
        add(&conn, 2, 7, "Two", 200, None, None);
        add(&conn, 3, 8, "Three", 200, None, None);
        conn.execute_batch(
            "INSERT INTO video_scenes (scene_id, name, hue, video_count, artist_count, channel_no)
                 VALUES (1, 'Seven and Eight', 120, 3, 2, 10), (2, 'Empty', 40, 0, 0, NULL);
             INSERT INTO video_artist_facets (artist_tidal_id, scene_id) VALUES (7, 1), (8, 1);
             INSERT INTO video_history (tidal_video_id, artist_tidal_id)
                 VALUES (1, 7), (1, 7), (2, NULL);",
        )
        .unwrap();
        // Facets give video 2 its artist; the scene rows are set by hand, so
        // the scene stage does not run here.
        crate::services::video_intel::cuts::build(&conn).unwrap();
        let rows = read(&conn, false).unwrap().scene_rows;
        assert_eq!(
            rows,
            vec![
                SceneRow {
                    id: 1,
                    name: "Seven and Eight".into(),
                    hue: 120.0,
                    videos: 3,
                    artists: 2,
                    seen_videos: 2,
                    lit_artists: 1,
                    channel_no: Some(10),
                },
                SceneRow {
                    id: 2,
                    name: "Empty".into(),
                    hue: 40.0,
                    videos: 0,
                    artists: 0,
                    seen_videos: 0,
                    lit_artists: 0,
                    channel_no: None,
                },
            ]
        );
    }
}
