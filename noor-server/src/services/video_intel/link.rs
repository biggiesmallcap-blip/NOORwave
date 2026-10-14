//! Final link pass: copy scene ids onto songs, artist playable counts
//! and hours, channel numbers for scenes that newly qualify.
//!
//! Channel numbers are stable forever: a scene gets the next free number the
//! first time it qualifies (30 programmable hours and 40 playable artists)
//! and keeps it across rebuilds (the scene stage carries `channel_no` over by
//! stable scene id). A high-water mark in server_config makes sure a number
//! is never handed out twice, even after its scene dissolves.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

/// Scene channels start here; 1-8 are the fixed channels.
pub const FIRST_SCENE_CHANNEL: i64 = 10;
pub const QUALIFY_HOURS: f64 = 30.0;
pub const QUALIFY_ARTISTS: i64 = 40;
const HIGH_WATER_KEY: &str = "video_intel.channel_high_water";

/// A scene's size as channel qualification sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneSize {
    pub scene_id: i64,
    pub channel_no: Option<i64>,
    pub hours: f64,
    pub artists: i64,
}

/// New channel numbers for scenes that qualify and have none yet, biggest
/// first, never reusing a number at or below the high-water mark. Pure.
pub fn assign_channels(scenes: &[SceneSize], high_water: i64) -> Vec<(i64, i64)> {
    let mut next = scenes
        .iter()
        .filter_map(|s| s.channel_no)
        .max()
        .unwrap_or(0)
        .max(high_water)
        .max(FIRST_SCENE_CHANNEL - 1)
        + 1;
    let mut fresh: Vec<&SceneSize> = scenes
        .iter()
        .filter(|s| {
            s.channel_no.is_none() && s.hours >= QUALIFY_HOURS && s.artists >= QUALIFY_ARTISTS
        })
        .collect();
    fresh.sort_by(|a, b| {
        b.hours
            .total_cmp(&a.hours)
            .then(a.scene_id.cmp(&b.scene_id))
    });
    fresh
        .into_iter()
        .map(|scene| {
            let number = next;
            next += 1;
            (scene.scene_id, number)
        })
        .collect()
}

