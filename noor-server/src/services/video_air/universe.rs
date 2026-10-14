//! The catalog as On Air sees it: every programmable video with its facets,
//! plus songs, concerts, scenes and each artist's scene. Loaded once per
//! build from the derived tables; before the first intelligence build it
//! falls back to `video_catalog` with the station exclusion rule and knows no
//! cuts, concerts, eras or scenes.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use chrono::Datelike;
use rusqlite::Connection;

use crate::services::video_radio::video_song_key;
use crate::services::video_sets::{VideoCandidate, build_seed};
use crate::services::video_stations::pool::is_excluded;

/// Nothing over 20 minutes airs outside Concert Hall: a concert film would
/// eat a third of a Wild Card hour.
pub const MAX_PROGRAM_S: i64 = 20 * 60;
const DEFAULT_DURATION_S: i64 = 240;
const MIN_DURATION_S: i64 = 30;
/// Premieres: catalog rows fetched within this many days...
pub const PREMIERE_DAYS: i64 = 14;
/// ...and released this year or last (re-harvests refresh the fetch time,
/// so the release year keeps the channel about new videos).
pub const PREMIERE_YEARS: i32 = 1;
const NEUTRAL_QUALITY: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cut {
    Official,
    Live,
    Session,
    Lyric,
    Visualizer,
    Remix,
    Dance,
    Interview,
    Trailer,
    Clip,
    Longform,
    /// Fallback rows and anything the classifier left unnamed.
    Other,
}

