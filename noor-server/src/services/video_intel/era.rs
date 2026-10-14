//! Era repair: song_year from library matches, context era tags, the
//! earliest cut year and the artist active range, with source and confidence.
//!
//! A video's release year is mostly its upload year (55% of the catalog says
//! 2020s), so it is the weakest signal. Precedence: the library album year
//! (a liked-song video match, or the artist plus the normalized title), then
//! a Last.fm era tag on a matched track, then the earliest cut year. An
//! earlier cut year beats a later album year: albums are often compilations
//! or remasters, and a song cannot be younger than its oldest video.
//!
//! Confidence for the cut-year fallback comes from the artist's own range,
//! measured on the snapshot against library years: a cut within 3 years of
//! the artist's first video names the right decade about 70% of the time,
//! 4-10 years about 65%, later than that about 35%; a year that holds a big
//! share of the artist's uploads (a back-catalog batch) is less trustworthy.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{Connection, params};

use super::cuts::plausible_year;
use crate::services::video_radio::video_song_key;

pub const SOURCE_LIBRARY: &str = "library";
pub const SOURCE_CONTEXT: &str = "context";
pub const SOURCE_CUT_YEAR: &str = "cut_year";

/// Era confidence at or above which the decade is more likely right than
/// wrong. The census counts these toward era coverage.
pub const TRUSTED_CONF: f64 = 0.5;

/// What is known about one song's age.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Evidence {
    /// Album year of a track a cut is matched to in library_videos.
    pub linked_year: Option<i64>,
    /// Album year of a library track by the artist with the same title.
    pub title_year: Option<i64>,
    /// Decade start from a track_context_tags era tag ("80s" -> 1980).
    pub context_decade: Option<i64>,
    pub earliest_cut_year: Option<i64>,
    /// The artist's earliest cut year across all their videos.
    pub artist_first_year: Option<i64>,
    /// Share of the artist's videos uploaded in `earliest_cut_year`.
    pub batch_share: f64,
    pub artist_videos: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Era {
    pub year: i64,
    pub source: &'static str,
    pub conf: f64,
}

/// Decide one song's year. Pure.
pub fn resolve(e: &Evidence) -> Option<Era> {
    let library = match (e.linked_year, e.title_year) {
        (Some(linked), Some(title)) => Some((linked.min(title), 0.9)),
        (Some(linked), None) => Some((linked, 0.9)),
        (None, Some(title)) => Some((title, 0.8)),
        (None, None) => None,
    };
    if let Some((year, conf)) = library {
        if let Some(cut) = e.earliest_cut_year.filter(|cut| *cut < year) {
            return Some(Era {
                year: cut,
                source: SOURCE_CUT_YEAR,
                conf: 0.75,
            });
        }
        return Some(Era {
            year,
            source: SOURCE_LIBRARY,
            conf,
        });
    }
    if let Some(decade) = e.context_decade {
        return Some(match e.earliest_cut_year {
            Some(cut) if (decade..decade + 10).contains(&cut) => Era {
                year: cut,
                source: SOURCE_CONTEXT,
                conf: 0.7,
            },
            _ => Era {
                year: decade,
                source: SOURCE_CONTEXT,
                conf: 0.6,
            },
        });
    }
    let cut = e.earliest_cut_year?;
    let gap = e.artist_first_year.map_or(0, |first| cut - first);
    let mut conf: f64 = if gap <= 3 {
        0.7
    } else if gap <= 10 {
        0.6
    } else {
        0.35
    };
    if e.artist_videos >= 5 && e.batch_share > 0.3 {
        conf -= 0.1;
    }
    Some(Era {
        year: cut,
        source: SOURCE_CUT_YEAR,
        conf,
    })
}

/// "80s" -> 1980, "00s" -> 2000, "1970s" -> 1970. None for anything else.
pub fn decade_of_tag(tag: &str) -> Option<i64> {
    let digits = tag.trim().strip_suffix('s')?;
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let decade = match digits.len() {
        2 => {
            let short: i64 = digits.parse().ok()?;
            if short % 10 != 0 {
                return None;
            }
            if short <= 20 {
                2000 + short
            } else {
                1900 + short
            }
        }
        4 => digits.parse().ok()?,
        _ => return None,
    };
    (decade % 10 == 0 && (1920..=2020).contains(&decade)).then_some(decade)
}

