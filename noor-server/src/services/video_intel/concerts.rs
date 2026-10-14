//! Concert batching: live cuts by one artist in one upload year with ids
//! close together become one concert; long-form live videos are concerts of
//! one. Writes video_concerts and video_facets.concert_id.
//!
//! TIDAL ingests a concert film's songs as one upload batch, so their video
//! ids sit close together (gaps of a few ids) while two different shows by
//! the same artist in the same year sit far apart. A concert's id is its
//! first video's id, so it stays the same across rebuilds as long as the
//! batch does.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::cuts::{self, Cut};
use crate::services::video_sets::{VideoCandidate, VideoSetItem};
use crate::services::video_stations::to_item;

/// Ids further apart than this belong to different upload batches.
pub const MAX_ID_GAP: i64 = 400;
/// A batch needs this many cuts to count as a concert.
pub const MIN_BATCH: usize = 3;
/// A single live or session video this long is a whole concert.
pub const FULL_SHOW_S: i64 = cuts::LONGFORM_FROM_S;

#[derive(Debug, Clone, PartialEq)]
pub struct LiveCut {
    pub id: i64,
    pub artist_id: i64,
    pub cut: Cut,
    pub year: Option<i64>,
    pub duration_s: i64,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Concert {
    pub id: i64,
    pub artist_id: i64,
    pub year: Option<i64>,
    pub label: String,
    pub video_ids: Vec<i64>,
    pub total_s: i64,
}

/// Words that say "this is live" without saying where: a segment made only
/// of these (plus years) cannot name a concert.
const FILLER: &[&str] = &[
    "live",
    "en",
    "vivo",
    "ao",
    "directo",
    "en directo",
    "concert",
    "concierto",
    "konzert",
    "official",
    "oficial",
    "video",
    "music",
    "performance",
    "version",
    "audio",
    "hd",
    "4k",
    "remastered",
    "remaster",
    "session",
    "sessions",
    "acoustic",
    "acustico",
    "the",
    "a",
    "from",
    "at",
    "in",
    "on",
    "de",
    "do",
    "da",
    "del",
    "la",
    "le",
    "el",
];

fn is_generic(segment: &str) -> bool {
    cuts::fold(segment)
        .split_whitespace()
        .all(|word| FILLER.contains(&word) || word.chars().all(|c| c.is_ascii_digit()))
}

/// The segment of a title that names the show ("Live at Wembley 1986").
fn show_segment(title: &str) -> Option<String> {
    cuts::title_parts(title)
        .segments
        .into_iter()
        .find(|segment| cuts::segment_is_live(segment) || cuts::segment_is_session(segment))
}

/// Longest common word prefix, trailing punctuation trimmed.
fn common_prefix(texts: &[&str]) -> String {
    let split: Vec<Vec<&str>> = texts
        .iter()
        .map(|text| text.split_whitespace().collect())
        .collect();
    let Some(first) = split.first() else {
        return String::new();
    };
    let mut len = first.len();
    for words in &split[1..] {
        len = len.min(
            first
                .iter()
                .zip(words)
                .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
                .count(),
        );
    }
    first[..len]
        .join(" ")
        .trim_end_matches([',', '-', ':', '/', '|'])
        .trim()
        .to_string()
}

/// A concert's name: the show segment most of its cuts share ("Live at
/// Trianon 2015"), else their shared prefix ("Live at Beacon Theatre, New
/// York, NY" across nightly dates), else "Concert, {year}".
pub fn concert_label(titles: &[&str], year: Option<i64>) -> String {
    let segments: Vec<String> = titles.iter().filter_map(|t| show_segment(t)).collect();
    let quorum = titles.len().div_ceil(2).max(1);
    if segments.len() >= quorum {
        let mut counts: HashMap<String, (usize, &str)> = HashMap::new();
        for segment in &segments {
            let entry = counts
                .entry(cuts::fold(segment))
                .or_insert((0, segment.as_str()));
            entry.0 += 1;
        }
        let best = counts
            .values()
            .filter(|(_, segment)| !is_generic(segment))
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(a.1)));
        if let Some((count, segment)) = best
            && *count >= quorum.max(2).min(titles.len())
        {
            return segment.to_string();
        }
        let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        let prefix = common_prefix(&refs);
        if prefix.split_whitespace().count() >= 3 && !is_generic(&prefix) {
            return prefix;
        }
    }
    if titles.len() == 1 {
        if let Some(segment) = segments.first().filter(|s| !is_generic(s)) {
            return segment.clone();
        }
        let base = cuts::title_parts(titles[0]).base;
        if !base.is_empty() && cuts::has_live_marker(&base) {
            return base;
        }
    }
    match year {
        Some(year) => format!("Concert, {year}"),
        None => "Concert".to_string(),
    }
}

