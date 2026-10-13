//! Builds a station from the listener's chosen source (Settings > Playback >
//! Radio). A non-default source that has too little for the seed (no TIDAL
//! session, no Last.fm key, no match) falls back to the balanced station, so
//! the choice never leaves a station empty. Starts and top-ups both come here.

use super::*;
use crate::metadata::lastfm::LastFmClient;
use crate::services::radio::{
    LastFmSimilarCache, RadioBlend, RadioCandidate, RadioQueue, RadioSeed,
    TIDAL_MIX_FALLBACK_MIN_PICKS,
};
use crate::services::radio_config::StationSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StationSeed {
    Track(i64),
    Album(i64),
    Artist(i64),
}

/// Everything a station build reads besides the seed.
pub(crate) struct StationContext<'a> {
    pub(crate) state: &'a SharedState,
    pub(crate) db: &'a crate::db::Database,
    pub(crate) lastfm: Option<&'a LastFmClient>,
    pub(crate) cache: &'a LastFmSimilarCache,
}

pub(crate) async fn build_station(
    ctx: &StationContext<'_>,
    seed: StationSeed,
    blend: RadioBlend,
    limit: usize,
    exclude: &[i64],
    source: StationSource,
) -> anyhow::Result<RadioQueue> {
    if source != StationSource::Balanced {
        match source_station(ctx, seed, blend, limit, exclude, source).await {
            Ok(queue) if queue.tracks.len() >= TIDAL_MIX_FALLBACK_MIN_PICKS => return Ok(queue),
            Ok(queue) => info!(
                ?source,
                picks = queue.tracks.len(),
                "radio: the chosen source had too few picks; balanced station instead"
            ),
            Err(error) => warn!(
                ?source,
                %error,
                "radio: the chosen source failed; balanced station instead"
            ),
        }
    }
    balanced_station(ctx, seed, blend, limit, exclude).await
}

/// Today's station: learned neighbors, Last.fm and the radio index, with
/// TIDAL's mix filling in for a seed with too little of its own.
async fn balanced_station(
    ctx: &StationContext<'_>,
    seed: StationSeed,
    blend: RadioBlend,
    limit: usize,
    exclude: &[i64],
) -> anyhow::Result<RadioQueue> {
    use crate::services::radio::{orchestrate_album, orchestrate_artist, orchestrate_song};
    let (db, lastfm, cache) = (ctx.db, ctx.lastfm, Some(ctx.cache));
    match seed {
        StationSeed::Track(id) => {
            let mut queue = orchestrate_song(db, lastfm, cache, id, blend, limit, exclude).await?;
            add_tidal_mix_fallback(
                ctx.state,
                db,
                TidalMixSeed::Track(id),
                &mut queue.tracks,
                limit,
            )
            .await;
            Ok(queue)
        }
        StationSeed::Album(id) => {
            orchestrate_album(db, lastfm, cache, id, blend, limit, exclude).await
        }
        StationSeed::Artist(id) => {
            let mut queue =
                orchestrate_artist(db, lastfm, cache, id, blend, limit, exclude).await?;
            add_tidal_mix_fallback(
                ctx.state,
                db,
                TidalMixSeed::Artist(id),
                &mut queue.tracks,
                limit,
            )
            .await;
            Ok(queue)
        }
    }
}

