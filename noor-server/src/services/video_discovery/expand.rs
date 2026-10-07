//! Relationship lookups: TIDAL "fans also like" plus Last.fm similar artists,
//! resolved to TIDAL ids and stored as weighted edges.

use std::collections::HashSet;

use anyhow::Result;
use rusqlite::{Connection, params};

use super::source::DiscoverySource;
use super::{artist_state, graph, names};
use crate::db::Database;
use crate::services::video_radio;

pub type WeightedArtist = (i64, String, f64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpandOutcome {
    pub ok: bool,
    pub tidal_calls: usize,
    pub related: usize,
}

/// The id is authoritative: request text never drives a lookup for a
/// different TIDAL artist.
pub fn canonical_artist_name(conn: &Connection, artist_id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT COALESCE(
                (SELECT name FROM artists WHERE tidal_id = ?1 AND name <> '' LIMIT 1),
                (SELECT name FROM video_artist_state WHERE artist_tidal_id = ?1 AND name <> ''),
                (SELECT artist_name FROM video_catalog
                  WHERE artist_tidal_id = ?1 AND artist_name <> '' LIMIT 1))",
            [artist_id],
            |row| row.get::<_, Option<String>>(0),
        )?
        .filter(|name| !name.trim().is_empty() && name.len() <= 120))
}

/// Replace each provider's snapshot only when that provider answered.
pub fn apply_related_refresh(
    conn: &Connection,
    seed_id: i64,
    tidal: Option<&[WeightedArtist]>,
    lastfm: Option<&[WeightedArtist]>,
    genres: Option<&[String]>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for (source, snapshot) in [("tidal", tidal), ("lastfm", lastfm)] {
        let Some(snapshot) = snapshot else {
            continue;
        };
        tx.execute(
            "DELETE FROM video_related_artists WHERE seed_tidal_id = ?1 AND source = ?2",
            params![seed_id, source],
        )?;
        let mut seen = HashSet::new();
        for (rank, (id, name, weight)) in snapshot.iter().enumerate() {
            if *id <= 0 || *id == seed_id || !seen.insert(*id) {
                continue;
            }
            tx.execute(
                "INSERT INTO video_related_artists
                    (seed_tidal_id, related_tidal_id, name, source, rank, weight)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![seed_id, id, name, source, rank as i64, weight],
            )?;
            artist_state::upsert_identity(&tx, *id, name, None, None)?;
        }
    }
    if let Some(genres) = genres {
        video_radio::store_seed_genres(&tx, seed_id, genres)?;
    }
    tx.commit()?;
    graph::mark_dirty();
    Ok(())
}

