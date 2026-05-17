//! Public Spotify stats via anonymous partner-GraphQL.
//!
//! Compiled in via the `spotify-public` cargo feature (default on). The
//! module is responsible for the full cache-miss path: token mint -> hash
//! discovery -> ISRC search -> playcount fetch -> artist overview ->
//! writeback to the SQLite cache tables. Read-side TTL policy lives in
//! [`cache`], the HTTP transport lives in [`client`].
//!
//! Schemas are private/undocumented; expect occasional breakage. The hash
//! constants in [`hashes`] and the TOTP secret are baked in; when a request
//! returns `PersistedQueryNotFound`, the client refreshes them from the
//! live web-player JS bundle (see [`hashes::refresh_from_js`]).
//!
//! ## Public surface
//!
//! - [`fetch_album_playcounts`] - populate `spotify_track_stats` for every
//!   ISRC in `seeds` that the cache is missing or considers stale.
//! - [`fetch_artist_stats`] - resolve a Tidal artist id to a Spotify artist
//!   id (via `spotify_artist_map`), then populate `spotify_artist_stats`
//!   plus per-track playcounts on demand.
//!
//! Both calls are best-effort: every internal failure is logged at
//! `tracing::warn!` and they fall through to whatever the cache already
//! holds. Callers therefore never need to handle errors - the worst that
//! happens is the cache stays empty for another round.

pub mod cache;
pub mod client;
pub mod hashes;
pub mod resolver;
pub mod token;

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::warn;

use crate::db::Database;

pub use client::SpotifyPublicClient;

/// Per-call input row. Carries everything the resolver needs (the
/// `isrc:<code>` search is primary, but it falls back to `<artist> <title>`,
/// and matches require a known title + primary artist).
#[derive(Debug, Clone)]
pub struct TrackSeed {
    pub isrc: String,
    pub title: String,
    pub artist_name: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ArtistStatsResult {
    pub monthly_listeners: Option<i64>,
    pub followers: Option<i64>,
    pub world_rank: Option<i64>,
    pub top_cities: Vec<TopCity>,
    pub tracks: Vec<TrackStat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopCity {
    pub city: String,
    pub country: String,
    pub listeners: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrackStat {
    pub isrc: String,
    pub title: String,
    pub playcount: Option<i64>,
}

/// Concurrency cap on the per-seed fan-out. Spotify's anonymous endpoints
/// rate-limit aggressively; 5 is the sweet spot empirically.
const FANOUT_PERMITS: usize = 5;

/// Populate `spotify_track_stats` for every ISRC in `seeds` that the cache
/// is missing or considers stale. The result is a fresh per-ISRC view (so
/// callers don't have to re-read the cache themselves).
pub async fn fetch_album_playcounts(
    client: &SpotifyPublicClient,
    db: &Database,
    seeds: &[TrackSeed],
) -> Vec<TrackStat> {
    if seeds.is_empty() {
        return Vec::new();
    }

    let isrcs: Vec<String> = seeds.iter().map(|s| s.isrc.clone()).collect();
    let states = match db.with_conn(|c| cache::read_track_states(c, &isrcs)) {
        Ok(s) => s,
        Err(e) => {
            warn!("spotify_public: cache read failed: {e:#}");
            return seeds
                .iter()
                .map(|s| TrackStat {
                    isrc: s.isrc.clone(),
                    title: s.title.clone(),
                    playcount: None,
                })
                .collect();
        }
    };

    let sem = Arc::new(Semaphore::new(FANOUT_PERMITS));
    let mut handles = Vec::with_capacity(seeds.len());

    for seed in seeds {
        let state = states
            .get(&seed.isrc)
            .cloned()
            .unwrap_or(cache::TrackState::NeedsResolution);
        let sem = sem.clone();
        let client = client.clone();
        let db = db.clone();
        let seed = seed.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.ok()?;
            resolve_and_writeback_track(&client, &db, &seed, state).await
        }));
    }

