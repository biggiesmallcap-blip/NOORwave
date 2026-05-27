use crate::SharedState;
use crate::db::queries;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const SPORTIFY_PLAYLIST_WARN_THROTTLE_MS: u64 = 30_000;
const SPORTIFY_PLAYLIST_WARN_KEY_CAP: usize = 1024;
static SPORTIFY_PLAYLIST_WARN_STATE: OnceLock<Mutex<HashMap<u64, u64>>> = OnceLock::new();
static SPORTIFY_PLAYLIST_WARN_CLOCK_START: OnceLock<Instant> = OnceLock::new();

#[derive(Debug, Deserialize)]
pub(super) struct SearchParams {
    q: String,
    limit: Option<i64>,
}

pub(super) async fn search(
    State(state): State<SharedState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Value>, StatusCode> {
    let limit = params.limit.unwrap_or(20);

    // Snapshot what each side needs without holding the read lock across the
    // Sportify HTTP call.
    let (db, sportify_client, cache_cfg) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.sportify_client.clone(),
            s.sportify_cache_config,
        )
    };

    // Local DB search and Sportify playlist search are independent: run them
    // concurrently. Local search must succeed (existing contract); Sportify is
    // best-effort (upstream may break).
    let q = params.q.clone();
    let db_for_local = db.clone();
    let local_fut = async move { db_for_local.with_conn(|conn| queries::search(conn, &q, limit)) };

    let spotify_fut = async {
        match sportify_client {
            Some(client) => fetch_spotify_playlist_search_compact(
                &client,
                &db,
                &cache_cfg,
                &params.q,
                limit.min(20).max(1) as u32,
            )
            .await
            .unwrap_or_else(|e| {
                let error_text = e.to_string();
                let warn_key = sportify_playlist_warn_key(&params.q, &error_text);
                if claim_throttled_warn_slot(
                    sportify_playlist_warn_state(),
                    warn_key,
                    monotonic_now_ms(),
                    SPORTIFY_PLAYLIST_WARN_THROTTLE_MS,
                ) {
                    tracing::warn!("sportify playlist search failed: {}", error_text);
                } else {
                    tracing::debug!(
                        "sportify playlist search failed (suppressed): {}",
                        error_text
                    );
                }
                Vec::new()
            }),
            None => Vec::new(),
        }
    };

    let (local_res, spotify_playlists) = tokio::join!(local_fut, spotify_fut);
    let local = local_res.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({
        "tracks": local.tracks,
        "albums": local.albums,
        "artists": local.artists,
        "spotify_playlists": spotify_playlists,
    })))
}

/// Compact playlist-search result tailored for inline rendering in /search
/// and Ctrl+K. Drops the heavyweight track-list payload. The ephemeral
/// view fetches that on click.
#[derive(Debug, Serialize)]
struct SpotifyPlaylistSearchItem {
    spotify_id: String,
    name: String,
    description: Option<String>,
    image_url: Option<String>,
    owner: Option<String>,
    follower_count: Option<i64>,
    total_tracks: Option<i32>,
}

async fn fetch_spotify_playlist_search_compact(
    client: &crate::services::sportify::SportifyClient,
    db: &crate::db::Database,
    cfg: &crate::services::sportify::cache::SportifyCacheConfig,
    query: &str,
    limit: u32,
) -> anyhow::Result<Vec<SpotifyPlaylistSearchItem>> {
    use crate::services::sportify::client::SportifySearchKind;
    use crate::services::sportify::recommend::cached_search;

    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    let results = cached_search(
        client,
        db,
        cfg,
        query,
        SportifySearchKind::Playlist,
        limit,
        0,
    )
    .await?;

    Ok(results
        .playlists
        .into_iter()
        .filter_map(|p| {
            let id = p.spotify_id()?;
            Some(SpotifyPlaylistSearchItem {
                spotify_id: id,
                name: p.title().unwrap_or_default(),
                description: p.description.clone(),
                image_url: p.best_thumbnail(),
                owner: p
                    .owner
                    .as_ref()
                    .and_then(|o| o.display_name().map(str::to_string)),
                follower_count: p.follower_count(),
                total_tracks: p.total_track_count(),
            })
        })
        .collect())
}

fn sportify_playlist_warn_state() -> &'static Mutex<HashMap<u64, u64>> {
    SPORTIFY_PLAYLIST_WARN_STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn monotonic_now_ms() -> u64 {
    SPORTIFY_PLAYLIST_WARN_CLOCK_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis() as u64
}

fn sportify_playlist_warn_key(query: &str, error: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    query.trim().to_ascii_lowercase().hash(&mut hasher);
    error.trim().to_ascii_lowercase().hash(&mut hasher);
    hasher.finish()
}