pub async fn expand<S: DiscoverySource>(
    db: &Database,
    src: &S,
    seed_id: i64,
    station: bool,
) -> Result<ExpandOutcome> {
    let mut tidal_calls = 0;
    let mut name = db.with_conn(|conn| canonical_artist_name(conn, seed_id))?;
    if name.is_none() {
        tidal_calls += 1;
        if let Ok(artist) = src.artist(seed_id).await {
            db.with_conn(|conn| {
                artist_state::upsert_identity(
                    conn,
                    artist.id,
                    &artist.name,
                    artist.popularity,
                    artist.mix_id.as_deref(),
                )
            })?;
            name = Some(artist.name);
        }
    }

    tidal_calls += 1;
    let tidal = src.similar_artists(seed_id).await.ok();
    if let Some(similar) = &tidal {
        db.with_conn(|conn| {
            for artist in similar {
                artist_state::upsert_identity(
                    conn,
                    artist.id,
                    &artist.name,
                    artist.popularity,
                    artist.mix_id.as_deref(),
                )?;
            }
            Ok(())
        })?;
    }

    let (lastfm, tags) = match name.as_deref() {
        Some(name) => (
            src.lastfm_similar(name).await,
            src.lastfm_tags(name).await.ok().flatten(),
        ),
        None => (Ok(None), None),
    };
    let lastfm_failed = lastfm.is_err();
    let mut resolved: Option<Vec<WeightedArtist>> = None;
    if let Ok(Some(similar)) = lastfm {
        let mut out = Vec::new();
        for (rank, (artist_name, score)) in similar.into_iter().enumerate() {
            if rank >= 8 && score.unwrap_or(0.0) < 0.2 {
                continue;
            }
            let weight = graph::lastfm_weight(score, rank);
            if let Some(id) = db.with_conn(|conn| names::find_local(conn, &artist_name))? {
                out.push((id, artist_name, weight));
                continue;
            }
            if !db.with_conn(|conn| names::resolution_due(conn, &artist_name))? {
                continue;
            }
            tidal_calls += 1;
            let Ok(found) = src.search_artists(&artist_name).await else {
                continue;
            };
            let pairs: Vec<(i64, String)> = found.iter().map(|a| (a.id, a.name.clone())).collect();
            match names::best_match(&artist_name, &pairs)
                .and_then(|id| found.iter().find(|a| a.id == id))
            {
                Some(artist) => {
                    db.with_conn(|conn| {
                        artist_state::upsert_identity(
                            conn,
                            artist.id,
                            &artist.name,
                            artist.popularity,
                            artist.mix_id.as_deref(),
                        )
                    })?;
                    out.push((artist.id, artist.name.clone(), weight));
                }
                None => db.with_conn(|conn| names::mark_resolution_failed(conn, &artist_name))?,
            }
        }
        resolved = Some(out);
    }

    let tidal_edges: Option<Vec<WeightedArtist>> = tidal.map(|similar| {
        similar
            .into_iter()
            .enumerate()
            .map(|(rank, artist)| (artist.id, artist.name, graph::tidal_weight(rank)))
            .collect()
    });
    let ok = tidal_edges.is_some() && !lastfm_failed;
    let related = tidal_edges.as_ref().map_or(0, Vec::len) + resolved.as_ref().map_or(0, Vec::len);
    db.with_conn(|conn| {
        apply_related_refresh(
            conn,
            seed_id,
            tidal_edges.as_deref(),
            resolved.as_deref(),
            tags.as_deref(),
        )?;
        artist_state::record_expand(conn, seed_id, ok, station)
    })?;
    Ok(ExpandOutcome {
        ok,
        tidal_calls,
        related,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::services::video_discovery::source::fake::{FakeSource, artist};

    fn db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        db
    }

    fn edge(db: &Database, to: i64, source: &str) -> Option<(i64, f64)> {
        db.with_conn(|conn| {
            Ok(rusqlite::OptionalExtension::optional(conn.query_row(
                "SELECT rank, weight FROM video_related_artists
                  WHERE seed_tidal_id = 10 AND related_tidal_id = ?1 AND source = ?2",
                params![to, source],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ))?)
        })
        .unwrap()
    }

    #[tokio::test]
    async fn expansion_stores_weighted_edges_and_resolves_names_locally_first() {
        let db = db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (tidal_id, name) VALUES (10, 'Seed')",
                [],
            )?;
            artist_state::upsert_identity(conn, 7, "Beyonc\u{e9}", Some(90), None)
        })
        .unwrap();
        let src = FakeSource {
            similar: HashMap::from([(
                10,
                vec![artist(20, "Popular", Some(80)), artist(30, "Second", None)],
            )]),
            lastfm: Some(HashMap::from([(
                "Seed".to_string(),
                vec![
                    ("Beyonce".to_string(), Some(0.9)),
                    ("Ghost Band".to_string(), Some(0.5)),
                    ("Searched".to_string(), Some(0.4)),
                ],
            )])),
            search: HashMap::from([(
                "Searched".to_string(),
                vec![artist(77, "Searched", Some(40))],
            )]),
            ..Default::default()
        };
        let outcome = expand(&db, &src, 10, false).await.unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.related, 4);
        assert_eq!(
            src.calls(),
            vec![
                "similar:10",
                "search_artists:Ghost Band",
                "search_artists:Searched"
            ]
        );
        assert_eq!(
            outcome.tidal_calls, 3,
            "similar + two searches; Beyonce resolved from the ledger"
        );
        assert_eq!(edge(&db, 20, "tidal"), Some((0, 1.0)));
        assert!((edge(&db, 30, "tidal").unwrap().1 - 0.96).abs() < 1e-9);
        assert_eq!(edge(&db, 7, "lastfm"), Some((0, 0.9)));
        assert!((edge(&db, 77, "lastfm").unwrap().1 - 0.4).abs() < 1e-9);
        let (popularity, ghost_failed, days): (Option<i32>, bool, f64) = db
            .with_conn(|conn| {
                Ok((
                    conn.query_row("SELECT popularity FROM video_artist_state WHERE artist_tidal_id = 20", [], |r| r.get(0))?,
                    !names::resolution_due(conn, "Ghost Band")?,
                    conn.query_row(
                        "SELECT julianday(next_expand_at) - julianday('now') FROM video_artist_state WHERE artist_tidal_id = 10",
                        [],
                        |r| r.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(popularity, Some(80));
        assert!(ghost_failed);
        assert!((days - 30.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn a_failed_tidal_lookup_keeps_lastfm_and_retries_within_the_hour() {
        let db = db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (tidal_id, name) VALUES (10, 'Seed')",
                [],
            )?;
            artist_state::upsert_identity(conn, 7, "Neighbor", None, None)
        })
        .unwrap();
        let src = FakeSource {
            failing_similar: HashSet::from([10]),
            lastfm: Some(HashMap::from([(
                "Seed".to_string(),
                vec![("Neighbor".to_string(), Some(0.7))],
            )])),
            ..Default::default()
        };
        let outcome = expand(&db, &src, 10, true).await.unwrap();
        assert!(!outcome.ok);
        assert!(edge(&db, 7, "lastfm").is_some());
        let hours: f64 = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT (julianday(next_expand_at) - julianday('now')) * 24 FROM video_artist_state WHERE artist_tidal_id = 10",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert!((hours - 1.0).abs() < 0.05);
    }

    #[test]
    fn provider_snapshots_replace_only_successful_sources() {
        let db = db();
        db.with_conn(|conn| {
            let count = |source: &str| -> i64 {
                conn.query_row(
                    "SELECT COUNT(*) FROM video_related_artists WHERE seed_tidal_id = 10 AND source = ?1",
                    [source],
                    |r| r.get(0),
                )
                .unwrap()
            };
            apply_related_refresh(
                conn,
                10,
                Some(&[(20, "Old TIDAL".into(), 1.0), (30, "Shared".into(), 0.96)]),
                Some(&[(30, "Shared".into(), 0.9), (40, "Old Last.fm".into(), 0.8)]),
                None,
            )?;
            apply_related_refresh(conn, 10, Some(&[(30, "Shared".into(), 1.0)]), None, None)?;
            assert_eq!((count("tidal"), count("lastfm")), (1, 2));
            apply_related_refresh(conn, 10, None, Some(&[]), None)?;
            assert_eq!((count("tidal"), count("lastfm")), (1, 0));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn canonical_artist_name_comes_from_the_id() {
        let db = db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (tidal_id, name) VALUES (10, 'Green Day')",
                [],
            )?;
            assert_eq!(
                canonical_artist_name(conn, 10)?.as_deref(),
                Some("Green Day")
            );
            assert_eq!(canonical_artist_name(conn, 11)?, None);
            Ok(())
        })
        .unwrap();
    }
}
