use crate::SharedState;
use crate::db::queries;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
                tracing::warn!("sportify playlist search failed: {}", e);
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