    let mut out = Vec::with_capacity(seeds.len());
    for (seed, handle) in seeds.iter().zip(handles.into_iter()) {
        let playcount = handle.await.ok().flatten();
        out.push(TrackStat {
            isrc: seed.isrc.clone(),
            title: seed.title.clone(),
            playcount,
        });
    }
    out
}

async fn resolve_and_writeback_track(
    client: &SpotifyPublicClient,
    db: &Database,
    seed: &TrackSeed,
    state: cache::TrackState,
) -> Option<i64> {
    use cache::TrackState;
    match state {
        TrackState::Fresh(pc) => Some(pc),
        TrackState::NegativeFresh => None,
        TrackState::StaleStats { spotify_track_id }
        | TrackState::NeedsStatsFetch { spotify_track_id } => {
            match fetch_playcount(client, &spotify_track_id).await {
                Some(pc) => {
                    if let Err(e) =
                        db.with_conn(|c| cache::write_track_playcount(c, &spotify_track_id, pc))
                    {
                        warn!("spotify_public: write_track_playcount failed: {e:#}");
                    }
                    Some(pc).filter(|p| *p > 0)
                }
                None => None,
            }
        }
        TrackState::NeedsResolution => {
            match resolver::resolve_track_for_isrc(
                client,
                &seed.isrc,
                &seed.title,
                &seed.artist_name,
            )
            .await
            {
                Ok(Some(resolved)) => {
                    if let Err(e) = db.with_conn(|c| {
                        cache::write_track_resolution(c, &seed.isrc, &resolved.spotify_track_id)
                    }) {
                        warn!("spotify_public: write_track_resolution failed: {e:#}");
                    }
                    let playcount = match resolved.playcount {
                        Some(pc) => Some(pc),
                        None => fetch_playcount(client, &resolved.spotify_track_id).await,
                    };
                    if let Some(pc) = playcount
                        && let Err(e) = db.with_conn(|c| {
                            cache::write_track_playcount(c, &resolved.spotify_track_id, pc)
                        })
                    {
                        warn!("spotify_public: write_track_playcount failed: {e:#}");
                    }
                    playcount.filter(|p| *p > 0)
                }
                Ok(None) => {
                    if let Err(e) = db.with_conn(|c| cache::write_track_negative(c, &seed.isrc)) {
                        warn!("spotify_public: write_track_negative failed: {e:#}");
                    }
                    None
                }
                Err(e) => {
                    warn!("spotify_public: resolve_track_for_isrc failed: {e:#}");
                    None
                }
            }
        }
    }
}

async fn fetch_playcount(client: &SpotifyPublicClient, spotify_track_id: &str) -> Option<i64> {
    match client.get_track(spotify_track_id).await {
        Ok(body) => body.pointer("/data/trackUnion/playcount").and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .or_else(|| v.as_i64())
        }),
        Err(e) => {
            warn!("spotify_public: get_track({spotify_track_id}) failed: {e:#}");
            None
        }
    }
}