/// Group live cuts into concerts. Pure: input order does not matter.
pub fn batch(cuts_in: &[LiveCut]) -> Vec<Concert> {
    let mut groups: HashMap<(i64, Option<i64>), Vec<&LiveCut>> = HashMap::new();
    for cut in cuts_in {
        groups
            .entry((cut.artist_id, cut.year))
            .or_default()
            .push(cut);
    }
    let mut concerts = Vec::new();
    let mut batched: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut keys: Vec<_> = groups.keys().copied().collect();
    keys.sort();
    for key in keys {
        let mut members = groups.remove(&key).unwrap_or_default();
        members.sort_by_key(|cut| cut.id);
        let mut runs: Vec<Vec<&LiveCut>> = Vec::new();
        for cut in members {
            match runs.last_mut() {
                Some(run) if cut.id - run.last().map_or(cut.id, |c| c.id) <= MAX_ID_GAP => {
                    run.push(cut)
                }
                _ => runs.push(vec![cut]),
            }
        }
        for run in runs.into_iter().filter(|run| run.len() >= MIN_BATCH) {
            let titles: Vec<&str> = run.iter().map(|c| c.title.as_str()).collect();
            batched.extend(run.iter().map(|c| c.id));
            concerts.push(Concert {
                id: run[0].id,
                artist_id: key.0,
                year: key.1,
                label: concert_label(&titles, key.1),
                video_ids: run.iter().map(|c| c.id).collect(),
                total_s: run.iter().map(|c| c.duration_s).sum(),
            });
        }
    }
    for cut in cuts_in {
        if cut.duration_s >= FULL_SHOW_S && !batched.contains(&cut.id) {
            concerts.push(Concert {
                id: cut.id,
                artist_id: cut.artist_id,
                year: cut.year,
                label: concert_label(&[cut.title.as_str()], cut.year),
                video_ids: vec![cut.id],
                total_s: cut.duration_s,
            });
        }
    }
    concerts.sort_by_key(|c| c.id);
    concerts
}

/// Rebuild video_concerts and video_facets.concert_id from live and session
/// cuts. Returns the number of concerts.
pub fn build(conn: &Connection) -> Result<usize> {
    let mut live = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT f.tidal_video_id, f.artist_tidal_id, f.cut, f.cut_year, f.duration_s,
                    COALESCE(json_extract(c.item_json, '$.title'), '')
               FROM video_facets f
               JOIN video_catalog c ON c.tidal_video_id = f.tidal_video_id
              WHERE f.cut IN ('live', 'session') AND f.programmable = 1
                AND f.artist_tidal_id > 0",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            live.push(LiveCut {
                id: row.get(0)?,
                artist_id: row.get(1)?,
                cut: Cut::parse(&row.get::<_, String>(2)?).unwrap_or(Cut::Live),
                year: row.get(3)?,
                duration_s: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                title: row.get(5)?,
            });
        }
    }
    let concerts = batch(&live);
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM video_concerts", [])?;
    tx.execute(
        "UPDATE video_facets SET concert_id = NULL WHERE concert_id IS NOT NULL",
        [],
    )?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO video_concerts (concert_id, artist_tidal_id, year, label, video_count,
                 total_s, first_video_id, video_ids_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        let mut tag =
            tx.prepare("UPDATE video_facets SET concert_id = ?1 WHERE tidal_video_id = ?2")?;
        for concert in &concerts {
            insert.execute(params![
                concert.id,
                concert.artist_id,
                concert.year,
                concert.label,
                concert.video_ids.len() as i64,
                concert.total_s,
                concert.video_ids[0],
                serde_json::to_string(&concert.video_ids)?,
            ])?;
            for id in &concert.video_ids {
                tag.execute(params![concert.id, id])?;
            }
        }
    }
    tx.commit()?;
    Ok(concerts.len())
}

/// `VideoConcert` on the wire: the setlist in upload order.
#[derive(Debug, Clone, Serialize)]
pub struct ConcertSetlist {
    pub id: i64,
    pub artist_id: i64,
    pub artist_name: Option<String>,
    pub year: Option<i64>,
    pub label: String,
    pub video_count: i64,
    pub total_s: i64,
    pub videos: Vec<VideoSetItem>,
}

