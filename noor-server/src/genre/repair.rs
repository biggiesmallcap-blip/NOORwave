//! One-time repair for genre rows the old fuzzy matcher wrote by swapping a
//! word ("new wave" -> New Age, "chicago house" -> Chicago Blues). The matcher
//! no longer does that (`mappings::words_align`); this removes what it wrote.
//!
//! Raw tags were never stored per track, so the repair works from what is:
//! - Last.fm: a collision-genre row survives only while the artist's cached
//!   Last.fm tags still resolve to that genre. Otherwise it is deleted and the
//!   track re-queued (`lastfm_checked` cleared) so the next enrichment pass
//!   re-reads its own track tags with the fixed matcher.
//! - MusicBrainz: raw tags are not cached, so tracks holding a collision genre
//!   are re-queued (`musicbrainz_checked` cleared); a successful re-fetch
//!   replaces their MusicBrainz rows (`musicbrainz::write_genres`).
//!
//! Runs once per install; the marker row is written in the same transaction.

use crate::genre::mappings::GenreCatalog;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use std::collections::{HashMap, HashSet};

pub const REPAIR_MARKER: &str = "genre_word_swap_repair_v1";

/// Genres the old matcher produced by swapping a word, measured over 4,832
/// cached Last.fm tags (radio audit, 2026-10-13). Parents reached by dropping
/// a word ("ambient dub" -> Ambient) are near enough and not listed.
pub const COLLISION_GENRES: &[&str] = &[
    "New Age",
    "Chicago Blues",
    "Psychedelic Rock",
    "Melodic Techno",
    "Progressive Trance",
    "Americana",
    "Spiritual Jazz",
    "Tropical",
    "Skate Punk",
    "Latin Rock",
    "Organic House",
    "K-Pop",
    "UK Garage",
    "Contemporary Folk",
    "Piano Solo",
    "Latin Pop",
    "Future Bass",
    "Future House",
    "Dirty South",
    "Post-Hardcore",
    "Lo-Fi Hip-Hop",
    "Modernist",
    "Asian",
];

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RepairStats {
    pub lastfm_rows_deleted: usize,
    pub lastfm_tracks_requeued: usize,
    pub musicbrainz_tracks_requeued: usize,
}

/// Run the repair unless this install already has. `Ok(None)` = already done.
pub fn repair_word_swaps_once(
    conn: &Connection,
    catalog: &GenreCatalog,
) -> anyhow::Result<Option<RepairStats>> {
    let done: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM server_config WHERE key = ?1)",
        params![REPAIR_MARKER],
        |row| row.get(0),
    )?;
    if done {
        return Ok(None);
    }
    let tx = conn.unchecked_transaction()?;
    let stats = repair_word_swaps(&tx, catalog)?;
    if stats != RepairStats::default() {
        crate::services::radio_similarity::bump_change_gen(&tx)?;
    }
    tx.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, '1')",
        params![REPAIR_MARKER],
    )?;
    tx.commit()?;
    Ok(Some(stats))
}

fn repair_word_swaps(conn: &Connection, catalog: &GenreCatalog) -> rusqlite::Result<RepairStats> {
    let placeholders = vec!["?"; COLLISION_GENRES.len()].join(", ");
    let mut stats = RepairStats::default();

    let lastfm_rows: Vec<(i64, i64, String, String)> = conn
        .prepare(&format!(
            "SELECT tg.track_id, tg.genre_id, g.name, COALESCE(ar.name, '')
             FROM track_genres tg
             JOIN genres g ON g.id = tg.genre_id
             JOIN tracks t ON t.id = tg.track_id
             LEFT JOIN artists ar ON ar.id = t.artist_id
             WHERE tg.source = 'lastfm' AND g.name IN ({placeholders})"
        ))?
        .query_map(params_from_iter(COLLISION_GENRES.iter()), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut supported_by_artist: HashMap<String, HashSet<String>> = HashMap::new();
    let mut requeue: HashSet<i64> = HashSet::new();
    for (track_id, genre_id, genre, artist) in lastfm_rows {
        if !supported_by_artist.contains_key(&artist) {
            let genres = artist_cache_genres(conn, catalog, &artist)?;
            supported_by_artist.insert(artist.clone(), genres);
        }
        if supported_by_artist[&artist].contains(&genre) {
            continue;
        }
        stats.lastfm_rows_deleted += conn.execute(
            "DELETE FROM track_genres WHERE track_id = ?1 AND genre_id = ?2 AND source = 'lastfm'",
            params![track_id, genre_id],
        )?;
        requeue.insert(track_id);
    }
    for track_id in &requeue {
        conn.execute(
            "DELETE FROM lastfm_checked WHERE track_id = ?1",
            params![track_id],
        )?;
    }
    stats.lastfm_tracks_requeued = requeue.len();

    stats.musicbrainz_tracks_requeued = conn.execute(
        &format!(
            "DELETE FROM musicbrainz_checked WHERE track_id IN (
                 SELECT tg.track_id FROM track_genres tg
                 JOIN genres g ON g.id = tg.genre_id
                 WHERE tg.source = 'musicbrainz' AND g.name IN ({placeholders}))"
        ),
        params_from_iter(COLLISION_GENRES.iter()),
    )?;
    Ok(stats)
}