/// Run the link pass. Returns the number of artist rows written.
pub fn build(conn: &Connection) -> Result<usize> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    // Artist playable counts and programmable hours. Rows the scene stage
    // has not written yet are created so the counts exist without scenes.
    tx.execute(
        "UPDATE video_artist_facets SET playable = 0, hours = 0
          WHERE playable <> 0 OR hours <> 0",
        [],
    )?;
    let artists = tx.execute(
        "INSERT INTO video_artist_facets (artist_tidal_id, name, videos, playable, hours)
         SELECT f.artist_tidal_id,
                COALESCE(MAX(c.artist_name), ''),
                COUNT(*),
                SUM(f.programmable),
                ROUND(SUM(CASE WHEN f.programmable = 1 THEN COALESCE(f.duration_s, 0) ELSE 0 END)
                      / 3600.0, 3)
           FROM video_facets f
           JOIN video_catalog c ON c.tidal_video_id = f.tidal_video_id
          WHERE f.artist_tidal_id > 0
          GROUP BY f.artist_tidal_id
         ON CONFLICT(artist_tidal_id) DO UPDATE SET
                playable = excluded.playable,
                hours = excluded.hours",
        [],
    )?;
    // Scene ids onto songs (NULL when the artist has no scene yet).
    tx.execute(
        "UPDATE video_songs SET scene_id = (
             SELECT a.scene_id FROM video_artist_facets a
              WHERE a.artist_tidal_id = video_songs.artist_tidal_id)",
        [],
    )?;
    // Channel numbers for scenes that newly qualify.
    let scenes: Vec<SceneSize> = {
        let mut stmt = tx.prepare(
            "SELECT s.scene_id, s.channel_no,
                    COALESCE((SELECT SUM(a.hours) FROM video_artist_facets a
                               WHERE a.scene_id = s.scene_id), 0),
                    (SELECT COUNT(*) FROM video_artist_facets a
                      WHERE a.scene_id = s.scene_id AND a.playable > 0)
               FROM video_scenes s",
        )?;
        stmt.query_map([], |row| {
            Ok(SceneSize {
                scene_id: row.get(0)?,
                channel_no: row.get(1)?,
                hours: row.get(2)?,
                artists: row.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?
    };
    let high_water: i64 = tx
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [HIGH_WATER_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let assigned = assign_channels(&scenes, high_water);
    for (scene_id, number) in &assigned {
        tx.execute(
            "UPDATE video_scenes SET channel_no = ?2 WHERE scene_id = ?1",
            params![scene_id, number],
        )?;
    }
    let top = scenes
        .iter()
        .filter_map(|s| s.channel_no)
        .chain(assigned.iter().map(|(_, n)| *n))
        .max()
        .unwrap_or(0)
        .max(high_water);
    if top > high_water {
        tx.execute(
            "INSERT OR REPLACE INTO server_config (key, value) VALUES (?1, ?2)",
            params![HIGH_WATER_KEY, top.to_string()],
        )?;
    }
    tx.commit()?;
    Ok(artists)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_intel::songs::tests::{add, conn};
    use crate::services::video_intel::{cuts, songs};

    fn size(scene_id: i64, channel_no: Option<i64>, hours: f64, artists: i64) -> SceneSize {
        SceneSize {
            scene_id,
            channel_no,
            hours,
            artists,
        }
    }

    #[test]
    fn channels_go_to_qualifying_scenes_biggest_first_and_never_repeat() {
        let scenes = vec![
            size(1, Some(10), 100.0, 90),
            size(2, None, 50.0, 60),
            size(3, None, 80.0, 45),
            size(4, None, 29.0, 100),
            size(5, None, 300.0, 39),
        ];
        assert_eq!(assign_channels(&scenes, 0), vec![(3, 11), (2, 12)]);
        // Number 14 was handed out before and its scene is gone: skip it.
        assert_eq!(assign_channels(&scenes, 14), vec![(3, 15), (2, 16)]);
        assert_eq!(
            assign_channels(&[size(9, None, 30.0, 40)], 0),
            vec![(9, FIRST_SCENE_CHANNEL)]
        );
    }

    #[test]
    fn link_counts_playable_videos_and_copies_scenes() {
        let conn = conn();
        add(&conn, 10, 7, "One", 3600, None, None);
        add(&conn, 11, 7, "Two (Official Trailer)", 120, None, None);
        add(&conn, 12, 8, "Three", 1800, None, None);
        cuts::build(&conn).unwrap();
        songs::build(&conn).unwrap();
        conn.execute(
            "INSERT INTO video_artist_facets (artist_tidal_id, name, scene_id, charted)
             VALUES (7, 'Artist 7', 3, 1)",
            [],
        )
        .unwrap();
        assert_eq!(build(&conn).unwrap(), 2);
        let (videos, playable, hours, scene): (i64, i64, f64, i64) = conn
            .query_row(
                "SELECT videos, playable, hours, scene_id FROM video_artist_facets
                  WHERE artist_tidal_id = 7",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!((playable, scene), (1, 3));
        assert_eq!(videos, 0, "the scene stage owns videos for its rows");
        assert!((hours - 1.0).abs() < 1e-9);
        let created: (i64, String) = conn
            .query_row(
                "SELECT playable, name FROM video_artist_facets WHERE artist_tidal_id = 8",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(created, (1, "Artist 8".to_string()));
        let scene_of = |key: &str| -> Option<i64> {
            conn.query_row(
                "SELECT scene_id FROM video_songs WHERE song_key = ?1",
                [key],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(scene_of("id:7:one"), Some(3));
        assert_eq!(scene_of("id:8:three"), None);
    }

    #[test]
    fn link_numbers_scenes_once() {
        let conn = conn();
        conn.execute_batch(
            "INSERT INTO video_scenes (scene_id, name) VALUES (1, 'Big'), (2, 'Small');",
        )
        .unwrap();
        for artist in 0..40 {
            conn.execute(
                "INSERT INTO video_artist_facets (artist_tidal_id, scene_id) VALUES (?1, 1)",
                [1000 + artist],
            )
            .unwrap();
            add(
                &conn,
                5000 + artist,
                1000 + artist,
                "Long",
                3000,
                None,
                None,
            );
        }
        cuts::build(&conn).unwrap();
        songs::build(&conn).unwrap();
        build(&conn).unwrap();
        build(&conn).unwrap();
        let channels: Vec<(i64, Option<i64>)> = conn
            .prepare("SELECT scene_id, channel_no FROM video_scenes ORDER BY scene_id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(channels, vec![(1, Some(10)), (2, None)]);
        let high: String = conn
            .query_row(
                "SELECT value FROM server_config WHERE key = ?1",
                [HIGH_WATER_KEY],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(high, "10");
    }
}
