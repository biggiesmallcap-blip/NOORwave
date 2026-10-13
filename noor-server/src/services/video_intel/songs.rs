//! Song rollup: video_facets grouped by song_key into video_songs with
//! the best cut (library_videos order: best match, then the preferred cut,
//! then longest), the cuts present as a bit mask, and a song's versions for
//! the Versions drawer.
//!
//! Same-titled cuts are real versions (a concert-film clip under the bare
//! title, a re-upload, a live take), never duplicates: every one survives as
//! a version, and the label says what tells it apart.

use std::cmp::Ordering;
use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::cuts::{self, Cut};
use crate::services::video_sets::{VideoCandidate, VideoSetItem};
use crate::services::video_stations::to_item;

/// What decides which cut fronts a song.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rank {
    pub id: i64,
    pub programmable: bool,
    /// library_videos match score when the cut matched a liked song, else 0.
    pub match_score: f64,
    pub cut: Cut,
    /// The title is the song's plain video (see `cuts::is_plain_title`).
    pub plain: bool,
    pub duration_s: i64,
    pub popularity: i64,
}

/// Best first: programmable, best library match, preferred cut (official
/// over live over the rest), the plain title over a variant ("Vevo
/// Footnotes", "British Sign Language"), most popular, longest, lowest id.
/// Popularity comes before runtime: with teasers already split out as
/// trailers and clips, the longest official cut is too often an extended
/// or annotated variant, while TIDAL's popularity picks the canonical one.
pub fn best_first(a: &Rank, b: &Rank) -> Ordering {
    b.programmable
        .cmp(&a.programmable)
        .then(b.match_score.total_cmp(&a.match_score))
        .then(a.cut.best_rank().cmp(&b.cut.best_rank()))
        .then(b.plain.cmp(&a.plain))
        .then(b.popularity.cmp(&a.popularity))
        .then(b.duration_s.cmp(&a.duration_s))
        .then(a.id.cmp(&b.id))
}

/// The song's name as a tile shows it: the best cut's title without its
/// bracketed or dashed suffix.
pub fn song_title(title: &str) -> String {
    let base = cuts::title_parts(title).base;
    if base.is_empty() {
        title.trim().to_string()
    } else {
        base
    }
}

/// Best library match per video, for liked songs.
pub(crate) fn match_scores(conn: &Connection) -> Result<HashMap<i64, f64>> {
    let mut stmt = conn.prepare(
        "SELECT tidal_video_id, MAX(match_score) FROM library_videos
          WHERE suppressed = 0 GROUP BY tidal_video_id",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

struct Member {
    rank: Rank,
    artist_id: Option<i64>,
    artist_name: Option<String>,
    title: String,
}

#[derive(Default)]
struct Carried {
    song_year: Option<i64>,
    era_source: Option<String>,
    era_conf: Option<f64>,
    scene_id: Option<i64>,
}

/// Rebuild video_songs from video_facets. Era and scene columns are carried
/// over from the previous build (era and link rewrite them later in the same
/// build), so the Archive never loses its lenses mid-rebuild.
pub fn build(conn: &Connection) -> Result<usize> {
    let scores = match_scores(conn)?;
    let mut carried: HashMap<String, Carried> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT song_key, song_year, era_source, era_conf, scene_id FROM video_songs",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            carried.insert(
                row.get(0)?,
                Carried {
                    song_year: row.get(1)?,
                    era_source: row.get(2)?,
                    era_conf: row.get(3)?,
                    scene_id: row.get(4)?,
                },
            );
        }
    }
    let mut groups: HashMap<String, Vec<Member>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT f.tidal_video_id, f.artist_tidal_id, c.artist_name,
                    COALESCE(json_extract(c.item_json, '$.title'), ''), f.cut,
                    f.programmable, f.song_key, f.duration_s, f.popularity
               FROM video_facets f
               JOIN video_catalog c ON c.tidal_video_id = f.tidal_video_id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let cut = Cut::parse(&row.get::<_, String>(4)?).unwrap_or(Cut::Official);
            let title: String = row.get(3)?;
            let member = Member {
                rank: Rank {
                    id,
                    programmable: row.get(5)?,
                    match_score: scores.get(&id).copied().unwrap_or(0.0),
                    cut,
                    plain: cuts::is_plain_title(&title),
                    duration_s: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
                    popularity: row.get::<_, Option<i64>>(8)?.unwrap_or(-1),
                },
                artist_id: row.get(1)?,
                artist_name: row.get(2)?,
                title,
            };
            groups.entry(row.get(6)?).or_default().push(member);
        }
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM video_songs", [])?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO video_songs (song_key, artist_tidal_id, artist_name, title,
                 best_video_id, cut_count, cuts_mask, programmable, song_year, era_source,
                 era_conf, scene_id, popularity, total_s)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        )?;
        for (key, mut members) in groups.drain() {
            members.sort_by(|a, b| best_first(&a.rank, &b.rank));
            let best = &members[0];
            let mask = members.iter().fold(0i64, |mask, m| mask | m.rank.cut.bit());
            let popularity = members
                .iter()
                .map(|m| m.rank.popularity)
                .max()
                .filter(|p| *p >= 0);
            let total_s: i64 = members.iter().map(|m| m.rank.duration_s).sum();
            let previous = carried.remove(&key).unwrap_or_default();
            insert.execute(params![
                key,
                best.artist_id,
                best.artist_name,
                song_title(&best.title),
                best.rank.id,
                members.len() as i64,
                mask,
                members.iter().any(|m| m.rank.programmable),
                previous.song_year,
                previous.era_source,
                previous.era_conf,
                previous.scene_id,
                popularity,
                total_s,
            ])?;
        }
    }
    tx.commit()?;
    Ok(
        conn.query_row("SELECT COUNT(*) FROM video_songs", [], |row| {
            row.get::<_, i64>(0)
        })? as usize,
    )
}