fn claim_throttled_warn_slot(
    state: &Mutex<HashMap<u64, u64>>,
    key: u64,
    now_ms: u64,
    min_interval_ms: u64,
) -> bool {
    let Ok(mut guard) = state.lock() else {
        // Fail-open on poisoned lock so we don't lose warning visibility.
        return true;
    };

    if guard.len() > SPORTIFY_PLAYLIST_WARN_KEY_CAP {
        guard.clear();
    }

    if let Some(last_ms) = guard.get(&key).copied()
        && now_ms.saturating_sub(last_ms) < min_interval_ms
    {
        return false;
    }

    guard.insert(key, now_ms);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> Mutex<HashMap<u64, u64>> {
        Mutex::new(HashMap::new())
    }

    #[test]
    fn throttle_claim_allows_first_event() {
        let state = state();
        assert!(claim_throttled_warn_slot(&state, 1, 10_000, 30_000));
    }

    #[test]
    fn throttle_claim_rejects_events_inside_window_for_same_key() {
        let state = state();
        assert!(claim_throttled_warn_slot(&state, 1, 10_000, 30_000));
        assert!(!claim_throttled_warn_slot(&state, 1, 15_000, 30_000));
    }

    #[test]
    fn throttle_claim_allows_after_window_and_updates_timestamp() {
        let state = state();
        assert!(claim_throttled_warn_slot(&state, 1, 10_000, 30_000));
        assert!(claim_throttled_warn_slot(&state, 1, 45_000, 30_000));
        let guard = state.lock().expect("lock state");
        assert_eq!(guard.get(&1).copied(), Some(45_000));
    }

    #[test]
    fn throttle_is_keyed_by_query_and_error() {
        let state = state();
        let k1 = sportify_playlist_warn_key("daft punk", "sportify request failed: /api/search");
        let k2 = sportify_playlist_warn_key("phoenix", "sportify request failed: /api/search");
        assert_ne!(k1, k2);
        assert!(claim_throttled_warn_slot(&state, k1, 10_000, 30_000));
        assert!(claim_throttled_warn_slot(&state, k2, 10_001, 30_000));
    }

    #[test]
    fn warn_key_normalizes_whitespace_and_case() {
        let a = sportify_playlist_warn_key("  DaFt PuNk ", "Sportify request failed: /api/search");
        let b = sportify_playlist_warn_key("daft punk", "sportify request failed: /api/search");
        assert_eq!(a, b);
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct AudioSearchRequest {
    free_text: Option<String>,
    bpm_min: Option<f64>,
    bpm_max: Option<f64>,
    energy_min: Option<f64>,
    energy_max: Option<f64>,
    danceability_min: Option<f64>,
    danceability_max: Option<f64>,
    key_signature: Option<String>,
    camelot_key: Option<String>,
    year_min: Option<i64>,
    year_max: Option<i64>,
    genre_ids: Option<Vec<i64>>,
    track_type: Option<String>,
    is_instrumental: Option<bool>,
    limit: Option<usize>,
}

pub(super) async fn search_audio(
    State(state): State<SharedState>,
    Json(body): Json<AudioSearchRequest>,
) -> Result<Json<Value>, StatusCode> {
    let filters = queries::AudioFilters {
        bpm_min: body.bpm_min,
        bpm_max: body.bpm_max,
        energy_min: body.energy_min,
        energy_max: body.energy_max,
        danceability_min: body.danceability_min,
        danceability_max: body.danceability_max,
        key_signature: body.key_signature,
        camelot_key: body.camelot_key,
        year_min: body.year_min,
        year_max: body.year_max,
        genre_ids: body.genre_ids.unwrap_or_default(),
        track_type: body.track_type,
        is_instrumental: body.is_instrumental,
    };
    let free_text = body.free_text.unwrap_or_default();
    let limit = body.limit.unwrap_or(50);

    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let tracks = queries::search_with_audio_filters(conn, &free_text, &filters, limit)?;
            Ok(Json(json!({ "tracks": tracks })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Deserialize)]
pub(super) struct VibeParams {
    track_id: i64,
    limit: Option<usize>,
}

pub(super) async fn search_vibe(
    State(state): State<SharedState>,
    Query(params): Query<VibeParams>,
) -> Result<Json<Value>, StatusCode> {
    let limit = params.limit.unwrap_or(6);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let results = queries::get_same_vibe_tracks(conn, params.track_id, limit as i64)?;
            Ok(Json(json!({ "tracks": results })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Deserialize)]
pub(super) struct UnderratedParams {
    artist_id: i64,
    limit: Option<usize>,
}

pub(super) async fn search_underrated(
    State(state): State<SharedState>,
    Query(params): Query<UnderratedParams>,
) -> Result<Json<Value>, StatusCode> {
    let limit = params.limit.unwrap_or(5);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let results = queries::get_underrated_tracks(conn, params.artist_id, limit as i64)?;
            Ok(Json(json!({ "tracks": results })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests_legacy {
    // Legacy mod name retained only to avoid duplicate symbol clashes if this file
    // is merged with older local work. This module intentionally has no tests.
}
