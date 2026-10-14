//! Canonical genres for an artist, by name, for tracks that carry none of their
//! own: Last.fm externals (which never get `track_genres` rows) and library
//! tracks the tagger has not reached.
//!
//! Sources, best first:
//!   1. the artist's own library tracks (`track_genres`, already canonical),
//!   2. the video catalogue's propagated artist genres
//!      (`video_artist_facets.genres_json`) when that table exists,
//!   3. TIDAL's seed genres for the artist (`video_seed_genres`).
//!
//! Free-form tags from 2 and 3 go through the genre resolver, so ranking only
//! ever compares canonical taxonomy names and a tag like "female vocalists"
//! never reaches the genre multiplier.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params_from_iter};

use crate::genre::builder::collect_clear_genres;
use crate::services::video_discovery::names::name_key;

/// At most this many genres per artist, strongest first.
pub const MAX_GENRES: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct ArtistGenres {
    pub genres: Vec<String>,
    /// "library", "video_facets" or "tidal_artist".
    pub source: &'static str,
}

/// Genres keyed by `name_key(artist)` for every artist name a source covers.
pub fn lookup(conn: &Connection, artist_names: &[String]) -> Result<HashMap<String, ArtistGenres>> {
    let keys: HashSet<String> = artist_names
        .iter()
        .map(|name| name_key(name))
        .filter(|key| !key.is_empty())
        .collect();
    let mut found: HashMap<String, ArtistGenres> = HashMap::new();
    if keys.is_empty() {
        return Ok(found);
    }

    for (key, genres) in library_genres(conn, artist_names, &keys)? {
        found.insert(
            key,
            ArtistGenres {
                genres,
                source: "library",
            },
        );
    }

    let missing: Vec<String> = keys
        .iter()
        .filter(|k| !found.contains_key(*k))
        .cloned()
        .collect();
    if !missing.is_empty() && table_exists(conn, "video_artist_facets")? {
        for (key, genres) in facet_genres(conn, &missing)? {
            found.insert(
                key,
                ArtistGenres {
                    genres,
                    source: "video_facets",
                },
            );
        }
    }

    let missing: Vec<String> = keys
        .iter()
        .filter(|k| !found.contains_key(*k))
        .cloned()
        .collect();
    if !missing.is_empty() && table_exists(conn, "video_seed_genres")? {
        for (key, genres) in seed_genres(conn, &missing)? {
            found.insert(
                key,
                ArtistGenres {
                    genres,
                    source: "tidal_artist",
                },
            );
        }
    }

    Ok(found)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

/// Library genres by share of the artist's tagged tracks. Matched on the
/// lowercased name in SQL, then confirmed on `name_key`.
fn library_genres(
    conn: &Connection,
    artist_names: &[String],
    keys: &HashSet<String>,
) -> Result<HashMap<String, Vec<String>>> {
    let lowered: Vec<String> = artist_names
        .iter()
        .map(|name| name.trim().to_lowercase())
        .filter(|name| !name.is_empty())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if lowered.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT a.name, g.name, COUNT(*) AS n
         FROM artists a
         JOIN tracks t ON t.artist_id = a.id
         JOIN track_genres tg ON tg.track_id = t.id
         JOIN genres g ON g.id = tg.genre_id
         WHERE lower(a.name) IN ({})
         GROUP BY a.id, g.id",
        placeholders(lowered.len())
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(lowered.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut counts: HashMap<String, BTreeMap<String, i64>> = HashMap::new();
    for row in rows {
        let (artist, genre, n) = row?;
        let key = name_key(&artist);
        if keys.contains(&key) {
            *counts.entry(key).or_default().entry(genre).or_insert(0) += n;
        }
    }
    Ok(counts
        .into_iter()
        .map(|(key, by_genre)| {
            let mut ranked: Vec<(String, i64)> = by_genre.into_iter().collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            (
                key,
                ranked
                    .into_iter()
                    .take(MAX_GENRES)
                    .map(|(g, _)| g)
                    .collect(),
            )
        })
        .collect())
}

/// Propagated artist genres from the video catalogue intelligence build.
fn facet_genres(conn: &Connection, keys: &[String]) -> Result<HashMap<String, Vec<String>>> {
    let sql = format!(
        "SELECT s.name_key, f.genres_json
         FROM video_artist_state s
         JOIN video_artist_facets f ON f.artist_tidal_id = s.artist_tidal_id
         WHERE s.name_key IN ({}) AND f.genres_json <> '[]'
         ORDER BY s.popularity DESC",
        placeholders(keys.len())
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(keys.iter()), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let (key, raw) = row?;
        if out.contains_key(&key) {
            continue; // most popular artist with this name wins
        }
        let tags: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
        let genres = canonical(&tags);
        if !genres.is_empty() {
            out.insert(key, genres);
        }
    }
    Ok(out)
}

/// TIDAL's genres for the artist, in TIDAL's rank order.
fn seed_genres(conn: &Connection, keys: &[String]) -> Result<HashMap<String, Vec<String>>> {
    let sql = format!(
        "SELECT s.name_key, s.artist_tidal_id, g.genre_name
         FROM video_artist_state s
         JOIN video_seed_genres g ON g.seed_tidal_id = s.artist_tidal_id
         WHERE s.name_key IN ({})
         ORDER BY s.popularity DESC, s.artist_tidal_id, g.rank",
        placeholders(keys.len())
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(keys.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    // One artist per name: the first (most popular) TIDAL id seen for a key.
    let mut chosen: HashMap<String, i64> = HashMap::new();
    let mut tags: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let (key, artist, tag) = row?;
        let owner = *chosen.entry(key.clone()).or_insert(artist);
        if owner == artist {
            tags.entry(key).or_default().push(tag);
        }
    }
    Ok(tags
        .into_iter()
        .filter_map(|(key, tags)| {
            let genres = canonical(&tags);
            (!genres.is_empty()).then_some((key, genres))
        })
        .collect())
}

/// Resolve free-form tags to canonical genre names, keeping the tags' order.
fn canonical(tags: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for tag in tags {
        for genre in collect_clear_genres([tag.as_str()]) {
            if seen.insert(genre.clone()) {
                out.push(genre);
            }
            if out.len() == MAX_GENRES {
                return out;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> crate::db::Database {
        let db = crate::db::Database::open(":memory:").expect("in-memory db");
        db.run_migrations().expect("run migrations");
        db
    }

    #[test]
    fn library_genres_come_first_and_rank_by_track_share() {
        let db = test_db();
        db.with_conn(|conn| {
            conn.execute_batch(
                "INSERT INTO artists (id, name) VALUES (1, 'Massive Attack');
                 INSERT INTO tracks (id, title, artist_id) VALUES (10, 'Teardrop', 1), (11, 'Angel', 1);
                 INSERT OR IGNORE INTO genres (name, slug) VALUES
                     ('Trip-Hop', 'trip-hop'), ('Electronic', 'electronic');
                 INSERT INTO track_genres (track_id, genre_id, source, confidence)
                     SELECT 10, id, 'test', 1.0 FROM genres WHERE name = 'Trip-Hop'
                     UNION ALL SELECT 11, id, 'test', 1.0 FROM genres WHERE name = 'Trip-Hop'
                     UNION ALL SELECT 11, id, 'test', 1.0 FROM genres WHERE name = 'Electronic';",
            )?;
            let found = lookup(conn, &["massive attack".to_string()])?;
            let hit = &found["massive attack"];
            assert_eq!(hit.source, "library");
            assert_eq!(hit.genres, vec!["Trip-Hop".to_string(), "Electronic".to_string()]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn tidal_artist_genres_resolve_to_canonical_names_and_skip_non_genre_tags() {
        let db = test_db();
        db.with_conn(|conn| {
            conn.execute_batch(
                "INSERT INTO video_artist_state (artist_tidal_id, name, name_key, popularity)
                     VALUES (500, 'Burial', 'burial', 40), (501, 'Burial', 'burial', 5);
                 INSERT INTO video_seed_genres (seed_tidal_id, genre_name, rank) VALUES
                     (500, 'female vocalists', 0), (500, 'dubstep', 1), (501, 'heavy metal', 0);",
            )?;
            let found = lookup(conn, &["Burial".to_string()])?;
            let hit = &found["burial"];
            assert_eq!(hit.source, "tidal_artist");
            assert_eq!(
                hit.genres,
                vec!["Dubstep".to_string()],
                "most popular Burial only, tags resolved"
            );
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn unknown_artists_and_missing_tables_are_not_errors() {
        let db = test_db();
        db.with_conn(|conn| {
            assert!(lookup(conn, &["Nobody At All".to_string()])?.is_empty());
            assert!(lookup(conn, &[])?.is_empty());
            Ok(())
        })
        .unwrap();
    }
}