// ---------------------------------------------------------------- versions

#[derive(Debug, Clone, Serialize)]
pub struct ConcertRef {
    pub id: i64,
    pub label: String,
}

/// One version as the Versions drawer shows it (`VideoVersion` on the wire).
#[derive(Debug, Clone, Serialize)]
pub struct Version {
    pub video: VideoSetItem,
    pub cut: Cut,
    pub cut_year: Option<i64>,
    pub label: String,
    pub concert: Option<ConcertRef>,
    pub best: bool,
}

/// `SongVersionsResponse` on the wire.
#[derive(Debug, Clone, Serialize)]
pub struct SongVersions {
    pub song_key: String,
    pub title: String,
    pub artist_id: Option<i64>,
    pub artist_name: Option<String>,
    pub song_year: Option<i64>,
    pub versions: Vec<Version>,
}

/// "4:07", or "1:02:30" for an hour and more.
pub fn runtime(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            (seconds % 3600) / 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

fn is_credit(segment: &str) -> bool {
    let lower = segment.to_lowercase();
    ["feat", "ft.", "ft ", "with ", "featuring", "prod"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// What tells a version apart: its concert, else the title's suffix
/// ("Live at Croke Park"), else "{cut}, {year} upload batch, 4:07".
pub fn version_label(
    title: &str,
    cut: Cut,
    cut_year: Option<i64>,
    duration_s: Option<i64>,
    concert_label: Option<&str>,
) -> String {
    if let Some(label) = concert_label.filter(|l| !l.trim().is_empty()) {
        return label.to_string();
    }
    let segments: Vec<String> = cuts::title_parts(title)
        .segments
        .into_iter()
        .filter(|segment| !is_credit(segment))
        .collect();
    if !segments.is_empty() {
        return segments.join(", ");
    }
    let mut label = cut.label().to_string();
    if let Some(year) = cut_year {
        label.push_str(&format!(", {year} upload batch"));
    }
    if let Some(duration) = duration_s {
        label.push_str(&format!(", {}", runtime(duration)));
    }
    label
}

/// Make equal labels distinct: add the runtime, then a counter.
fn disambiguate(labels: &mut [String], durations: &[Option<i64>]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for label in labels.iter() {
        *counts.entry(label.clone()).or_default() += 1;
    }
    for (label, duration) in labels.iter_mut().zip(durations) {
        if counts.get(label.as_str()).copied().unwrap_or(0) > 1
            && let Some(duration) = duration
        {
            let time = runtime(*duration);
            if !label.ends_with(&time) {
                label.push_str(&format!(", {time}"));
            }
        }
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    for label in labels.iter_mut() {
        let n = seen.entry(label.clone()).or_default();
        *n += 1;
        if *n > 1 {
            label.push_str(&format!(" ({n})"));
        }
    }
}

/// The versions of the song a video belongs to. None when the build has
/// not seen the video.
pub fn versions_for_video(conn: &Connection, video_id: i64) -> Result<Option<SongVersions>> {
    let key: Option<String> = conn
        .query_row(
            "SELECT song_key FROM video_facets WHERE tidal_video_id = ?1",
            [video_id],
            |row| row.get(0),
        )
        .optional()?;
    match key {
        Some(key) => versions(conn, &key),
        None => Ok(None),
    }
}

/// Every version of one song, best first. None when the song is unknown.
pub fn versions(conn: &Connection, song_key: &str) -> Result<Option<SongVersions>> {
    let Some((title, artist_id, artist_name, song_year, best_id)) = conn
        .query_row(
            "SELECT title, artist_tidal_id, artist_name, song_year, best_video_id
               FROM video_songs WHERE song_key = ?1",
            [song_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    struct Row {
        rank: Rank,
        video: VideoCandidate,
        cut_year: Option<i64>,
        concert: Option<ConcertRef>,
    }
    let mut rows = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT f.tidal_video_id, f.cut, f.cut_year, f.concert_id, f.duration_s,
                    f.popularity, f.programmable, c.item_json, vc.label,
                    (SELECT MAX(lv.match_score) FROM library_videos lv
                      WHERE lv.tidal_video_id = f.tidal_video_id AND lv.suppressed = 0)
               FROM video_facets f
               JOIN video_catalog c ON c.tidal_video_id = f.tidal_video_id
               LEFT JOIN video_concerts vc ON vc.concert_id = f.concert_id
              WHERE f.song_key = ?1",
        )?;
        let mut query = stmt.query([song_key])?;
        while let Some(row) = query.next()? {
            let Ok(video) = serde_json::from_str::<VideoCandidate>(&row.get::<_, String>(7)?)
            else {
                continue;
            };
            let concert = match (
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(8)?,
            ) {
                (Some(id), Some(label)) => Some(ConcertRef { id, label }),
                _ => None,
            };
            rows.push(Row {
                rank: Rank {
                    id: row.get(0)?,
                    programmable: row.get(6)?,
                    match_score: row.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
                    cut: Cut::parse(&row.get::<_, String>(1)?).unwrap_or(Cut::Official),
                    plain: cuts::is_plain_title(&video.title),
                    duration_s: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    popularity: row.get::<_, Option<i64>>(5)?.unwrap_or(-1),
                },
                video,
                cut_year: row.get(2)?,
                concert,
            });
        }
    }
    rows.sort_by(|a, b| {
        (b.rank.id == best_id)
            .cmp(&(a.rank.id == best_id))
            .then(best_first(&a.rank, &b.rank))
    });
    let mut labels: Vec<String> = rows
        .iter()
        .map(|row| {
            version_label(
                &row.video.title,
                row.rank.cut,
                row.cut_year,
                row.video.duration_s,
                row.concert.as_ref().map(|c| c.label.as_str()),
            )
        })
        .collect();
    let durations: Vec<Option<i64>> = rows.iter().map(|row| row.video.duration_s).collect();
    disambiguate(&mut labels, &durations);
    let versions = rows
        .into_iter()
        .zip(labels)
        .map(|(row, label)| Version {
            video: to_item(&row.video),
            cut: row.rank.cut,
            cut_year: row.cut_year,
            label,
            concert: row.concert,
            best: row.rank.id == best_id,
        })
        .collect();
    Ok(Some(SongVersions {
        song_key: song_key.to_string(),
        title,
        artist_id,
        artist_name,
        song_year,
        versions,
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn
    }

    pub(crate) fn add(
        conn: &Connection,
        id: i64,
        artist: i64,
        title: &str,
        duration_s: i64,
        popularity: Option<i32>,
        year: Option<i32>,
    ) {
        let video = VideoCandidate {
            tidal_id: id,
            title: title.into(),
            duration_s: Some(duration_s),
            artist_id: Some(artist),
            artist_name: Some(format!("Artist {artist}")),
            artwork_url: Some(format!("https://example.test/{id}.jpg")),
            release_year: year,
            popularity,
            video_type: Some("Music Video".into()),
            ..Default::default()
        };
        conn.execute(
            "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                id,
                artist,
                video.artist_name,
                serde_json::to_string(&video).unwrap()
            ],
        )
        .unwrap();
    }

    fn rank(id: i64, cut: Cut, duration_s: i64) -> Rank {
        Rank {
            id,
            programmable: cut.is_programmable(),
            match_score: 0.0,
            cut,
            plain: true,
            duration_s,
            popularity: 0,
        }
    }

    #[test]
    fn best_cut_order_prefers_match_then_official_then_plain_popular_longest() {
        let mut ranks = [
            rank(1, Cut::Lyric, 300),
            rank(2, Cut::Live, 400),
            rank(3, Cut::Official, 200),
            rank(4, Cut::Official, 250),
            rank(5, Cut::Clip, 60),
        ];
        ranks.sort_by(best_first);
        let ids: Vec<i64> = ranks.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![4, 3, 2, 1, 5]);
        let mut liked = rank(1, Cut::Lyric, 300);
        liked.match_score = 0.95;
        let mut ranks = [rank(3, Cut::Official, 200), liked];
        ranks.sort_by(best_first);
        assert_eq!(ranks[0].id, 1, "the liked-song match leads");
        let mut variant = rank(6, Cut::Official, 300);
        variant.plain = false;
        variant.popularity = 90;
        let mut popular = rank(7, Cut::Official, 200);
        popular.popularity = 40;
        let mut ranks = [variant, popular, rank(8, Cut::Official, 260)];
        ranks.sort_by(best_first);
        let ids: Vec<i64> = ranks.iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            vec![7, 8, 6],
            "plain first, then popular, then longest"
        );
    }

    #[test]
    fn songs_roll_up_every_cut_with_a_mask() {
        let conn = conn();
        add(&conn, 10, 7, "Swear It Again", 250, Some(30), Some(1999));
        add(&conn, 11, 7, "Swear It Again", 262, Some(5), Some(2016));
        add(
            &conn,
            12,
            7,
            "Swear It Again (Live At Croke Park Stadium)",
            270,
            None,
            Some(2008),
        );
        add(
            &conn,
            13,
            7,
            "Swear It Again (Lyric Video)",
            240,
            None,
            Some(2020),
        );
        add(
            &conn,
            14,
            7,
            "Flying Without Wings",
            240,
            Some(10),
            Some(1999),
        );
        cuts::build(&conn).unwrap();
        assert_eq!(build(&conn).unwrap(), 2);
        let (best, count, mask, title): (i64, i64, i64, String) = conn
            .query_row(
                "SELECT best_video_id, cut_count, cuts_mask, title FROM video_songs
                  WHERE song_key = 'id:7:swear it again'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(best, 10, "the most popular plain official cut");
        assert_eq!(count, 4);
        assert_eq!(
            Cut::from_mask(mask),
            vec![Cut::Official, Cut::Live, Cut::Lyric]
        );
        assert_eq!(title, "Swear It Again");
    }

    #[test]
    fn rebuild_keeps_era_and_scene_until_they_are_recomputed() {
        let conn = conn();
        add(&conn, 10, 7, "Song", 250, None, Some(2001));
        cuts::build(&conn).unwrap();
        build(&conn).unwrap();
        conn.execute(
            "UPDATE video_songs SET song_year = 1999, era_source = 'library', scene_id = 4",
            [],
        )
        .unwrap();
        cuts::build(&conn).unwrap();
        build(&conn).unwrap();
        let (year, scene): (i64, i64) = conn
            .query_row("SELECT song_year, scene_id FROM video_songs", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!((year, scene), (1999, 4));
    }

    #[test]
    fn versions_are_labelled_by_what_tells_them_apart() {
        let conn = conn();
        add(&conn, 10, 7, "Swear It Again", 250, Some(30), Some(1999));
        add(&conn, 11, 7, "Swear It Again", 262, Some(5), Some(2016));
        add(
            &conn,
            12,
            7,
            "Swear It Again (Live At Croke Park Stadium)",
            270,
            None,
            Some(2008),
        );
        add(
            &conn,
            13,
            7,
            "Swear It Again (feat. Someone)",
            262,
            Some(5),
            Some(2016),
        );
        cuts::build(&conn).unwrap();
        build(&conn).unwrap();
        let song = versions(&conn, "id:7:swear it again").unwrap().unwrap();
        let labels: Vec<(&str, bool)> = song
            .versions
            .iter()
            .map(|v| (v.label.as_str(), v.best))
            .collect();
        assert_eq!(
            labels,
            vec![
                ("Official, 1999 upload batch, 4:10", true),
                ("Official, 2016 upload batch, 4:22", false),
                ("Official, 2016 upload batch, 4:22 (2)", false),
                ("Live At Croke Park Stadium", false),
            ]
        );
        assert!(versions(&conn, "id:7:nothing").unwrap().is_none());
    }

    #[test]
    fn a_video_finds_its_song() {
        let conn = conn();
        add(&conn, 10, 7, "Swear It Again", 250, Some(30), Some(1999));
        add(
            &conn,
            12,
            7,
            "Swear It Again (Live At Croke Park Stadium)",
            270,
            None,
            Some(2008),
        );
        assert!(
            versions_for_video(&conn, 12).unwrap().is_none(),
            "not built yet"
        );
        cuts::build(&conn).unwrap();
        build(&conn).unwrap();
        let song = versions_for_video(&conn, 12).unwrap().unwrap();
        assert_eq!(song.song_key, "id:7:swear it again");
        assert_eq!(song.versions.len(), 2);
        assert!(song.versions[0].best);
        assert!(versions_for_video(&conn, 99).unwrap().is_none());
    }

    #[test]
    fn runtimes_read_like_a_clock() {
        assert_eq!(runtime(247), "4:07");
        assert_eq!(runtime(3750), "1:02:30");
    }
}