/// Rebuild song_year, era_source and era_conf for every song. Returns the
/// number of songs that got a year.
pub fn build(conn: &Connection) -> Result<usize> {
    let mut songs: HashMap<String, (Option<i64>, Evidence)> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT song_key, artist_tidal_id FROM video_songs")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            songs.insert(row.get(0)?, (row.get(1)?, Evidence::default()));
        }
    }
    // Earliest cut year per song.
    {
        let mut stmt = conn.prepare(
            "SELECT song_key, MIN(cut_year) FROM video_facets
              WHERE cut_year IS NOT NULL GROUP BY song_key",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            if let Some((_, evidence)) = songs.get_mut(&key) {
                evidence.earliest_cut_year = plausible_year(row.get(1)?);
            }
        }
    }
    // The artist's range: first year, and how uploads pile into years.
    let mut artist_years: HashMap<i64, HashMap<i64, usize>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT artist_tidal_id, cut_year, COUNT(*) FROM video_facets
              WHERE cut_year IS NOT NULL AND artist_tidal_id > 0 GROUP BY 1, 2",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            if let Some(year) = plausible_year(row.get(1)?) {
                artist_years
                    .entry(row.get(0)?)
                    .or_default()
                    .insert(year, row.get::<_, i64>(2)? as usize);
            }
        }
    }
    // Library album years through liked-song video matches.
    let mut matched_tracks: HashMap<String, Vec<i64>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT f.song_key, al.year, t.id
               FROM library_videos lv
               JOIN video_facets f ON f.tidal_video_id = lv.tidal_video_id
               JOIN tracks t ON t.id = lv.track_id
               LEFT JOIN albums al ON al.id = t.album_id
              WHERE lv.suppressed = 0",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            if let Some((_, evidence)) = songs.get_mut(&key) {
                if let Some(year) = plausible_year(row.get(1)?) {
                    evidence.linked_year =
                        Some(evidence.linked_year.map_or(year, |y: i64| y.min(year)));
                }
                matched_tracks.entry(key).or_default().push(row.get(2)?);
            }
        }
    }
    // Library album years through the artist plus the normalized title.
    {
        let mut stmt = conn.prepare(
            "SELECT a.tidal_id, t.title, al.year, t.id
               FROM tracks t
               JOIN artists a ON a.id = t.artist_id
               LEFT JOIN albums al ON al.id = t.album_id
              WHERE a.tidal_id > 0",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let title: String = row.get(1)?;
            let key = video_song_key(Some(row.get(0)?), None, &title);
            if let Some((_, evidence)) = songs.get_mut(&key) {
                if let Some(year) = plausible_year(row.get(2)?) {
                    evidence.title_year =
                        Some(evidence.title_year.map_or(year, |y: i64| y.min(year)));
                }
                matched_tracks.entry(key).or_default().push(row.get(3)?);
            }
        }
    }
    // Era tags on matched tracks: the most confident decade wins.
    {
        let mut tags: HashMap<i64, (f64, i64)> = HashMap::new();
        let mut stmt = conn.prepare(
            "SELECT track_id, normalized_tag, confidence FROM track_context_tags
              WHERE context = 'era'",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let Some(decade) = decade_of_tag(&row.get::<_, String>(1)?) else {
                continue;
            };
            let conf: f64 = row.get(2)?;
            let entry = tags.entry(row.get(0)?).or_insert((conf, decade));
            if conf > entry.0 || (conf == entry.0 && decade < entry.1) {
                *entry = (conf, decade);
            }
        }
        for (key, tracks) in &matched_tracks {
            let best = tracks
                .iter()
                .filter_map(|track| tags.get(track))
                .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)));
            if let (Some((_, decade)), Some((_, evidence))) = (best, songs.get_mut(key)) {
                evidence.context_decade = Some(*decade);
            }
        }
    }
    let mut dated = 0usize;
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    {
        let mut update = tx.prepare(
            "UPDATE video_songs SET song_year = ?2, era_source = ?3, era_conf = ?4
              WHERE song_key = ?1",
        )?;
        for (key, (artist, mut evidence)) in songs {
            if let Some(years) = artist.and_then(|a| artist_years.get(&a)) {
                evidence.artist_first_year = years.keys().min().copied();
                evidence.artist_videos = years.values().sum();
                if let Some(cut) = evidence.earliest_cut_year {
                    evidence.batch_share = years.get(&cut).copied().unwrap_or(0) as f64
                        / evidence.artist_videos.max(1) as f64;
                }
            }
            let era = resolve(&evidence);
            dated += usize::from(era.is_some());
            update.execute(params![
                key,
                era.as_ref().map(|e| e.year),
                era.as_ref().map(|e| e.source),
                era.as_ref().map(|e| (e.conf * 100.0).round() / 100.0),
            ])?;
        }
    }
    tx.commit()?;
    Ok(dated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_intel::songs::tests::{add, conn};
    use crate::services::video_intel::{cuts, songs};

    #[test]
    fn precedence_library_then_context_then_cut_year() {
        let library = Evidence {
            linked_year: Some(1999),
            title_year: Some(2005),
            context_decade: Some(1990),
            earliest_cut_year: Some(2016),
            ..Default::default()
        };
        assert_eq!(
            resolve(&library),
            Some(Era {
                year: 1999,
                source: SOURCE_LIBRARY,
                conf: 0.9
            })
        );
        let title_only = Evidence {
            title_year: Some(1987),
            earliest_cut_year: Some(2020),
            ..Default::default()
        };
        assert_eq!(resolve(&title_only).unwrap().conf, 0.8);
        let compilation = Evidence {
            title_year: Some(2009),
            earliest_cut_year: Some(1985),
            ..Default::default()
        };
        assert_eq!(
            resolve(&compilation),
            Some(Era {
                year: 1985,
                source: SOURCE_CUT_YEAR,
                conf: 0.75
            })
        );
        let tagged = Evidence {
            context_decade: Some(1980),
            earliest_cut_year: Some(2019),
            ..Default::default()
        };
        assert_eq!(resolve(&tagged).unwrap().year, 1980);
        let tagged_and_dated = Evidence {
            context_decade: Some(1980),
            earliest_cut_year: Some(1984),
            ..Default::default()
        };
        assert_eq!(resolve(&tagged_and_dated).unwrap().year, 1984);
        assert_eq!(resolve(&Evidence::default()), None);
    }

    #[test]
    fn cut_year_confidence_follows_the_artist_range() {
        let early = Evidence {
            earliest_cut_year: Some(2016),
            artist_first_year: Some(2015),
            artist_videos: 3,
            ..Default::default()
        };
        assert_eq!(resolve(&early).unwrap().conf, 0.7);
        let late_batch = Evidence {
            earliest_cut_year: Some(2020),
            artist_first_year: Some(1985),
            artist_videos: 40,
            batch_share: 0.6,
            ..Default::default()
        };
        let era = resolve(&late_batch).unwrap();
        assert_eq!(era.source, SOURCE_CUT_YEAR);
        assert!((era.conf - 0.25).abs() < 1e-9);
    }

    #[test]
    fn era_tags_parse_to_decades() {
        assert_eq!(decade_of_tag("80s"), Some(1980));
        assert_eq!(decade_of_tag("00s"), Some(2000));
        assert_eq!(decade_of_tag("10s"), Some(2010));
        assert_eq!(decade_of_tag("1970s"), Some(1970));
        assert_eq!(decade_of_tag("2010s"), Some(2010));
        assert_eq!(decade_of_tag("85s"), None);
        assert_eq!(decade_of_tag("chill"), None);
    }

    #[test]
    fn build_reads_library_albums_and_tags() {
        let conn = conn();
        conn.execute_batch(
            "INSERT INTO artists (id, tidal_id, name) VALUES (1, 7, 'Artist 7');
             INSERT INTO albums (id, title, artist_id, year) VALUES (1, 'Hits', 1, 1999);
             INSERT INTO tracks (id, title, artist_id, album_id) VALUES (1, 'Swear It Again', 1, 1);
             INSERT INTO tracks (id, title, artist_id) VALUES (2, 'Tagged Song', 1);
             INSERT INTO track_context_tags (track_id, tag, normalized_tag, context, source, confidence)
                 VALUES (2, '80s', '80s', 'era', 'lastfm', 0.8);",
        )
        .unwrap();
        add(&conn, 10, 7, "Swear It Again", 250, None, Some(2016));
        add(
            &conn,
            11,
            7,
            "Tagged Song (Official Video)",
            250,
            None,
            Some(2019),
        );
        add(&conn, 12, 7, "Upload Only", 250, None, Some(2021));
        add(&conn, 13, 7, "No Year", 250, None, None);
        cuts::build(&conn).unwrap();
        songs::build(&conn).unwrap();
        assert_eq!(build(&conn).unwrap(), 3);
        let year = |key: &str| -> (Option<i64>, Option<String>) {
            conn.query_row(
                "SELECT song_year, era_source FROM video_songs WHERE song_key = ?1",
                [key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(
            year("id:7:swear it again"),
            (Some(1999), Some("library".into()))
        );
        assert_eq!(
            year("id:7:tagged song"),
            (Some(1980), Some("context".into()))
        );
        assert_eq!(
            year("id:7:upload only"),
            (Some(2021), Some("cut_year".into()))
        );
        assert_eq!(year("id:7:no year"), (None, None));
    }
}
