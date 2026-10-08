//! Every TIDAL video payload the app sees teaches the crawler something: the
//! video exists, who made it, who is featured on it, and which artists TIDAL's
//! editors list together.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use super::{artist_state, graph};
use crate::services::tidal::client::TidalHomeItem;
use crate::services::video_sets::VideoCandidate;

/// Each artist in a curated list links to the next 8 distinct artists.
pub const COLIST_WINDOW: usize = 8;

#[derive(Debug, Clone, Copy)]
pub enum HarvestContext<'a> {
    /// Search results: videos only, no relationship evidence.
    Search,
    /// A curated list (mix, playlist, editorial module) identified by `key`.
    List { key: &'a str },
    /// One page of an artist's own video list.
    ArtistPage { artist_id: i64, name: &'a str },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HarvestSummary {
    pub stored: usize,
    pub new_videos: usize,
    pub edges_added: usize,
}

/// Featured (non-main) artist ids from TIDAL's `artists` array.
pub fn featured_artist_ids(extra: &HashMap<String, Value>) -> Vec<i64> {
    extra
        .get("artists")
        .and_then(Value::as_array)
        .map(|artists| {
            artists
                .iter()
                .filter(|artist| artist.get("type").and_then(Value::as_str) == Some("FEATURED"))
                .filter_map(|artist| artist.get("id").and_then(Value::as_i64))
                .filter(|id| *id > 0)
                .collect()
        })
        .unwrap_or_default()
}

/// Popularity and artist-mix id carried on TIDAL artist objects. A popularity
/// of 0 means "not computed" on TIDAL's side and is treated as unknown.
pub fn artist_facts(extra: &HashMap<String, Value>) -> (Option<i32>, Option<String>) {
    let popularity = extra
        .get("popularity")
        .and_then(Value::as_i64)
        .and_then(|p| i32::try_from(p).ok())
        .filter(|p| *p > 0);
    let mix_id = extra
        .get("mixes")
        .and_then(|mixes| mixes.get("ARTIST_MIX"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    (popularity, mix_id)
}

/// Editorial page items carry enough to index a video, but no facts.
pub fn candidate_from_home_item(item: &TidalHomeItem) -> Option<VideoCandidate> {
    if item.kind != "video" {
        return None;
    }
    let tidal_id = item.id.parse::<i64>().ok().filter(|id| *id > 0)?;
    Some(VideoCandidate {
        tidal_id,
        title: item.title.clone(),
        duration_s: item.duration,
        artist_id: item.artist_id,
        artist_name: item.artist_name.clone(),
        album_tidal_id: item.album_id,
        artwork_url: item.artwork_url.clone(),
        ..Default::default()
    })
}

/// Distinct artist pairs within `window` places of each other in a curated
/// list, each pair once with the smaller id first.
pub fn colist_pairs(artists: &[i64], window: usize) -> Vec<(i64, i64)> {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    for id in artists {
        if *id > 0 && seen.insert(*id) {
            order.push(*id);
        }
    }
    let mut pairs = Vec::new();
    for (index, a) in order.iter().enumerate() {
        for b in order.iter().skip(index + 1).take(window) {
            pairs.push(((*a).min(*b), (*a).max(*b)));
        }
    }
    pairs
}

/// A richer earlier record must not be thinned by a sparse later sighting.
fn merge_facts(mut fresh: VideoCandidate, old: VideoCandidate) -> VideoCandidate {
    fresh.duration_s = fresh.duration_s.or(old.duration_s);
    fresh.artist_id = fresh.artist_id.or(old.artist_id);
    fresh.artist_name = fresh.artist_name.or(old.artist_name);
    fresh.album_tidal_id = fresh.album_tidal_id.or(old.album_tidal_id);
    fresh.artwork_url = fresh.artwork_url.or(old.artwork_url);
    fresh.release_year = fresh.release_year.or(old.release_year);
    fresh.popularity = fresh.popularity.or(old.popularity);
    fresh.video_type = fresh.video_type.or(old.video_type);
    if fresh.featured_artist_ids.is_empty() {
        fresh.featured_artist_ids = old.featured_artist_ids;
    }
    fresh.quality = fresh.quality.or(old.quality);
    fresh.explicit = fresh.explicit.or(old.explicit);
    fresh
}

/// Savepoints nest inside a caller's transaction (the liked-video scanner
/// writes inside one) and also work on their own.
///
/// On its own it takes the write lock up front (`BEGIN IMMEDIATE`). A deferred
/// transaction that reads first and then writes cannot wait for the lock: in
/// WAL mode SQLite fails the upgrade with "database is locked" at once when
/// another connection committed in between, regardless of `busy_timeout`.
fn with_savepoint<T>(conn: &Connection, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let (begin, commit, rollback) = if conn.is_autocommit() {
        ("BEGIN IMMEDIATE", "COMMIT", "ROLLBACK")
    } else {
        (
            "SAVEPOINT video_harvest",
            "RELEASE video_harvest",
            "ROLLBACK TO video_harvest; RELEASE video_harvest",
        )
    };
    conn.execute_batch(begin)?;
    match f(conn) {
        Ok(value) => {
            conn.execute_batch(commit)?;
            Ok(value)
        }
        Err(error) => {
            let _ = conn.execute_batch(rollback);
            Err(error)
        }
    }
}

fn ledger_name(conn: &Connection, artist_id: i64) -> Result<String> {
    Ok(conn
        .query_row(
            "SELECT name FROM video_artist_state WHERE artist_tidal_id = ?1",
            [artist_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_default())
}

fn add_edge(conn: &Connection, from: i64, to: i64, source: &str, weight: f64) -> Result<usize> {
    Ok(conn.execute(
        "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, rank, weight)
         VALUES (?1, ?2, ?3, ?4, 0, ?5)
         ON CONFLICT(seed_tidal_id, related_tidal_id, source) DO NOTHING",
        params![from, to, ledger_name(conn, to)?, source, weight],
    )?)
}

fn add_colist(conn: &Connection, key: &str, a: i64, b: i64) -> Result<usize> {
    let first_time = conn.execute(
        "INSERT OR IGNORE INTO video_colist_seen (list_key, artist_a, artist_b) VALUES (?1, ?2, ?3)",
        params![key, a, b],
    )? > 0;
    if !first_time {
        return Ok(0);
    }
    let mut changed = 0;
    for (from, to) in [(a, b), (b, a)] {
        changed += conn.execute(
            "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, rank, weight)
             VALUES (?1, ?2, ?3, 'colist', 0, ?4)
             ON CONFLICT(seed_tidal_id, related_tidal_id, source) DO UPDATE SET
               rank = video_related_artists.rank + 1,
               weight = MIN(0.8, 0.4 + 0.1 * (video_related_artists.rank + 1))",
            params![from, to, ledger_name(conn, to)?, graph::colist_weight(0)],
        )?;
    }
    Ok(changed)
}

pub fn ingest(
    conn: &Connection,
    videos: &[VideoCandidate],
    ctx: HarvestContext<'_>,
) -> Result<HarvestSummary> {
    let summary = with_savepoint(conn, |conn| {
        let mut summary = HarvestSummary::default();
        let mut list_artists = Vec::new();
        for video in videos {
            if video.tidal_id <= 0 || video.title.trim().is_empty() {
                continue;
            }
            let mut video = video.clone();
            if let HarvestContext::ArtistPage { artist_id, name } = ctx {
                if video.artist_id.is_none() {
                    video.artist_id = Some(artist_id);
                }
                if video.artist_name.is_none() && video.artist_id == Some(artist_id) {
                    video.artist_name = Some(name.to_string());
                }
            }
            let previous: Option<String> = conn
                .query_row(
                    "SELECT item_json FROM video_catalog WHERE tidal_video_id = ?1",
                    [video.tidal_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(old) = previous
                .as_deref()
                .and_then(|json| serde_json::from_str::<VideoCandidate>(json).ok())
            {
                video = merge_facts(video, old);
            }
            conn.execute(
                "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(tidal_video_id) DO UPDATE SET
                   artist_tidal_id = excluded.artist_tidal_id,
                   artist_name = excluded.artist_name,
                   item_json = excluded.item_json,
                   fetched_at = datetime('now')",
                params![
                    video.tidal_id,
                    video.artist_id,
                    video.artist_name.as_deref(),
                    serde_json::to_string(&video)?,
                ],
            )?;
            summary.stored += 1;
            if previous.is_none() {
                summary.new_videos += 1;
            }
            let Some(main) = video.artist_id.filter(|id| *id > 0) else {
                continue;
            };
            artist_state::mark_seen(
                conn,
                main,
                video.artist_name.as_deref().unwrap_or(""),
                false,
            )?;
            for featured in video
                .featured_artist_ids
                .iter()
                .copied()
                .filter(|id| *id > 0 && *id != main)
            {
                artist_state::mark_seen(conn, featured, "", true)?;
                summary.edges_added +=
                    add_edge(conn, main, featured, "featured", graph::FEATURED_WEIGHT)?;
                summary.edges_added +=
                    add_edge(conn, featured, main, "featured", graph::FEATURED_WEIGHT)?;
            }
            list_artists.push(main);
        }
        if let HarvestContext::List { key } = ctx {
            for (a, b) in colist_pairs(&list_artists, COLIST_WINDOW) {
                summary.edges_added += add_colist(conn, key, a, b)?;
            }
        }
        Ok(summary)
    })?;
    if summary.edges_added > 0 {
        graph::mark_dirty();
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::tidal::client::TidalSearchVideo;
    use serde_json::json;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn
    }

    fn video(id: i64, artist: i64) -> VideoCandidate {
        VideoCandidate {
            tidal_id: id,
            title: format!("Video {id}"),
            artist_id: Some(artist),
            artist_name: Some(format!("Artist {artist}")),
            ..Default::default()
        }
    }

    fn edge_weight(conn: &Connection, from: i64, to: i64, source: &str) -> Option<(i64, f64)> {
        conn.query_row(
            "SELECT rank, weight FROM video_related_artists
              WHERE seed_tidal_id = ?1 AND related_tidal_id = ?2 AND source = ?3",
            params![from, to, source],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap()
    }

    #[test]
    fn search_payload_facts_survive_into_the_candidate() {
        let extra: HashMap<String, Value> = serde_json::from_value(json!({
            "popularity": 33,
            "artists": [
                {"id": 3571, "name": "Max Raabe", "type": "MAIN"},
                {"id": 6132980, "name": "Palast Orchester", "type": "FEATURED"}
            ]
        }))
        .unwrap();
        let raw = TidalSearchVideo {
            id: 122618603,
            title: "Du bist".into(),
            artist_id: Some(3571),
            r#type: "Music Video".into(),
            quality: Some("MP4_1080P".into()),
            explicit: Some(false),
            extra,
            ..Default::default()
        };
        let candidate = VideoCandidate::from(&raw);
        assert_eq!(candidate.popularity, Some(33));
        assert_eq!(candidate.featured_artist_ids, vec![6132980]);
        assert_eq!(candidate.video_type.as_deref(), Some("Music Video"));
        assert_eq!(candidate.quality.as_deref(), Some("MP4_1080P"));
    }

    #[test]
    fn artist_facts_treat_zero_popularity_as_unknown() {
        let extra: HashMap<String, Value> =
            serde_json::from_value(json!({"popularity": 77, "mixes": {"ARTIST_MIX": "abc"}}))
                .unwrap();
        assert_eq!(artist_facts(&extra), (Some(77), Some("abc".into())));
        let zero: HashMap<String, Value> =
            serde_json::from_value(json!({"popularity": 0})).unwrap();
        assert_eq!(artist_facts(&zero), (None, None));
    }

    #[test]
    fn a_video_marks_its_artists_and_links_featured_guests() {
        let conn = conn();
        let mut duet = video(1, 10);
        duet.featured_artist_ids = vec![20];
        let first = ingest(&conn, &[duet.clone()], HarvestContext::Search).unwrap();
        assert_eq!((first.stored, first.new_videos), (1, 1));
        assert_eq!(edge_weight(&conn, 10, 20, "featured"), Some((0, 0.85)));
        assert_eq!(edge_weight(&conn, 20, 10, "featured"), Some((0, 0.85)));
        let main = artist_state::get(&conn, 10).unwrap().unwrap();
        let guest = artist_state::get(&conn, 20).unwrap().unwrap();
        assert!(main.seen_main && !guest.seen_main && guest.seen_featured);
        let again = ingest(&conn, &[duet], HarvestContext::Search).unwrap();
        assert_eq!((again.new_videos, again.edges_added), (0, 0));
    }

    #[test]
    fn a_sparse_sighting_keeps_earlier_facts() {
        let conn = conn();
        let mut rich = video(2, 10);
        rich.popularity = Some(61);
        ingest(&conn, &[rich], HarvestContext::Search).unwrap();
        ingest(
            &conn,
            &[video(2, 10)],
            HarvestContext::List {
                key: "page:videos:x",
            },
        )
        .unwrap();
        let json: String = conn
            .query_row(
                "SELECT item_json FROM video_catalog WHERE tidal_video_id = 2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let stored: VideoCandidate = serde_json::from_str(&json).unwrap();
        assert_eq!(stored.popularity, Some(61));
    }

    #[test]
    fn curated_lists_link_neighbors_once_per_list() {
        let conn = conn();
        let list = [video(1, 10), video(2, 20), video(3, 10), video(4, 30)];
        ingest(&conn, &list, HarvestContext::List { key: "mix:a" }).unwrap();
        assert_eq!(edge_weight(&conn, 10, 20, "colist"), Some((0, 0.4)));
        assert_eq!(edge_weight(&conn, 30, 10, "colist"), Some((0, 0.4)));
        ingest(&conn, &list, HarvestContext::List { key: "mix:a" }).unwrap();
        assert_eq!(edge_weight(&conn, 10, 20, "colist"), Some((0, 0.4)));
        ingest(
            &conn,
            &list[..2],
            HarvestContext::List { key: "playlist:b" },
        )
        .unwrap();
        let (rank, weight) = edge_weight(&conn, 10, 20, "colist").unwrap();
        assert_eq!(rank, 1);
        assert!((weight - 0.5).abs() < 1e-9);
    }

    #[test]
    fn colist_pairs_respect_the_window() {
        let artists: Vec<i64> = (1..=12).collect();
        let pairs = colist_pairs(&artists, 8);
        assert!(pairs.contains(&(1, 9)));
        assert!(!pairs.contains(&(1, 10)));
        assert_eq!(colist_pairs(&[5, 5, -1], 8), Vec::<(i64, i64)>::new());
    }

    #[test]
    fn ingest_waits_for_another_writer_instead_of_failing() {
        let path = std::env::temp_dir().join(format!(
            "noor-harvest-lock-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let open = || {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;")
                .unwrap();
            conn
        };
        let writer = open();
        crate::db::schema::run_migrations(&writer).unwrap();
        let crawler = open();
        // Another writer holds the lock and commits while the harvest waits.
        writer
            .execute_batch(
                "BEGIN IMMEDIATE; INSERT INTO server_config (key, value) VALUES ('busy', '1');",
            )
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            writer.execute_batch("COMMIT").unwrap();
        });
        let summary = ingest(&crawler, &[video(9, 90)], HarvestContext::Search);
        release.join().unwrap();
        drop(crawler);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        assert_eq!(summary.unwrap().stored, 1);
    }

    #[test]
    fn ingest_nests_inside_a_caller_transaction() {
        let conn = conn();
        let tx = conn.unchecked_transaction().unwrap();
        ingest(
            &tx,
            &[video(5, 50)],
            HarvestContext::ArtistPage {
                artist_id: 50,
                name: "Fifty",
            },
        )
        .unwrap();
        tx.commit().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM video_catalog", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn catalog_rows_from_before_the_facts_still_load() {
        let old = r#"{"tidal_id":1,"title":"Song","duration_s":200,"artist_id":10,"artist_name":"A","album_tidal_id":null,"artwork_url":null,"release_year":null}"#;
        let parsed: VideoCandidate = serde_json::from_str(old).unwrap();
        assert!(parsed.featured_artist_ids.is_empty() && parsed.popularity.is_none());
    }
}