pub fn read(conn: &Connection, id: i64) -> Result<Option<ConcertSetlist>> {
    let Some((artist_id, year, label, video_count, total_s, ids_json)) = conn
        .query_row(
            "SELECT artist_tidal_id, year, label, video_count, total_s, video_ids_json
               FROM video_concerts WHERE concert_id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    let ids: Vec<i64> = serde_json::from_str(&ids_json).unwrap_or_default();
    let mut by_id: HashMap<i64, VideoCandidate> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT tidal_video_id, item_json FROM video_catalog
              WHERE tidal_video_id IN (SELECT value FROM json_each(?1))",
        )?;
        let mut rows = stmt.query([&ids_json])?;
        while let Some(row) = rows.next()? {
            if let Ok(video) = serde_json::from_str::<VideoCandidate>(&row.get::<_, String>(1)?) {
                by_id.insert(row.get(0)?, video);
            }
        }
    }
    let videos: Vec<VideoSetItem> = ids
        .iter()
        .filter_map(|id| by_id.get(id))
        .map(to_item)
        .collect();
    let artist_name = videos.iter().find_map(|v| v.artist_name.clone());
    Ok(Some(ConcertSetlist {
        id,
        artist_id,
        artist_name,
        year,
        label,
        video_count,
        total_s,
        videos,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_intel::songs::tests::{add, conn};

    fn cut(id: i64, artist: i64, year: i64, title: &str) -> LiveCut {
        LiveCut {
            id,
            artist_id: artist,
            cut: Cut::Live,
            year: Some(year),
            duration_s: 240,
            title: title.into(),
        }
    }

    #[test]
    fn batches_split_on_id_gaps_and_need_three_cuts() {
        let cuts = vec![
            cut(1000, 7, 2015, "A (Live at Trianon 2015)"),
            cut(1003, 7, 2015, "B (Live at Trianon 2015)"),
            cut(1390, 7, 2015, "C (Live at Trianon 2015)"),
            // A gap over 400 starts a new run; two cuts are not a concert.
            cut(2000, 7, 2015, "D (Live)"),
            cut(2001, 7, 2015, "E (Live)"),
            // Same ids, other year: separate.
            cut(1001, 7, 2016, "F (Live)"),
            cut(1002, 9, 2015, "G (Live)"),
        ];
        let concerts = batch(&cuts);
        assert_eq!(concerts.len(), 1);
        assert_eq!(concerts[0].id, 1000);
        assert_eq!(concerts[0].video_ids, vec![1000, 1003, 1390]);
        assert_eq!(concerts[0].label, "Live at Trianon 2015");
        assert_eq!(concerts[0].total_s, 720);
    }

    #[test]
    fn long_live_singles_are_concerts_of_one() {
        let mut show = cut(5, 7, 2009, "Live at Pompeii (Live)");
        show.duration_s = 3600;
        let concerts = batch(&[show]);
        assert_eq!(concerts.len(), 1);
        assert_eq!(concerts[0].video_ids, vec![5]);
        assert_eq!(concerts[0].label, "Live at Pompeii");
    }

    #[test]
    fn labels_fall_back_to_a_shared_prefix_then_the_year() {
        assert_eq!(
            concert_label(
                &[
                    "Jessica (Live at Beacon Theatre, New York, NY, 3/26/2009)",
                    "Melissa (Live at Beacon Theatre, New York, NY, 3/27/2009)",
                    "Dreams (Live at Beacon Theatre, New York, NY, 3/28/2009)",
                ],
                Some(2009)
            ),
            "Live at Beacon Theatre, New York, NY"
        );
        assert_eq!(
            concert_label(&["A (En Vivo)", "B (En Vivo)", "C (Ao Vivo)"], Some(2018)),
            "Concert, 2018"
        );
        assert_eq!(concert_label(&["A", "B", "C"], None), "Concert");
        assert_eq!(
            concert_label(
                &[
                    "Medley (4 Latidos Tour - En Vivo)",
                    "Otra (4 Latidos Tour - En Vivo)",
                    "Mas (4 Latidos Tour - En Vivo)"
                ],
                Some(2019)
            ),
            "4 Latidos Tour - En Vivo"
        );
    }

    #[test]
    fn build_writes_concerts_and_tags_facets() {
        let conn = conn();
        for (id, title) in [
            (100, "One (Live at Wembley 1986)"),
            (101, "Two (Live at Wembley 1986)"),
            (102, "Three (Live at Wembley 1986)"),
            (103, "Four"),
        ] {
            add(&conn, id, 7, title, 240, None, Some(1986));
        }
        super::super::cuts::build(&conn).unwrap();
        assert_eq!(build(&conn).unwrap(), 1);
        let tagged: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM video_facets WHERE concert_id = 100",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tagged, 3);
        let setlist = read(&conn, 100).unwrap().unwrap();
        assert_eq!(setlist.label, "Live at Wembley 1986");
        assert_eq!(
            setlist
                .videos
                .iter()
                .map(|v| v.tidal_id)
                .collect::<Vec<_>>(),
            vec![100, 101, 102]
        );
        assert_eq!(setlist.artist_name.as_deref(), Some("Artist 7"));
        assert!(read(&conn, 999).unwrap().is_none());
        // A rebuild keeps the same id.
        super::super::cuts::build(&conn).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM video_facets WHERE concert_id = 100",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            3
        );
        assert_eq!(build(&conn).unwrap(), 1);
    }
}