/// Canonical genres the artist's cached Last.fm tags resolve to today.
fn artist_cache_genres(
    conn: &Connection,
    catalog: &GenreCatalog,
    artist: &str,
) -> rusqlite::Result<HashSet<String>> {
    let tags_json: Option<String> = conn
        .query_row(
            "SELECT tags_json FROM lastfm_artist_cache WHERE artist_name = ?1",
            params![artist],
            |row| row.get(0),
        )
        .optional()?;
    let tags: Vec<(String, Option<u32>)> = tags_json
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    Ok(tags
        .iter()
        .filter_map(|(name, _)| catalog.resolve_single(name))
        .map(|m| m.canonical_name)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        db.seed_genres_from_taxonomy().unwrap();
        db
    }

    fn catalog() -> &'static GenreCatalog {
        crate::genre::builder::embedded_builder().catalog()
    }

    fn add_track(conn: &Connection, id: i64, artist_id: i64, artist: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO artists (id, name) VALUES (?1, ?2)",
            params![artist_id, artist],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id) VALUES (?1, ?2, ?3)",
            params![id, format!("track {id}"), artist_id],
        )
        .unwrap();
    }

    fn tag(conn: &Connection, track_id: i64, genre: &str, source: &str) {
        conn.execute(
            "INSERT INTO track_genres (track_id, genre_id, source, confidence)
             VALUES (?1, (SELECT id FROM genres WHERE name = ?2), ?3, 0.5)",
            params![track_id, genre, source],
        )
        .unwrap();
    }

    fn cache_artist(conn: &Connection, artist: &str, tags_json: &str) {
        conn.execute(
            "INSERT INTO lastfm_artist_cache (artist_name, tags_json) VALUES (?1, ?2)",
            params![artist, tags_json],
        )
        .unwrap();
    }

    fn mark(conn: &Connection, table: &str, track_id: i64) {
        conn.execute(
            &format!("INSERT INTO {table} (track_id) VALUES (?1)"),
            params![track_id],
        )
        .unwrap();
    }

    fn has_genre(conn: &Connection, track_id: i64, genre: &str) -> bool {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
             WHERE tg.track_id = ?1 AND g.name = ?2)",
            params![track_id, genre],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn is_marked(conn: &Connection, table: &str, track_id: i64) -> bool {
        conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE track_id = ?1)"),
            params![track_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn lastfm_collision_row_without_artist_support_is_removed_and_requeued() {
        let db = db();
        db.with_conn(|c| {
            add_track(c, 1, 1, "The Smiths");
            cache_artist(c, "The Smiths", r#"[["new wave",100],["post-punk",80]]"#);
            tag(c, 1, "New Age", "lastfm");
            tag(c, 1, "Post-Punk", "lastfm");
            mark(c, "lastfm_checked", 1);
            let stats = repair_word_swaps_once(c, catalog())?.expect("first run");
            assert_eq!(stats.lastfm_rows_deleted, 1);
            assert_eq!(stats.lastfm_tracks_requeued, 1);
            assert!(!has_genre(c, 1, "New Age"));
            assert!(has_genre(c, 1, "Post-Punk"));
            assert!(!is_marked(c, "lastfm_checked", 1));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn lastfm_collision_row_backed_by_artist_tags_is_kept() {
        let db = db();
        db.with_conn(|c| {
            add_track(c, 2, 2, "Ram Dass");
            cache_artist(c, "Ram Dass", r#"[["new age",100],["spiritual",40]]"#);
            tag(c, 2, "New Age", "lastfm");
            mark(c, "lastfm_checked", 2);
            let stats = repair_word_swaps_once(c, catalog())?.expect("first run");
            assert_eq!(stats.lastfm_rows_deleted, 0);
            assert!(has_genre(c, 2, "New Age"));
            assert!(is_marked(c, "lastfm_checked", 2));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn musicbrainz_collision_tracks_are_requeued_not_deleted() {
        let db = db();
        db.with_conn(|c| {
            add_track(c, 3, 3, "The Killers");
            add_track(c, 4, 4, "Enya");
            add_track(c, 5, 5, "Depeche Mode");
            tag(c, 3, "New Age", "musicbrainz");
            tag(c, 4, "New Age", "musicbrainz");
            tag(c, 5, "Synthpop", "musicbrainz");
            for id in [3, 4, 5] {
                mark(c, "musicbrainz_checked", id);
            }
            let stats = repair_word_swaps_once(c, catalog())?.expect("first run");
            assert_eq!(stats.musicbrainz_tracks_requeued, 2);
            // Rows stay until a successful re-fetch replaces them.
            assert!(has_genre(c, 3, "New Age"));
            assert!(!is_marked(c, "musicbrainz_checked", 3));
            assert!(!is_marked(c, "musicbrainz_checked", 4));
            assert!(is_marked(c, "musicbrainz_checked", 5));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn repair_runs_once_and_marks_the_engine_index_dirty() {
        let db = db();
        db.with_conn(|c| {
            add_track(c, 6, 6, "Duran Duran");
            cache_artist(c, "Duran Duran", r#"[["new wave",100]]"#);
            tag(c, 6, "New Age", "lastfm");
            assert!(repair_word_swaps_once(c, catalog())?.is_some());
            assert!(repair_word_swaps_once(c, catalog())?.is_none());
            let generation: String = c.query_row(
                "SELECT value FROM server_config WHERE key = 'radio_similarity_change_gen'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(generation, "1");
            Ok(())
        })
        .unwrap();
    }
}