async fn source_station(
    ctx: &StationContext<'_>,
    seed: StationSeed,
    blend: RadioBlend,
    limit: usize,
    exclude: &[i64],
    source: StationSource,
) -> anyhow::Result<RadioQueue> {
    let seed_meta = station_seed(ctx.db, seed)?;
    let seed_tracks = ctx.db.with_conn(move |conn| match seed {
        StationSeed::Track(id) => Ok(vec![id]),
        StationSeed::Album(id) => crate::services::radio::album_seed_track_ids(conn, id),
        StationSeed::Artist(id) => crate::services::radio::artist_seed_track_ids(conn, id),
    })?;

    let mut picks: Vec<RadioCandidate> = Vec::new();
    if let (StationSource::Tidal, StationSeed::Artist(id)) = (source, seed) {
        // TIDAL has a radio for the artist itself.
        picks = tidal_picks(ctx, TidalMixSeed::Artist(id), limit).await;
    } else {
        for &track_id in &seed_tracks {
            let seed_picks = match source {
                StationSource::Tidal => {
                    tidal_picks(ctx, TidalMixSeed::Track(track_id), limit).await
                }
                StationSource::Lastfm => lastfm_picks(ctx, track_id, limit).await,
                StationSource::Hybrid => {
                    let tidal = tidal_picks(ctx, TidalMixSeed::Track(track_id), limit).await;
                    let lastfm = lastfm_picks(ctx, track_id, limit).await;
                    crate::services::radio::hybrid::hybrid_station(
                        ctx.db, track_id, tidal, lastfm, blend, limit,
                    )
                    .await
                }
                StationSource::Balanced => Vec::new(),
            };
            picks.extend(seed_picks);
        }
    }

    let exclude: HashSet<i64> = exclude.iter().chain(&seed_tracks).copied().collect();
    let picks: Vec<RadioCandidate> = picks
        .into_iter()
        .filter(|pick| !(pick.track_id > 0 && exclude.contains(&pick.track_id)))
        .collect();
    let tracks = if seed_tracks.len() > 1 {
        crate::services::radio::select_across_seeds(ctx.db, picks, blend, limit)
    } else {
        picks.into_iter().take(limit).collect()
    };
    Ok(RadioQueue {
        session_id: crate::services::radio::new_session_id(),
        blend_used: blend,
        seed: seed_meta,
        tracks,
    })
}

async fn tidal_picks(
    ctx: &StationContext<'_>,
    seed: TidalMixSeed,
    limit: usize,
) -> Vec<RadioCandidate> {
    let Ok((mix, seed_tidal_id)) = fetch_seed_tidal_mix(ctx.state, ctx.db, seed).await else {
        return Vec::new();
    };
    ctx.db
        .with_conn(move |conn| {
            Ok(crate::services::radio::tidal_mix_candidates(
                conn,
                mix,
                seed_tidal_id,
                &[],
                limit,
            ))
        })
        .unwrap_or_default()
}

async fn lastfm_picks(
    ctx: &StationContext<'_>,
    track_id: i64,
    limit: usize,
) -> Vec<RadioCandidate> {
    let Some(client) = ctx.lastfm else {
        return Vec::new();
    };
    let Some(track) = ctx
        .db
        .with_conn(move |conn| crate::playback::queue::get_track_by_id(conn, track_id))
        .ok()
        .flatten()
    else {
        return Vec::new();
    };
    let Some(artist) = track.artist_name.as_deref().filter(|a| !a.is_empty()) else {
        return Vec::new();
    };
    crate::services::radio::lastfm_similar_candidates(
        client,
        Some(ctx.cache),
        artist,
        &track.title,
        limit,
    )
    .await
}