impl Cut {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "official" => Self::Official,
            "live" => Self::Live,
            "session" => Self::Session,
            "lyric" => Self::Lyric,
            "visualizer" => Self::Visualizer,
            "remix" => Self::Remix,
            "dance" => Self::Dance,
            "interview" => Self::Interview,
            "trailer" => Self::Trailer,
            "clip" => Self::Clip,
            "longform" => Self::Longform,
            _ => Self::Other,
        }
    }

    /// The why chip a cut earns when the channel is not about that cut.
    pub fn chip(self) -> Option<&'static str> {
        match self {
            Self::Live => Some("Live"),
            Self::Session => Some("Session"),
            Self::Lyric => Some("Lyric video"),
            Self::Visualizer => Some("Visualizer"),
            Self::Remix => Some("Remix"),
            Self::Dance => Some("Dance"),
            Self::Longform => Some("Long form"),
            _ => None,
        }
    }

    /// How much a general channel wants this cut to stand for its song.
    pub fn preference(self) -> f64 {
        match self {
            Self::Official | Self::Other => 1.0,
            Self::Session => 0.7,
            Self::Live => 0.6,
            Self::Remix => 0.5,
            Self::Dance => 0.4,
            Self::Lyric | Self::Visualizer => 0.3,
            Self::Longform => 0.2,
            Self::Interview | Self::Trailer | Self::Clip => 0.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VideoRow {
    pub id: i64,
    /// TIDAL artist id; `-id` when the video has none, so it is its own artist.
    pub artist: i64,
    pub scene: Option<i64>,
    pub scene2: Option<i64>,
    pub cut: Cut,
    pub song: u64,
    pub concert: Option<i64>,
    pub cut_year: Option<i32>,
    pub duration_s: i64,
    pub quality: f64,
    /// Fetched into the catalog within `PREMIERE_DAYS`.
    pub fresh: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SongRow {
    pub best_video: Option<i64>,
    pub year: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct SceneRow {
    pub id: i64,
    pub name: String,
    pub hue: f64,
    pub channel_no: Option<i64>,
    pub exemplars: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ConcertRow {
    pub id: i64,
    pub artist: i64,
    pub label: String,
    /// Setlist order.
    pub video_ids: Vec<i64>,
}

#[derive(Debug, Default)]
pub struct Universe {
    /// False until the intelligence build has written video_facets; the
    /// catalog fallback knows no cuts, concerts, eras or scenes.
    pub facets: bool,
    /// Programmable videos sorted by (artist, id).
    pub videos: Vec<VideoRow>,
    pub by_id: HashMap<i64, usize>,
    pub songs: HashMap<u64, SongRow>,
    /// Sorted by scene id.
    pub scenes: Vec<SceneRow>,
    /// Sorted by concert id.
    pub concerts: Vec<ConcertRow>,
    /// Video indices per primary or secondary scene, ascending.
    pub scene_members: HashMap<i64, Vec<usize>>,
    /// Artists per primary scene with at least one programmable video.
    pub scene_artists: HashMap<i64, usize>,
    /// Each scene's family on the dial (families::family_of its tags).
    pub families: HashMap<i64, &'static str>,
    pub this_year: i32,
}

/// Stable 64-bit key for a song key string.
pub fn song_hash(key: &str) -> u64 {
    build_seed(key, "song")
}

fn duration(raw: Option<i64>) -> i64 {
    raw.filter(|d| *d > 0)
        .unwrap_or(DEFAULT_DURATION_S)
        .max(MIN_DURATION_S)
}

/// Quality for fallback rows: TIDAL popularity, square-rooted like the
/// facets builder, neutral when unknown.
fn popularity_quality(popularity: Option<i32>) -> f64 {
    popularity.map_or(NEUTRAL_QUALITY, |p| {
        (f64::from(p.clamp(0, 100)) / 100.0).sqrt()
    })
}

pub fn facets_ready(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM video_facets WHERE programmable = 1)",
        [],
        |row| row.get(0),
    )?)
}

impl Universe {
    pub fn load(conn: &Connection) -> Result<Self> {
        let this_year = chrono::Local::now().year();
        let mut universe = if facets_ready(conn)? {
            load_facets(conn, this_year)?
        } else {
            load_fallback(conn)?
        };
        universe.this_year = this_year;
        universe.index();
        Ok(universe)
    }

    /// Sort, index by id, and group by scene. Called after any load.
    pub fn index(&mut self) {
        self.videos
            .sort_by(|a, b| a.artist.cmp(&b.artist).then(a.id.cmp(&b.id)));
        self.by_id = self
            .videos
            .iter()
            .enumerate()
            .map(|(index, video)| (video.id, index))
            .collect();
        self.scene_members.clear();
        let mut artists: HashMap<i64, HashSet<i64>> = HashMap::new();
        for (index, video) in self.videos.iter().enumerate() {
            if let Some(scene) = video.scene {
                self.scene_members.entry(scene).or_default().push(index);
                artists.entry(scene).or_default().insert(video.artist);
            }
            if let Some(scene2) = video.scene2.filter(|s| Some(*s) != video.scene) {
                self.scene_members.entry(scene2).or_default().push(index);
            }
        }
        for members in self.scene_members.values_mut() {
            members.sort_unstable();
        }
        self.scene_artists = artists
            .into_iter()
            .map(|(scene, set)| (scene, set.len()))
            .collect();
        self.scenes.sort_by_key(|scene| scene.id);
        self.concerts.sort_by_key(|concert| concert.id);
    }

    pub fn video(&self, id: i64) -> Option<&VideoRow> {
        self.by_id.get(&id).map(|index| &self.videos[*index])
    }

    pub fn scene(&self, id: i64) -> Option<&SceneRow> {
        self.scenes
            .binary_search_by_key(&id, |scene| scene.id)
            .ok()
            .map(|index| &self.scenes[index])
    }

    pub fn song_year(&self, video: &VideoRow) -> Option<i32> {
        self.songs.get(&video.song).and_then(|song| song.year)
    }
}

fn load_facets(conn: &Connection, this_year: i32) -> Result<Universe> {
    let mut scenes_of: HashMap<i64, (Option<i64>, Option<i64>)> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT artist_tidal_id, scene_id, scene2_id FROM video_artist_facets
              WHERE scene_id IS NOT NULL OR scene2_id IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?;
        for row in rows {
            let (artist, scene, scene2) = row?;
            scenes_of.insert(artist, (scene, scene2));
        }
    }
    let mut videos = Vec::new();
    {
        // No join with video_catalog here: its rows are wide, and touching all
        // of them triples the load. Freshness is looked up below for the few
        // rows Premieres can use.
        let mut stmt = conn.prepare(
            "SELECT tidal_video_id, artist_tidal_id, cut, song_key, concert_id,
                    cut_year, duration_s, quality
               FROM video_facets WHERE programmable = 1",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<f64>>(7)?,
            ))
        })?;
        for row in rows {
            let (id, artist, cut, song_key, concert, cut_year, duration_s, quality) = row?;
            let artist = artist.filter(|a| *a > 0).unwrap_or(-id);
            let (scene, scene2) = scenes_of.get(&artist).copied().unwrap_or((None, None));
            videos.push(VideoRow {
                id,
                artist,
                scene,
                scene2,
                cut: Cut::parse(&cut),
                song: song_hash(&song_key),
                concert,
                cut_year: cut_year.and_then(|y| i32::try_from(y).ok()),
                duration_s: duration(duration_s),
                quality: quality
                    .filter(|q| q.is_finite())
                    .unwrap_or(NEUTRAL_QUALITY)
                    .clamp(0.0, 1.0),
                fresh: false,
            });
        }
    }
    mark_fresh(conn, &mut videos, this_year)?;
    let mut songs = HashMap::new();
    {
        let mut stmt =
            conn.prepare("SELECT song_key, best_video_id, song_year FROM video_songs")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?;
        for row in rows {
            let (key, best_video, year) = row?;
            songs.insert(
                song_hash(&key),
                SongRow {
                    best_video,
                    year: year.and_then(|y| i32::try_from(y).ok()),
                },
            );
        }
    }
    let mut scenes = Vec::new();
    let mut families = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT scene_id, name, hue, channel_no, exemplars_json, tags_json FROM video_scenes",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        for row in rows {
            let (id, name, hue, channel_no, exemplars, tags) = row?;
            let tags: Vec<String> = serde_json::from_str(&tags).unwrap_or_default();
            families.insert(id, super::families::family_of(&tags));
            scenes.push(SceneRow {
                id,
                name,
                hue,
                channel_no,
                exemplars: parse_exemplars(&exemplars),
            });
        }
    }
    let mut concerts = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT concert_id, artist_tidal_id, label, video_ids_json FROM video_concerts",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (id, artist, label, ids) = row?;
            let video_ids: Vec<i64> = serde_json::from_str(&ids).unwrap_or_default();
            if !video_ids.is_empty() {
                concerts.push(ConcertRow {
                    id,
                    artist,
                    label,
                    video_ids,
                });
            }
        }
    }
    Ok(Universe {
        facets: true,
        videos,
        songs,
        scenes,
        concerts,
        families,
        ..Default::default()
    })
}