/// Resolve a Tidal artist to a Spotify artist (via `spotify_artist_map`),
/// populate `spotify_artist_stats` if it's missing/stale, and fetch per-track
/// playcounts on demand. Always returns a value (empty if the resolver can't
/// land on a Spotify ID).
pub async fn fetch_artist_stats(
    client: &SpotifyPublicClient,
    db: &Database,
    tidal_artist_id: &str,
    tidal_artist_name: &str,
    seeds: &[TrackSeed],
) -> ArtistStatsResult {
    let mut result = ArtistStatsResult::default();

    // Top tracks first (same fan-out as album endpoint).
    result.tracks = fetch_album_playcounts(client, db, seeds).await;

    let map_state = match db.with_conn(|c| cache::read_artist_map(c, tidal_artist_id)) {
        Ok(s) => s,
        Err(e) => {
            warn!("spotify_public: read_artist_map failed: {e:#}");
            return result;
        }
    };

    let spotify_artist_id = match map_state {
        cache::ArtistMapState::Resolved(id) => id,
        cache::ArtistMapState::NegativeFresh => return result,
        cache::ArtistMapState::Missing => {
            let pairs: Vec<(String, String)> = seeds
                .iter()
                .map(|s| (s.isrc.clone(), s.title.clone()))
                .collect();
            match resolver::resolve_artist_id(client, tidal_artist_name, &pairs).await {
                Ok(Some(spid)) => {
                    if let Err(e) =
                        db.with_conn(|c| cache::write_artist_map(c, tidal_artist_id, Some(&spid)))
                    {
                        warn!("spotify_public: write_artist_map failed: {e:#}");
                    }
                    spid
                }
                Ok(None) => {
                    if let Err(e) =
                        db.with_conn(|c| cache::write_artist_map(c, tidal_artist_id, None))
                    {
                        warn!("spotify_public: write_artist_map (negative) failed: {e:#}");
                    }
                    return result;
                }
                Err(e) => {
                    warn!("spotify_public: resolve_artist_id failed: {e:#}");
                    return result;
                }
            }
        }
    };

    let cached = db
        .with_conn(|c| cache::read_artist_stats(c, &spotify_artist_id))
        .ok()
        .flatten();

    let need_fetch = match &cached {
        None => true,
        Some((_, stale)) => *stale,
    };

    if need_fetch {
        match client.query_artist_overview(&spotify_artist_id).await {
            Ok(body) => {
                let parsed = parse_artist_overview(&body);
                let top_cities_json = serde_json::to_string(&parsed.top_cities).ok();
                if let Err(e) = db.with_conn(|c| {
                    cache::write_artist_stats(
                        c,
                        &spotify_artist_id,
                        parsed.monthly_listeners,
                        parsed.followers,
                        parsed.world_rank,
                        top_cities_json.as_deref(),
                    )
                }) {
                    warn!("spotify_public: write_artist_stats failed: {e:#}");
                }
                result.monthly_listeners = parsed.monthly_listeners;
                result.followers = parsed.followers;
                result.world_rank = parsed.world_rank;
                result.top_cities = parsed.top_cities;
                return result;
            }
            Err(e) => {
                warn!("spotify_public: query_artist_overview failed: {e:#}");
            }
        }
    }

    if let Some((row, _)) = cached {
        result.monthly_listeners = row.monthly_listeners;
        result.followers = row.followers;
        result.world_rank = row.world_rank;
        if let Some(json) = row.top_cities_json
            && let Ok(cities) = serde_json::from_str::<Vec<TopCity>>(&json)
        {
            result.top_cities = cities;
        }
    }

    result
}

#[derive(Debug, Default)]
struct ParsedArtistOverview {
    monthly_listeners: Option<i64>,
    followers: Option<i64>,
    world_rank: Option<i64>,
    top_cities: Vec<TopCity>,
}

fn parse_artist_overview(body: &serde_json::Value) -> ParsedArtistOverview {
    let stats = body
        .pointer("/data/artistUnion/stats")
        .or_else(|| body.pointer("/data/artist/stats"));
    let mut out = ParsedArtistOverview::default();
    if let Some(stats) = stats {
        out.monthly_listeners = stats.get("monthlyListeners").and_then(|v| v.as_i64());
        out.followers = stats.get("followers").and_then(|v| v.as_i64());
        out.world_rank = stats.get("worldRank").and_then(|v| v.as_i64());
        if let Some(cities) = stats.get("topCities").and_then(|v| v.as_array()) {
            for c in cities.iter().take(5) {
                let city = c
                    .get("city")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let country = c
                    .get("country")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let listeners = c
                    .get("numberOfListeners")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if !city.is_empty() {
                    out.top_cities.push(TopCity {
                        city,
                        country,
                        listeners,
                    });
                }
            }
        }
    }
    out
}