/// Seed details for the response and the queue header.
fn station_seed(db: &crate::db::Database, seed: StationSeed) -> anyhow::Result<RadioSeed> {
    db.with_conn(move |conn| {
        let (kind, title, artist_name): (&'static str, Option<String>, Option<String>) = match seed
        {
            StationSeed::Track(id) => {
                let track = crate::playback::queue::get_track_by_id(conn, id)?;
                (
                    "track",
                    track.as_ref().map(|t| t.title.clone()),
                    track.and_then(|t| t.artist_name),
                )
            }
            StationSeed::Album(id) => {
                let row = conn
                    .query_row(
                        "SELECT al.title, ar.name FROM albums al
                         LEFT JOIN artists ar ON ar.id = al.artist_id WHERE al.id = ?1",
                        [id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                    )
                    .optional()?;
                (
                    "album",
                    row.as_ref().map(|r| r.0.clone()),
                    row.and_then(|r| r.1),
                )
            }
            StationSeed::Artist(id) => {
                let name: Option<String> = conn
                    .query_row("SELECT name FROM artists WHERE id = ?1", [id], |row| {
                        row.get(0)
                    })
                    .optional()?;
                ("artist", name.clone(), name)
            }
        };
        let (track_id, album_id, artist_id) = match seed {
            StationSeed::Track(id) => (Some(id), None, None),
            StationSeed::Album(id) => (None, Some(id), None),
            StationSeed::Artist(id) => (None, None, Some(id)),
        };
        Ok(RadioSeed {
            kind,
            track_id,
            album_id,
            artist_id,
            title: title.unwrap_or_else(|| format!("{kind} seed")),
            artist_name,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::routes::tests::{fresh_migrated_db, fresh_test_state};

    fn seed_library(db: &crate::db::Database) {
        db.with_conn(|conn| {
            conn.execute_batch(
                "INSERT INTO artists (id, name) VALUES (1, 'Ram Dass'), (2, 'Ashana');
                 INSERT INTO tracks (id, title, artist_id, play_count) VALUES (1, 'The Water Poem', 1, 3);",
            )?;
            let model = queries::create_embedding_model(
                conn,
                "discovery-fusion-v2:station",
                queries::DISCOVERY_ENGINE_V2_FAMILY,
                8,
                "ready",
                None,
            )?;
            queries::activate_embedding_model(conn, model.id)?;
            for id in 10..22 {
                conn.execute(
                    "INSERT INTO artists (id, name) VALUES (?1, ?2)",
                    rusqlite::params![id, format!("Artist {id}")],
                )?;
                conn.execute(
                    "INSERT INTO tracks (id, title, artist_id) VALUES (?1, ?2, ?1)",
                    rusqlite::params![id, format!("Neighbor {id}")],
                )?;
                conn.execute(
                    "INSERT INTO track_neighbors (track_id, neighbor_track_id, model_id, rank, score)
                     VALUES (1, ?1, ?2, ?3, 0.9)",
                    rusqlite::params![id, model.id, id - 9],
                )?;
            }
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn a_source_with_nothing_falls_back_to_the_balanced_station() {
        let db = fresh_migrated_db();
        seed_library(&db);
        let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db.clone())));
        let cache = crate::services::radio::new_lastfm_similar_cache();
        let ctx = StationContext {
            state: &state,
            db: &db,
            lastfm: None,
            cache: &cache,
        };
        // No TIDAL session and no Last.fm key: every chosen source is empty.
        for source in [
            StationSource::Tidal,
            StationSource::Lastfm,
            StationSource::Hybrid,
        ] {
            let queue = build_station(
                &ctx,
                StationSeed::Track(1),
                RadioBlend::Mixed,
                20,
                &[],
                source,
            )
            .await
            .expect("station");
            assert_eq!(
                queue.tracks.len(),
                12,
                "{source:?} fell back to the learned neighbors"
            );
            assert!(
                queue.tracks.iter().all(|c| c.track_id != 1),
                "seed left out"
            );
            assert_eq!(queue.seed.kind, "track");
        }
    }

    #[tokio::test]
    async fn radio_settings_default_save_and_reject_junk() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            fresh_migrated_db(),
        )));
        let call = |method: &'static str, body: Option<&'static str>| {
            let state = state.clone();
            async move {
                let request = Request::builder()
                    .method(method)
                    .uri("/api/radio/settings")
                    .header("content-type", "application/json")
                    .body(body.map(Body::from).unwrap_or_else(Body::empty))
                    .unwrap();
                let response = api_routes(state).oneshot(request).await.unwrap();
                let status = response.status();
                let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                (
                    status,
                    serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
                )
            }
        };

        let (status, body) = call("GET", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({ "source": "balanced", "blend": "mixed" }));

        let (status, _) = call("PUT", Some(r#"{"source":"tidal","blend":"familiar"}"#)).await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = call("GET", None).await;
        assert_eq!(body, json!({ "source": "tidal", "blend": "familiar" }));

        let (status, _) = call("PUT", Some(r#"{"source":"spotify","blend":"familiar"}"#)).await;
        assert!(
            status.is_client_error(),
            "unknown source rejected, got {status}"
        );
        let (_, body) = call("GET", None).await;
        assert_eq!(body["source"], "tidal", "a rejected save changes nothing");
    }

    #[test]
    fn station_seed_names_each_kind() {
        let db = fresh_migrated_db();
        seed_library(&db);
        let track = station_seed(&db, StationSeed::Track(1)).unwrap();
        assert_eq!(
            (track.title.as_str(), track.artist_name.as_deref()),
            ("The Water Poem", Some("Ram Dass"))
        );
        let artist = station_seed(&db, StationSeed::Artist(1)).unwrap();
        assert_eq!((artist.kind, artist.title.as_str()), ("artist", "Ram Dass"));
    }
}