/// Premieres only air videos released this year or last, so only those rows
/// need their catalog fetch time.
fn mark_fresh(conn: &Connection, videos: &mut [VideoRow], this_year: i32) -> Result<()> {
    let recent: Vec<i64> = videos
        .iter()
        .filter(|v| v.cut_year.is_some_and(|y| y >= this_year - PREMIERE_YEARS))
        .map(|v| v.id)
        .collect();
    if recent.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(&format!(
        "SELECT tidal_video_id FROM video_catalog
          WHERE tidal_video_id IN (SELECT value FROM json_each(?1))
            AND fetched_at >= datetime('now', '-{PREMIERE_DAYS} days')"
    ))?;
    let fresh: HashSet<i64> = stmt
        .query_map([serde_json::to_string(&recent)?], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<_, _>>()?;
    for video in videos.iter_mut() {
        video.fresh = fresh.contains(&video.id);
    }
    Ok(())
}

/// Exemplars are written as names; tolerate objects with a `name` field.
fn parse_exemplars(raw: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .or_else(|| item.get("name").and_then(|name| name.as_str()))
                        .map(str::to_string)
                })
                .filter(|name| !name.trim().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Before the first intelligence build: the catalog minus the station
/// exclusions. Every row is its own cut-less song; TIDAL's Live type is the
/// only cut it can tell.
fn load_fallback(conn: &Connection) -> Result<Universe> {
    let mut stmt = conn.prepare(&format!(
        "SELECT item_json, fetched_at >= datetime('now', '-{PREMIERE_DAYS} days')
           FROM video_catalog"
    ))?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<bool>>(1)?))
    })?;
    let mut videos = Vec::new();
    for row in rows {
        let (json, fresh) = row?;
        let Ok(video) = serde_json::from_str::<VideoCandidate>(&json) else {
            continue;
        };
        if is_excluded(&video) {
            continue;
        }
        let song = song_hash(&video_song_key(
            video.artist_id,
            video.artist_name.as_deref(),
            &video.title,
        ));
        videos.push(VideoRow {
            id: video.tidal_id,
            artist: video
                .artist_id
                .filter(|a| *a > 0)
                .unwrap_or(-video.tidal_id),
            scene: None,
            scene2: None,
            cut: if video.video_type.as_deref() == Some("Live") {
                Cut::Live
            } else {
                Cut::Other
            },
            song,
            concert: None,
            cut_year: video.release_year,
            duration_s: duration(video.duration_s),
            quality: popularity_quality(video.popularity),
            fresh: fresh.unwrap_or(false),
        });
    }
    Ok(Universe {
        facets: false,
        videos,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_air::test_support::*;

    #[test]
    fn fallback_reads_the_catalog_minus_exclusions() {
        let conn = conn();
        add_catalog(&conn, 1, 10, "Song One", 200);
        add_catalog(&conn, 2, 10, "Song One (Live)", 210);
        add_catalog(&conn, 3, 20, "Official Trailer", 200);
        add_catalog(&conn, 4, 30, "Short", 40);
        let universe = Universe::load(&conn).unwrap();
        assert!(!universe.facets);
        let ids: Vec<i64> = universe.videos.iter().map(|v| v.id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(
            universe.videos[0].song, universe.videos[1].song,
            "one song key across cuts"
        );
        assert!(universe.scenes.is_empty());
    }

    #[test]
    fn facets_win_once_built_and_carry_scenes_songs_and_concerts() {
        let conn = conn();
        add_catalog(&conn, 1, 10, "A", 200);
        add_catalog(&conn, 2, 10, "B (Live)", 300);
        add_catalog(&conn, 3, 20, "C", 100);
        add_facet(&conn, 1, 10, "official", "id:10:a", 0.9);
        add_facet(&conn, 2, 10, "live", "id:10:b", 0.4);
        conn.execute(
            "UPDATE video_facets SET concert_id = 7, cut_year = 2015 WHERE tidal_video_id = 2",
            [],
        )
        .unwrap();
        add_facet(&conn, 3, 20, "clip", "id:20:c", 0.1);
        conn.execute(
            "UPDATE video_facets SET programmable = 0 WHERE tidal_video_id = 3",
            [],
        )
        .unwrap();
        add_song(&conn, "id:10:a", 10, 1, Some(1999));
        add_scene(&conn, 5, "Scene Five", Some(10));
        add_artist_scene(&conn, 10, 5);
        add_concert(&conn, 7, 10, "Live at Home 2015", &[2]);
        let universe = Universe::load(&conn).unwrap();
        assert!(universe.facets);
        assert_eq!(universe.videos.len(), 2, "non-programmable rows stay out");
        let a = universe.video(1).unwrap();
        assert_eq!(a.cut, Cut::Official);
        assert_eq!(a.scene, Some(5));
        assert_eq!(universe.song_year(a), Some(1999));
        assert_eq!(universe.video(2).unwrap().concert, Some(7));
        assert_eq!(universe.scene_members[&5], vec![0, 1]);
        assert_eq!(universe.scene_artists[&5], 1);
        assert_eq!(universe.concerts[0].video_ids, vec![2]);
        assert_eq!(universe.scene(5).unwrap().channel_no, Some(10));
    }

    #[test]
    fn exemplars_parse_names_or_objects() {
        assert_eq!(parse_exemplars(r#"["A","B"]"#), vec!["A", "B"]);
        assert_eq!(parse_exemplars(r#"[{"name":"C"},{"id":1}]"#), vec!["C"]);
        assert!(parse_exemplars("nope").is_empty());
    }
}
